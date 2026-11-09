use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

pub const DESKTOP_ENDPOINT_VERSION: u8 = 1;
pub const DESKTOP_READY_PREFIX: &str = "RUA_BACKEND_READY ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopEndpoint {
    pub version: u8,
    pub host: IpAddr,
    pub port: u16,
    pub pid: u32,
    pub rua_version: String,
}

impl DesktopEndpoint {
    pub fn new(address: SocketAddr) -> Self {
        Self {
            version: DESKTOP_ENDPOINT_VERSION,
            host: address.ip(),
            port: address.port(),
            pid: std::process::id(),
            rua_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    fn reusable_port(&self, host: IpAddr) -> Option<u16> {
        (self.version == DESKTOP_ENDPOINT_VERSION && self.host == host && self.port > 0)
            .then_some(self.port)
    }
}

pub fn validate_desktop_host(host: IpAddr) -> io::Result<()> {
    if host.is_loopback() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Desktop Shell Runtime must bind to a loopback address",
        ))
    }
}

pub async fn bind_listener(
    host: IpAddr,
    requested_port: u16,
    endpoint_path: Option<&Path>,
) -> io::Result<TcpListener> {
    if endpoint_path.is_some() {
        validate_desktop_host(host)?;
    }

    if requested_port != 0 {
        return TcpListener::bind(SocketAddr::from((host, requested_port))).await;
    }

    if let Some(port) = endpoint_path.and_then(|path| reusable_port(path, host)) {
        match TcpListener::bind(SocketAddr::from((host, port))).await {
            Ok(listener) => return Ok(listener),
            Err(error) => {
                tracing::warn!(
                    port,
                    %error,
                    "persisted desktop port unavailable; selecting a new port"
                );
            }
        }
    }

    TcpListener::bind(SocketAddr::from((host, 0))).await
}

pub fn publish(path: &Path, endpoint: &DesktopEndpoint) -> io::Result<()> {
    let value = serde_json::to_value(endpoint)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    rua_config::atomic_write_json(path, &value)?;

    let line =
        ready_line(endpoint).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    writeln!(stdout, "{line}")?;
    stdout.flush()
}

pub fn ready_line(endpoint: &DesktopEndpoint) -> serde_json::Result<String> {
    Ok(format!(
        "{DESKTOP_READY_PREFIX}{}",
        serde_json::to_string(endpoint)?
    ))
}

fn reusable_port(path: &Path, host: IpAddr) -> Option<u16> {
    let content = std::fs::read(path).ok()?;
    serde_json::from_slice::<DesktopEndpoint>(&content)
        .ok()?
        .reusable_port(host)
}
