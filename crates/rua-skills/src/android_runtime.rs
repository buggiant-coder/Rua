use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchRequest, ToolchainIntent,
};
use rua_core::runtime_layout::{
    RuntimeKind, RuntimeLayout, LINUX_COMPAT_GUEST_HOME, MOBILE_SHARED_STORAGE_ANDROID,
    MOBILE_SHARED_STORAGE_PRIMARY,
};
use serde::{Deserialize, Serialize};

use crate::signature::is_ignored_skill_path;

const MAX_TEXT_BYTES: usize = 1024 * 1024;

const NATIVE_WHEEL_REQUIREMENTS: &[&str] = &[
    "ctranslate2",
    "faster-whisper",
    "llama-cpp-python",
    "onnxruntime",
    "tensorflow",
    "torch",
    "opencv-python",
    "faiss-cpu",
    "scipy",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidSkillRuntime {
    RequiresLinuxCompat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidRuntimeSignal {
    pub path: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidRuntimeScan {
    pub recommendation: AndroidSkillRuntime,
    pub signals: Vec<AndroidRuntimeSignal>,
    pub path_contract: SkillRuntimePathContract,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<AndroidRuntimeProbe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidRuntimeProbe {
    pub source: String,
    pub status: AndroidRuntimeProbeStatus,
    pub plans: Vec<AndroidRuntimeProbePlan>,
    pub diagnostics: Vec<String>,
    pub mobile_tool_profile: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidRuntimeProbeStatus {
    Planned,
    RuntimeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidRuntimeProbePlan {
    pub process_shape: ProcessShape,
    pub toolchain_intent: ToolchainIntent,
    pub runtime_kind: RuntimeKind,
    pub program: String,
    pub cwd: String,
    pub env_keys: Vec<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRuntimePathContract {
    pub skill_base_dir: String,
    pub aliases: std::collections::BTreeMap<String, String>,
    pub rules: Vec<String>,
    pub warnings: Vec<String>,
}

impl AndroidRuntimeScan {
    fn from_signals(skill_dir: &Path, signals: Vec<AndroidRuntimeSignal>) -> Self {
        Self {
            recommendation: AndroidSkillRuntime::RequiresLinuxCompat,
            signals,
            path_contract: skill_runtime_path_contract(skill_dir),
            probe: None,
        }
    }
}

pub fn scan_android_skill_runtime(skill_dir: &Path) -> io::Result<AndroidRuntimeScan> {
    let mut signals = Vec::new();
    scan_dir(skill_dir, skill_dir, &mut signals)?;
    Ok(AndroidRuntimeScan::from_signals(skill_dir, signals))
}

pub fn scan_android_skill_runtime_value(skill_dir: &Path) -> io::Result<serde_json::Value> {
    let scan = scan_android_skill_runtime(skill_dir)?;
    serde_json::to_value(scan).map_err(io::Error::other)
}

pub fn scan_android_skill_runtime_with_authority(
    skill_dir: &Path,
    authority: &RuntimeAuthority,
) -> io::Result<AndroidRuntimeScan> {
    let mut scan = scan_android_skill_runtime(skill_dir)?;
    let probe = plan_android_skill_runtime_probe(skill_dir, &scan.signals, authority);
    if probe
        .plans
        .iter()
        .any(|plan| plan.runtime_kind == RuntimeKind::LinuxCompat)
    {
        scan.recommendation = AndroidSkillRuntime::RequiresLinuxCompat;
    }
    scan.probe = Some(probe);
    Ok(scan)
}

pub fn scan_android_skill_runtime_value_with_authority(
    skill_dir: &Path,
    authority: &RuntimeAuthority,
) -> io::Result<serde_json::Value> {
    let scan = scan_android_skill_runtime_with_authority(skill_dir, authority)?;
    serde_json::to_value(scan).map_err(io::Error::other)
}

fn plan_android_skill_runtime_probe(
    skill_dir: &Path,
    signals: &[AndroidRuntimeSignal],
    authority: &RuntimeAuthority,
) -> AndroidRuntimeProbe {
    let intents = probe_intents(skill_dir, signals);
    let mut diagnostics = Vec::new();
    diagnostics.extend(authority.inventory().diagnostics.iter().cloned());

    let mut plans = Vec::new();
    for intent in intents {
        let request = RuntimeLaunchRequest {
            args: probe_args(&intent),
            cwd: Some(skill_dir.to_path_buf()),
            ..Default::default()
        };
        let plan = authority.plan(ProcessShape::SkillProbe, intent, request);
        diagnostics.extend(plan.diagnostics.iter().cloned());
        let env_keys = plan.process().map(launch_plan_env_keys).unwrap_or_default();
        plans.push(AndroidRuntimeProbePlan {
            process_shape: plan.process_shape,
            program: public_probe_program(&plan.toolchain_intent, plan.runtime_kind),
            toolchain_intent: plan.toolchain_intent,
            runtime_kind: plan.runtime_kind,
            cwd: "skill_base_dir".to_string(),
            env_keys,
            diagnostics: plan.diagnostics,
        });
    }

    diagnostics.sort();
    diagnostics.dedup();

    let status = if authority.inventory().default_external_runtime == RuntimeKind::LinuxCompat
        && !authority.inventory().linux_compat_available
    {
        AndroidRuntimeProbeStatus::RuntimeUnavailable
    } else {
        AndroidRuntimeProbeStatus::Planned
    };
    let mobile_tool_profile = mobile_tool_profile_diagnostics(authority, status);

    AndroidRuntimeProbe {
        source: "runtime_authority".to_string(),
        status,
        plans,
        diagnostics,
        mobile_tool_profile,
    }
}

fn launch_plan_env_keys(plan: &rua_core::runtime_layout::ProcessLaunchPlan) -> Vec<String> {
    let mut keys = plan.env.keys().cloned().collect::<Vec<_>>();
    keys.extend(plan.args.iter().filter_map(|arg| {
        let (key, _) = arg.split_once('=')?;
        (!key.is_empty()
            && key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
        .then(|| key.to_string())
    }));
    keys.sort();
    keys.dedup();
    keys
}
fn scan_dir(root: &Path, dir: &Path, signals: &mut Vec<AndroidRuntimeSignal>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if is_ignored_skill_path(rel) {
            continue;
        }
        let ty = entry.file_type()?;
        if ty.is_dir() {
            scan_dir(root, &path, signals)?;
        } else if ty.is_file() {
            scan_file(root, &path, signals)?;
        }
    }
    Ok(())
}

fn probe_intents(skill_dir: &Path, signals: &[AndroidRuntimeSignal]) -> Vec<ToolchainIntent> {
    let mut intents = Vec::new();
    let mut keys = BTreeSet::new();

    if skill_has_python_surface(skill_dir) {
        push_probe_intent(&mut intents, &mut keys, ToolchainIntent::Python);
    }
    if skill_has_node_surface(skill_dir) {
        push_probe_intent(&mut intents, &mut keys, ToolchainIntent::NodePackageManager);
    }
    if signals.iter().any(|signal| signal.kind == "linux_shebang") {
        push_probe_intent(
            &mut intents,
            &mut keys,
            ToolchainIntent::GenericCommand {
                command: "/bin/bash".to_string(),
            },
        );
    }
    if signals
        .iter()
        .any(|signal| signal.kind == "debian_package_manager")
    {
        push_probe_intent(
            &mut intents,
            &mut keys,
            ToolchainIntent::GenericCommand {
                command: "/usr/bin/apt-get".to_string(),
            },
        );
    }
    if intents.is_empty() {
        push_probe_intent(
            &mut intents,
            &mut keys,
            ToolchainIntent::GenericCommand {
                command: "/usr/bin/env".to_string(),
            },
        );
    }

    intents
}

fn push_probe_intent(
    intents: &mut Vec<ToolchainIntent>,
    keys: &mut BTreeSet<String>,
    intent: ToolchainIntent,
) {
    let key = match &intent {
        ToolchainIntent::Shell => "shell".to_string(),
        ToolchainIntent::Python => "python".to_string(),
        ToolchainIntent::Uv => "uv".to_string(),
        ToolchainIntent::Node => "node".to_string(),
        ToolchainIntent::Npm => "npm".to_string(),
        ToolchainIntent::Npx => "npx".to_string(),
        ToolchainIntent::NodeRunner { command } => format!("node_runner:{command}"),
        ToolchainIntent::NodePackageManager => "node_package_manager".to_string(),
        ToolchainIntent::GenericCommand { command } => format!("generic:{command}"),
    };
    if keys.insert(key) {
        intents.push(intent);
    }
}

fn probe_args(intent: &ToolchainIntent) -> Vec<String> {
    match intent {
        ToolchainIntent::Python => vec![
            "-c".to_string(),
            "import sys; print(sys.version_info[0])".to_string(),
        ],
        ToolchainIntent::Node | ToolchainIntent::Npm | ToolchainIntent::NodePackageManager => {
            vec!["--version".to_string()]
        }
        ToolchainIntent::Npx => vec!["--version".to_string()],
        ToolchainIntent::NodeRunner { .. } => vec!["--version".to_string()],
        ToolchainIntent::GenericCommand { command } if command == "/bin/bash" => {
            vec!["-lc".to_string(), "true".to_string()]
        }
        ToolchainIntent::GenericCommand { command } if command == "/usr/bin/apt-get" => {
            vec!["--version".to_string()]
        }
        ToolchainIntent::GenericCommand { .. } => Vec::new(),
        ToolchainIntent::Shell | ToolchainIntent::Uv => vec!["--version".to_string()],
    }
}

fn public_probe_program(intent: &ToolchainIntent, runtime_kind: RuntimeKind) -> String {
    if runtime_kind == RuntimeKind::LinuxCompat {
        return match intent {
            ToolchainIntent::Shell => "/bin/bash".to_string(),
            ToolchainIntent::Python => "/usr/bin/python3".to_string(),
            ToolchainIntent::Uv => "/usr/local/bin/uv".to_string(),
            ToolchainIntent::Node => "/usr/local/bin/node".to_string(),
            ToolchainIntent::Npm | ToolchainIntent::NodePackageManager => {
                "/usr/local/bin/npm".to_string()
            }
            ToolchainIntent::Npx => "/usr/local/bin/npx".to_string(),
            ToolchainIntent::NodeRunner { command }
            | ToolchainIntent::GenericCommand { command } => command.clone(),
        };
    }

    match intent {
        ToolchainIntent::Shell => "shell".to_string(),
        ToolchainIntent::Python => "python3".to_string(),
        ToolchainIntent::Uv => "uv".to_string(),
        ToolchainIntent::Node => "node".to_string(),
        ToolchainIntent::Npm | ToolchainIntent::NodePackageManager => "npm".to_string(),
        ToolchainIntent::Npx => "npx".to_string(),
        ToolchainIntent::NodeRunner { command } | ToolchainIntent::GenericCommand { command } => {
            command.clone()
        }
    }
}

fn mobile_tool_profile_diagnostics(
    authority: &RuntimeAuthority,
    status: AndroidRuntimeProbeStatus,
) -> Vec<String> {
    let inventory = authority.inventory();
    let mut diagnostics = vec![
        format!("resident_runtime={:?}", inventory.resident_runtime),
        format!(
            "default_external_runtime={:?}",
            inventory.default_external_runtime
        ),
        format!(
            "linux_compat_available={}",
            inventory.linux_compat_available
        ),
        format!("skill_probe_status={status:?}"),
    ];
    if inventory.default_external_runtime == RuntimeKind::LinuxCompat {
        diagnostics.push("Android skill probes use Linux Compatibility Runtime".to_string());
    }
    diagnostics
}

fn skill_has_python_surface(skill_dir: &Path) -> bool {
    skill_dir.join("requirements.txt").is_file()
        || skill_dir.join("requirements.in").is_file()
        || skill_dir.join("pyproject.toml").is_file()
        || skill_dir.join("setup.py").is_file()
        || dir_contains_extension(skill_dir, "py")
}

fn skill_has_node_surface(skill_dir: &Path) -> bool {
    skill_dir.join("package.json").is_file()
        || skill_dir.join("package-lock.json").is_file()
        || skill_dir.join("pnpm-lock.yaml").is_file()
        || skill_dir.join("yarn.lock").is_file()
}

fn dir_contains_extension(dir: &Path, extension: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path.strip_prefix(dir).unwrap_or(&path);
        if is_ignored_skill_path(rel) {
            continue;
        }
        if path.is_dir() {
            if dir_contains_extension(&path, extension) {
                return true;
            }
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
        {
            return true;
        }
    }
    false
}

fn scan_file(root: &Path, path: &Path, signals: &mut Vec<AndroidRuntimeSignal>) -> io::Result<()> {
    let bytes = fs::read(path)?;
    if bytes.contains(&0) {
        scan_binary_file(root, path, &bytes, signals);
        return Ok(());
    }

    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_TEXT_BYTES)]);
    let rel = rel_string(root, path);
    scan_text_file(&rel, &text, signals);
    Ok(())
}

fn scan_binary_file(
    root: &Path,
    path: &Path,
    bytes: &[u8],
    signals: &mut Vec<AndroidRuntimeSignal>,
) {
    let rel = rel_string(root, path);
    if bytes
        .windows(b"ld-linux-aarch64.so.1".len())
        .any(|w| w == b"ld-linux-aarch64.so.1")
    {
        signals.push(AndroidRuntimeSignal {
            path: rel,
            kind: "glibc_loader".into(),
            detail: "binary references ld-linux-aarch64.so.1".into(),
        });
    }
}

fn scan_text_file(path: &str, text: &str, signals: &mut Vec<AndroidRuntimeSignal>) {
    let lower = text.to_ascii_lowercase();

    if let Some(line) = text.lines().next().filter(|line| line.starts_with("#!")) {
        let line_lower = line.to_ascii_lowercase();
        if line_lower.contains("/bin/bash")
            || line_lower.contains("/usr/bin/python")
            || line_lower.contains("/usr/bin/env python")
        {
            signals.push(AndroidRuntimeSignal {
                path: path.into(),
                kind: "linux_shebang".into(),
                detail: line.trim().into(),
            });
        }
    }

    for token in ["apt", "apt-get", "dpkg"] {
        if lower.contains(&format!("/usr/bin/{token}")) || contains_shell_word(&lower, token) {
            signals.push(AndroidRuntimeSignal {
                path: path.into(),
                kind: "debian_package_manager".into(),
                detail: token.into(),
            });
            break;
        }
    }

    for token in [
        "/usr/lib",
        "/etc/",
        "/var/lib",
        "/lib/ld-linux-aarch64.so.1",
    ] {
        if lower.contains(token) {
            signals.push(AndroidRuntimeSignal {
                path: path.into(),
                kind: "fhs_path".into(),
                detail: token.into(),
            });
            break;
        }
    }

    if lower.contains("requires_linux_compat")
        || lower.contains("linux_compat")
        || lower.contains("linux compatibility runtime")
    {
        signals.push(AndroidRuntimeSignal {
            path: path.into(),
            kind: "runtime_metadata".into(),
            detail: "linux compatibility runtime requested".into(),
        });
    }

    if is_python_requirement_file(path) {
        for req in NATIVE_WHEEL_REQUIREMENTS {
            if requirement_mentions(&lower, req) {
                signals.push(AndroidRuntimeSignal {
                    path: path.into(),
                    kind: "native_python_dependency".into(),
                    detail: (*req).into(),
                });
            }
        }
    }
}

fn is_python_requirement_file(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.ends_with("requirements.txt")
        || path.ends_with("requirements.in")
        || path.ends_with("pyproject.toml")
        || path.ends_with("setup.py")
}

fn requirement_mentions(text: &str, package: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with('#') && line.starts_with(package)
    })
}

fn contains_shell_word(text: &str, word: &str) -> bool {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
        .any(|part| part == word)
}

fn skill_runtime_path_contract(skill_dir: &Path) -> SkillRuntimePathContract {
    let layout = RuntimeLayout::from_current_process(None);
    let mut aliases = std::collections::BTreeMap::new();
    aliases.insert(
        "skill_base_dir".to_string(),
        skill_dir.to_string_lossy().into_owned(),
    );

    let mut rules = vec![
        "Use skill_base_dir for files bundled with this skill.".to_string(),
        "Use runtime_info.agent_path_contract.workspace_dir for Rua workspace files.".to_string(),
    ];
    let mut warnings = Vec::new();

    if layout.host_profile.is_mobile() || layout.os_family == "android" {
        aliases.insert(
            "mobile_shared_storage".to_string(),
            MOBILE_SHARED_STORAGE_PRIMARY.to_string(),
        );
        aliases.insert(
            "mobile_shared_storage_alt".to_string(),
            MOBILE_SHARED_STORAGE_ANDROID.to_string(),
        );
        aliases.insert(
            "linux_guest_home".to_string(),
            LINUX_COMPAT_GUEST_HOME.to_string(),
        );
        if let Some(linux) = layout.linux_compat.as_ref() {
            aliases.insert(
                "app_home".to_string(),
                linux.host_home.to_string_lossy().into_owned(),
            );
        }
        rules.push("Use /sdcard/... for user-visible phone files on Android.".to_string());
        rules.push("Avoid ~ in skill script arguments on Android.".to_string());
        rules.push(
            "/root is only the Linux Compatibility Runtime guest HOME for external CLI config."
                .to_string(),
        );
        warnings.push(
            "/sdcard access depends on Android storage permission being granted.".to_string(),
        );
    }

    SkillRuntimePathContract {
        skill_base_dir: skill_dir.to_string_lossy().into_owned(),
        aliases,
        rules,
        warnings,
    }
}

fn rel_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .replace('\\', "/")
}
