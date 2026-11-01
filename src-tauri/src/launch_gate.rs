//! A child must not create descendants before its owned Job is assigned.
//! The console helper first waits on an unguessable, local named event. Only
//! after Job assignment does the parent release the actual vendor/shell launch.
//! Keep LaunchGate alive until the helper exits so a late-starting helper can
//! still open the named event. No credentials or user config enter this protocol.
#[cfg(windows)]
mod platform {
    use portable_pty::CommandBuilder;
    use std::{ffi::{OsStr, OsString}, os::windows::ffi::OsStrExt, process::{Command, Stdio}};
    use windows_sys::Win32::{Foundation::{CloseHandle, HANDLE}, System::Threading::{CreateEventW, OpenEventW, SetEvent, WaitForSingleObject}};

    pub struct LaunchGate { handle: HANDLE, name: OsString }
    unsafe impl Send for LaunchGate {}
    unsafe impl Sync for LaunchGate {}

    fn wide(value: &OsStr) -> Vec<u16> { value.encode_wide().chain(Some(0)).collect() }

    fn helper_executable() -> Result<std::path::PathBuf, String> {
        let application = std::env::current_exe().map_err(|e| format!("locate OpenDIY Desktop executable: {e}"))?;
        let helper = application.parent().ok_or("OpenDIY Desktop executable has no parent directory")?.join("opendiy-desktop-helper.exe");
        #[cfg(test)]
        let helper = if !helper.is_file() && application.parent().and_then(|path| path.file_name()) == Some(OsStr::new("deps")) {
            application.parent().and_then(|path| path.parent()).ok_or("Test executable has no target directory")?.join("opendiy-desktop-helper.exe")
        } else { helper };
        if !helper.is_file() { return Err(format!("required console helper is missing: {}", helper.display())); }
        Ok(helper)
    }

    impl LaunchGate {
        fn new() -> Result<Self, String> {
            let name = OsString::from(format!("Local\\AgentGridLaunch_{}", uuid::Uuid::new_v4().simple()));
            let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide(&name).as_ptr()) };
            if handle.is_null() { return Err(format!("create owned launch gate: {}", std::io::Error::last_os_error())); }
            Ok(Self { handle, name })
        }

        pub fn wrap_pty(mut command: CommandBuilder) -> Result<(CommandBuilder, Self), String> {
            let gate = Self::new()?;
            let original = command.get_argv().to_vec();
            if original.is_empty() { return Err("owned PTY command has no executable".into()); }
            let mut argv = vec![helper_executable()?.into_os_string(), OsString::from("--pty-child"), gate.name.clone(), OsString::from("--")];
            argv.extend(original);
            *command.get_argv_mut() = argv;
            Ok((command, gate))
        }

        pub fn wrap_std(executable: &OsStr) -> Result<(Command, Self), String> {
            let gate = Self::new()?;
            let mut command = Command::new(helper_executable()?);
            command.arg("--owned-child").arg(&gate.name).arg("--").arg(executable);
            Ok((command, gate))
        }

        pub fn release(&self) -> Result<(), String> {
            if unsafe { SetEvent(self.handle) } == 0 { Err(format!("release owned launch gate: {}", std::io::Error::last_os_error())) } else { Ok(()) }
        }
    }
    impl Drop for LaunchGate { fn drop(&mut self) { unsafe { CloseHandle(self.handle); } } }

    pub fn run_child() -> Option<i32> {
        let args: Vec<_> = std::env::args_os().collect();
        let mode = args.get(1)?.to_str()?;
        if !matches!(mode, "--pty-child" | "--owned-child") { return None; }
        // 전용 도우미의 동기 main이며 아직 다른 스레드를 만들지 않았다.
        // 자격은 실제 TUI를 한 번 생성할 때만 넘기고, 도우미 환경에 남기지 않는다.
        let control_keys=["CODEX_DESKTOP_CRON_BOOTSTRAP","CODEX_DESKTOP_CRON_TOKEN"];
        let mut control_env: Vec<_>=control_keys.iter().filter_map(|key| std::env::var_os(key).map(|value|(*key,value))).collect();
        for key in control_keys {std::env::remove_var(key);}
        if mode!="--pty-child" || !args.get(5..).is_some_and(|arguments|arguments.iter().any(|arg|arg=="--desktop-cron-control=pipe-v1")) {control_env.clear();}
        let mut run = || -> Result<i32, String> {
            let name = args.get(2).ok_or("owned child has no launch gate")?;
            if !name.to_string_lossy().starts_with("Local\\AgentGridLaunch_") || !args.get(3).is_some_and(|arg| arg == "--") || args.len() < 5 { return Err("invalid owned launch arguments".into()); }
            // SYNCHRONIZE only: the helper cannot signal its own release.
            let event = unsafe { OpenEventW(0x00100000, 0, wide(name).as_ptr()) };
            if event.is_null() { return Err("owned launch gate is unavailable".into()); }
            let result = unsafe { WaitForSingleObject(event, 15_000) };
            unsafe { CloseHandle(event); }
            if result != 0 { return Err("owned launch gate was not released within 15 seconds".into()); }
            let mut command = Command::new(&args[4]);
            command.args(&args[5..]);
            if mode=="--pty-child" && args[5..].iter().any(|arg|arg=="--desktop-cron-control=pipe-v1") {
                command.envs(control_env.drain(..));
            }
            if mode == "--pty-child" {
                // The dedicated helper uses the console subsystem in debug and
                // release, so ConPTY attaches this exact console. Open its
                // handles explicitly; never allocate a separate console.
                let input = std::fs::OpenOptions::new().read(true).write(true).open("CONIN$").map_err(|e| format!("open owned console input: {e}"))?;
                let output = std::fs::OpenOptions::new().read(true).write(true).open("CONOUT$").map_err(|e| format!("open owned console output: {e}"))?;
                let error = output.try_clone().map_err(|e| e.to_string())?;
                command.stdin(Stdio::from(input)).stdout(Stdio::from(output)).stderr(Stdio::from(error));
            } else {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000); // CREATE_NO_WINDOW for protocol pipes.
            }
            // Every child inherits the assigned Job. Only the daemon launcher
            // Job permits the shared server's explicit breakaway from that Job.
            let mut child = command.spawn().map_err(|e| format!("launch owned child: {e}"))?;
            // Command가 보유한 자격 복사본도 실행 직후 해제한다.
            for key in control_keys {command.env_remove(key);}
            drop(command);
            match child.wait() {
                Ok(status) => Ok(status.code().unwrap_or(1)),
                Err(error) => { let _ = child.kill(); let _ = child.wait(); Err(format!("reap owned child: {error}")) }
            }
        };
        Some(match run() { Ok(code) => code, Err(error) => { eprintln!("{error}"); 2 } })
    }
}

