#[test]
fn shared_mcp_mutations_publish_shared_mcp_facts() {
    let src = include_str!("../../src/endpoints/shared_mcp.rs");

    assert!(
        src.contains("async fn publish_shared_mcp_after_write")
            && src.contains("publish_shared_mcp_catalog_committed"),
        "shared MCP create/update/broadcast/sync-from-agent must publish shared_mcp catalog facts"
    );
    assert!(
        src.contains("publish_shared_mcp_client_deleted"),
        "shared MCP delete must publish a shared_mcp client tombstone fact"
    );

    let create = src
        .split("async fn create_shared")
        .nth(1)
        .and_then(|tail| tail.split("async fn update_shared").next())
        .expect("create_shared should exist");
    assert!(
        create.contains("cell.write")
            && create.contains("publish_shared_mcp_after_write(&state).await"),
        "shared MCP create must publish after writing shared_mcp.json"
    );

    let update = src
        .split("async fn update_shared")
        .nth(1)
        .and_then(|tail| tail.split("async fn delete_shared").next())
        .expect("update_shared should exist");
    assert!(
        update.contains("cell.write")
            && update.contains("publish_shared_mcp_after_write(&state).await"),
        "shared MCP update must publish after writing shared_mcp.json"
    );

    let delete = src
        .split("async fn delete_shared")
        .nth(1)
        .and_then(|tail| tail.split("async fn broadcast").next())
        .expect("delete_shared should exist");
    assert!(
        delete.contains("cell.delete")
            && delete.contains("publish_shared_mcp_client_deleted(&state, &client_key).await"),
        "shared MCP delete must publish a tombstone after deleting the local client"
    );

    let broadcast = src
        .split("async fn broadcast")
        .nth(1)
        .and_then(|tail| tail.split("async fn sync_from_agent").next())
        .expect("broadcast should exist");
    assert!(
        broadcast.contains("store.broadcast")
            && broadcast.contains("publish_shared_mcp_after_write(&state).await"),
        "shared MCP broadcast must publish after mutating shared_mcp.json"
    );

    let sync_from_agent = src
        .split("async fn sync_from_agent")
        .nth(1)
        .expect("sync_from_agent should exist");
    assert!(
        sync_from_agent.contains("store.sync_from_agent")
            && sync_from_agent.contains("publish_shared_mcp_after_write(&state).await"),
        "shared MCP sync-from-agent must publish after mutating shared_mcp.json"
    );
}
