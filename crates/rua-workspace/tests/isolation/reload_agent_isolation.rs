use super::test_helpers;

use std::sync::Arc;

use rua_workspace::registry::DefaultWorkspaceRegistry;
use rua_workspace::{PlatformProfile, RuaRoot, WorkspaceRegistry};
use tempfile::TempDir;

pub(super) struct EmptyAuthority;
#[async_trait::async_trait]
impl rua_workspace::AgentWorkAuthority for EmptyAuthority {
    async fn resume(&self, _: &str) -> Result<(), rua_workspace::WorkspaceError> {
        Ok(())
    }
    async fn begin_shutdown(&self, _: &str) -> Result<(), rua_workspace::WorkspaceError> {
        Ok(())
    }
    async fn outstanding_work(
        &self,
        _: &str,
    ) -> Result<rua_workspace::AgentWork, rua_workspace::WorkspaceError> {
        Ok(rua_workspace::AgentWork::default())
    }
    async fn cancel_work(
        &self,
        _: &str,
    ) -> Result<rua_workspace::AgentWork, rua_workspace::WorkspaceError> {
        Ok(rua_workspace::AgentWork::default())
    }
    async fn terminated_units(
        &self,
        _: &[rua_workspace::WorkUnit],
    ) -> Result<Vec<rua_workspace::WorkUnit>, rua_workspace::WorkspaceError> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn reload_agent_a_does_not_swap_agent_b_runtime() {
    let tmp = TempDir::new().unwrap();
    let root = RuaRoot::new(tmp.path());

    for id in &["agent_a", "agent_b"] {
        let agent_dir = test_helpers::agent_workspace(&root, id)
            .as_path()
            .to_path_buf();
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.json"),
            format!(r#"{{"id": "{id}", "name": "{id}"}}"#),
        )
        .unwrap();
    }

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let registry = Arc::new(DefaultWorkspaceRegistry::new(
        root,
        PlatformProfile::Desktop,
        "agent_a",
        ctx,
    ));
    registry.register_agent("agent_a", "Agent A").await;
    registry.set_work_authority(Arc::new(EmptyAuthority)).await;
    registry.register_agent("agent_b", "Agent B").await;

    let rt_a_before = registry.get_or_create("agent_a").await.unwrap();
    let rt_b_before = registry.get_or_create("agent_b").await.unwrap();

    registry.reload_agent("agent_a").await.unwrap();

    let rt_a_after = registry.get("agent_a").await.unwrap();
    assert!(
        !Arc::ptr_eq(&rt_a_before, &rt_a_after),
        "agent_a runtime should have been swapped after reload_agent(a)"
    );

    let rt_b_after = registry.get("agent_b").await.unwrap();
    assert!(
        Arc::ptr_eq(&rt_b_before, &rt_b_after),
        "agent_b runtime must NOT change when agent_a is reloaded"
    );
}

#[tokio::test]
async fn reload_agent_a_preserves_agent_b_mcp_manager_identity() {
    let tmp = TempDir::new().unwrap();
    let root = RuaRoot::new(tmp.path());

    for id in &["agent_a", "agent_b"] {
        let agent_dir = test_helpers::agent_workspace(&root, id)
            .as_path()
            .to_path_buf();
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.json"),
            format!(r#"{{"id": "{id}", "name": "{id}"}}"#),
        )
        .unwrap();
    }

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let registry = Arc::new(DefaultWorkspaceRegistry::new(
        root,
        PlatformProfile::Desktop,
        "agent_a",
        ctx,
    ));
    registry.register_agent("agent_a", "Agent A").await;
    registry.set_work_authority(Arc::new(EmptyAuthority)).await;
    registry.register_agent("agent_b", "Agent B").await;

    let _rt_a = registry.get_or_create("agent_a").await.unwrap();
    let rt_b = registry.get_or_create("agent_b").await.unwrap();

    let mcp_b_before = rt_b.mcp_manager().await;

    registry.reload_agent("agent_a").await.unwrap();

    let rt_b_after = registry.get("agent_b").await.unwrap();
    assert!(Arc::ptr_eq(&rt_b, &rt_b_after));

    let mcp_b_after = rt_b_after.mcp_manager().await;
    match (&mcp_b_before, &mcp_b_after) {
        (Some(before), Some(after)) => {
            assert!(
                Arc::ptr_eq(before, after),
                "agent_b mcp_manager Arc must not change when agent_a reloads"
            );
        }
        (None, None) => {}
        _ => panic!(
            "agent_b mcp_manager presence changed: before={}, after={}",
            mcp_b_before.is_some(),
            mcp_b_after.is_some()
        ),
    }
}
