//! 설정 화면에서만 실행하는 읽기 전용 호환성 조회.
use super::*;

static COMPATIBILITY_REQUEST: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCompatibility {
    pub executable: String,
    pub cli_version: Option<String>,
    pub cli_error: Option<String>,
    pub server_state: &'static str,
    pub server_version: Option<String>,
    pub server_error: Option<String>,
    pub subagents: Option<bool>,
    pub compact_if_idle: Option<bool>,
    pub expected_turn_interrupt: Option<bool>,
}

fn version_text(bytes: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "실행 파일 버전 응답이 올바른 문자열이 아닙니다.")?.trim();
    if text.is_empty() || text.len() > 256 || text.chars().any(char::is_control) {
        return Err("실행 파일 버전 응답 형식을 확인할 수 없습니다.".into());
    }
    Ok(text.to_owned())
}

fn discover(mut query: impl FnMut(&[&str]) -> Result<Vec<u8>, String>) -> Result<Option<Value>, String> {
    if !shared::supports_shared(&query(&["app-server", "--help"])?)? { return Ok(None); }
    let value = shared::lifecycle_value(&query(&["app-server", "daemon", "version"])?)?;
    shared::lifecycle_socket(&value)?;
    version_text(value["appServerVersion"].as_str().ok_or("공유 서버 버전이 없습니다.")?.as_bytes())?;
    Ok(Some(value))
}

fn capability(value: &Value, field: &str) -> Result<Option<bool>, String> {
    match value.get(field) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err("공유 서버의 기능 지원 응답을 확인할 수 없습니다.".into()),
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RuntimeCapabilities {
    subagents: Option<bool>, expected_turn_interrupt: Option<bool>,
}
pub(super) fn observed_capabilities(value: &Value) -> Result<RuntimeCapabilities, String> {
    if !value.is_object() { return Err("공유 서버의 초기화 응답이 올바르지 않습니다.".into()); }
    Ok(RuntimeCapabilities {
        subagents: capability(value, "supportsSubagentManagement")?,
        expected_turn_interrupt: capability(value, "supportsExpectedTurnInterrupt")?,
    })
}

pub fn inspect(exe: &OsString, cancelled: &Arc<AtomicBool>, observed: &[shared::SharedControl]) -> Result<RuntimeCompatibility, String> {
    let _request = COMPATIBILITY_REQUEST.try_lock().map_err(|_| "호환성을 확인 중입니다. 잠시 후 다시 시도하세요.")?;
    check_cancelled(Some(cancelled))?;
    let cwd = env::current_dir().map_err(|e| format!("호환성 확인 폴더 조회 실패: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let query = |args: &[&str]| extensions::command_stdout_until(exe, &cwd, &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(), deadline, Some(cancelled));
    let cli = query(&["--version"]).and_then(|bytes| version_text(&bytes));
    let mut report = RuntimeCompatibility {
        executable: exe.to_string_lossy().into_owned(), cli_version: cli.as_ref().ok().cloned(), cli_error: cli.err(),
        server_state: "unavailable", server_version: None, server_error: None,
        subagents: None, compact_if_idle: None, expected_turn_interrupt: None,
    };
    let server = (|| {
        let Some(value) = discover(query)? else { report.server_state = "unsupported"; return Ok(()); };
        // 진단을 위해 연결을 초기화하지 않는다. 기존 세션의 같은 서버 선언만 재사용한다.
        let mut snapshot: Option<RuntimeCapabilities> = None;
        for control in observed {
            if let Some(value) = control.observed_capabilities(exe, &value)? {
                if snapshot.as_ref().is_some_and(|previous| previous != &value) {
                    return Err("기존 연결의 지원 정보가 서로 달라 확인하지 못했습니다.".into());
                }
                snapshot = Some(value);
            }
        }
        if let Some(value) = snapshot { report.subagents = value.subagents; report.expected_turn_interrupt = value.expected_turn_interrupt; }
        report.server_version = Some(value["appServerVersion"].as_str().ok_or("공유 서버 버전이 없습니다.")?.to_owned());
        report.server_state = "running";
        Ok(())
    })();
    if let Err(error) = server {
        report.server_error = Some(error); report.subagents = None; report.compact_if_idle = None; report.expected_turn_interrupt = None;
    }
    check_cancelled(Some(cancelled))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report() -> RuntimeCompatibility { RuntimeCompatibility { executable: "fixture.exe".into(), cli_version: None, cli_error: None, server_state: "unavailable", server_version: None, server_error: None, subagents: None, compact_if_idle: None, expected_turn_interrupt: None } }
    #[test]
    fn discovery_only_reads_help_and_version_and_never_starts_a_missing_server() {
        let socket = env::current_dir().unwrap().join("diagnostic.sock");
        let help = b"Usage: fixture app-server [OPTIONS] [COMMAND]\n\nCommands:\n  daemon  Manage\n  proxy  Relay\n\nOptions:\n  -h, --help\n          Print help\n";
        let mut calls = Vec::new();
        let value = discover(|args| { calls.push(args.join(" ")); if args.last() == Some(&"--help") { Ok(help.to_vec()) } else { Ok(serde_json::to_vec(&json!({"status":"running","socketPath":socket,"appServerVersion":"0.1.0"})).unwrap()) } }).unwrap().unwrap();
        assert_eq!(value["appServerVersion"], "0.1.0");
        assert_eq!(calls, ["app-server --help", "app-server daemon version"]);
        calls.clear();
        assert!(discover(|args| { calls.push(args.join(" ")); if args.last() == Some(&"--help") { Ok(help.to_vec()) } else { Err("서버 없음".into()) } }).is_err());
        assert_eq!(calls, ["app-server --help", "app-server daemon version"]);
        assert!(version_text(b"wrong\nmultiple").is_err());
    }
    #[test]
    fn support_is_not_inferred_from_version_or_malformed_flags() {
        let snapshot = observed_capabilities(&json!({"supportsSubagentManagement":true,"supportsCompactIfIdle":true})).unwrap();
        assert_eq!(snapshot.subagents, Some(true)); assert_eq!(snapshot.expected_turn_interrupt, None);
        // 압축 선언은 passive 구독의 범위이므로 initialize에 있어도 표시하지 않는다.
        assert!(observed_capabilities(&json!({"supportsSubagentManagement":true,"supportsExpectedTurnInterrupt":"true"})).is_err());
        assert!(observed_capabilities(&Value::Null).is_err());
        assert_eq!(report().compact_if_idle, None);
    }
}
