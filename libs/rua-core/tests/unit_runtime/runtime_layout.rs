use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use rua_core::runtime_layout::{
    LinuxCompatEnvStrategy, LinuxCompatLaunchOptions, LinuxCompatLayout, NativeShellKind,
    RuntimeLayout,
};

fn layout() -> LinuxCompatLayout {
    LinuxCompatLayout::android(
        &PathBuf::from("/data/app/com.rua.mobile-test/lib/arm64"),
        &PathBuf::from("/data/data/com.rua.mobile/files"),
        &PathBuf::from("/data/data/com.rua.mobile/files/home"),
    )
}

#[test]
fn linux_compat_layout_maps_mobile_storage_to_sdcard_contract() {
    let layout = layout();
    let mapped = layout
        .map_host_path(&PathBuf::from("/storage/emulated/0/Download/report.pdf"))
        .expect("storage path should map");
    assert_eq!(mapped, PathBuf::from("/sdcard/Download/report.pdf"));
}

#[test]
fn linux_compat_layout_keeps_app_workspace_visible_in_guest() {
    let layout = layout();
    let workspace = PathBuf::from("/data/data/com.rua.mobile/files/home/.rua/workspaces/default");
    assert_eq!(layout.map_host_path(&workspace), Some(workspace));
}

#[test]
fn linux_compat_launch_plan_does_not_hide_unmapped_cwd_with_root_fallback() {
    let layout = layout();
    let unmapped = PathBuf::from("/data/local/tmp/not-bound");
    let plan = layout.launch_plan(
        "/bin/bash",
        LinuxCompatLaunchOptions {
            cwd: Some(unmapped.clone()),
            ..Default::default()
        },
    );

    assert!(plan.args.contains(&format!("--cwd={}", unmapped.display())));
    assert!(!plan.args.contains(&"--cwd=/root".to_string()));
}

#[test]
fn linux_compat_clean_launch_plan_includes_shared_binds_and_env_reset() {
    let layout = layout();
    let plan = layout.launch_plan(
        "/usr/local/bin/npm",
        LinuxCompatLaunchOptions {
            env: BTreeMap::from([(
                "npm_config_registry".to_string(),
                "https://registry.npmmirror.com".to_string(),
            )]),
            env_strategy: LinuxCompatEnvStrategy::Clean,
            ..Default::default()
        },
    );
    assert_eq!(
        plan.command,
        PathBuf::from("/data/app/com.rua.mobile-test/lib/arm64/librua-runtime-gate.so")
    );
    assert!(plan
        .args
        .contains(&"--bind=/storage/emulated/0:/sdcard".to_string()));
    assert!(plan.args.contains(&"--bind=/storage".to_string()));
    assert!(plan.args.contains(&"/usr/bin/env".to_string()));
    assert!(!plan.args.contains(&"-i".to_string()));
    assert_eq!(
        plan.env["npm_config_registry"],
        "https://registry.npmmirror.com"
    );
    assert!(plan.args.contains(&"/usr/local/bin/npm".to_string()));
    assert_eq!(plan.env["HOME"], "/root");
}

#[test]
fn linux_compat_preserve_env_plan_keeps_runner_secrets_out_of_args() {
    let layout = layout();
    let plan = layout.launch_plan(
        "codex-acp",
        LinuxCompatLaunchOptions {
            extra_path_entries: vec![LinuxCompatLayout::node_lts_bin_dir("24.18.0")],
            env: BTreeMap::from([("OPENAI_API_KEY".to_string(), "sk-secret".to_string())]),
            env_strategy: LinuxCompatEnvStrategy::PreserveProcessEnv,
            ..Default::default()
        },
    );
    assert!(!plan.args.iter().any(|arg| arg.contains("sk-secret")));
    assert_eq!(plan.env["OPENAI_API_KEY"], "sk-secret");
    assert!(plan.env["PATH"].starts_with("/usr/local/lib/nodejs/node-v24.18.0-linux-arm64/bin:"));
    assert_eq!(plan.env["HOME"], "/root");
    assert!(!plan.args.contains(&"-i".to_string()));
}

#[test]
fn agent_path_contract_explains_mobile_paths() {
    let layout = RuntimeLayout::android(
        "/data/app/com.rua.mobile-test/lib/arm64",
        "/data/data/com.rua.mobile/files",
        "/data/data/com.rua.mobile/files/home",
    );
    let contract = layout.agent_path_contract(&PathBuf::from(
        "/data/data/com.rua.mobile/files/home/.rua/workspaces/default",
    ));
    assert_eq!(contract.aliases["mobile_shared_storage"], "/sdcard");
    assert_eq!(contract.aliases["linux_guest_home"], "/root");
    assert!(contract
        .rules
        .iter()
        .any(|rule| rule.contains("Use /sdcard")));
    assert!(contract.rules.iter().any(|rule| rule.contains("Avoid ~")));
}

