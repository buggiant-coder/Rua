//! Private Shell lifecycle operations requested through the resident HTTP API.
//! Automation and CLI installation run directly in the Runtime.
use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{Result, RuaError};

pub const ENDPOINT_ENV: &str = "RUA_DESKTOP_CONTROL";
const MAX_FRAME: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Serialize, Deserialize)]
pub struct DesktopControlEndpoint {
    address: SocketAddr,
    token: String,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum DesktopControlCommand {
    LegacyStatus,
    UpdateStatus,
    CheckUpdate,
    InstallUpdate,
    MigrateLegacy { confirmed: bool },
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    command: DesktopControlCommand,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    result: Result<Value>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl DesktopControlEndpoint {
    pub fn new(address: SocketAddr) -> Result<Self> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(RuaError::Config {
                message: "Native control must use a private loopback address".into(),
            });
        }
        Ok(Self {
            address,
            token: uuid::Uuid::new_v4().to_string(),
            extra: BTreeMap::new(),
        })
    }

    pub fn from_environment() -> Result<Option<Self>> {
        let Some(value) = std::env::var_os(ENDPOINT_ENV) else {
            return Ok(None);
        };
        let endpoint: Self =
            serde_json::from_str(&value.to_string_lossy()).map_err(|_| RuaError::Config {
                message: "Invalid native control connection".into(),
            })?;
        if !endpoint.address.ip().is_loopback()
            || endpoint.address.port() == 0
            || endpoint.token.len() < 32
        {
            return Err(RuaError::Config {
                message: "Invalid native control connection".into(),
            });
        }
        Ok(Some(endpoint))
    }

    /// Only pass this value to the owned resident process; never expose it through HTTP.
    pub fn environment_value(&self) -> Result<String> {
        serde_json::to_string(self).map_err(codec_error)
    }

    pub async fn call(&self, command: DesktopControlCommand) -> Result<Value> {
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            let mut stream = TcpStream::connect(self.address).await?;
            write_frame(
                &mut stream,
                &Request {
                    token: self.token.clone(),
                    command,
                    extra: BTreeMap::new(),
                },
            )
            .await?;
            read_frame::<Reply>(&mut stream).await?.result
        })
        .await
        .map_err(|_| RuaError::Timeout {
            operation: "native desktop operation".into(),
        })?
    }

    /// A bounded native command transport, not a second application HTTP server.
    pub async fn serve<F, Fut>(
        self,
        listener: TcpListener,
        cancel: CancellationToken,
        handler: F,
    ) -> Result<()>
    where
        F: Fn(DesktopControlCommand) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                accepted = listener.accept(), if connections.len() < 16 => {
                    let (mut stream, _) = accepted?;
                    let expected = self.token.clone();
                    let handler = handler.clone();
                    let cancel = cancel.child_token();
                    connections.spawn(async move {
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {},
                            _ = async {
                                let request = match tokio::time::timeout(Duration::from_secs(5), read_frame::<Request>(&mut stream)).await {
                                    Ok(Ok(request)) => request,
                                    _ => return,
                                };
                                use sha2::{Digest, Sha256};
                                use subtle::ConstantTimeEq;
                                let valid = Sha256::digest(expected.as_bytes()).ct_eq(&Sha256::digest(request.token.as_bytes()));
                                if !bool::from(valid) { return; }
                                let result = handler(request.command).await;
                                let _ = write_frame(&mut stream, &Reply { result, extra: BTreeMap::new() }).await;
                            } => {},
                        }
                    });
                }
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        Ok(())
    }
}

async fn read_frame<T: DeserializeOwned>(stream: &mut TcpStream) -> Result<T> {
    let size = stream.read_u32().await? as usize;
    if size == 0 || size > MAX_FRAME {
        return Err(RuaError::Validation {
            message: "Invalid native control frame size".into(),
        });
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(codec_error)
}

async fn write_frame(stream: &mut TcpStream, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(codec_error)?;
    if bytes.len() > MAX_FRAME {
        return Err(RuaError::Validation {
            message: "Native control response is too large".into(),
        });
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

fn codec_error(error: serde_json::Error) -> RuaError {
    RuaError::Validation {
        message: error.to_string(),
    }
}
