use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::runtime_layout::{
    AgentPathContract, LinuxCompatEnvStrategy, LinuxCompatLaunchOptions, NativeShellKind,
    NativeShellSpec, ProcessLaunchPlan, RuntimeKind, RuntimeLayout,
};

pub mod browser;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessShape {
    Shell,
    Interactive,
    Stdio,
    Direct,
    Install,
    SkillProbe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolchainIntent {
    Shell,
    Python,
    Uv,
    Node,
    Npm,
    Npx,
    NodeRunner { command: String },
    NodePackageManager,
    GenericCommand { command: String },
}

impl ToolchainIntent {
    fn command_name(&self) -> &str {
        match self {
            Self::Shell => "",
            Self::Python => "python3",
            Self::Uv => "uv",
            Self::Node => "node",
            Self::Npm | Self::NodePackageManager => "npm",
            Self::Npx => "npx",
            Self::NodeRunner { command } | Self::GenericCommand { command } => command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCapability {
    pub name: String,
    pub available: bool,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub source: RuntimeCapabilitySource,
    pub diagnostics: Vec<String>,
}

impl RuntimeCapability {
    fn available(name: &str, path: PathBuf, source: RuntimeCapabilitySource) -> Self {
        Self {
            name: name.to_string(),
            available: true,
            path: Some(path),
            version: None,
            source,
            diagnostics: Vec::new(),
        }
    }

    fn unavailable(name: &str, diagnostic: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            available: false,
            path: None,
            version: None,
            source: RuntimeCapabilitySource::Unavailable,
            diagnostics: vec![diagnostic.into()],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCapabilitySource {
    Configured,
    Bundled,
    SystemPath,
    RuntimeLayout,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCachePolicy {
    pub npm_cache_dir: PathBuf,
    pub tmp_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeResourceLimits {
    pub address_space_bytes: u64,
    pub max_processes: u64,
    pub max_file_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInventory {
    pub resident_runtime: RuntimeKind,
    pub default_external_runtime: RuntimeKind,
    pub host_profile: String,
    pub os_family: String,
    pub shell: NativeShellSpec,
    pub python: RuntimeCapability,
    pub uv: RuntimeCapability,
    pub node: RuntimeCapability,
    pub npm: RuntimeCapability,
    pub npx: RuntimeCapability,
    pub git: RuntimeCapability,
    pub rg: RuntimeCapability,
    pub ffmpeg: RuntimeCapability,
    pub linux_compat_available: bool,
    pub cache_policy: RuntimeCachePolicy,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAuthority {
    layout: RuntimeLayout,
    inventory: RuntimeInventory,
    workspace_dir: PathBuf,
    path_entries: Vec<PathBuf>,
    pathext: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAuthorityConfig {
    pub workspace_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub path_entries: Vec<PathBuf>,
    pub pathext: Vec<String>,
    pub windows_shell_preference: Option<OsString>,
}

impl RuntimeAuthorityConfig {
    pub fn current(workspace_dir: impl Into<PathBuf>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            cache_dir: cache_dir.into(),
            path_entries: env::var_os("PATH")
                .map(|paths| env::split_paths(&paths).collect())
                .unwrap_or_default(),
            pathext: env::var("PATHEXT")
                .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
                .split(';')
                .filter(|ext| !ext.trim().is_empty())
                .map(str::to_string)
                .collect(),
            windows_shell_preference: env::var_os("RUA_WINDOWS_SHELL"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeLaunchRequest {
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub env_remove: BTreeSet<String>,
    pub cwd: Option<PathBuf>,
    pub resource_limits: Option<RuntimeResourceLimits>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLaunchPlan {
    pub process_shape: ProcessShape,
    pub toolchain_intent: ToolchainIntent,
    pub runtime_kind: RuntimeKind,
    process: ProcessLaunchPlan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_resource_limits: Option<RuntimeResourceLimits>,
    pub cache_policy: RuntimeCachePolicy,
    pub agent_path_contract: AgentPathContract,
    pub diagnostics: Vec<String>,
}

impl RuntimeLaunchPlan {
    /// The only public access to executable material. Fatal diagnostics cannot be ignored.
    pub fn process(&self) -> crate::Result<&ProcessLaunchPlan> {
        if !self.diagnostics.is_empty() {
            return Err(crate::RuaError::Runtime {
                message: self.diagnostics.join("; "),
            });
        }
        Ok(&self.process)
    }

    pub fn into_process(self) -> crate::Result<ProcessLaunchPlan> {
        self.process()?;
        Ok(self.process)
    }
}

impl RuntimeAuthority {
    pub fn from_current_process(
        host_profile: Option<&str>,
        workspace_dir: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        let workspace_dir = workspace_dir.into();
        let cache_dir = cache_dir.into();
        let layout = RuntimeLayout::from_current_process(host_profile);
        let config = RuntimeAuthorityConfig::current(&workspace_dir, &cache_dir);
        Self::from_layout_with_probe(layout, config, |path| path.is_file())
    }

    pub fn from_layout_with_probe<F>(
        layout: RuntimeLayout,
        config: RuntimeAuthorityConfig,
        is_file: F,
    ) -> Self
    where
        F: Fn(&Path) -> bool,
    {
        let inventory = build_inventory(&layout, &config, &is_file);
        Self {
            layout,
            workspace_dir: config.workspace_dir,
            path_entries: config.path_entries,
            pathext: config.pathext,
            inventory,
        }
    }

    pub fn inventory(&self) -> &RuntimeInventory {
        &self.inventory
    }

    pub fn plan(
        &self,
        process_shape: ProcessShape,
        toolchain_intent: ToolchainIntent,
        request: RuntimeLaunchRequest,
    ) -> RuntimeLaunchPlan {
        let runtime_kind = self.runtime_kind_for();
        let mut diagnostics = Vec::new();
        let (process, host_resource_limits) = match runtime_kind {
            RuntimeKind::Native => (
                self.native_launch_plan(
                    process_shape,
                    &toolchain_intent,
                    &request,
                    &mut diagnostics,
                ),
                request.resource_limits,
            ),
            RuntimeKind::LinuxCompat => (
                self.linux_compat_launch_plan(
                    process_shape,
                    &toolchain_intent,
                    &request,
                    &mut diagnostics,
                ),
                None,
            ),
        };

        RuntimeLaunchPlan {
            process_shape,
            toolchain_intent,
            runtime_kind,
            process,
            host_resource_limits,
            cache_policy: self.inventory.cache_policy.clone(),
            agent_path_contract: self.layout.agent_path_contract(&self.workspace_dir),
            diagnostics,
        }
    }

    fn runtime_kind_for(&self) -> RuntimeKind {
        if self.layout.host_profile.is_mobile() || self.layout.os_family == "android" {
            RuntimeKind::LinuxCompat
        } else {
            RuntimeKind::Native
        }
    }

    fn native_launch_plan(
        &self,
        process_shape: ProcessShape,
        intent: &ToolchainIntent,
        request: &RuntimeLaunchRequest,
        diagnostics: &mut Vec<String>,
    ) -> ProcessLaunchPlan {
        if process_shape == ProcessShape::Shell && matches!(intent, ToolchainIntent::Shell) {
            let mut env = request.env.clone();
            for key in &request.env_remove {
                env.remove(key);
            }
            let mut args = self.inventory.shell.args.clone();
            args.extend(request.args.iter().cloned());
            return ProcessLaunchPlan {
                command: PathBuf::from(&self.inventory.shell.program),
                args,
                env,
                cwd: request.cwd.clone(),
            };
        }

        let (command, available) = if matches!(intent, ToolchainIntent::Shell) {
            (PathBuf::from(&self.inventory.shell.program), true)
        } else if matches!(
            intent,
            ToolchainIntent::GenericCommand { .. } | ToolchainIntent::NodeRunner { .. }
        ) {
            let name = intent.command_name();
            let path = Path::new(name);
            let candidate = if path.is_relative() && path.components().count() > 1 {
                request
                    .cwd
                    .as_deref()
                    .unwrap_or(&self.workspace_dir)
                    .join(path)
            } else {
                PathBuf::from(name)
            };
            let resolved = find_command_on_path(
                &candidate.to_string_lossy(),
                &self.path_entries,
                &self.pathext,
                &|path| path.is_file(),
            );
            let available = resolved.is_some();
            (resolved.unwrap_or(candidate), available)
        } else {
            let command_name = intent.command_name();
            (
                self.native_capability_path(command_name)
                    .unwrap_or_else(|| PathBuf::from(command_name)),
                self.native_capability_available(command_name),
            )
        };
        if !available {
            diagnostics.push(format!(
                "{} was not found in Runtime Inventory",
                intent.command_name()
            ));
        }
        let (command, args) = wrap_windows_batch_command(command, request.args.clone());
        let mut env = request.env.clone();
        apply_cache_env(intent, &self.inventory.cache_policy, &mut env);
        for key in &request.env_remove {
            env.remove(key);
        }

        ProcessLaunchPlan {
            command,
            args,
            env,
            cwd: request.cwd.clone(),
        }
    }

    fn linux_compat_launch_plan(
        &self,
        process_shape: ProcessShape,
        intent: &ToolchainIntent,
        request: &RuntimeLaunchRequest,
        diagnostics: &mut Vec<String>,
    ) -> ProcessLaunchPlan {
        let Some(layout) = &self.layout.linux_compat else {
            diagnostics.push("Linux Compatibility Runtime is not configured".to_string());
            return missing_runtime_plan(intent.command_name(), request);
        };
        if !layout.is_available() {
            diagnostics.push("Linux Compatibility Runtime is not available".to_string());
        }

        if matches!(
            intent,
            ToolchainIntent::NodeRunner { .. }
                | ToolchainIntent::Npm
                | ToolchainIntent::Npx
                | ToolchainIntent::NodePackageManager
        ) && !self.inventory.node.available
        {
            diagnostics.push("node was not found in Linux Compatibility Runtime".to_string());
        }
        let program = linux_compat_program(intent);
        if !matches!(
            intent,
            ToolchainIntent::NodeRunner { .. } | ToolchainIntent::GenericCommand { .. }
        ) && Path::new(&program).is_absolute()
            && !layout
                .rootfs_path
                .join(program.trim_start_matches('/'))
                .is_file()
        {
            diagnostics.push(format!(
                "{program} was not found in Linux Compatibility Runtime"
            ));
        }
        let mut args = request.args.clone();
        if process_shape == ProcessShape::Shell && matches!(intent, ToolchainIntent::Shell) {
            args.insert(0, "-lc".to_string());
        }
        let (program, args) =
            wrap_linux_compat_resource_limits(program, args, request.resource_limits);
        if request.resource_limits.is_some()
            && !layout.rootfs_path.join("usr/bin/prlimit").is_file()
        {
            diagnostics.push(
                "Linux Compatibility Runtime is missing /usr/bin/prlimit required for resource limits"
                    .to_string(),
            );
        }
        let mut env = request.env.clone();
        apply_cache_env(intent, &self.inventory.cache_policy, &mut env);
        let env_strategy = match process_shape {
            ProcessShape::Shell
            | ProcessShape::Interactive
            | ProcessShape::Direct
            | ProcessShape::SkillProbe => LinuxCompatEnvStrategy::Clean,
            ProcessShape::Stdio | ProcessShape::Install => {
                LinuxCompatEnvStrategy::PreserveProcessEnv
            }
        };
        let cwd = request
            .cwd
            .clone()
            .or_else(|| {
                matches!(process_shape, ProcessShape::Stdio | ProcessShape::Install)
                    .then(|| self.workspace_dir.clone())
            })
            .map(|cwd| {
                if cwd.is_absolute() {
                    cwd
                } else {
                    self.workspace_dir.join(cwd)
                }
            });
        if let Some(cwd) = &cwd {
            let host = if layout.binds.iter().any(|bind| cwd.starts_with(&bind.host)) {
                cwd.clone()
            } else if let Some(bind) = layout
                .binds
                .iter()
                .find(|bind| cwd.starts_with(&bind.guest))
            {
                bind.host
                    .join(cwd.strip_prefix(&bind.guest).expect("matched bind"))
            } else {
                layout
                    .rootfs_path
                    .join(cwd.strip_prefix("/").unwrap_or(cwd))
            };
            if !host.is_dir() {
                diagnostics.push(format!(
                    "Working directory is unavailable in Linux Compatibility Runtime: {}",
                    cwd.display()
                ));
            }
        }
        let mut plan = layout.launch_plan(
            program,
            LinuxCompatLaunchOptions {
                cwd,
                extra_path_entries: Vec::new(),
                env,
                env_remove: request.env_remove.clone(),
                env_strategy,
            },
        );
        plan.args.extend(args);
        plan
    }

    fn native_capability_path(&self, command: &str) -> Option<PathBuf> {
        self.capability(command)
            .and_then(|capability| capability.path.clone())
    }

    fn native_capability_available(&self, command: &str) -> bool {
        self.capability(command)
            .map(|capability| capability.available)
            .unwrap_or(false)
    }

    fn capability(&self, command: &str) -> Option<&RuntimeCapability> {
        match command {
            "python" | "python3" => Some(&self.inventory.python),
            "uv" => Some(&self.inventory.uv),
            "node" => Some(&self.inventory.node),
            "npm" => Some(&self.inventory.npm),
            "npx" => Some(&self.inventory.npx),
            _ => None,
        }
    }
}

fn apply_cache_env(
    intent: &ToolchainIntent,
    cache_policy: &RuntimeCachePolicy,
    env: &mut BTreeMap<String, String>,
) {
    if is_node_toolchain_intent(intent) {
        env.entry("NPM_CONFIG_CACHE".to_string())
            .or_insert_with(|| cache_policy.npm_cache_dir.to_string_lossy().into_owned());
        env.entry("npm_config_cache".to_string())
            .or_insert_with(|| cache_policy.npm_cache_dir.to_string_lossy().into_owned());
    }
}

fn is_node_toolchain_intent(intent: &ToolchainIntent) -> bool {
    matches!(
        intent,
        ToolchainIntent::Node
            | ToolchainIntent::Npm
            | ToolchainIntent::Npx
            | ToolchainIntent::NodePackageManager
            | ToolchainIntent::NodeRunner { .. }
    )
}

fn build_inventory<F>(
    layout: &RuntimeLayout,
    config: &RuntimeAuthorityConfig,
    is_file: &F,
) -> RuntimeInventory
where
    F: Fn(&Path) -> bool,
{
    let linux_compat_available = layout
        .linux_compat
        .as_ref()
        .is_some_and(|linux| linux.is_available());
    let default_external_runtime =
        if layout.host_profile.is_mobile() || layout.os_family == "android" {
            RuntimeKind::LinuxCompat
        } else {
            RuntimeKind::Native
        };
    let path_env = join_path_entries(&config.path_entries);
    let shell = if default_external_runtime == RuntimeKind::LinuxCompat {
        NativeShellSpec {
            program: "/bin/bash".to_string(),
            args: vec!["-lc".to_string()],
            kind: NativeShellKind::Unix,
        }
    } else if layout.os_family == "windows" {
        RuntimeLayout::native_shell_for_windows(
            config.windows_shell_preference.as_deref(),
            Some(path_env.as_os_str()),
        )
    } else {
        RuntimeLayout::native_shell_for_unix()
    };
    let cache_policy = RuntimeCachePolicy {
        npm_cache_dir: config.cache_dir.join("npm"),
        tmp_dir: config.cache_dir.join("tmp"),
    };
    let mut diagnostics = Vec::new();
    if default_external_runtime == RuntimeKind::LinuxCompat && !linux_compat_available {
        diagnostics.push(
            "Android external toolchains default to Linux Compatibility Runtime, but it is unavailable"
                .to_string(),
        );
    }
    let (python, uv, node, npm, npx, git, rg, ffmpeg) = if default_external_runtime
        == RuntimeKind::LinuxCompat
    {
        match layout.linux_compat.as_ref() {
            Some(linux) => (
                linux_compat_capability("python3", "/usr/bin/python3", linux, is_file),
                linux_compat_capability("uv", "/usr/local/bin/uv", linux, is_file),
                linux_compat_capability("node", "/usr/local/bin/node", linux, is_file),
                linux_compat_capability("npm", "/usr/local/bin/npm", linux, is_file),
                linux_compat_capability("npx", "/usr/local/bin/npx", linux, is_file),
                linux_compat_capability("git", "/usr/bin/git", linux, is_file),
                linux_compat_capability("rg", "/usr/bin/rg", linux, is_file),
                linux_compat_capability("ffmpeg", "/usr/bin/ffmpeg", linux, is_file),
            ),
            None => (
                RuntimeCapability::unavailable("python3", "Linux Compatibility Runtime missing"),
                RuntimeCapability::unavailable("uv", "Linux Compatibility Runtime missing"),
                RuntimeCapability::unavailable("node", "Linux Compatibility Runtime missing"),
                RuntimeCapability::unavailable("npm", "Linux Compatibility Runtime missing"),
                RuntimeCapability::unavailable("npx", "Linux Compatibility Runtime missing"),
                RuntimeCapability::unavailable("git", "Linux Compatibility Runtime missing"),
                RuntimeCapability::unavailable("rg", "Linux Compatibility Runtime missing"),
                RuntimeCapability::unavailable("ffmpeg", "Linux Compatibility Runtime missing"),
            ),
        }
    } else {
        (
            command_capability_first(&["python3", "python"], config, is_file),
            command_capability("uv", config, is_file),
            command_capability("node", config, is_file),
            command_capability("npm", config, is_file),
            command_capability("npx", config, is_file),
            command_capability("git", config, is_file),
            command_capability("rg", config, is_file),
            command_capability("ffmpeg", config, is_file),
        )
    };

    RuntimeInventory {
        resident_runtime: RuntimeKind::Native,
        default_external_runtime,
        host_profile: layout.host_profile.as_str().to_string(),
        os_family: layout.os_family.clone(),
        shell,
        python,
        uv,
        node,
        npm,
        npx,
        git,
        rg,
        ffmpeg,
        linux_compat_available,
        cache_policy,
        diagnostics,
    }
}

fn linux_compat_capability<F>(
    name: &str,
    guest_path: &str,
    layout: &crate::runtime_layout::LinuxCompatLayout,
    is_file: &F,
) -> RuntimeCapability
where
    F: Fn(&Path) -> bool,
{
    let host_path = layout.rootfs_path.join(guest_path.trim_start_matches('/'));
    if is_file(&host_path) {
        let mut capability = RuntimeCapability::available(
            name,
            PathBuf::from(guest_path),
            RuntimeCapabilitySource::RuntimeLayout,
        );
        // The installer records versions only after executing the pinned guest binaries.
        // Follow the command's actual target, so an unrelated user replacement cannot inherit it.
        let tools = layout.rootfs_path.join("usr/local/lib/rua/toolchains");
        if let (Ok(target), Ok(tools)) = (host_path.canonicalize(), tools.canonicalize()) {
            for parent in target
                .ancestors()
                .skip(1)
                .take_while(|p| p.starts_with(&tools))
            {
                let marker = parent.join(".rua-versions.json");
                if let Ok(content) = std::fs::read_to_string(marker) {
                    capability.version = serde_json::from_str::<serde_json::Value>(&content)
                        .ok()
                        .and_then(|versions| versions.get(name)?.as_str().map(str::to_owned));
                    break;
                }
            }
        }
        capability
    } else {
        RuntimeCapability::unavailable(
            name,
            format!("{guest_path} not found in Linux Compatibility Runtime"),
        )
    }
}

fn command_capability_first<F>(
    names: &[&str],
    config: &RuntimeAuthorityConfig,
    is_file: &F,
) -> RuntimeCapability
where
    F: Fn(&Path) -> bool,
{
    for name in names {
        let capability = command_capability(name, config, is_file);
        if capability.available {
            return capability;
        }
    }
    RuntimeCapability::unavailable(names[0], format!("{} not found on PATH", names.join("/")))
}

fn command_capability<F>(
    name: &str,
    config: &RuntimeAuthorityConfig,
    is_file: &F,
) -> RuntimeCapability
where
    F: Fn(&Path) -> bool,
{
    match find_command_on_path(name, &config.path_entries, &config.pathext, is_file) {
        Some(path) => RuntimeCapability::available(name, path, RuntimeCapabilitySource::SystemPath),
        None => RuntimeCapability::unavailable(name, format!("{name} not found on PATH")),
    }
}

fn find_command_on_path<F>(
    command: &str,
    path_entries: &[PathBuf],
    pathext: &[String],
    is_file: &F,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    let command_path = Path::new(command);
    if command_path.components().count() > 1 && is_file(command_path) {
        return Some(command_path.to_path_buf());
    }

    for dir in path_entries {
        for name in command_file_names(command, pathext) {
            let candidate = dir.join(name);
            if is_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn command_file_names(command: &str, pathext: &[String]) -> Vec<String> {
    let mut names = vec![command.to_string()];
    if (cfg!(windows) || !pathext.is_empty()) && Path::new(command).extension().is_none() {
        names.extend(
            pathext
                .iter()
                .filter(|ext| !ext.trim().is_empty())
                .map(|ext| {
                    if ext.starts_with('.') {
                        format!("{command}{ext}")
                    } else {
                        format!("{command}.{ext}")
                    }
                }),
        );
    }
    names
}

fn linux_compat_program(intent: &ToolchainIntent) -> String {
    match intent {
        ToolchainIntent::Shell => "/bin/bash".to_string(),
        ToolchainIntent::Python => "/usr/bin/python3".to_string(),
        ToolchainIntent::Uv => "/usr/local/bin/uv".to_string(),
        ToolchainIntent::Node => "/usr/local/bin/node".to_string(),
        ToolchainIntent::Npm | ToolchainIntent::NodePackageManager => {
            "/usr/local/bin/npm".to_string()
        }
        ToolchainIntent::Npx => "/usr/local/bin/npx".to_string(),
        ToolchainIntent::NodeRunner { command } | ToolchainIntent::GenericCommand { command } => {
            command.clone()
        }
    }
}

fn wrap_linux_compat_resource_limits(
    program: String,
    args: Vec<String>,
    limits: Option<RuntimeResourceLimits>,
) -> (String, Vec<String>) {
    let Some(limits) = limits else {
        return (program, args);
    };

    let mut wrapped_args = vec![
        format!("--as={}", limits.address_space_bytes),
        format!("--nproc={}", limits.max_processes),
        format!("--fsize={}", limits.max_file_size_bytes),
        "--".to_string(),
        program,
    ];
    wrapped_args.extend(args);
    ("/usr/bin/prlimit".to_string(), wrapped_args)
}

fn missing_runtime_plan(command: &str, request: &RuntimeLaunchRequest) -> ProcessLaunchPlan {
    let mut env = request.env.clone();
    for key in &request.env_remove {
        env.remove(key);
    }
    ProcessLaunchPlan {
        command: PathBuf::from(command),
        args: request.args.clone(),
        env,
        cwd: request.cwd.clone(),
    }
}

fn wrap_windows_batch_command(command: PathBuf, args: Vec<String>) -> (PathBuf, Vec<String>) {
    let extension = command
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    if !matches!(extension.as_deref(), Some("cmd" | "bat")) {
        return (command, args);
    }

    let mut wrapped_args = vec![
        "/D".to_string(),
        "/C".to_string(),
        command.to_string_lossy().into_owned(),
    ];
    wrapped_args.extend(args);
    (PathBuf::from("cmd.exe"), wrapped_args)
}

fn join_path_entries(entries: &[PathBuf]) -> OsString {
    env::join_paths(entries).unwrap_or_default()
}
