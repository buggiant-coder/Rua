#[test]
fn acp_config_writes_publish_agent_config_fact() {
    let src = include_str!("../../src/endpoints/acp.rs");

    let write_acp_config = function_body(src, "write_acp_config");
    assert!(
        write_acp_config.contains("publish_agent_config_file_committed")
            && write_acp_config.contains("\"acp agent config\""),
        "ACP runner config writes must publish agent_config facts after the local cell write"
    );
}

fn function_body<'a>(src: &'a str, name: &str) -> &'a str {
    src.split(&format!("async fn {name}"))
        .nth(1)
        .and_then(|tail| tail.split("\nasync fn ").next())
        .expect("function should exist")
}
