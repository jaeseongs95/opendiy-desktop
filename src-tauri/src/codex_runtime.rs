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
    pub selected_server_executable: Option<String>,
    pub selected_server_version: Option<String>,
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

fn capability(value: &Value, field: &str) -> Result<Option<bool>, String> {
    match value.get(field) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err("공유 서버의 기능 지원 응답을 확인할 수 없습니다.".into()),
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RuntimeCapabilities {
    subagents: Option<bool>, expected_turn_interrupt: Option<bool>, server_version: Option<String>,
}
pub(super) fn observed_capabilities(value: &Value) -> Result<RuntimeCapabilities, String> {
    if !value.is_object() { return Err("공유 서버의 초기화 응답이 올바르지 않습니다.".into()); }
    let server_version = match value.get("userAgent") {
        None => None,
        Some(Value::String(agent)) if agent.len() <= 1024 && !agent.chars().any(char::is_control) => {
            // 기존 initialize 응답의 originator/version만 읽고 새 연결은 만들지 않는다.
            let (originator, rest) = agent.split_once('/').ok_or("공유 서버의 보고 버전 형식이 올바르지 않습니다.")?;
            if originator.is_empty() || originator.chars().any(char::is_whitespace) {
                return Err("공유 서버의 보고 버전 형식이 올바르지 않습니다.".into());
            }
            let version = rest.split_whitespace().next().filter(|version| !version.contains('/'))
                .ok_or("공유 서버의 보고 버전이 없습니다.")?;
            Some(version_text(version.as_bytes())?)
        }
        _ => return Err("공유 서버의 보고 버전 응답이 올바르지 않습니다.".into()),
    };
    Ok(RuntimeCapabilities {
        subagents: capability(value, "supportsSubagentManagement")?,
        expected_turn_interrupt: capability(value, "supportsExpectedTurnInterrupt")?,
        server_version,
    })
}

// 명령 경계를 주입해 호환성 조회가 서버 초기화 명령을 실행하지 않음을 검사한다.
fn inspect_commands(exe: &OsString, mut query: impl FnMut(&[&str]) -> Result<Vec<u8>, String>,
    observed: impl IntoIterator<Item = Result<Option<RuntimeCapabilities>, String>>) -> RuntimeCompatibility {
    let cli = query(&["--version"]).and_then(|bytes| version_text(&bytes));
    let mut report = RuntimeCompatibility {
        executable: exe.to_string_lossy().into_owned(), cli_version: cli.as_ref().ok().cloned(), cli_error: cli.err(),
        server_state: "unavailable", server_version: None, selected_server_executable: None, selected_server_version: None, server_error: None,
        subagents: None, compact_if_idle: None, expected_turn_interrupt: None,
    };
    let server = (|| {
        if !shared::supports_shared(&query(&["app-server", "--help"])?)? { report.server_state = "unsupported"; return Ok(()); }
        // 진단을 위해 연결을 초기화하지 않는다. 기존 세션의 같은 서버 선언만 재사용한다.
        let mut snapshot: Option<RuntimeCapabilities> = None;
        for value in observed {
            if let Some(value) = value? {
                if snapshot.as_ref().is_some_and(|previous| previous != &value) {
                    return Err("기존 연결의 지원 정보 또는 보고 버전이 서로 달라 확인하지 못했습니다.".into());
                }
                snapshot = Some(value);
            }
        }
        if let Some(value) = snapshot {
            report.subagents = value.subagents; report.expected_turn_interrupt = value.expected_turn_interrupt;
            report.server_version = value.server_version;
        }
        // 새 연결로 실행 상태나 다음 시작 대상을 확인하지 않았다.
        Ok(())
    })();
    if let Err(error) = server {
        report.server_error = Some(error); report.subagents = None; report.compact_if_idle = None; report.expected_turn_interrupt = None;
    }
    report
}

pub fn inspect(exe: &OsString, cancelled: &Arc<AtomicBool>, observed: &[shared::SharedControl]) -> Result<RuntimeCompatibility, String> {
    let _request = COMPATIBILITY_REQUEST.try_lock().map_err(|_| "호환성을 확인 중입니다. 잠시 후 다시 시도하세요.")?;
    check_cancelled(Some(cancelled))?;
    let cwd = env::current_dir().map_err(|e| format!("호환성 확인 폴더 조회 실패: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let query = |args: &[&str]| extensions::command_stdout_until(exe, &cwd, &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(), deadline, Some(cancelled));
    let report = inspect_commands(exe, query, observed.iter().map(|control| control.observed_capabilities(exe)));
    check_cancelled(Some(cancelled))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shared_help() -> Vec<u8> {
        b"Usage: fixture app-server [OPTIONS] [COMMAND]\n\nCommands:\n  daemon  Manage\n  proxy  Relay\n\nOptions:\n  -h, --help\n          Print help\n".to_vec()
    }
    #[test]
    fn compatibility_only_runs_local_version_and_help_on_both_support_paths() {
        let legacy = b"Usage: fixture app-server [OPTIONS]\n\nOptions:\n  -h, --help\n          Print help\n".to_vec();
        for (help, expected) in [(shared_help(), "unavailable"), (legacy, "unsupported"), (b"bad help".to_vec(), "unavailable")] {
            let mut calls = Vec::new();
            let report = inspect_commands(&OsString::from("fixture.exe"), |args| {
                calls.push(args.join(" "));
                match args {
                    ["--version"] => Ok(b"cli-v3".to_vec()),
                    ["app-server", "--help"] => Ok(help.clone()),
                    _ => panic!("호환성 조회 중 서버 명령·연결·초기화 금지: {args:?}"),
                }
            }, std::iter::empty());
            assert_eq!(calls, ["--version", "app-server --help"]);
            assert_eq!(report.cli_version.as_deref(), Some("cli-v3"));
            assert_eq!(report.server_state, expected);
            assert!(report.server_version.is_none());
            assert!(report.selected_server_executable.is_none() && report.selected_server_version.is_none());
            assert_eq!(report.server_error.is_some(), help.as_slice() == b"bad help");
        }
        assert!(version_text(b"wrong\nmultiple").is_err());
    }
    #[test]
    fn compatibility_reports_existing_declarations_without_reconfirming_server_state() {
        let original = observed_capabilities(&json!({"userAgent":"any-origin/1.2.3 (Linux 6.8.0; x86_64) any-client/9.0","supportsSubagentManagement":true,"supportsExpectedTurnInterrupt":false})).unwrap();
        let inspect = |observed| inspect_commands(&OsString::from("fixture.exe"), |args| match args {
            ["--version"] => Ok(b"cli-v3".to_vec()),
            ["app-server", "--help"] => Ok(shared_help()),
            _ => panic!("서버 명령 금지: {args:?}"),
        }, observed);
        let report = inspect(vec![Ok(Some(original.clone())), Ok(None), Ok(Some(original.clone()))]);
        assert_eq!(report.server_version.as_deref(), Some("1.2.3"));
        assert_eq!(report.subagents, Some(true)); assert_eq!(report.expected_turn_interrupt, Some(false));
        assert_eq!(report.server_state, "unavailable");
        assert!(report.selected_server_executable.is_none() && report.selected_server_version.is_none());
        assert!(report.compact_if_idle.is_none() && report.server_error.is_none());
        for conflicting in [
            json!({"userAgent":"other-origin/2.0","supportsSubagentManagement":true,"supportsExpectedTurnInterrupt":false}),
            json!({"userAgent":"other-origin/1.2.3","supportsSubagentManagement":false,"supportsExpectedTurnInterrupt":false}),
            json!({"supportsSubagentManagement":true,"supportsExpectedTurnInterrupt":false}),
        ] {
            let report = inspect(vec![Ok(Some(original.clone())), observed_capabilities(&conflicting).map(Some)]);
            assert!(report.server_error.is_some());
            assert!(report.server_version.is_none() && report.subagents.is_none() && report.expected_turn_interrupt.is_none());
        }
        let report = inspect(vec![Ok(Some(original)), Err("기존 연결 관측 오류".into())]);
        assert_eq!(report.server_error.as_deref(), Some("기존 연결 관측 오류"));
        assert!(report.server_version.is_none() && report.subagents.is_none());
    }
    #[test]
    fn support_is_not_inferred_from_version_or_malformed_flags() {
        let snapshot = observed_capabilities(&json!({"supportsSubagentManagement":true,"supportsCompactIfIdle":true})).unwrap();
        assert_eq!(snapshot.subagents, Some(true)); assert_eq!(snapshot.expected_turn_interrupt, None);
        assert_eq!(snapshot.server_version, None);
        // 압축 선언은 passive 구독의 범위이므로 initialize에 있어도 표시하지 않는다.
        assert!(observed_capabilities(&json!({"supportsSubagentManagement":true,"supportsExpectedTurnInterrupt":"true"})).is_err());
        assert!(observed_capabilities(&json!({"supportsSubagentManagement":null})).is_err());
        assert!(observed_capabilities(&Value::Null).is_err());
        let version_only = observed_capabilities(&json!({"userAgent":"another-origin/1.2.3"})).unwrap();
        assert_eq!(version_only.server_version.as_deref(), Some("1.2.3"));
        assert_eq!(version_only.subagents, None); assert_eq!(version_only.expected_turn_interrupt, None);
        for invalid in [json!(null), json!(42), json!(false), json!(""), json!("legacy-server"),
            json!("/1.2.3"), json!("origin/"), json!("origin/   "), json!("bad origin/1.2.3"),
            json!("origin//1.2.3"), json!("origin/1.2.3\nextra"), json!(format!("origin/{}", "v".repeat(257))),
            json!(format!("origin/1.2.3 {}", "x".repeat(1024)))] {
            assert!(observed_capabilities(&json!({"userAgent":invalid})).is_err(), "{invalid}");
        }
    }
}
