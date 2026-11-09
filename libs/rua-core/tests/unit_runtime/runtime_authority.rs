use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeAuthorityConfig, RuntimeLaunchRequest,
    RuntimeResourceLimits, ToolchainIntent,
};
use rua_core::runtime_layout::{HostProfile, LinuxCompatLayout, RuntimeKind, RuntimeLayout};

fn touch(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, b"").expect("write");
}

fn desktop_layout(os_family: &str) -> RuntimeLayout {
    RuntimeLayout {
        host_profile: HostProfile::Desktop,
        os_family: os_family.to_string(),
        linux_compat: None,
    }
}

#[test]
fn desktop_inventory_and_node_install_plan_use_selected_native_toolchain() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("bin");
    let node = bin.join("node");
    let npm = bin.join("npm");
    let npx = bin.join("npx");
    let python = bin.join("python3");
    for path in [&node, &npm, &npx, &python] {
        touch(path);
    }

    let authority = RuntimeAuthority::from_layout_with_probe(
        desktop_layout("macos"),
        RuntimeAuthorityConfig {
            workspace_dir: temp.path().join("workspace"),
            cache_dir: temp.path().join("cache"),
            path_entries: vec![bin],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    let inventory = authority.inventory();
    assert_eq!(inventory.resident_runtime, RuntimeKind::Native);
    assert_eq!(inventory.default_external_runtime, RuntimeKind::Native);
    assert_eq!(inventory.node.path.as_deref(), Some(node.as_path()));
    assert_eq!(inventory.npm.path.as_deref(), Some(npm.as_path()));
    assert_eq!(inventory.npx.path.as_deref(), Some(npx.as_path()));
    assert!(inventory.cache_policy.npm_cache_dir.ends_with("npm"));

    let plan = authority.plan(
        ProcessShape::Install,
        ToolchainIntent::NodePackageManager,
        RuntimeLaunchRequest {
            args: vec!["install".into(), "-g".into(), "pkg".into()],
            ..Default::default()
        },
    );
    assert_eq!(plan.runtime_kind, RuntimeKind::Native);
    assert_eq!(plan.process().expect("valid launch").command, npm);
    assert_eq!(
        plan.process().expect("valid launch").args,
        vec!["install", "-g", "pkg"]
    );
    assert_eq!(
        plan.process().expect("valid launch").env["NPM_CONFIG_CACHE"],
        temp.path().join("cache/npm").to_string_lossy()
    );
}

#[test]
fn windows_direct_plan_wraps_cmd_shims_and_shares_shell_profile() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("bin");
    let npm = bin.join("npm.CMD");
    let bash = bin.join("bash.exe");
    touch(&npm);
    touch(&bash);

    let authority = RuntimeAuthority::from_layout_with_probe(
        desktop_layout("windows"),
        RuntimeAuthorityConfig {
            workspace_dir: temp.path().join("workspace"),
            cache_dir: temp.path().join("cache"),
            path_entries: vec![bin.clone()],
            pathext: vec![".EXE".into(), ".CMD".into(), ".BAT".into()],
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    assert_eq!(authority.inventory().shell.program, bash.to_string_lossy());
    assert_eq!(authority.inventory().shell.contract_kind(), "git-bash");

    let plan = authority.plan(
        ProcessShape::Direct,
        ToolchainIntent::Npm,
        RuntimeLaunchRequest {
            args: vec!["--version".into()],
            ..Default::default()
        },
    );
    assert_eq!(plan.runtime_kind, RuntimeKind::Native);
    assert_eq!(
        plan.process().expect("valid launch").command,
        PathBuf::from("cmd.exe")
    );
    assert_eq!(
        plan.process().expect("valid launch").args[0..3],
        ["/D", "/C", &npm.to_string_lossy()]
    );
    assert_eq!(plan.process().expect("valid launch").args[3], "--version");
}

#[test]
fn android_external_toolchain_defaults_to_linux_compat_runtime() {
    let temp = tempfile::tempdir().expect("tempdir");
    let native = temp.path().join("native");
    touch(&native.join("libproot.so"));
    let rootfs = native.parent().unwrap().join("files/runtime/debian");
    for path in [
        "usr/bin/env",
        "usr/bin/python3",
        "usr/local/bin/uv",
        "usr/local/bin/node",
        "usr/local/bin/npm",
        "usr/local/bin/npx",
        "usr/bin/git",
        "usr/bin/rg",
        "usr/bin/ffmpeg",
    ] {
        touch(&rootfs.join(path));
    }
    let workspace = temp.path().join("files/home/.rua/workspaces/default");
    fs::create_dir_all(&workspace).unwrap();
    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout::android(
            &native,
            native.parent().unwrap().join("files"),
            native.parent().unwrap().join("files/home"),
        ),
        RuntimeAuthorityConfig {
            workspace_dir: workspace.clone(),
            cache_dir: temp.path().join("files/home/.rua.cache"),
            path_entries: vec![native.clone()],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    let inventory = authority.inventory();
    assert_eq!(inventory.resident_runtime, RuntimeKind::Native);
    assert_eq!(inventory.default_external_runtime, RuntimeKind::LinuxCompat);
    assert!(inventory.linux_compat_available);
    assert_eq!(inventory.shell.program, "/bin/bash");
    assert_eq!(
        inventory.python.path.as_deref(),
        Some(std::path::Path::new("/usr/bin/python3"))
    );
    assert_eq!(
        inventory.node.path.as_deref(),
        Some(std::path::Path::new("/usr/local/bin/node"))
    );
    assert_eq!(
        inventory.git.path.as_deref(),
        Some(std::path::Path::new("/usr/bin/git"))
    );

    let plan = authority.plan(
        ProcessShape::Stdio,
        ToolchainIntent::NodeRunner {
            command: "claude-agent-acp".to_string(),
        },
        RuntimeLaunchRequest {
            env: BTreeMap::from([("ANTHROPIC_API_KEY".to_string(), "secret".to_string())]),
            cwd: Some(workspace.clone()),
            ..Default::default()
        },
    );

    assert_eq!(plan.runtime_kind, RuntimeKind::LinuxCompat);
    assert_eq!(
        plan.process().expect("valid launch").command,
        native.join("librua-runtime-gate.so")
    );
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .contains(&"--bind=/storage/emulated/0:/sdcard".to_string()));
    assert!(plan.process().expect("valid launch").env["PATH"].starts_with("/root/.local/bin:"));
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .any(|arg| arg == "claude-agent-acp"));
    assert_eq!(
        plan.process().expect("valid launch").env["ANTHROPIC_API_KEY"],
        "secret"
    );
    assert_eq!(
        plan.process().expect("valid launch").env["NPM_CONFIG_CACHE"],
        temp.path()
            .join("files/home/.rua.cache/npm")
            .to_string_lossy()
    );
    assert!(!plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .any(|arg| arg.contains("secret")));
    assert_eq!(
        plan.agent_path_contract.workspace_dir,
        workspace.to_string_lossy()
    );
}

#[test]
fn android_os_family_cannot_be_overridden_to_native_external_runtime() {
    let temp = tempfile::tempdir().expect("tempdir");
    let native = temp.path().join("native");
    touch(&native.join("libproot.so"));
    touch(
        &native
            .parent()
            .unwrap()
            .join("files/runtime/debian/usr/bin/env"),
    );
    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout {
            host_profile: HostProfile::Desktop,
            os_family: "android".to_string(),
            linux_compat: Some(LinuxCompatLayout::android(
                &native,
                &native.parent().unwrap().join("files"),
                &native.parent().unwrap().join("files/home"),
            )),
        },
        RuntimeAuthorityConfig {
            workspace_dir: temp.path().join("files/home/.rua/workspaces/default"),
            cache_dir: temp.path().join("files/home/.rua.cache"),
            path_entries: vec![native.clone()],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    assert_eq!(
        authority.inventory().default_external_runtime,
        RuntimeKind::LinuxCompat
    );
    let plan = authority.plan(
        ProcessShape::Stdio,
        ToolchainIntent::Node,
        RuntimeLaunchRequest::default(),
    );
    assert_eq!(plan.runtime_kind, RuntimeKind::LinuxCompat);
}

#[test]
fn android_agent_shell_uses_linux_compat_runtime() {
    let temp = tempfile::tempdir().expect("tempdir");
    let native = temp.path().join("native");
    touch(&native.join("libproot.so"));
    let rootfs = native.parent().unwrap().join("files/runtime/debian");
    touch(&rootfs.join("usr/bin/env"));
    touch(&rootfs.join("bin/bash"));
    let workspace = temp.path().join("files/home/.rua/workspaces/default");
    fs::create_dir_all(&workspace).unwrap();
    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout::android(
            &native,
            native.parent().unwrap().join("files"),
            native.parent().unwrap().join("files/home"),
        ),
        RuntimeAuthorityConfig {
            workspace_dir: workspace.clone(),
            cache_dir: temp.path().join("files/home/.rua.cache"),
            path_entries: vec![native.clone()],
            pathext: Vec::new(),
            windows_shell_preference: Some(OsString::from("ignored")),
        },
        |path| path.is_file(),
    );

    let plan = authority.plan(
        ProcessShape::Shell,
        ToolchainIntent::Shell,
        RuntimeLaunchRequest {
            cwd: Some(workspace.clone()),
            ..Default::default()
        },
    );

    assert_eq!(plan.runtime_kind, RuntimeKind::LinuxCompat);
    assert_eq!(
        plan.process().expect("valid launch").command,
        native.join("librua-runtime-gate.so")
    );
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .contains(&format!("--cwd={}", workspace.display())));
    assert_eq!(
        &plan.process().expect("valid launch").args
            [plan.process().expect("valid launch").args.len() - 2..],
        ["/bin/bash", "-lc"]
    );
}

