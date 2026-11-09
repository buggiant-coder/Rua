use crate::test_helpers::{agent_workspace, test_context};
use rua_workspace::{PlatformProfile, RuaRoot, WorkspaceRuntime};
use std::sync::Arc;

#[tokio::test]
async fn embedding_configuration_updates_without_replacing_memory_or_react() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(
                    count > 0,
                    "embedding request closed before its HTTP headers"
                );
                request.extend_from_slice(&buffer[..count]);
            }
            let header_end = request
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
                .unwrap()
                + 4;
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            while request.len() < header_end + length {
                let mut buffer = [0; 1024];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0, "embedding request body closed early");
                request.extend_from_slice(&buffer[..count]);
            }
            let payload: serde_json::Value =
                serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
            assert_eq!(payload["model"], "fixture");
            let body = serde_json::json!({"data":[{"embedding":vec![0.5;32]}]}).to_string();
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(temp.path());
    let workspace = agent_workspace(&root, "embedding-apply");
    workspace.ensure().unwrap();
    let context = test_context(temp.path().to_path_buf());
    rua_config::atomic_write_json(
        &workspace.as_path().join("agent.json"),
        &serde_json::json!({
            "id":"embedding-apply", "name":"Embedding application"
        }),
    )
    .unwrap();
    let runtime = WorkspaceRuntime::new(
        "embedding-apply",
        "Embedding application",
        workspace,
        PlatformProfile::Desktop,
        Arc::clone(&context),
    );
    runtime.initialize_for_platform().await.unwrap();
    let memory = runtime.memory_manager().await.unwrap();
    let react = runtime.react_runtime().await.unwrap();
    let cell = context
        .agent_cells
        .section::<serde_json::Value>("embedding-apply", "running");
    cell.write(serde_json::json!({"embedding_config": {"base_url":base_url, "model_name":"fixture", "dimensions":32}})).await.unwrap();
    runtime.apply_embedding_configuration().await.unwrap();
    assert!(Arc::ptr_eq(
        &memory,
        &runtime.memory_manager().await.unwrap()
    ));
    assert!(Arc::ptr_eq(&react, &runtime.react_runtime().await.unwrap()));
    cell.write(serde_json::json!({"embedding_config": {"base_url":"invalid", "model_name":"fixture", "dimensions":32}})).await.unwrap();
    assert!(runtime.apply_embedding_configuration().await.is_err());
    cell.write(serde_json::json!({"embedding_config": {"base_url":base_url, "model_name":"fixture", "dimensions":32}})).await.unwrap();
    runtime.apply_embedding_configuration().await.unwrap();
    runtime.stop().await;
    server.await.unwrap();
}
