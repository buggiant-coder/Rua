use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeAuthorityConfig, ToolchainIntent,
};
use rua_core::runtime_layout::{RuntimeKind, RuntimeLayout};
use rua_skills::{
    scan_android_skill_runtime, scan_android_skill_runtime_value,
    scan_android_skill_runtime_with_authority, AndroidRuntimeProbeStatus, AndroidSkillRuntime,
};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvRestore {
    rua_mobile: Option<OsString>,
}

impl EnvRestore {
    fn capture() -> Self {
        Self {
            rua_mobile: std::env::var_os("RUA_MOBILE"),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.rua_mobile {
            Some(value) => std::env::set_var("RUA_MOBILE", value),
            None => std::env::remove_var("RUA_MOBILE"),
        }
    }
}

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, b"").unwrap();
}

fn mobile_runtime_authority(native: &Path, workspace: &Path, available: bool) -> RuntimeAuthority {
    touch(&native.join("libproot.so"));
    if available {
        touch(
            &native
                .parent()
                .unwrap()
                .join("files/runtime/debian/usr/bin/env"),
        );
    }
    RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout::android(
            native,
            native.parent().unwrap().join("files"),
            native.parent().unwrap().join("files/home"),
        ),
        RuntimeAuthorityConfig {
            workspace_dir: workspace.to_path_buf(),
            cache_dir: native.parent().unwrap().join("files/home/.rua.cache"),
            path_entries: vec![native.to_path_buf()],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    )
}

#[test]
fn android_runtime_scan_defaults_to_linux_compat_runtime() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    std::fs::write(
        dir.path().join("SKILL.md"),
        "---\nname: demo\n---\nRun a portable helper.",
    )
    .unwrap();
    std::fs::write(dir.path().join("scripts/run.py"), "print('ok')\n").unwrap();

    let scan = scan_android_skill_runtime(dir.path()).unwrap();
    let value = scan_android_skill_runtime_value(dir.path()).unwrap();

    assert_eq!(
        scan.recommendation,
        AndroidSkillRuntime::RequiresLinuxCompat
    );
    assert!(scan.signals.is_empty());
    assert_eq!(
        scan.path_contract.aliases["skill_base_dir"],
        dir.path().to_string_lossy()
    );
    assert!(scan
        .path_contract
        .rules
        .iter()
        .any(|rule| rule.contains("skill_base_dir")));
    assert_eq!(
        value
            .pointer("/path_contract/aliases/skill_base_dir")
            .and_then(serde_json::Value::as_str),
        Some(dir.path().to_string_lossy().as_ref())
    );
    assert!(value.pointer("/probe").is_none());
}

#[test]
fn android_runtime_scan_routes_apt_to_linux_compat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("SKILL.md"),
        "---\nname: apt-demo\n---\nInstall with apt-get install poppler-utils.",
    )
    .unwrap();

    let scan = scan_android_skill_runtime(dir.path()).unwrap();

    assert_eq!(
        scan.recommendation,
        AndroidSkillRuntime::RequiresLinuxCompat
    );
    assert!(scan
        .signals
        .iter()
        .any(|s| s.kind == "debian_package_manager"));
}

#[test]
fn android_runtime_scan_routes_linux_shebang_to_linux_compat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    std::fs::write(dir.path().join("scripts/run.sh"), "#!/bin/bash\necho ok\n").unwrap();

    let scan = scan_android_skill_runtime(dir.path()).unwrap();

    assert_eq!(
        scan.recommendation,
        AndroidSkillRuntime::RequiresLinuxCompat
    );
    assert!(scan.signals.iter().any(|s| s.kind == "linux_shebang"));
}

#[test]
fn android_runtime_scan_routes_glibc_binary_to_linux_compat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("helper"),
        b"\0ELF....../lib/ld-linux-aarch64.so.1\0",
    )
    .unwrap();

    let scan = scan_android_skill_runtime(dir.path()).unwrap();

    assert_eq!(
        scan.recommendation,
        AndroidSkillRuntime::RequiresLinuxCompat
    );
    assert!(scan.signals.iter().any(|s| s.kind == "glibc_loader"));
}