#[test]
fn android_shell_probe_executes_bash_directly_without_command_mode() {
    let temp = tempfile::tempdir().expect("tempdir");
    let native = temp.path().join("native");
    touch(&native.join("libproot.so"));
    let rootfs = native.parent().unwrap().join("files/runtime/debian");
    touch(&rootfs.join("usr/bin/env"));
    touch(&rootfs.join("bin/bash"));
    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout::android(
            &native,
            native.parent().unwrap().join("files"),
            native.parent().unwrap().join("files/home"),
        ),
        RuntimeAuthorityConfig {
            workspace_dir: temp.path().join("files/home/.rua/workspaces/default"),
            cache_dir: temp.path().join("files/home/.rua.cache"),
            path_entries: vec![native.clone()],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    let plan = authority.plan(
        ProcessShape::SkillProbe,
        ToolchainIntent::Shell,
        RuntimeLaunchRequest {
            args: vec!["--version".into()],
            ..Default::default()
        },
    );

    assert_eq!(plan.runtime_kind, RuntimeKind::LinuxCompat);
    assert_eq!(
        &plan.process().expect("valid launch").args
            [plan.process().expect("valid launch").args.len() - 2..],
        ["/bin/bash", "--version"]
    );
}

#[test]
fn android_bound_workspace_command_is_not_checked_inside_rootfs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let native = temp.path().join("native");
    touch(&native.join("libproot.so"));
    touch(
        &native
            .parent()
            .unwrap()
            .join("files/runtime/debian/usr/bin/env"),
    );
    let workspace = temp.path().join("files/home/.rua/workspaces/default");
    fs::create_dir_all(&workspace).unwrap();
    let command = workspace.join("plugin/run");
    touch(&command);
    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout::android(
            &native,
            native.parent().unwrap().join("files"),
            native.parent().unwrap().join("files/home"),
        ),
        RuntimeAuthorityConfig {
            workspace_dir: workspace.clone(),
            cache_dir: temp.path().join("files/home/.rua.cache"),
            path_entries: vec![native.clone()],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    let plan = authority.plan(
        ProcessShape::Stdio,
        ToolchainIntent::GenericCommand {
            command: command.to_string_lossy().into_owned(),
        },
        RuntimeLaunchRequest {
            cwd: Some(workspace),
            ..Default::default()
        },
    );

    assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .contains(&command.to_string_lossy().into_owned()));
}

