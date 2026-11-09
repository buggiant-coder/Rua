use std::process::Command;

fn rua_bin() -> String {
    env!("CARGO_BIN_EXE_rua").to_string()
}

fn spawn_mock_http(body: Vec<u8>, status: u16) -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    addr
}

#[test]
fn download_success_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let body = b"fake-model-content-for-test".to_vec();
    let addr = spawn_mock_http(body.clone(), 200);
    let model_url = format!("http://{addr}/model.gguf");

    let output = Command::new(rua_bin())
        .args([
            "--rua-root",
            tmp.path().to_str().unwrap(),
            "internal-download-task",
            &model_url,
        ])
        .output()
        .expect("failed to run rua");

    assert!(
        output.status.success(),
        "expected exit 0, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let safe_name = model_url.replace(['/', ':'], "_");
    let model_file = tmp.path().join("models").join(format!("{safe_name}.gguf"));
    assert!(
        model_file.exists(),
        "model file should exist at {:?}",
        model_file
    );
    let content = std::fs::read(&model_file).unwrap();
    assert_eq!(content, b"fake-model-content-for-test");
}

#[test]
fn download_404_exits_nonzero() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_mock_http(b"not found".to_vec(), 404);
    let model_url = format!("http://{addr}/missing.gguf");

    let output = Command::new(rua_bin())
        .args([
            "--rua-root",
            tmp.path().to_str().unwrap(),
            "internal-download-task",
            &model_url,
        ])
        .output()
        .expect("failed to run rua");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}\nstdout: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed") || stderr.contains("404"),
        "stderr should mention failure.\nGot: {stderr}"
    );
}
