use std::io::{Cursor, Write};

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn make_plugin_zip() -> Vec<u8> {
    let buf = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(buf);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let manifest = r#"{"id":"url-test-plugin","name":"URL Test Plugin","version":"1.0.0"}"#;
    writer.start_file("plugin.json", options).unwrap();
    writer.write_all(manifest.as_bytes()).unwrap();

    writer.start_file("main.py", options).unwrap();
    writer.write_all(b"print('hello')").unwrap();

    writer.finish().unwrap().into_inner()
}

async fn start_mock_server(body: Vec<u8>, status: u16) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/plugin.zip");

    let handle = tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;

            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            let _ = stream.flush().await;
        }
    });

    (url, handle)
}

#[tokio::test]
async fn from_url_loads_plugin_zip() {
    let zip_data = make_plugin_zip();
    let (url, _server) = start_mock_server(zip_data, 200).await;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("plugin");

    let plugin = rua_plugin_runtime::PluginLoader::from_url(&url, &target)
        .await
        .unwrap();

    assert_eq!(plugin.id, "url-test-plugin");
    assert_eq!(plugin.manifest.name, "URL Test Plugin");
    assert_eq!(plugin.manifest.version, "1.0.0");
}

#[tokio::test]
async fn from_url_rejects_http_error() {
    let (url, _server) = start_mock_server(vec![], 404).await;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("plugin");

    let err = rua_plugin_runtime::PluginLoader::from_url(&url, &target)
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("404"), "expected 404 in error: {msg}");
}

#[tokio::test]
async fn from_url_zip_slip_rejected() {
    let buf = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(buf);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("../../../evil.txt", options).unwrap();
    writer.write_all(b"pwned").unwrap();
    let zip_data = writer.finish().unwrap().into_inner();

    let (url, _server) = start_mock_server(zip_data, 200).await;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("plugin");

    let err = rua_plugin_runtime::PluginLoader::from_url(&url, &target)
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("zip-slip"),
        "expected zip-slip in error: {msg}"
    );
}