#[test]
fn android_python_limits_apply_to_guest_payload_not_proot_adapter() {
    let temp = tempfile::tempdir().expect("tempdir");
    let native = temp.path().join("native");
    touch(&native.join("libproot.so"));
    let rootfs = native.parent().unwrap().join("files/runtime/debian");
    touch(&rootfs.join("usr/bin/env"));
    touch(&rootfs.join("usr/bin/prlimit"));
    touch(&rootfs.join("usr/bin/python3"));
    let workspace = temp.path().join("files/home/.rua/workspaces/default");
    fs::create_dir_all(&workspace).unwrap();
    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout::android(
            &native,
            native.parent().unwrap().join("files"),
            native.parent().unwrap().join("files/home"),
        ),
        RuntimeAuthorityConfig {
            workspace_dir: workspace.clone(),
            cache_dir: temp.path().join("files/home/.rua.cache"),
            path_entries: vec![native.clone()],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );
    let limits = RuntimeResourceLimits {
        address_space_bytes: 256 * 1024 * 1024,
        max_processes: 64,
        max_file_size_bytes: 10 * 1024 * 1024,
    };

    let plan = authority.plan(
        ProcessShape::Direct,
        ToolchainIntent::Python,
        RuntimeLaunchRequest {
            args: vec!["-I".into(), "-c".into(), "print('ok')".into()],
            cwd: Some(workspace),
            resource_limits: Some(limits),
            ..Default::default()
        },
    );

    assert_eq!(plan.runtime_kind, RuntimeKind::LinuxCompat);
    assert_eq!(
        plan.process().expect("valid launch").command,
        native.join("librua-runtime-gate.so")
    );
    assert_eq!(plan.host_resource_limits, None);
    let prlimit_index = plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .position(|arg| arg == "/usr/bin/prlimit")
        .expect("guest prlimit");
    assert_eq!(
        &plan.process().expect("valid launch").args[prlimit_index..],
        [
            "/usr/bin/prlimit",
            "--as=268435456",
            "--nproc=64",
            "--fsize=10485760",
            "--",
            "/usr/bin/python3",
            "-I",
            "-c",
            "print('ok')",
        ]
    );
}

