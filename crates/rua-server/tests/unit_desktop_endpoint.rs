use std::net::{IpAddr, Ipv4Addr};

use rua_server::desktop_endpoint::{
    bind_listener, publish, ready_line, validate_desktop_host, DesktopEndpoint,
    DESKTOP_ENDPOINT_VERSION, DESKTOP_READY_PREFIX,
};

fn endpoint(port: u16) -> DesktopEndpoint {
    DesktopEndpoint {
        version: DESKTOP_ENDPOINT_VERSION,
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port,
        pid: 42,
        rua_version: "0.1.0".to_string(),
    }
}

#[test]
fn desktop_ready_line_is_versioned_structured_output() {
    let line = ready_line(&endpoint(41234)).expect("ready line");
    let payload = line
        .strip_prefix(DESKTOP_READY_PREFIX)
        .expect("ready prefix");
    let parsed: DesktopEndpoint = serde_json::from_str(payload).expect("ready payload");

    assert_eq!(parsed, endpoint(41234));
}

#[test]
fn desktop_endpoint_publish_atomically_replaces_existing_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("desktop-endpoint.json");
    std::fs::write(&path, b"stale").expect("write stale endpoint");

    publish(&path, &endpoint(41235)).expect("publish endpoint");

    let parsed: DesktopEndpoint =
        serde_json::from_slice(&std::fs::read(&path).expect("read endpoint"))
            .expect("parse endpoint");
    assert_eq!(parsed, endpoint(41235));
    assert!(
        std::fs::read_dir(temp.path())
            .expect("read tempdir")
            .all(|entry| !entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .contains(".tmp.")),
        "atomic write must not leave a temporary file"
    );
}

#[tokio::test]
async fn dynamic_desktop_bind_reuses_the_persisted_port() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("desktop-endpoint.json");
    let probe = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind probe");
    let port = probe.local_addr().expect("probe address").port();
    drop(probe);
    rua_config::atomic_write_json(
        &path,
        &serde_json::to_value(endpoint(port)).expect("endpoint value"),
    )
    .expect("write endpoint");

    let listener = bind_listener(IpAddr::V4(Ipv4Addr::LOCALHOST), 0, Some(&path))
        .await
        .expect("bind persisted port");

    assert_eq!(
        listener.local_addr().expect("listener address").port(),
        port
    );
}

#[tokio::test]
async fn dynamic_desktop_bind_falls_back_when_the_persisted_port_is_busy() {
    let occupied = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind occupied");
    let occupied_port = occupied.local_addr().expect("occupied address").port();
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("desktop-endpoint.json");
    rua_config::atomic_write_json(
        &path,
        &serde_json::to_value(endpoint(occupied_port)).expect("endpoint value"),
    )
    .expect("write endpoint");

    let listener = bind_listener(IpAddr::V4(Ipv4Addr::LOCALHOST), 0, Some(&path))
        .await
        .expect("bind fallback port");
    let selected_port = listener.local_addr().expect("listener address").port();

    assert_ne!(selected_port, occupied_port);
    assert!(selected_port > 0);
}

#[tokio::test]
async fn desktop_endpoint_contract_rejects_non_loopback_hosts() {
    let host = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

    assert!(validate_desktop_host(host).is_err());
    assert!(
        bind_listener(host, 0, Some(std::path::Path::new("endpoint.json")))
            .await
            .is_err()
    );
}
