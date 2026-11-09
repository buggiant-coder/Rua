use rua_core::runtime_layout::RuntimeLayout;
use serde::Serialize;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

#[derive(Debug, Clone, Serialize)]
pub struct WindowsShellDiagnostic {
    pub platform: String,
    pub selected_shell: String,
    pub shell_kind: String,
    pub git_bash_available: bool,
    pub install_fix: Option<String>,
}

pub fn select_windows_shell_for_path(path: Option<&std::ffi::OsStr>) -> (String, String) {
    let shell = RuntimeLayout::native_shell_for_windows_current_env(path);
    let kind = shell.contract_kind().to_string();
    (shell.program, kind)
}

fn check_windows_shell(_ctx: &CheckContext) -> CheckResult {
    let platform = std::env::consts::OS.to_string();
    if platform != "windows" {
        let details = WindowsShellDiagnostic {
            platform,
            selected_shell: "not-applicable".into(),
            shell_kind: "not-applicable".into(),
            git_bash_available: false,
            install_fix: None,
        };
        return CheckResult {
            name: "windows_shell".into(),
            status: CheckStatus::Pass,
            message: "Windows shell check is not applicable on this platform".into(),
            details: Some(serde_json::to_value(details).unwrap()),
        };
    }

    let path = std::env::var_os("PATH");
    let (selected_shell, shell_kind) = select_windows_shell_for_path(path.as_deref());
    let shell_supported = matches!(shell_kind.as_str(), "git-bash" | "powershell" | "wsl");
    let details = WindowsShellDiagnostic {
        platform,
        selected_shell: selected_shell.clone(),
        shell_kind: shell_kind.clone(),
        git_bash_available: shell_kind == "git-bash",
        install_fix: (!shell_supported).then(|| {
            "winget install --id Git.Git -e; winget install --id Microsoft.PowerShell -e".into()
        }),
    };

    let (status, message) = if shell_supported {
        (
            CheckStatus::Pass,
            format!("Windows shell will use {selected_shell}"),
        )
    } else {
        (
            CheckStatus::Warn,
            "Git Bash and PowerShell 7 were not found; shell execution will fall back to cmd.exe. Install Git for Windows and PowerShell 7 with `winget install --id Git.Git -e` and `winget install --id Microsoft.PowerShell -e`, then restart your terminal.".into(),
        )
    };

    CheckResult {
        name: "windows_shell".into(),
        status,
        message,
        details: Some(serde_json::to_value(details).unwrap()),
    }
}

inventory::submit! {
    register_check("windows_shell", check_windows_shell)
}