#[test]
fn desktop_python_limits_apply_to_native_process() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("bin");
    let python = bin.join("python3");
    touch(&python);
    let authority = RuntimeAuthority::from_layout_with_probe(
        desktop_layout("linux"),
        RuntimeAuthorityConfig {
            workspace_dir: temp.path().join("workspace"),
            cache_dir: temp.path().join("cache"),
            path_entries: vec![bin],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );
    let limits = RuntimeResourceLimits {
        address_space_bytes: 256 * 1024 * 1024,
        max_processes: 64,
        max_file_size_bytes: 10 * 1024 * 1024,
    };

    let plan = authority.plan(
        ProcessShape::Direct,
        ToolchainIntent::Python,
        RuntimeLaunchRequest {
            args: vec!["-I".into(), "-c".into(), "print('ok')".into()],
            resource_limits: Some(limits),
            ..Default::default()
        },
    );

    assert_eq!(plan.runtime_kind, RuntimeKind::Native);
    assert_eq!(plan.process().expect("valid launch").command, python);
    assert_eq!(plan.host_resource_limits, Some(limits));
}

#[test]
fn missing_android_runtime_cannot_expose_executable_material_for_any_process_shape() {
    let temp = tempfile::tempdir().unwrap();
    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout {
            host_profile: HostProfile::MobileApk,
            os_family: "android".into(),
            linux_compat: None,
        },
        RuntimeAuthorityConfig::current(temp.path(), temp.path()),
        |_| false,
    );
    for shape in [
        ProcessShape::Shell,
        ProcessShape::Interactive,
        ProcessShape::Stdio,
        ProcessShape::Direct,
        ProcessShape::Install,
        ProcessShape::SkillProbe,
    ] {
        let plan = authority.plan(
            shape,
            ToolchainIntent::GenericCommand {
                command: "/bin/sh".into(),
            },
            RuntimeLaunchRequest::default(),
        );
        assert!(plan.process().is_err());
        assert!(plan.into_process().is_err());
    }
}

