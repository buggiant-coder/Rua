#[cfg(unix)]
use std::ffi::CStr;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(any(unix, windows))]
use std::time::Duration;

#[cfg(unix)]
const DELIM: &str = "__RUA_PATH_DELIM__";
#[cfg(windows)]
const DELIM: &str = "__RUA_PATH_DELIM__";
#[cfg(any(unix, windows))]
const TIMEOUT: Duration = Duration::from_secs(3);

pub async fn augment_from_user_shell() {
    #[cfg(unix)]
    {
        augment_unix_path().await;
    }

    #[cfg(windows)]
    {
        augment_windows_path().await;
    }

    #[cfg(not(any(unix, windows)))]
    {
        tracing::debug!("PATH augmentation is only supported on Unix and Windows");
    }
}

#[cfg(unix)]
async fn augment_unix_path() {
    let Some((shell, user_path)) = capture_user_shell_path().await else {
        tracing::debug!("PATH augmentation: no usable login shell found");
        return;
    };

    let current = std::env::var("PATH").unwrap_or_default();
    let new_path = merge_path_entries(&current, [user_path.as_str()], ':', false);

    if new_path == current {
        tracing::debug!(shell = %shell, "PATH augmentation: no new entries");
        return;
    }

    let added = path_entry_count(&new_path, ':').saturating_sub(path_entry_count(&current, ':'));
    set_process_path(&new_path);
    tracing::info!(
        shell = %shell,
        added,
        "PATH augmented from user shell"
    );
}

#[cfg(windows)]
async fn augment_windows_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut sources = capture_windows_environment_paths().await;
    sources.extend(
        common_windows_path_entries()
            .into_iter()
            .map(|path| (path, ';')),
    );
    let new_path = merge_mixed_path_entries(&current, sources.iter(), ';', true);

    if new_path == current {
        tracing::debug!("PATH augmentation: no new Windows PATH entries");
        return;
    }

    let added = path_entry_count(&new_path, ';').saturating_sub(path_entry_count(&current, ';'));
    set_process_path(&new_path);
    tracing::info!(
        added,
        "PATH augmented from Windows user and machine environment"
    );
}

#[cfg(unix)]
async fn capture_user_shell_path() -> Option<(String, String)> {
    let candidates = shell_candidates();
    if candidates.is_empty() {
        return None;
    }

    for shell in candidates {
        if let Some(path) = capture_path(&shell).await {
            return Some((shell, path));
        }
    }
    None
}

#[cfg(any(unix, windows))]
fn set_process_path(new_path: &str) {
    unsafe {
        std::env::set_var("PATH", new_path);
    }
}

#[cfg(unix)]
fn merge_path_entries<'a>(
    current: &str,
    sources: impl IntoIterator<Item = &'a str>,
    separator: char,
    case_insensitive: bool,
) -> String {
    merge_mixed_path_entries(
        current,
        sources
            .into_iter()
            .map(|source| (source.to_string(), separator))
            .collect::<Vec<_>>()
            .iter(),
        separator,
        case_insensitive,
    )
}

#[cfg(any(unix, windows))]
fn merge_mixed_path_entries<'a>(
    current: &str,
    sources: impl IntoIterator<Item = &'a (String, char)>,
    output_separator: char,
    case_insensitive: bool,
) -> String {
    let mut seen = std::collections::HashSet::<String>::new();
    let mut additions = Vec::<String>::new();

    for entry in split_path_entries(current, output_separator) {
        seen.insert(path_key(&entry, case_insensitive));
    }

    for (source, separator) in sources {
        for raw_entry in split_path_entries(source, *separator) {
            let Some(entry) = normalize_path_entry(raw_entry) else {
                continue;
            };
            let key = path_key(&entry, case_insensitive);
            if seen.insert(key) {
                additions.push(entry);
            }
        }
    }

    if additions.is_empty() {
        current.to_string()
    } else if current.is_empty() {
        additions.join(&output_separator.to_string())
    } else {
        format!(
            "{}{}{}",
            additions.join(&output_separator.to_string()),
            output_separator,
            current
        )
    }
}

#[cfg(any(unix, windows))]
fn normalize_path_entry(entry: String) -> Option<String> {
    if !cfg!(windows) {
        return Some(entry);
    }

    normalize_windows_path_entry(&entry)
}

#[cfg(any(unix, windows))]
fn normalize_windows_path_entry(entry: &str) -> Option<String> {
    let bytes = entry.as_bytes();
    if bytes.get(1) == Some(&b':')
        || entry.starts_with("\\\\")
        || entry.starts_with('%')
        || entry.starts_with('$')
    {
        return Some(entry.to_string());
    }

    let parts = entry
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let (drive, rest) = match parts.as_slice() {
        [drive, rest @ ..] if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() => {
            (*drive, rest)
        }
        ["mnt", drive, rest @ ..]
            if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() =>
        {
            (*drive, rest)
        }
        ["cygdrive", drive, rest @ ..]
            if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() =>
        {
            (*drive, rest)
        }
        _ => return None,
    };

    let mut normalized = format!("{}:", drive.to_ascii_uppercase());
    if !rest.is_empty() {
        normalized.push('\\');
        normalized.push_str(&rest.join("\\"));
    }
    Some(normalized)
}

