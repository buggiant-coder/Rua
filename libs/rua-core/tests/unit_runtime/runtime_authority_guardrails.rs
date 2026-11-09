use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("rua-core lives under libs/rua-core")
        .to_path_buf()
}

fn source(path: &str) -> String {
    std::fs::read_to_string(workspace_root().join(path))
        .unwrap_or_else(|err| panic!("read {path}: {err}"))
}

#[test]
fn migrated_subprocess_surfaces_consume_runtime_authority() {
    for path in [
        "crates/rua-acp/src/client.rs",
        "crates/rua-server/src/endpoints/acp.rs",
        "crates/rua-mcp/src/manager.rs",
        "crates/rua-plugin-runtime/src/client.rs",
        "crates/rua-plugin/src/installer.rs",
        "crates/rua-skills/src/android_runtime.rs",
        "crates/rua-tool/src/builtin/execute_python_code.rs",
        "crates/rua-tool/src/executor/runtime.rs",
    ] {
        let src = source(path);
        assert!(
            src.contains("runtime_authority") || src.contains("RuntimeAuthority"),
            "{path} must consume Runtime Authority launch planning"
        );
    }
}

#[test]
fn migrated_launch_surfaces_do_not_reintroduce_direct_runtime_discovery() {
    let guarded = [
        "crates/rua-acp/src/client.rs",
        "crates/rua-server/src/endpoints/acp.rs",
        "crates/rua-mcp/src/manager.rs",
        "crates/rua-plugin-runtime/src/client.rs",
        "crates/rua-plugin/src/installer.rs",
        "crates/rua-tool/src/builtin/execute_python_code.rs",
        "crates/rua-tool/src/executor/runtime.rs",
    ];
    let forbidden = [
        "std::env::var_os(\"PATH\")",
        "env::var_os(\"PATH\")",
        "std::env::var(\"PATHEXT\")",
        "env::var(\"PATHEXT\")",
        "std::env::split_paths",
        "env::split_paths",
        "LinuxCompatLaunchOptions",
        "LinuxCompatEnvStrategy",
        ".launch_plan(",
        "proot-distro",
        "proot_path",
        "cmd.exe",
    ];

    for path in guarded {
        let src = source(path);
        for needle in forbidden {
            assert!(
                !src.contains(needle),
                "{path} reintroduced direct runtime discovery or launch assembly: {needle}"
            );
        }
    }
}

#[test]
fn android_shell_command_text_router_stays_removed() {
    for path in [
        "crates/rua-tool/src/executor/android_router.rs",
        "crates/rua-tool/src/executor/proot.rs",
    ] {
        assert!(
            !workspace_root().join(path).exists(),
            "{path} duplicates Runtime Authority runtime selection"
        );
    }
}

#[test]
fn removed_legacy_mcp_proot_stdio_helper_stays_removed() {
    assert!(
        !workspace_root()
            .join("crates/rua-mcp/src/proot_stdio.rs")
            .exists(),
        "MCP proot stdio helper duplicates Runtime Authority launch planning"
    );
    assert!(
        !source("crates/rua-mcp/src/lib.rs").contains("proot_stdio"),
        "rua-mcp must not re-export the legacy proot stdio helper"
    );
}

#[test]
fn skill_manifest_builders_do_not_persist_runtime_authority_probe_plans() {
    for path in [
        "crates/rua-skills/src/service.rs",
        "crates/rua-skills/src/pool_service.rs",
        "crates/rua-skills/src/reconcile.rs",
    ] {
        let src = source(path);
        assert!(
            src.contains("scan_android_skill_runtime_value(skill_dir)"),
            "{path} should persist only portable static Android runtime scan metadata"
        );
        assert!(
            !src.contains("scan_android_skill_runtime_value_with_authority"),
            "{path} must not persist device-local Runtime Authority probe plans"
        );
    }
}

#[test]
fn runtime_diagnostics_remain_read_models_over_runtime_authority() {
    let doctor = source("crates/rua-doctor/src/lib.rs");
    assert!(
        doctor.contains("RuntimeAuthority::from_current_process"),
        "doctor runtime diagnostics must read Runtime Authority inventory"
    );

    let runtime_tool = source("crates/rua-tool/src/builtin/runtime_environment.rs");
    assert!(
        runtime_tool.contains("RuntimeAuthority::from_current_process"),
        "runtime_info must read Runtime Authority inventory"
    );
    for capability in ["python", "uv", "node", "npm", "npx", "git", "rg", "ffmpeg"] {
        assert!(
            runtime_tool.contains(&format!(
                "\"{capability}\": capability_json(&inventory.{capability})"
            )),
            "runtime_info {capability} capability must render Runtime Authority inventory"
        );
        assert!(
            !runtime_tool.contains(&format!(
                "\"{capability}\": command_capability(\"{capability}\")"
            )),
            "runtime_info {capability} capability must not independently probe PATH"
        );
    }
}

#[test]
fn linux_compat_terminal_profile_consumes_runtime_authority_without_direct_proot_assembly() {
    let terminal = source("crates/rua-server/src/services/terminal.rs");
    assert!(
        terminal.contains("RuntimeAuthority::from_current_process"),
        "Linux Compatibility terminal profile must consume Runtime Authority launch planning"
    );
    for forbidden in [
        "LinuxCompatLaunchOptions",
        "LinuxCompatEnvStrategy",
        ".launch_plan(",
    ] {
        assert!(
            !terminal.contains(forbidden),
            "terminal reintroduced direct Linux Compatibility launch assembly: {forbidden}"
        );
    }
}