#[cfg(unix)]
#[test]
fn mobile_inventory_reports_probed_versions_for_the_actual_toolchain_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let linux = LinuxCompatLayout::android(
        &temp.path().join("native"),
        &temp.path().join("data"),
        &temp.path().join("home"),
    );
    assert_eq!(linux.base_env(&[])["UV_LINK_MODE"], "copy");
    assert_eq!(linux.base_env(&[])["NPM_CONFIG_PREFIX"], "/root/.local");
    assert!(linux.base_env(&[])["PATH"].starts_with("/root/.local/bin:"));
    let installed = linux
        .rootfs_path
        .join("usr/local/lib/rua/toolchains/pinned-node");
    let binary = installed.join("node/bin/node");
    touch(&binary);
    fs::write(
        installed.join(".rua-versions.json"),
        r#"{"node":"v24.18.0","npm":"11.16.0"}"#,
    )
    .unwrap();
    let bin = linux.rootfs_path.join("usr/local/bin");
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(&binary, bin.join("node")).unwrap();
    let layout = RuntimeLayout {
        host_profile: HostProfile::MobileApk,
        os_family: "android".into(),
        linux_compat: Some(linux),
    };
    let config = RuntimeAuthorityConfig {
        workspace_dir: temp.path().join("workspace"),
        cache_dir: temp.path().join("cache"),
        path_entries: Vec::new(),
        pathext: Vec::new(),
        windows_shell_preference: None,
    };
    let authority =
        RuntimeAuthority::from_layout_with_probe(layout.clone(), config.clone(), |p| p.is_file());
    assert_eq!(
        authority.inventory().node.version.as_deref(),
        Some("v24.18.0")
    );
    assert!(!authority.inventory().npm.available);
    assert!(authority.inventory().npm.version.is_none());
    fs::remove_file(bin.join("node")).unwrap();
    touch(&bin.join("node"));
    let authority = RuntimeAuthority::from_layout_with_probe(layout, config, |p| p.is_file());
    assert!(authority.inventory().node.available);
    assert!(authority.inventory().node.version.is_none());
}

#[test]
fn native_generic_commands_resolve_declared_paths_without_bypassing_missing_command_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let script = workspace.join("scripts/fixture");
    touch(&script);
    let bin = temp.path().join("bin");
    touch(&bin.join("custom-mcp"));
    let authority = RuntimeAuthority::from_layout_with_probe(
        desktop_layout("linux"),
        RuntimeAuthorityConfig {
            workspace_dir: workspace.clone(),
            cache_dir: temp.path().join("cache"),
            path_entries: vec![bin.clone()],
            pathext: vec![],
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );
    for (command, expected) in [
        ("custom-mcp".to_string(), bin.join("custom-mcp")),
        ("scripts/fixture".to_string(), script.clone()),
        (script.to_string_lossy().into_owned(), script),
    ] {
        let plan = authority.plan(
            ProcessShape::Stdio,
            ToolchainIntent::GenericCommand { command },
            RuntimeLaunchRequest::default(),
        );
        assert_eq!(plan.process().unwrap().command, expected);
    }
    let plan = authority.plan(
        ProcessShape::Stdio,
        ToolchainIntent::GenericCommand {
            command: "missing-command".into(),
        },
        RuntimeLaunchRequest::default(),
    );
    assert!(plan
        .process()
        .unwrap_err()
        .to_string()
        .contains("not found in Runtime Inventory"));
}
