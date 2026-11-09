use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const LINUX_COMPAT_GUEST_HOME: &str = "/root";
pub const MOBILE_SHARED_STORAGE_PRIMARY: &str = "/sdcard";
pub const MOBILE_SHARED_STORAGE_ANDROID: &str = "/storage/emulated/0";

const LINUX_COMPAT_DEFAULT_PATH: &str =
    "/root/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostProfile {
    MobileApk,
    Desktop,
    Headless,
}

impl HostProfile {
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            Some("mobile-apk" | "mobile" | "android") => Self::MobileApk,
            Some("headless" | "server") => Self::Headless,
            Some("desktop") => Self::Desktop,
            _ => Self::Desktop,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MobileApk => "mobile-apk",
            Self::Desktop => "desktop",
            Self::Headless => "headless",
        }
    }

    pub fn is_mobile(self) -> bool {
        matches!(self, Self::MobileApk)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Native,
    LinuxCompat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessShape {
    Shell,
    Stdio,
    Direct,
    Install,
    SkillProbe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeShellKind {
    Unix,
    Bash,
    PowerShell,
    Cmd,
    Wsl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeShellSpec {
    pub program: String,
    pub args: Vec<String>,
    pub kind: NativeShellKind,
}

impl NativeShellSpec {
    fn new(program: impl Into<String>, args: &[&str], kind: NativeShellKind) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            kind,
        }
    }

    fn bash(program: impl Into<String>) -> Self {
        Self::new(program, &["-lc"], NativeShellKind::Bash)
    }

    fn unix(program: impl Into<String>) -> Self {
        Self::new(program, &["-c"], NativeShellKind::Unix)
    }

    fn powershell(program: impl Into<String>) -> Self {
        Self::new(
            program,
            &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"],
            NativeShellKind::PowerShell,
        )
    }

    fn cmd(program: impl Into<String>) -> Self {
        Self::new(program, &["/C"], NativeShellKind::Cmd)
    }

    fn wsl(program: impl Into<String>) -> Self {
        Self::new(program, &["--exec", "sh", "-lc"], NativeShellKind::Wsl)
    }

    pub fn contract_kind(&self) -> &'static str {
        match self.kind {
            NativeShellKind::Unix => "unix",
            NativeShellKind::Bash => "git-bash",
            NativeShellKind::PowerShell => "powershell",
            NativeShellKind::Cmd => "cmd",
            NativeShellKind::Wsl => "wsl",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeBind {
    pub host: PathBuf,
    pub guest: PathBuf,
}

impl RuntimeBind {
    pub fn new(host: impl Into<PathBuf>, guest: impl Into<PathBuf>) -> Self {
        Self {
            host: host.into(),
            guest: guest.into(),
        }
    }

    fn proot_arg(&self) -> String {
        if self.host == self.guest {
            format!("--bind={}", self.host.display())
        } else {
            format!("--bind={}:{}", self.host.display(), self.guest.display())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLayout {
    pub host_profile: HostProfile,
    pub os_family: String,
    pub linux_compat: Option<LinuxCompatLayout>,
}

impl RuntimeLayout {
    pub fn from_current_process(host_profile: Option<&str>) -> Self {
        let os_family = if cfg!(target_os = "android") {
            "android".to_string()
        } else {
            env::consts::OS.to_string()
        };
        let host_profile = match host_profile
            .map(str::to_string)
            .or_else(|| env::var("RUA_HOST_PROFILE").ok())
            .or_else(|| env::var("RUA_SERVER_PROFILE").ok())
        {
            Some(value) => HostProfile::parse(Some(&value)),
            None if os_family == "android"
                || env::var("RUA_MOBILE").ok().as_deref() == Some("1") =>
            {
                HostProfile::MobileApk
            }
            None => HostProfile::Desktop,
        };
        let mobile_runtime = host_profile.is_mobile()
            || os_family == "android"
            || env::var("RUA_MOBILE").ok().as_deref() == Some("1");
        // APK-owned native paths and app data are explicit. PREFIX is not a runtime contract.
        let linux_compat = if mobile_runtime {
            env::var_os("RUA_ANDROID_NATIVE_DIR")
                .zip(env::var_os("RUA_ANDROID_DATA_DIR"))
                .zip(env::var_os("HOME"))
                .filter(|((native, data), home)| {
                    !native.is_empty() && !data.is_empty() && !home.is_empty()
                })
                .map(|((native, data), home)| {
                    LinuxCompatLayout::android(
                        Path::new(&native),
                        Path::new(&data),
                        Path::new(&home),
                    )
                })
        } else {
            None
        };
        Self {
            host_profile,
            os_family,
            linux_compat,
        }
    }

    pub fn android(
        native_dir: impl AsRef<Path>,
        data_dir: impl AsRef<Path>,
        host_home: impl AsRef<Path>,
    ) -> Self {
        Self {
            host_profile: HostProfile::MobileApk,
            os_family: "android".to_string(),
            linux_compat: Some(LinuxCompatLayout::android(
                native_dir.as_ref(),
                data_dir.as_ref(),
                host_home.as_ref(),
            )),
        }
    }

    pub fn process_temp_dir_for_rua_root(&self, rua_root: &Path) -> Option<PathBuf> {
        if self.host_profile.is_mobile() || self.os_family == "android" {
            Some(rua_root.join("tmp"))
        } else {
            None
        }
    }

    pub fn native_shell_for_current_process() -> NativeShellSpec {
        if cfg!(unix) {
            Self::native_shell_for_unix()
        } else if cfg!(windows) {
            let path = env::var_os("PATH");
            Self::native_shell_for_windows(
                env::var_os("RUA_WINDOWS_SHELL").as_deref(),
                path.as_deref(),
            )
        } else {
            NativeShellSpec::cmd("cmd")
        }
    }

    pub fn native_shell_for_unix() -> NativeShellSpec {
        let candidates = ["/bin/bash", "/bin/sh", "/system/bin/sh"].map(PathBuf::from);

        for candidate in candidates {
            if candidate.exists() {
                return NativeShellSpec::unix(candidate.to_string_lossy().into_owned());
            }
        }

        NativeShellSpec::unix("/bin/sh")
    }

    pub fn native_shell_for_windows(
        explicit_shell: Option<&OsStr>,
        path: Option<&OsStr>,
    ) -> NativeShellSpec {
        if let Some(shell) = explicit_shell {
            if let Some(shell) = resolve_windows_shell(shell, path) {
                return shell_spec_for_windows_path(shell);
            }
        }

        if let Some(shell) = find_on_windows_path("bash.exe", path) {
            return NativeShellSpec::bash(shell.to_string_lossy().into_owned());
        }

        for candidate in standard_git_bash_candidates() {
            if is_executable_candidate(&candidate) {
                return NativeShellSpec::bash(candidate.to_string_lossy().into_owned());
            }
        }

        if let Some(shell) = find_on_windows_path("pwsh.exe", path) {
            return NativeShellSpec::powershell(shell.to_string_lossy().into_owned());
        }
        if let Some(shell) = find_on_windows_path("powershell.exe", path) {
            return NativeShellSpec::powershell(shell.to_string_lossy().into_owned());
        }
        NativeShellSpec::cmd("cmd.exe")
    }

    pub fn native_shell_for_windows_current_env(path: Option<&OsStr>) -> NativeShellSpec {
        Self::native_shell_for_windows(env::var_os("RUA_WINDOWS_SHELL").as_deref(), path)
    }

    pub fn agent_path_contract(&self, workspace_dir: &Path) -> AgentPathContract {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "workspace_dir".to_string(),
            workspace_dir.to_string_lossy().into_owned(),
        );

        let home_dir = if let Some(linux) = &self.linux_compat {
            aliases.insert(
                "app_home".to_string(),
                linux.host_home.to_string_lossy().into_owned(),
            );
            aliases.insert(
                "linux_guest_home".to_string(),
                linux.guest_home.to_string_lossy().into_owned(),
            );
            Some(linux.host_home.to_string_lossy().into_owned())
        } else {
            env::var("HOME").ok()
        };
        if let Some(home) = &home_dir {
            aliases.insert("home".to_string(), home.clone());
        }

        let temp_dir = env::var("TMPDIR").or_else(|_| env::var("TEMP")).ok();
        if let Some(tmp) = &temp_dir {
            aliases.insert("tmp".to_string(), tmp.clone());
        }

        let mut user_file_roots = Vec::new();
        let mut rules = Vec::new();
        let mut warnings = Vec::new();
        if self.host_profile.is_mobile() || self.os_family == "android" {
            user_file_roots.push(MOBILE_SHARED_STORAGE_PRIMARY.to_string());
            user_file_roots.push(MOBILE_SHARED_STORAGE_ANDROID.to_string());
            aliases.insert(
                "mobile_shared_storage".to_string(),
                MOBILE_SHARED_STORAGE_PRIMARY.to_string(),
            );
            aliases.insert(
                "mobile_shared_storage_alt".to_string(),
                MOBILE_SHARED_STORAGE_ANDROID.to_string(),
            );
            rules.push("Use /sdcard/... for user-visible phone files.".to_string());
            rules.push("Use workspace_dir for Rua workspace files.".to_string());
            rules.push("Avoid ~ in tool arguments on Android.".to_string());
            rules.push(
                "/root is only the Linux Compatibility Runtime guest HOME for CLI config."
                    .to_string(),
            );
            warnings.push(
                "/sdcard access depends on Android storage permission being granted.".to_string(),
            );
        } else {
            rules.push("Use workspace_dir for Rua workspace files.".to_string());
            rules.push("Use absolute paths when passing files between tools.".to_string());
            if self.os_family == "windows" {
                warnings.push(
                    "Windows shell adapters may use PowerShell, cmd, Git Bash, or WSL path rules."
                        .to_string(),
                );
            }
        }

        AgentPathContract {
            workspace_dir: workspace_dir.to_string_lossy().into_owned(),
            home_dir,
            temp_dir,
            user_file_roots,
            aliases,
            rules,
            warnings,
        }
    }
}

fn is_executable_candidate(path: &Path) -> bool {
    path.is_file()
}

fn resolve_windows_shell(shell: &OsStr, path: Option<&OsStr>) -> Option<PathBuf> {
    let shell_text = shell.to_string_lossy();
    if shell_text.trim().is_empty() {
        return None;
    }

    let shell_path = PathBuf::from(shell);
    if shell_text.contains('\\') || shell_text.contains('/') || shell_path.is_absolute() {
        return is_executable_candidate(&shell_path).then_some(shell_path);
    }

    find_on_windows_path(&shell_text, path)
        .or_else(|| is_executable_candidate(&shell_path).then_some(shell_path))
}

fn shell_spec_for_windows_path(shell: PathBuf) -> NativeShellSpec {
    let program = shell.to_string_lossy().into_owned();
    let name = shell
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);

    match stem {
        "bash" | "sh" => NativeShellSpec::bash(program),
        "pwsh" | "powershell" => NativeShellSpec::powershell(program),
        "cmd" => NativeShellSpec::cmd(program),
        "wsl" => NativeShellSpec::wsl(program),
        _ => NativeShellSpec::bash(program),
    }
}

#[cfg(windows)]
fn standard_git_bash_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"),
        PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Git\usr\bin\bash.exe"),
    ]
}

#[cfg(not(windows))]
fn standard_git_bash_candidates() -> Vec<PathBuf> {
    Vec::new()
}

fn find_on_windows_path(command: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    path?
        .to_string_lossy()
        .split(';')
        .filter(|entry| !entry.trim().is_empty())
        .find_map(|entry| {
            windows_command_file_names(command)
                .into_iter()
                .map(|name| PathBuf::from(entry).join(name))
                .find(|candidate| is_executable_candidate(candidate))
        })
}

fn windows_command_file_names(command: &str) -> Vec<String> {
    let mut names = vec![command.to_string()];
    if Path::new(command).extension().is_none() {
        let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        for ext in pathext.split(';').filter(|ext| !ext.trim().is_empty()) {
            names.push(format!("{command}{ext}"));
        }
    }
    names
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPathContract {
    pub workspace_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp_dir: Option<String>,
    pub user_file_roots: Vec<String>,
    pub aliases: BTreeMap<String, String>,
    pub rules: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinuxCompatLayout {
    pub proot_path: PathBuf,
    pub rootfs_path: PathBuf,
    pub host_home: PathBuf,
    pub guest_home: PathBuf,
    pub binds: Vec<RuntimeBind>,
}

impl LinuxCompatLayout {
    /// App data survives APK replacement; nativeLibraryDir is supplied anew by the APK.
    pub fn android(native_dir: &Path, data_dir: &Path, host_home: &Path) -> Self {
        let home = host_home.to_path_buf();
        let rootfs = data_dir.join("runtime/debian");
        Self {
            proot_path: native_dir.join("libproot.so"),
            rootfs_path: rootfs.clone(),
            host_home: home.clone(),
            guest_home: PathBuf::from(LINUX_COMPAT_GUEST_HOME),
            binds: vec![
                RuntimeBind::new(&home, &home),
                RuntimeBind::new(data_dir.join("guest-home"), LINUX_COMPAT_GUEST_HOME),
                RuntimeBind::new(
                    data_dir.join("app-resources"),
                    data_dir.join("app-resources"),
                ),
                RuntimeBind::new(data_dir.join("tmp"), "/tmp"),
                RuntimeBind::new(data_dir.join("tmp"), "/dev/shm"),
                RuntimeBind::new(MOBILE_SHARED_STORAGE_ANDROID, MOBILE_SHARED_STORAGE_PRIMARY),
                RuntimeBind::new("/storage", "/storage"),
                RuntimeBind::new("/proc/self/fd", "/dev/fd"),
                RuntimeBind::new("/dev/urandom", "/dev/random"),
                RuntimeBind::new("/sys", "/sys"),
                RuntimeBind::new("/proc", "/proc"),
                RuntimeBind::new("/dev", "/dev"),
            ],
        }
    }

    pub fn is_available(&self) -> bool {
        self.proot_path.is_file() && self.rootfs_path.join("usr/bin/env").is_file()
    }

    pub fn node_lts_bin_dir(version: &str) -> String {
        format!("/usr/local/lib/nodejs/node-v{version}-linux-arm64/bin")
    }

    pub fn base_env(&self, extra_path_entries: &[String]) -> BTreeMap<String, String> {
        let path = if extra_path_entries.is_empty() {
            LINUX_COMPAT_DEFAULT_PATH.to_string()
        } else {
            let mut entries = extra_path_entries.to_vec();
            entries.push(LINUX_COMPAT_DEFAULT_PATH.to_string());
            entries.join(":")
        };
        BTreeMap::from([
            ("PATH".to_string(), path),
            (
                "HOME".to_string(),
                self.guest_home.to_string_lossy().into_owned(),
            ),
            ("USER".to_string(), "root".to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("TMPDIR".to_string(), "/tmp".to_string()),
            ("UV_CACHE_DIR".to_string(), "/root/.cache/uv".to_string()),
            (
                "RUA_PYTHON_VENV".to_string(),
                "/root/.venv/rua-mobile".to_string(),
            ),
            ("UV_PYTHON_DOWNLOADS".to_string(), "never".to_string()),
            // PRoot link2symlink can expose host cache paths that do not exist in the guest.
            ("UV_LINK_MODE".to_string(), "copy".to_string()),
            ("NPM_CONFIG_PREFIX".to_string(), "/root/.local".to_string()),
            ("PIP_ROOT_USER_ACTION".to_string(), "ignore".to_string()),
            ("RUA_MOBILE".to_string(), "1".to_string()),
        ])
    }

    pub fn map_host_path(&self, path: &Path) -> Option<PathBuf> {
        if path.as_os_str().is_empty() {
            return None;
        }
        if path.starts_with(MOBILE_SHARED_STORAGE_ANDROID) {
            let suffix = path
                .strip_prefix(MOBILE_SHARED_STORAGE_ANDROID)
                .unwrap_or_else(|_| Path::new(""));
            return Some(PathBuf::from(MOBILE_SHARED_STORAGE_PRIMARY).join(suffix));
        }
        for bind in &self.binds {
            if path.starts_with(&bind.host) {
                let suffix = path
                    .strip_prefix(&bind.host)
                    .unwrap_or_else(|_| Path::new(""));
                return Some(bind.guest.join(suffix));
            }
        }
        if path.starts_with(&self.guest_home) || path.starts_with("/tmp") {
            return Some(path.to_path_buf());
        }
        None
    }

    pub fn launch_plan(
        &self,
        program: impl Into<String>,
        options: LinuxCompatLaunchOptions,
    ) -> ProcessLaunchPlan {
        let program = program.into();
        let mut args = self.proot_base_args(options.cwd.as_deref());
        args.push("/usr/bin/env".to_string());

        let mut env = self.merged_env(
            &options.extra_path_entries,
            options.env,
            &options.env_remove,
        );
        // Values (including credentials) travel through the process environment, never argv.
        // Android adapters clear the parent environment before applying this explicit map.
        let native_dir = self.proot_path.parent().unwrap_or_else(|| Path::new(""));
        env.insert(
            "LD_LIBRARY_PATH".into(),
            native_dir.to_string_lossy().into_owned(),
        );
        env.insert(
            "PROOT_LOADER".into(),
            native_dir
                .join("libproot-loader.so")
                .to_string_lossy()
                .into_owned(),
        );
        env.insert(
            "PROOT_TMP_DIR".into(),
            self.host_home
                .parent()
                .unwrap()
                .join("tmp")
                .to_string_lossy()
                .into_owned(),
        );
        env.insert("PROOT_NO_SECCOMP".into(), "1".into());
        for name in [
            "LD_LIBRARY_PATH",
            "PROOT_LOADER",
            "PROOT_TMP_DIR",
            "PROOT_NO_SECCOMP",
            "RUA_PHONE_SOCKET",
            "RUA_PHONE_HOST_PID",
            "RUA_PHONE_TOKEN",
            "RUA_ANDROID_NATIVE_DIR",
            "RUA_ANDROID_DATA_DIR",
            "RUA_XTERM_BRIDGE",
        ] {
            args.push(format!("--unset={name}"));
        }
        args.push(program);
        let runtime_dir = self
            .rootfs_path
            .parent()
            .expect("Android guest has a parent");
        let mut guarded = vec![
            "--shared".to_string(),
            runtime_dir.join("use.lock").to_string_lossy().into_owned(),
            runtime_dir
                .join("update.pending")
                .to_string_lossy()
                .into_owned(),
            self.proot_path.to_string_lossy().into_owned(),
        ];
        guarded.extend(args);
        args = guarded;
        let command = native_dir.join("librua-runtime-gate.so");
        ProcessLaunchPlan {
            command,
            args,
            env,
            cwd: None,
        }
    }

    fn proot_base_args(&self, cwd: Option<&Path>) -> Vec<String> {
        let cwd = match cwd {
            Some(path) => self
                .map_host_path(path)
                .filter(|mapped| !mapped.as_os_str().is_empty())
                .unwrap_or_else(|| path.to_path_buf()),
            None => self.guest_home.clone(),
        };
        let mut args: Vec<String> = self.binds.iter().map(RuntimeBind::proot_arg).collect();
        args.extend([
            "-L".to_string(),
            "--sysvipc".to_string(),
            "--link2symlink".to_string(),
            "--kill-on-exit".to_string(),
            format!("--cwd={}", cwd.display()),
            format!("--rootfs={}", self.rootfs_path.display()),
            "--change-id=0:0".to_string(),
        ]);
        args
    }

    fn merged_env(
        &self,
        extra_path_entries: &[String],
        env: BTreeMap<String, String>,
        env_remove: &BTreeSet<String>,
    ) -> BTreeMap<String, String> {
        let mut merged = self.base_env(extra_path_entries);
        merged.extend(env);
        for key in env_remove {
            merged.remove(key);
        }
        merged
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinuxCompatEnvStrategy {
    Clean,

    PreserveProcessEnv,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxCompatLaunchOptions {
    pub cwd: Option<PathBuf>,
    pub extra_path_entries: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub env_remove: BTreeSet<String>,
    pub env_strategy: LinuxCompatEnvStrategy,
}

impl Default for LinuxCompatLaunchOptions {
    fn default() -> Self {
        Self {
            cwd: None,
            extra_path_entries: Vec::new(),
            env: BTreeMap::new(),
            env_remove: BTreeSet::new(),
            env_strategy: LinuxCompatEnvStrategy::Clean,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessLaunchPlan {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}
