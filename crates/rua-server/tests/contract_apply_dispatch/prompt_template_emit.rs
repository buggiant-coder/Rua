#[test]
fn agent_template_writes_publish_template_facts() {
    let src = include_str!("../../src/endpoints/agent_templates.rs");

    assert!(
        src.contains("publish_sync_fact")
            && src.contains("SyncPublicationFact::AgentTemplateCommitted")
            && src.contains("SyncPublicationFact::AgentTemplateDeleted"),
        "agent-template mutations must publish template facts through the Replication Publication Seam"
    );
    assert!(
        !src.contains("sync_ops::emit_agent_template_v4_upsert")
            && !src.contains("sync_ops::emit_agent_template_v4_delete"),
        "agent-template mutations must not call legacy endpoint-level template emit helpers"
    );

    let create = src
        .split("async fn create_template")
        .nth(1)
        .and_then(|tail| tail.split("async fn update_template").next())
        .expect("create_template should exist");
    assert!(
        create.contains("publish_agent_template_committed"),
        "agent template create must publish AgentTemplateCommitted"
    );

    let update = src
        .split("async fn update_template")
        .nth(1)
        .and_then(|tail| tail.split("async fn delete_template").next())
        .expect("update_template should exist");
    assert!(
        update.contains("publish_agent_template_committed"),
        "agent template update must publish AgentTemplateCommitted"
    );

    let delete = src
        .split("async fn delete_template")
        .nth(1)
        .expect("delete_template should exist");
    assert!(
        delete.contains("publish_agent_template_deleted"),
        "agent template delete must publish AgentTemplateDeleted"
    );
}
