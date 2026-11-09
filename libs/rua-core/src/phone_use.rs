//! Private APK-to-resident connection. This is not an Agent-facing transport API.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;

const FRAME_LIMIT: usize = 64 * 1024;
const MEDIA_FRAME_LIMIT: usize = 1200 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
static BRIDGE: OnceLock<PhoneBridge> = OnceLock::new();

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PhoneStatus {
    pub generation: String,
    pub platform: String,
    pub state: String,
    pub accessibility: bool,
    pub camera_permission: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub struct PhoneBridge {
    socket: PathBuf,
    host_pid: u32,
    generation: String,
    auth_token: String,
    token_revoked: AtomicBool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PhoneNode {
    pub token: String,
    pub path: Vec<u32>,
    pub text: String,
    pub description: String,
    pub view_id: String,
    pub class_name: String,
    pub bounds: [i32; 4],
    pub clickable: bool,
    pub editable: bool,
    pub scrollable: bool,
    pub enabled: bool,
    pub visible: bool,
    pub password: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PhoneObservation {
    pub observation_id: uuid::Uuid,
    pub service_generation: uuid::Uuid,
    pub package_name: String,
    pub window_id: i32,
    pub width: u32,
    pub height: u32,
    pub rotation: u8,
    pub nodes: Vec<PhoneNode>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<PhoneImage>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PhoneImage {
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub data: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PhoneObservation {
    pub fn validate(&self, max_nodes: u32, max_depth: u32) -> Result<(), String> {
        if self.screenshot.as_ref().is_some_and(|image| {
            image.mime_type != "image/jpeg"
                || image.width == 0
                || image.height == 0
                || image.width > 1280
                || image.height > 1280
                || image.data.len() > 1024 * 1024
        }) {
            return Err("Invalid Phone Use screenshot".into());
        }
        if self.width == 0
            || self.height == 0
            || self.rotation > 3
            || self.package_name.is_empty()
            || self.nodes.len() > max_nodes as usize
            || self.nodes.is_empty()
        {
            return Err("Invalid Phone Use observation geometry or nodes".into());
        }
        for (index, node) in self.nodes.iter().enumerate() {
            if node.token != format!("{}:{index}", self.observation_id)
                || node.path.len() > max_depth as usize
                || node.bounds[0] > node.bounds[2]
                || node.bounds[1] > node.bounds[3]
                || (node.password && (!node.text.is_empty() || !node.description.is_empty()))
            {
                return Err("Invalid Phone Use node reference or protected content".into());
            }
        }
        Ok(())
    }
}

impl PhoneBridge {
    pub async fn connect(
        socket: PathBuf,
        host_pid: u32,
        cancel: &CancellationToken,
    ) -> Result<Self, String> {
        if !socket.is_absolute() || host_pid == 0 {
            return Err("Invalid private Phone Use endpoint".into());
        }
        let hello = Self::exchange(&socket, host_pid, "hello", json!({}), cancel).await?;
        if hello["protocol_version"] != 1 {
            return Err("Unsupported Phone Use protocol".into());
        }
        let generation = hello["generation"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Missing Phone Use generation")?
            .to_owned();
        let auth_token = hello["auth_token"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("Invalid private Phone Use credential")?
            .to_owned();
        Ok(Self {
            socket,
            host_pid,
            generation,
            auth_token,
            token_revoked: AtomicBool::new(false),
        })
    }

    /// Publish the actual bound loopback listener over the authenticated private channel.
    pub async fn publish_http_port(
        &self,
        port: u16,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        if port == 0 {
            return Err("Resident listener has no assigned port".into());
        }
        let ready = Self::exchange(
            &self.socket,
            self.host_pid,
            "resident_ready",
            json!({"generation": self.generation, "port": port}),
            cancel,
        )
        .await?;
        if ready["generation"] != self.generation || ready["port"] != port {
            return Err("Host rejected the resident HTTP endpoint".into());
        }
        Ok(())
    }

    pub async fn status(&self, cancel: &CancellationToken) -> Result<PhoneStatus, String> {
        let value = Self::exchange(
            &self.socket,
            self.host_pid,
            "status",
            json!({"generation":self.generation}),
            cancel,
        )
        .await?;
        let status: PhoneStatus =
            serde_json::from_value(value).map_err(|_| "Invalid Phone Use status")?;
        if status.generation != self.generation || status.platform != "android" {
            return Err("Phone Use instance changed; restart the app".into());
        }
        Ok(status)
    }

    pub fn accepts_token(&self, token: &str) -> bool {
        !self.token_revoked.load(Ordering::Acquire)
            && bool::from(self.auth_token.as_bytes().ct_eq(token.as_bytes()))
    }

    pub fn revoke_token(&self) {
        self.token_revoked.store(true, Ordering::Release);
    }

    pub async fn list_apps(&self, cancel: &CancellationToken) -> Result<Value, String> {
        Self::exchange(
            &self.socket,
            self.host_pid,
            "list_apps",
            json!({"generation":self.generation}),
            cancel,
        )
        .await
    }

    pub async fn list_cameras(&self, cancel: &CancellationToken) -> Result<Value, String> {
        Self::exchange(
            &self.socket,
            self.host_pid,
            "list_cameras",
            json!({"generation":self.generation}),
            cancel,
        )
        .await
    }

    pub async fn take_photo(
        &self,
        owner: &str,
        operation_id: &str,
        camera_id: &str,
        cancel: &CancellationToken,
    ) -> Result<PhoneImage, String> {
        let params = json!({"generation":self.generation,"owner":owner,"operation_id":operation_id,"camera_id":camera_id});
        let mut guard = PhoneCallGuard {
            socket: self.socket.clone(),
            host_pid: self.host_pid,
            params: params.clone(),
            settled: false,
        };
        let value =
            Self::exchange(&self.socket, self.host_pid, "take_photo", params, cancel).await?;
        if matches!(
            value["state"].as_str(),
            Some("cancelled" | "outcome_unknown")
        ) {
            guard.settled = true;
            return Err("Photo capture cancelled; an already submitted capture may have an unknown outcome. Do not automatically retry.".into());
        }
        let image: PhoneImage =
            serde_json::from_value(value.get("image").cloned().ok_or("Photo not returned")?)
                .map_err(|_| "Invalid photo image")?;
        if image.mime_type != "image/jpeg"
            || image.width == 0
            || image.height == 0
            || image.width > 1280
            || image.height > 1280
            || image.data.len() > 1024 * 1024
        {
            return Err("Invalid photo dimensions or size".into());
        }
        guard.settled = true;
        Ok(image)
    }

    pub async fn screenshot(
        &self,
        owner: &str,
        operation_id: &str,
        max_nodes: u32,
        max_depth: u32,
        cancel: &CancellationToken,
    ) -> Result<PhoneObservation, String> {
        let params = json!({"generation":self.generation,"owner":owner,"operation_id":operation_id,"max_nodes":max_nodes,"max_depth":max_depth});
        let mut guard = PhoneCallGuard {
            socket: self.socket.clone(),
            host_pid: self.host_pid,
            params: params.clone(),
            settled: false,
        };
        let value =
            Self::exchange(&self.socket, self.host_pid, "screenshot", params, cancel).await?;
        let observation: PhoneObservation =
            serde_json::from_value(value).map_err(|_| "Invalid screenshot observation")?;
        observation.validate(max_nodes, max_depth)?;
        if observation.screenshot.is_none() {
            return Err("Missing screenshot image".into());
        }
        guard.settled = true;
        Ok(observation)
    }

    pub async fn observe(
        &self,
        owner: &str,
        operation_id: &str,
        max_nodes: u32,
        max_depth: u32,
        cancel: &CancellationToken,
    ) -> Result<PhoneObservation, String> {
        if owner.is_empty()
            || owner.len() > 128
            || !(1..=40).contains(&max_nodes)
            || !(1..=12).contains(&max_depth)
        {
            return Err("Invalid Phone Use observation limits".into());
        }
        let params = json!({"generation":self.generation,"owner":owner,"operation_id":operation_id,"max_nodes":max_nodes,"max_depth":max_depth});
        let mut guard = PhoneCallGuard {
            socket: self.socket.clone(),
            host_pid: self.host_pid,
            params: params.clone(),
            settled: false,
        };
        let value = Self::exchange(&self.socket, self.host_pid, "observe", params, cancel).await?;
        let observation: PhoneObservation =
            serde_json::from_value(value).map_err(|_| "Invalid Phone Use observation response")?;
        observation.validate(max_nodes, max_depth)?;
        guard.settled = true;
        Ok(observation)
    }

    pub async fn interact(
        &self,
        action: &str,
        owner: &str,
        operation_id: &str,
        mut params: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        if !matches!(
            action,
            "click" | "long_press" | "type_text" | "swipe" | "back" | "home" | "launch_app"
        ) {
            return Err("Unsupported Phone Use action".into());
        }
        let fields = params.as_object_mut().ok_or("Invalid phone interaction")?;
        fields.insert("owner".into(), json!(owner));
        fields.insert("operation_id".into(), json!(operation_id));
        fields.insert("generation".into(), json!(self.generation));
        let mut guard = PhoneCallGuard {
            socket: self.socket.clone(),
            host_pid: self.host_pid,
            params: json!({"generation":self.generation,"owner":owner,"operation_id":operation_id}),
            settled: false,
        };
        let mut result = Self::exchange(&self.socket, self.host_pid, action, params, cancel).await;
        if let Ok(value) = &mut result {
            guard.settled = true;
            sanitize_action_observation(value);
        }
        result
    }

    async fn exchange(
        socket: &Path,
        host_pid: u32,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        #[cfg(any(target_os = "android", target_os = "linux", target_os = "macos"))]
        {
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
            use tokio::net::UnixStream;
            let request_id = uuid::Uuid::new_v4().to_string();
            let operation = async {
                let mut stream = UnixStream::connect(socket)
                    .await
                    .map_err(|_| "Phone Use native service is not connected")?;
                let peer = stream
                    .peer_cred()
                    .map_err(|_| "Cannot verify Phone Use peer")?;
                // The caller owns the socket directory; a foreign UID or another host PID is never accepted.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    let uid =
                        std::fs::metadata(socket.parent().ok_or("Invalid Phone Use directory")?)
                            .map_err(|_| "Cannot verify Phone Use directory")?
                            .uid();
                    if peer.uid() != uid
                        || peer.pid().and_then(|p| u32::try_from(p).ok()) != Some(host_pid)
                    {
                        return Err("Untrusted Phone Use host".to_string());
                    }
                }
                let mut bytes =
                    serde_json::to_vec(&json!({"id":request_id,"method":method,"params":params}))
                        .map_err(|_| "Invalid Phone Use request")?;
                bytes.push(b'\n');
                if bytes.len() > FRAME_LIMIT {
                    return Err("Phone Use request too large".into());
                }
                stream
                    .write_all(&bytes)
                    .await
                    .map_err(|_| "Phone Use request delivery failed")?;
                let mut response = Vec::new();
                let response_limit = if matches!(method, "screenshot" | "take_photo") {
                    MEDIA_FRAME_LIMIT
                } else {
                    FRAME_LIMIT
                };
                BufReader::new(stream)
                    .take((response_limit + 1) as u64)
                    .read_until(b'\n', &mut response)
                    .await
                    .map_err(|_| "Phone Use response failed")?;
                if response.len() > response_limit || response.last() != Some(&b'\n') {
                    return Err("Invalid Phone Use response frame".into());
                }
                let value: Value = serde_json::from_slice(&response)
                    .map_err(|_| "Invalid Phone Use response JSON")?;
                if value["id"] != request_id {
                    return Err("Mismatched Phone Use response".into());
                }
                if !value["error"].is_null() {
                    let code = value["error"]["code"].as_str().unwrap_or("native_refusal");
                    let message = value["error"]["message"]
                        .as_str()
                        .unwrap_or("Phone Use native request was refused");
                    return Err(format!(
                        "{}: {}",
                        code.chars().take(80).collect::<String>(),
                        message.chars().take(300).collect::<String>()
                    ));
                }
                value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "Missing Phone Use result".into())
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err("Phone Use request cancelled".into()),
                result = tokio::time::timeout(if method=="take_photo" {Duration::from_secs(15)}else{REQUEST_TIMEOUT}, operation) => result.map_err(|_| "Phone Use request timed out".to_string())?,
            }
        }
        #[cfg(not(any(target_os = "android", target_os = "linux", target_os = "macos")))]
        {
            let _ = (socket, host_pid, method, params, cancel);
            Err("Phone Use requires Android".into())
        }
    }
}

struct PhoneCallGuard {
    socket: PathBuf,
    host_pid: u32,
    params: Value,
    settled: bool,
}

impl Drop for PhoneCallGuard {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let socket = self.socket.clone();
            let host_pid = self.host_pid;
            let params = self.params.clone();
            runtime.spawn(async move {
                let cancel = CancellationToken::new();
                let _ = PhoneBridge::exchange(&socket, host_pid, "cancel", params, &cancel).await;
            });
        }
    }
}

pub async fn initialize_from_host(cancel: &CancellationToken) -> Result<(), String> {
    if !cfg!(target_os = "android") {
        return Err("Phone Use requires an Android resident".into());
    }
    let socket = std::env::var_os("RUA_PHONE_SOCKET").ok_or("Missing private Phone Use socket")?;
    let host_pid = std::env::var("RUA_PHONE_HOST_PID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or("Missing Phone Use host identity")?;
    let bridge = PhoneBridge::connect(socket.into(), host_pid, cancel).await?;
    BRIDGE
        .set(bridge)
        .map_err(|_| "Phone Use already initialized".into())
}

pub fn bridge() -> Option<&'static PhoneBridge> {
    BRIDGE.get()
}

pub fn accepts_native_token(token: &str) -> bool {
    bridge().is_some_and(|bridge| bridge.accepts_token(token))
}

pub fn revoke_native_token() {
    if let Some(bridge) = bridge() {
        bridge.revoke_token();
    }
}

pub fn sanitize_action_observation(value: &mut Value) {
    let error = value.get("observation").and_then(|v| {
        match serde_json::from_value::<PhoneObservation>(v.clone()) {
            Ok(observation) => observation.validate(30, 10).err(),
            Err(_) => Some("Invalid post-action observation".into()),
        }
    });
    if let Some(error) = error {
        if let Some(object) = value.as_object_mut() {
            object.remove("observation");
            object.insert("observation_error".into(), json!(error));
        }
    }
}
