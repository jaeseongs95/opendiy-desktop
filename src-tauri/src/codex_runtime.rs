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
fn apply_capabilities(report: &mut RuntimeCompatibility, value: &Value) -> Result<(), String> {
    if !value.is_object() { return Err("공유 서버의 초기화 응답이 올바르지 않습니다.".into()); }
    // 모두 확인된 뒤에만 게시하여 부분 응답을 지원됨으로 표시하지 않는다.
    let subagents = capability(value, "supportsSubagentManagement")?;
    let compact = capability(value, "supportsCompactIfIdle")?;
    let interrupt = capability(value, "supportsExpectedTurnInterrupt")?;
    report.subagents = subagents; report.compact_if_idle = compact; report.expected_turn_interrupt = interrupt;
    Ok(())
}

pub fn inspect(exe: &OsString, cancelled: &Arc<AtomicBool>) -> Result<RuntimeCompatibility, String> {
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
        let endpoint = shared::endpoint_from_status(exe, &value)?;
        let mut proxy = endpoint.open_until(Some(cancelled.clone()), Some(deadline))?;
        let result = (|| {
            let value = proxy.call("initialize", initialize())?;
            proxy.rpc.as_mut().ok_or("호환성 연결이 닫혔습니다.")?.send(json!({"method":"initialized","params":{}}))?;
            apply_capabilities(&mut report, &value)
        })();
        let cleanup = proxy.finish();
        match (result, cleanup) {
            (Ok(()), Ok(())) => {},
            (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
            (Err(error), Err(cleanup)) => return Err(format!("{error}; 호환성 연결 정리 실패: {cleanup}")),
        }
        report.server_version = Some(value["appServerVersion"].as_str().ok_or("공유 서버 버전이 없습니다.")?.to_owned());
        report.server_state = "connected";
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
        let mut snapshot = report();
        apply_capabilities(&mut snapshot, &json!({"supportsSubagentManagement":true,"supportsCompactIfIdle":false})).unwrap();
        assert_eq!(snapshot.subagents, Some(true)); assert_eq!(snapshot.compact_if_idle, Some(false)); assert_eq!(snapshot.expected_turn_interrupt, None);
        let mut snapshot = report();
        assert!(apply_capabilities(&mut snapshot, &json!({"supportsSubagentManagement":true,"supportsCompactIfIdle":"true"})).is_err());
        assert_eq!(snapshot.subagents, None);
        assert!(apply_capabilities(&mut snapshot, &Value::Null).is_err());
    }
}