#[test]
fn android_runtime_scan_routes_hard_native_python_requirement_to_linux_compat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("requirements.txt"),
        "requests\nfaster-whisper==1.0\n",
    )
    .unwrap();

    let scan = scan_android_skill_runtime(dir.path()).unwrap();

    assert_eq!(
        scan.recommendation,
        AndroidSkillRuntime::RequiresLinuxCompat
    );
    assert!(scan
        .signals
        .iter()
        .any(|s| s.kind == "native_python_dependency" && s.detail == "faster-whisper"));
}

#[test]
fn android_runtime_scan_reports_mobile_agent_path_contract() {
    let _lock = env_lock().lock().expect("env lock");
    let _restore = EnvRestore::capture();
    std::env::set_var("RUA_MOBILE", "1");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("SKILL.md"), "---\nname: mobile\n---\n").unwrap();

    let scan = scan_android_skill_runtime(dir.path()).unwrap();

    assert_eq!(
        scan.path_contract.aliases["mobile_shared_storage"],
        "/sdcard"
    );
    assert_eq!(scan.path_contract.aliases["linux_guest_home"], "/root");
    assert!(scan
        .path_contract
        .rules
        .iter()
        .any(|rule| rule.contains("runtime_info")));
    assert!(scan
        .path_contract
        .rules
        .iter()
        .any(|rule| rule.contains("Avoid ~")));
}

#[test]
fn static_scan_only_keeps_probe_absent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("SKILL.md"), "---\nname: fallback\n---\n").unwrap();

    let scan = scan_android_skill_runtime(dir.path()).unwrap();
    let value = scan_android_skill_runtime_value(dir.path()).unwrap();

    assert_eq!(
        scan.recommendation,
        AndroidSkillRuntime::RequiresLinuxCompat
    );
    assert!(scan.probe.is_none());
    assert!(value.pointer("/probe").is_none());
}

#[test]
fn runtime_authority_probe_plans_android_skills_in_linux_compat() {
    let temp = tempfile::tempdir().unwrap();
    let native = temp.path().join("native");
    let workspace = temp.path().join("files/home/.rua/workspaces/default");
    let authority = mobile_runtime_authority(&native, &workspace, true);
    let skill = temp.path().join("skill");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("requirements.txt"), "requests\n").unwrap();
    fs::write(skill.join("scripts.py"), "print('ok')\n").unwrap();

    let scan = scan_android_skill_runtime_with_authority(&skill, &authority).unwrap();
    let probe = scan.probe.expect("runtime authority probe");

    assert_eq!(
        scan.recommendation,
        AndroidSkillRuntime::RequiresLinuxCompat
    );
    assert_eq!(probe.source, "runtime_authority");
    assert_eq!(probe.status, AndroidRuntimeProbeStatus::Planned);
    assert!(probe
        .mobile_tool_profile
        .iter()
        .any(|entry| entry == "default_external_runtime=LinuxCompat"));
    let plan = probe
        .plans
        .iter()
        .find(|plan| matches!(plan.toolchain_intent, ToolchainIntent::Python))
        .expect("python probe plan");
    assert_eq!(plan.process_shape, ProcessShape::SkillProbe);
    assert_eq!(plan.runtime_kind, RuntimeKind::LinuxCompat);
    assert_eq!(plan.program, "/usr/bin/python3");
    assert_eq!(plan.cwd, "skill_base_dir");
}

#[test]
fn runtime_authority_probe_reports_unavailable_linux_compat_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let native = temp.path().join("native");
    let workspace = temp.path().join("files/home/.rua/workspaces/default");
    let authority = mobile_runtime_authority(&native, &workspace, false);
    let skill = temp.path().join("skill");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("package.json"), "{}\n").unwrap();

    let scan = scan_android_skill_runtime_with_authority(&skill, &authority).unwrap();
    let probe = scan.probe.expect("runtime authority probe");

    assert_eq!(probe.status, AndroidRuntimeProbeStatus::RuntimeUnavailable);
    assert!(probe
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("Linux Compatibility Runtime is not available")));
    let plan = probe
        .plans
        .iter()
        .find(|plan| matches!(plan.toolchain_intent, ToolchainIntent::NodePackageManager))
        .expect("npm probe plan");
    assert_eq!(plan.runtime_kind, RuntimeKind::LinuxCompat);
    assert_eq!(plan.program, "/usr/local/bin/npm");
    assert!(
        plan.env_keys.is_empty(),
        "unavailable plans must not expose executable material"
    );
}