#[test]
fn runtime_layout_owns_mobile_process_temp_dir() {
    let layout = RuntimeLayout::android(
        "/data/app/com.rua.mobile-test/lib/arm64",
        "/data/data/com.rua.mobile/files",
        "/data/data/com.rua.mobile/files/home",
    );
    let workspace = PathBuf::from("/data/data/com.rua.mobile/files/home/.rua/workspaces/default");

    assert_eq!(
        layout.process_temp_dir_for_rua_root(&workspace),
        Some(workspace.join("tmp"))
    );

    let desktop = RuntimeLayout {
        host_profile: rua_core::runtime_layout::HostProfile::Desktop,
        os_family: "linux".to_string(),
        linux_compat: None,
    };
    assert_eq!(desktop.process_temp_dir_for_rua_root(&workspace), None);
}

#[test]
fn native_shell_for_unix_uses_system_shell() {
    let shell = RuntimeLayout::native_shell_for_unix();
    assert!(["/bin/bash", "/bin/sh", "/system/bin/sh"].contains(&shell.program.as_str()));
    assert_eq!(shell.args, vec!["-c"]);
    assert_eq!(shell.kind, NativeShellKind::Unix);
}

#[test]
fn native_shell_for_windows_prefers_git_bash_and_explicit_wsl() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("Git").join("bin");
    fs::create_dir_all(&bin).expect("mkdir");
    let bash = bin.join("bash.exe");
    fs::write(&bash, b"").expect("write bash");
    let path = OsString::from(bin.to_string_lossy().into_owned());

    let shell = RuntimeLayout::native_shell_for_windows(None, Some(&path));
    assert_eq!(shell.program, bash.to_string_lossy());
    assert_eq!(shell.kind, NativeShellKind::Bash);
    assert_eq!(shell.contract_kind(), "git-bash");

    let wsl = temp.path().join("wsl.exe");
    fs::write(&wsl, b"").expect("write wsl");
    let shell = RuntimeLayout::native_shell_for_windows(Some(wsl.as_os_str()), Some(&path));
    assert_eq!(shell.program, wsl.to_string_lossy());
    assert_eq!(shell.kind, NativeShellKind::Wsl);
    assert_eq!(shell.contract_kind(), "wsl");
}

#[test]
fn android_paths_keep_data_outside_the_guest_and_secrets_out_of_argv() {
    let layout = LinuxCompatLayout::android(
        &PathBuf::from("/data/app/random/lib/arm64"),
        &PathBuf::from("/data/user/0/com.rua.mobile/files"),
        &PathBuf::from("/data/data/com.rua.mobile/files/home"),
    );
    let plan = layout.launch_plan(
        "/usr/bin/python3",
        LinuxCompatLaunchOptions {
            env: BTreeMap::from([("API_TOKEN".into(), "private-value".into())]),
            ..Default::default()
        },
    );
    assert_eq!(
        plan.command,
        PathBuf::from("/data/app/random/lib/arm64/librua-runtime-gate.so")
    );
    assert_eq!(
        &plan.args[..4],
        &[
            "--shared",
            "/data/user/0/com.rua.mobile/files/runtime/use.lock",
            "/data/user/0/com.rua.mobile/files/runtime/update.pending",
            "/data/app/random/lib/arm64/libproot.so",
        ]
    );
    assert_eq!(plan.env["API_TOKEN"], "private-value");
    assert!(plan
        .args
        .contains(&"--bind=/data/data/com.rua.mobile/files/home".to_string()));
    assert!(!plan.args.iter().any(|arg| arg.contains("private-value")));
    assert!(plan.args.contains(&"--unset=RUA_PHONE_TOKEN".to_string()));
    assert!(plan
        .args
        .contains(&"--bind=/data/user/0/com.rua.mobile/files/guest-home:/root".to_string()));
    assert!(plan
        .args
        .contains(&"--rootfs=/data/user/0/com.rua.mobile/files/runtime/debian".to_string()));
    assert_eq!(
        plan.env["PROOT_LOADER"],
        "/data/app/random/lib/arm64/libproot-loader.so"
    );
}