#[cfg(any(unix, windows))]
fn split_path_entries(path: &str, separator: char) -> Vec<String> {
    path.split(separator)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(any(unix, windows))]
fn path_key(entry: &str, case_insensitive: bool) -> String {
    let normalized = trim_trailing_path_separators(entry);
    if case_insensitive {
        normalized.to_ascii_lowercase()
    } else {
        normalized.to_string()
    }
}

#[cfg(any(unix, windows))]
fn trim_trailing_path_separators(entry: &str) -> &str {
    let trimmed = entry.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        entry
    } else {
        trimmed
    }
}

#[cfg(any(unix, windows))]
fn path_entry_count(path: &str, separator: char) -> usize {
    split_path_entries(path, separator).len()
}

#[cfg(windows)]
async fn capture_windows_environment_paths() -> Vec<(String, char)> {
    let mut sources = Vec::new();
    let mut captured_windows_environment = false;

    for shell in windows_bash_candidates() {
        if let Some(path) = capture_windows_bash_path(&shell).await {
            sources.push((path, ':'));
            break;
        }
    }

    for shell in windows_pwsh_candidates() {
        if let Some(paths) = capture_windows_environment_paths_with_powershell(&shell).await {
            sources.extend(paths);
            captured_windows_environment = true;
            break;
        }
    }

    if let Some(path) = capture_windows_cmd_path().await {
        sources.push((path, ';'));
    }

    if !captured_windows_environment {
        for shell in windows_powershell_candidates() {
            if let Some(paths) = capture_windows_environment_paths_with_powershell(&shell).await {
                sources.extend(paths);
                break;
            }
        }
    }

    sources
}

#[cfg(windows)]
fn windows_bash_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    candidates.push("bash.exe".to_string());
    if let Some(program_files) = std::env::var_os("ProgramFiles").filter(|value| !value.is_empty())
    {
        candidates.push(
            std::path::PathBuf::from(&program_files)
                .join("Git")
                .join("bin")
                .join("bash.exe")
                .to_string_lossy()
                .to_string(),
        );
        candidates.push(
            std::path::PathBuf::from(program_files)
                .join("Git")
                .join("usr")
                .join("bin")
                .join("bash.exe")
                .to_string_lossy()
                .to_string(),
        );
    }
    if let Some(program_files_x86) =
        std::env::var_os("ProgramFiles(x86)").filter(|value| !value.is_empty())
    {
        candidates.push(
            std::path::PathBuf::from(&program_files_x86)
                .join("Git")
                .join("bin")
                .join("bash.exe")
                .to_string_lossy()
                .to_string(),
        );
        candidates.push(
            std::path::PathBuf::from(program_files_x86)
                .join("Git")
                .join("usr")
                .join("bin")
                .join("bash.exe")
                .to_string_lossy()
                .to_string(),
        );
    }
    candidates.push("wsl.exe".to_string());
    candidates
}

#[cfg(windows)]
fn windows_pwsh_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    candidates.push("pwsh.exe".to_string());
    if let Some(program_files) = std::env::var_os("ProgramFiles").filter(|value| !value.is_empty())
    {
        candidates.push(
            std::path::PathBuf::from(program_files)
                .join("PowerShell")
                .join("7")
                .join("pwsh.exe")
                .to_string_lossy()
                .to_string(),
        );
    }
    candidates
}

#[cfg(windows)]
fn windows_powershell_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(system_root) = std::env::var_os("SystemRoot").filter(|value| !value.is_empty()) {
        candidates.push(
            std::path::PathBuf::from(system_root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
                .to_string_lossy()
                .to_string(),
        );
    }
    candidates.push("powershell.exe".to_string());
    candidates
}

