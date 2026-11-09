// FileManager owns prompt-file mutation and publication. Endpoint contracts must
// preserve that seam rather than require the former endpoint-local emit helpers.
fn assert_publishing_manager_calls(methods: &[&str]) {
    let endpoints = include_str!("../../src/endpoints/files.rs");
    let wiring = include_str!("../../src/services/agent_prompt_files.rs");
    assert!(endpoints.contains("agent_prompt_files as build_manager"));
    assert!(wiring.contains(".with_publisher(Arc::new(PromptFilePublication"));
    assert!(wiring.contains("publish_sync_fact_with_runtime("));
    assert!(wiring.contains("SyncPublicationFact::AgentPromptFilesCommitted"));
    for method in methods {
        let body = function_body(endpoints, &format!("{method}_impl"));
        assert!(
            body.contains("build_manager(state, agent_id)"),
            "{method} bypasses publisher"
        );
        assert!(
            body.contains(&format!(".{method}(")),
            "{method} bypasses mutation owner"
        );
    }
}

#[test]
fn agent_prompt_root_markdown_writes_publish_agent_prompt_file_facts() {
    assert_publishing_manager_calls(&["write_file", "delete_file", "rename_file"]);
}

#[test]
fn agent_prompt_subdir_markdown_writes_publish_agent_prompt_file_facts() {
    assert_publishing_manager_calls(&[
        "create_subdir_file",
        "write_subdir_file",
        "rename_subdir_file",
        "delete_subdir_file",
    ]);
}

#[test]
fn system_prompt_file_selection_publishes_agent_config_fact() {
    let src = include_str!("../../src/endpoints/files.rs");
    let update_system_prompt_files_impl = function_body(src, "update_system_prompt_files_impl");

    assert!(
        update_system_prompt_files_impl.contains("publish_agent_config_file_committed"),
        "updating system_prompt_files must publish agent_config facts because the selection is stored in agent.json"
    );
}

fn function_body<'a>(src: &'a str, name: &str) -> &'a str {
    src.split(&format!("async fn {name}"))
        .nth(1)
        .and_then(|tail| tail.split("\nasync fn ").next())
        .expect("function should exist")
}
