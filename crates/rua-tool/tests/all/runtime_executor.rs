use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use rua_core::runtime_authority::ToolchainIntent;
use rua_tool::{CommandRequest, RuntimeCommandExecutor, ShellKind};

#[test]
fn runtime_executor_uses_system_shell_for_native_hosts() {
    let shell = RuntimeCommandExecutor::shell_for_unix();
    assert!(["/bin/bash", "/bin/sh", "/system/bin/sh"].contains(&shell.program.as_str()));
    assert_eq!(shell.args, vec!["-c"]);
    assert_eq!(shell.kind, ShellKind::Unix);
}

#[test]
fn runtime_executor_shell_plan_comes_from_runtime_authority() {
    let temp = tempfile::tempdir().unwrap();
    let req = CommandRequest {
        command: "echo ok".into(),
        working_dir: temp.path().to_path_buf(),
        tool_result_store_dir: temp.path().join("tool_call_logs"),
        timeout: Duration::from_secs(5),
        env: vec![
            ("KEEP_ME".into(), "1".into()),
            ("DROP_ME".into(), "gone".into()),
        ],
        env_remove: vec!["DROP_ME".into()],
        raw_capture_max_bytes: 1024 * 1024,
        raw_capture_id: None,
        task_session: None,
    };

    let plan = RuntimeCommandExecutor::shell_launch_plan_for_request(&req);

    assert_eq!(
        plan.process().expect("valid launch").cwd,
        Some(temp.path().to_path_buf())
    );
    assert_eq!(
        plan.process().expect("valid launch").env.get("KEEP_ME"),
        Some(&"1".to_string())
    );
    assert!(!plan
        .process()
        .expect("valid launch")
        .env
        .contains_key("DROP_ME"));
    assert!(matches!(plan.toolchain_intent, ToolchainIntent::Shell));
}

#[test]
fn windows_executor_prefers_explicit_shell() {
    let temp = tempfile::tempdir().unwrap();
    let bash = temp.path().join("bash.exe");
    fs::write(&bash, b"").unwrap();

    let shell = RuntimeCommandExecutor::shell_for_windows(Some(bash.as_os_str()), None);

    assert_eq!(shell.program, bash.to_string_lossy());
    assert_eq!(shell.args, vec!["-lc"]);
    assert_eq!(shell.kind, ShellKind::Bash);
}

#[test]
fn windows_executor_prefers_git_bash_on_path() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("Git").join("bin");
    fs::create_dir_all(&bin).unwrap();
    let bash = bin.join("bash.exe");
    fs::write(&bash, b"").unwrap();
    let path = OsString::from(bin.to_string_lossy().into_owned());

    let shell = RuntimeCommandExecutor::shell_for_windows(None, Some(&path));

    assert_eq!(shell.program, bash.to_string_lossy());
    assert_eq!(shell.args, vec!["-lc"]);
    assert_eq!(shell.kind, ShellKind::Bash);
}

#[test]
fn windows_executor_falls_back_to_cmd_when_git_bash_missing() {
    let shell = RuntimeCommandExecutor::shell_for_windows(None, Some(&OsString::new()));

    assert_eq!(shell.program, "cmd.exe");
    assert_eq!(shell.args, vec!["/C"]);
    assert_eq!(shell.kind, ShellKind::Cmd);
}

#[test]
fn windows_executor_supports_powershell_7_on_path() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("PowerShell").join("7");
    fs::create_dir_all(&bin).unwrap();
    let pwsh = bin.join("pwsh.exe");
    fs::write(&pwsh, b"").unwrap();
    let path = OsString::from(bin.to_string_lossy().into_owned());

    let shell = RuntimeCommandExecutor::shell_for_windows(None, Some(&path));

    assert_eq!(shell.program, pwsh.to_string_lossy());
    assert_eq!(
        shell.args,
        vec!["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
    );
    assert_eq!(shell.kind, ShellKind::PowerShell);
}

#[test]
fn windows_executor_supports_explicit_wsl_shell() {
    let temp = tempfile::tempdir().unwrap();
    let wsl = temp.path().join("wsl.exe");
    fs::write(&wsl, b"").unwrap();

    let shell = RuntimeCommandExecutor::shell_for_windows(Some(wsl.as_os_str()), None);

    assert_eq!(shell.program, wsl.to_string_lossy());
    assert_eq!(shell.args, vec!["--exec", "sh", "-lc"]);
    assert_eq!(shell.kind, ShellKind::Wsl);
}

#[test]
fn windows_executor_does_not_auto_select_wsl_from_path() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("Windows").join("System32");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("wsl.exe"), b"").unwrap();
    let path = OsString::from(bin.to_string_lossy().into_owned());

    let shell = RuntimeCommandExecutor::shell_for_windows(None, Some(&path));

    assert_eq!(shell.program, "cmd.exe");
    assert_eq!(shell.kind, ShellKind::Cmd);
}

#[test]
fn windows_taskkill_args_kill_process_tree() {
    assert_eq!(
        RuntimeCommandExecutor::windows_taskkill_args(1234),
        vec!["/PID", "1234", "/T", "/F"]
    );
}