#[cfg(windows)]
async fn capture_windows_bash_path(shell: &str) -> Option<String> {
    let child = if shell.ends_with("wsl.exe") {
        tokio::process::Command::new(shell)
            .args(["sh", "-lc", "printf '%s' \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
    } else {
        tokio::process::Command::new(shell)
            .args(["-lc", "printf '%s' \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    let child = match child {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(shell, error = %error, "Windows PATH augmentation: bash spawn failed");
            return None;
        }
    };

    let output = match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(output)) => {
            tracing::debug!(shell, status = ?output.status, "Windows PATH augmentation: bash exited non-zero");
            return None;
        }
        Ok(Err(error)) => {
            tracing::debug!(shell, error = %error, "Windows PATH augmentation: bash wait failed");
            return None;
        }
        Err(_) => {
            tracing::debug!(shell, ?TIMEOUT, "Windows PATH augmentation: bash timeout");
            return None;
        }
    };

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

#[cfg(windows)]
async fn capture_windows_cmd_path() -> Option<String> {
    let child = match tokio::process::Command::new("cmd.exe")
        .args(["/d", "/s", "/c", "echo %PATH%"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(error = %error, "Windows PATH augmentation: cmd spawn failed");
            return None;
        }
    };

    let output = match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(output)) => {
            tracing::debug!(status = ?output.status, "Windows PATH augmentation: cmd exited non-zero");
            return None;
        }
        Ok(Err(error)) => {
            tracing::debug!(error = %error, "Windows PATH augmentation: cmd wait failed");
            return None;
        }
        Err(_) => {
            tracing::debug!(?TIMEOUT, "Windows PATH augmentation: cmd timeout");
            return None;
        }
    };

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

#[cfg(windows)]
async fn capture_windows_environment_paths_with_powershell(
    shell: &str,
) -> Option<Vec<(String, char)>> {
    let script = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; \
         [Environment]::GetEnvironmentVariable('Path','User'); \
         Write-Output '{DELIM}'; \
         [Environment]::GetEnvironmentVariable('Path','Machine')"
    );
    let child = match tokio::process::Command::new(shell)
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(shell, error = %error, "Windows PATH augmentation: PowerShell spawn failed");
            return None;
        }
    };

    let output = match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(output)) => {
            tracing::debug!(shell, status = ?output.status, "Windows PATH augmentation: PowerShell exited non-zero");
            return None;
        }
        Ok(Err(error)) => {
            tracing::debug!(shell, error = %error, "Windows PATH augmentation: PowerShell wait failed");
            return None;
        }
        Err(_) => {
            tracing::debug!(
                shell,
                ?TIMEOUT,
                "Windows PATH augmentation: PowerShell timeout"
            );
            return None;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parts = stdout.split(DELIM);
    let user = parts.next().unwrap_or_default().trim();
    let machine = parts.next().unwrap_or_default().trim();
    let paths = [user, machine]
        .into_iter()
        .filter(|value| !value.is_empty())
        .map(|value| (value.to_string(), ';'))
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some(paths)
}

#[cfg(windows)]
fn common_windows_path_entries() -> Vec<String> {
    let mut entries = Vec::new();
    push_windows_env_path(&mut entries, "APPDATA", ["npm"]);
    push_windows_env_path(&mut entries, "LOCALAPPDATA", ["pnpm"]);
    push_windows_env_path(&mut entries, "ProgramFiles", ["nodejs"]);
    push_windows_env_path(&mut entries, "ProgramFiles(x86)", ["nodejs"]);
    entries
}

#[cfg(windows)]
fn push_windows_env_path<const N: usize>(
    entries: &mut Vec<String>,
    var: &str,
    segments: [&str; N],
) {
    let Some(base) = std::env::var_os(var).filter(|value| !value.is_empty()) else {
        return;
    };
    let mut path = std::path::PathBuf::from(base);
    for segment in segments {
        path.push(segment);
    }
    if path.is_dir() {
        entries.push(path.to_string_lossy().to_string());
    }
}

#[cfg(unix)]
fn shell_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    push_shell_candidate(std::env::var("SHELL").ok(), &mut candidates);
    push_shell_candidate(std::env::var("RUA_USER_SHELL").ok(), &mut candidates);
    push_shell_candidate(passwd_shell(), &mut candidates);
    for shell in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        push_shell_candidate(Some(shell.to_string()), &mut candidates);
    }
    candidates
}

#[cfg(unix)]
fn push_shell_candidate(candidate: Option<String>, candidates: &mut Vec<String>) {
    let Some(shell) = candidate.map(|value| value.trim().to_string()) else {
        return;
    };
    if shell.is_empty() || candidates.iter().any(|existing| existing == &shell) {
        return;
    }
    if std::path::Path::new(&shell).is_file() {
        candidates.push(shell);
    }
}

#[cfg(unix)]
fn passwd_shell() -> Option<String> {
    unsafe {
        let passwd = libc::getpwuid(libc::geteuid());
        if passwd.is_null() || (*passwd).pw_shell.is_null() {
            return None;
        }
        CStr::from_ptr((*passwd).pw_shell)
            .to_str()
            .ok()
            .map(ToString::to_string)
    }
}

#[cfg(unix)]
async fn capture_path(shell: &str) -> Option<String> {
    let snippet = format!("printf '{DELIM}%s{DELIM}' \"$PATH\"");

    let mut command = tokio::process::Command::new(shell);
    command
        .args(["-l", "-i", "-c", &snippet])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    unsafe {
        #[allow(unused_imports)]
        use std::os::unix::process::CommandExt as _;
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }

    let child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(shell = %shell, error = %e, "PATH augmentation: spawn failed");
            return None;
        }
    };

    let output = match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(o)) if o.status.success() => o,
        Ok(Ok(o)) => {
            tracing::warn!(shell = %shell, status = ?o.status, "PATH augmentation: shell exited non-zero");
            return None;
        }
        Ok(Err(e)) => {
            tracing::warn!(shell = %shell, error = %e, "PATH augmentation: shell wait failed");
            return None;
        }
        Err(_) => {
            tracing::warn!(shell = %shell, ?TIMEOUT, "PATH augmentation: shell timeout");
            return None;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut parts = stdout.split(DELIM);
    parts.next()?;
    let path = parts.next()?;
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}