#[cfg(not(windows))]
mod platform {
    pub struct LaunchGate;
    impl LaunchGate {
        pub fn wrap_pty(command: portable_pty::CommandBuilder) -> Result<(portable_pty::CommandBuilder, Self), String> { Ok((command, Self)) }
        pub fn wrap_std(executable: &std::ffi::OsStr) -> Result<(std::process::Command, Self), String> { Ok((std::process::Command::new(executable), Self)) }
        pub fn release(&self) -> Result<(), String> { Ok(()) }
    }
    pub fn run_child() -> Option<i32> { None }
}
pub use platform::{run_child, LaunchGate};

#[cfg(all(test, windows))]
mod tests {
    use super::LaunchGate;
    use crate::process_job::ProcessJob;
    use std::{
        ffi::OsString,
        io::{BufRead, BufReader, Read, Write},
        os::windows::{ffi::OsStrExt, process::CommandExt},
        process::{Child, ExitStatus, Stdio},
        sync::mpsc::{self, Receiver},
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    const FIXTURE: &str = "launch_gate::tests::environment_fixture_child";
    const REPORT: &str = "opendiy-launch-environment:";
    const KEYS: [&str; 2] = ["APP_CACHE_WIRE_DIAGNOSTICS", "CODEX_HOME"];
    const TIMEOUT: Duration = Duration::from_secs(10);

    fn environment() -> [Option<Vec<u16>>; 2] {
        KEYS.map(|key| std::env::var_os(key).map(|value| value.encode_wide().collect()))
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct EnvironmentReport { pid: u32, values: [Option<Vec<u16>>; 2], has_cron_token: bool }

    // 실제 자식이 대기하는 동안 도우미와 Job을 함께 보유한다. 단언 실패도 자손을 회수한다.
    struct EnvironmentChild {
        child: Child,
        job: Option<ProcessJob>,
        gate: LaunchGate,
        reports: Receiver<Result<EnvironmentReport, String>>,
        reader: Option<JoinHandle<()>>,
    }
    impl EnvironmentChild {
        fn spawn(role: &str, values: Option<[OsString; 2]>) -> Self {
            let (mut command, gate) = LaunchGate::wrap_std(std::env::current_exe().unwrap().as_os_str()).unwrap();
            command.args(["--ignored", "--exact", FIXTURE, "--nocapture"])
                .env("OPENDIY_LAUNCH_ENVIRONMENT_ROLE", role)
                .env("CODEX_DESKTOP_CRON_TOKEN", "시험용-상속금지-토큰")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                .creation_flags(0x08000000); // CREATE_NO_WINDOW
            if let Some(values) = values { for (key, value) in KEYS.into_iter().zip(values) { command.env(key, value); } }
            let (sender, reports) = mpsc::channel();
            let mut owned = Self { child: command.spawn().unwrap(), job: None, gate, reports, reader: None };
            owned.job = Some(ProcessJob::assign_std(&owned.child).unwrap());
            let output = owned.child.stdout.take().unwrap();
            owned.reader = Some(thread::spawn(move || {
                for line in BufReader::new(output).lines() {
                    match line {
                        Ok(line) => if let Some(offset) = line.find(REPORT) {
                            let report = serde_json::from_str(&line[offset + REPORT.len()..]).map_err(|error| format!("환경 보고 형식: {error}"));
                            if sender.send(report).is_err() { break; }
                        },
                        Err(error) => { let _ = sender.send(Err(format!("환경 보고 읽기: {error}"))); break; }
                    }
                }
            }));
            owned
        }

        fn report(&self) -> EnvironmentReport {
            let report = self.reports.recv_timeout(TIMEOUT).expect("실제 자식의 환경 보고 시간 초과").unwrap();
            assert_ne!(report.pid, self.child.id(), "도우미 대신 실제 자식의 환경을 관측해야 합니다.");
            assert!(!report.has_cron_token, "일반 자식에 예약 제어 토큰이 전달됐습니다.");
            report
        }

        fn wait_until(&mut self, deadline: Instant) -> Result<ExitStatus, String> {
            loop {
                if let Some(status) = self.child.try_wait().map_err(|error| format!("도우미 종료 조회: {error}"))? { return Ok(status); }
                if Instant::now() >= deadline { return Err("도우미 종료 시간 초과".into()); }
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn assert_reaped(&mut self) {
            let deadline = Instant::now() + TIMEOUT;
            while self.job.as_ref().unwrap().active_processes().unwrap() != 0 {
                assert!(Instant::now() < deadline, "시험 Job의 자손이 회수되지 않았습니다.");
                thread::sleep(Duration::from_millis(10));
            }
            self.child.wait().unwrap();
        }
    }
    impl Drop for EnvironmentChild {
        fn drop(&mut self) {
            self.job.take();
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() { let _ = reader.join(); }
        }
    }

    #[test]
    fn owned_helper_preserves_child_environment_without_cross_launch_leaks() {
        let parent = environment();
        let homes = [OsString::from("시험용-활성-홈"), OsString::from("시험용-비활성-홈")];
        let mut children = vec![
            EnvironmentChild::spawn("hold", Some(["1".into(), homes[0].clone()])),
            EnvironmentChild::spawn("hold", Some(["0".into(), homes[1].clone()])),
            EnvironmentChild::spawn("hold", None),
        ];
        assert_eq!(environment(), parent, "Command별 환경 설정이 부모를 바꿨습니다.");
        for child in &children {
            assert!(matches!(child.reports.try_recv(), Err(mpsc::TryRecvError::Empty)), "관문 해제 전에 실제 자식이 실행됐습니다.");
            child.gate.release().unwrap();
        }
        let expected = [
            [Some("1".encode_utf16().collect()), Some(homes[0].encode_wide().collect())],
            [Some("0".encode_utf16().collect()), Some(homes[1].encode_wide().collect())],
            parent.clone(),
        ];
        for (child, expected) in children.iter().zip(expected) { assert_eq!(child.report().values, expected, "동시 자식의 환경이 섞이거나 상속값이 바뀌었습니다."); }
        assert!(children.iter_mut().all(|child| child.child.try_wait().unwrap().is_none()), "환경 비교 동안 모든 실제 자식이 함께 실행되어야 합니다.");
        assert_eq!(environment(), parent, "자식 실행이 부모 환경을 바꿨습니다.");
        for child in &mut children {
            child.child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
            assert!(child.wait_until(Instant::now() + TIMEOUT).unwrap().success());
            child.assert_reaped();
        }
        assert_eq!(environment(), parent, "자식 회수 뒤 부모 환경이 바뀌었습니다.");
    }

    #[test]
    fn owned_helper_environment_fixture_reaps_failure_and_deadline_children() {
        let parent = environment();
        for role in ["fail", "hold"] {
            let mut child = EnvironmentChild::spawn(role, None);
            child.gate.release().unwrap();
            assert_eq!(child.report().values, parent);
            if role == "fail" {
                assert!(!child.wait_until(Instant::now() + TIMEOUT).unwrap().success(), "실제 자식의 실패를 도우미가 성공으로 바꿨습니다.");
            } else {
                assert!(child.wait_until(Instant::now()).unwrap_err().contains("시간 초과"));
                child.job.as_ref().unwrap().terminate().unwrap();
                child.wait_until(Instant::now() + TIMEOUT).unwrap();
            }
            child.assert_reaped();
        }
        assert_eq!(environment(), parent);
    }

    #[test]
    #[ignore = "실행 도우미 환경 시험의 실제 자식 진입점"]
    fn environment_fixture_child() {
        let role = std::env::var("OPENDIY_LAUNCH_ENVIRONMENT_ROLE").unwrap();
        let report = EnvironmentReport { pid: std::process::id(), values: environment(), has_cron_token: std::env::var_os("CODEX_DESKTOP_CRON_TOKEN").is_some() };
        println!("\n{REPORT}{}", serde_json::to_string(&report).unwrap());
        std::io::stdout().flush().unwrap();
        assert_ne!(role, "fail", "시험용 자식 실패");
        assert_eq!(role, "hold");
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
        assert_eq!(byte, [b'x']);
    }
}
