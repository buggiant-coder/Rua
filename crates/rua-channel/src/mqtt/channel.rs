use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Transport};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::core::channel::{self, AgentRequest, Channel, OutMessage, Process, ToHandle};
use crate::core::content::{AgentChannelRequest, IncomingContentPart, OutgoingContentPart};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::ChannelId;
use rua_core::{Result, RuaError};

use crate::mqtt::types::MqttConfig;

const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

pub struct MqttChannel {
    channel_id: ChannelId,
    config: MqttConfig,
    clean_session: bool,
    transport: String,
    event_task: Mutex<Option<(CancellationToken, tokio::task::JoinHandle<()>)>>,
    client: RwLock<Option<AsyncClient>>,
    started: RwLock<bool>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    event_deduper: Arc<StdMutex<EventDeduper>>,
}

impl MqttChannel {
    pub fn new(config: MqttConfig) -> Self {
        Self {
            channel_id: "mqtt".into(),
            config,
            clean_session: true,
            transport: "tcp".into(),
            event_task: Mutex::new(None),
            client: RwLock::new(None),
            started: RwLock::new(false),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
    }

    pub fn with_transport(mut self, transport: String) -> Self {
        self.transport = transport;
        self
    }

    pub fn with_clean_session(mut self, clean_session: bool) -> Self {
        self.clean_session = clean_session;
        self
    }

    pub fn with_render_options(
        mut self,
        options: crate::core::renderer::MessageRenderOptions,
    ) -> Self {
        self.render_options = options;
        self
    }

    pub fn with_acl_config(mut self, acl_metadata: serde_json::Value) -> Self {
        self.acl_metadata = acl_metadata;
        self
    }

    pub fn build_mqtt_options(&self) -> Result<MqttOptions> {
        let client_id = self
            .config
            .client_id
            .clone()
            .unwrap_or_else(|| format!("rua-mqtt-{}", uuid::Uuid::new_v4()));

        let port = if self.config.tls_enabled && self.config.port == 1883 {
            tracing::warn!("TLS enabled but port is 1883 (plain MQTT); auto-adjusting to 8883");
            8883
        } else {
            self.config.port
        };

        let websocket = match self.transport.as_str() {
            "" | "tcp" | "mqtt" => false,
            "websockets" => true,
            other => {
                return Err(RuaError::Validation {
                    message: format!("unsupported MQTT transport: {other}"),
                })
            }
        };
        let host = if websocket {
            let scheme = if self.config.tls_enabled { "wss" } else { "ws" };
            let host = &self.config.broker_url;
            let authority = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host.clone()
            };
            format!("{scheme}://{authority}:{port}/mqtt")
        } else {
            self.config.broker_url.clone()
        };
        let mut opts = MqttOptions::new(client_id, host, port);
        opts.set_keep_alive(Duration::from_secs(self.config.keep_alive_secs));
        opts.set_clean_session(self.clean_session);

        if let Some(ref creds) = self.config.credentials {
            opts.set_credentials(&creds.username, &creds.password);
        }

        if self.config.tls_enabled {
            let transport = self.build_tls_transport()?;
            opts.set_transport(match transport {
                Transport::Tls(config) if websocket => Transport::Wss(config),
                transport => transport,
            });
        } else if websocket {
            opts.set_transport(Transport::Ws);
        }

        Ok(opts)
    }

    fn build_tls_transport(&self) -> Result<Transport> {
        match &self.config.tls_ca_cert {
            Some(ca_path) => {
                let ca = std::fs::read(ca_path).map_err(|e| rua_core::RuaError::Runtime {
                    message: format!("failed to read TLS CA cert at {ca_path}: {e}"),
                })?;

                let client_auth = match (&self.config.tls_client_cert, &self.config.tls_client_key)
                {
                    (Some(cert_path), Some(key_path)) => {
                        let cert =
                            std::fs::read(cert_path).map_err(|e| rua_core::RuaError::Runtime {
                                message: format!(
                                    "failed to read TLS client cert at {cert_path}: {e}"
                                ),
                            })?;
                        let key =
                            std::fs::read(key_path).map_err(|e| rua_core::RuaError::Runtime {
                                message: format!(
                                    "failed to read TLS client key at {key_path}: {e}"
                                ),
                            })?;
                        Some((cert, key))
                    }
                    _ => None,
                };

                Ok(Transport::tls_with_config(
                    rumqttc::TlsConfiguration::Simple {
                        ca,
                        alpn: None,
                        client_auth,
                    },
                ))
            }
            None => Ok(Transport::tls_with_default_config()),
        }
    }

    pub async fn publish(&self, topic: &str, payload: &[u8]) -> Result<()> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: "mqtt client not started".into(),
            })?;
        client
            .publish(topic, self.config.qos.into(), false, payload)
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("mqtt publish failed: {e}"),
            })
    }

    pub fn render_content(parts: &[OutgoingContentPart]) -> String {
        let mut segments = Vec::new();
        for part in parts {
            match part {
                OutgoingContentPart::Text { text }
                | OutgoingContentPart::Markdown { text }
                | OutgoingContentPart::Refusal { text } => segments.push(text.clone()),
                OutgoingContentPart::Tool { name, status, .. } => {
                    segments.push(format!("[Tool: {name} - {status:?}]"));
                }
                OutgoingContentPart::Image { url, alt, .. } => {
                    let value = url
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .or(alt.as_deref())
                        .unwrap_or("");
                    segments.push(format!("[Image] {value}").trim_end().to_string());
                }
                OutgoingContentPart::File { url, name, .. } => {
                    let value = url
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .or(name.as_deref())
                        .unwrap_or("");
                    segments.push(format!("[File] {value}").trim_end().to_string());
                }
                OutgoingContentPart::Audio { .. } => {
                    segments.push("[Audio]".to_string());
                }
                OutgoingContentPart::Video { url, name, .. } => {
                    let value = url
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .or(name.as_deref())
                        .unwrap_or("");
                    segments.push(format!("[Video] {value}").trim_end().to_string());
                }
                OutgoingContentPart::Raw { value } => segments.push(value.to_string()),
                OutgoingContentPart::Thinking { .. } => {}
            }
        }
        segments.join("\n")
    }

    fn build_request_from_native_value(
        channel_id: ChannelId,
        native: serde_json::Value,
    ) -> Result<AgentRequest> {
        let raw_payload = native
            .get("payload")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let topic = native
            .get("topic")
            .and_then(|v| v.as_str())
            .unwrap_or("mqtt")
            .to_string();
        let mut message_id = mqtt_event_id(&native);

        if raw_payload.trim().is_empty() {
            return Err(RuaError::Runtime {
                message: "mqtt payload is empty".into(),
            });
        }

        let mut text = raw_payload.clone();
        let mut user_id = topic
            .split('/')
            .nth(1)
            .filter(|part| !part.is_empty())
            .unwrap_or("unknown-client")
            .to_string();
        let mut metadata = match native.get("metadata").cloned() {
            Some(value) if value.is_object() => value,
            Some(value) if !value.is_null() => serde_json::json!({ "metadata": value }),
            _ => serde_json::json!({}),
        };

        if let Ok(json_payload) = serde_json::from_str::<serde_json::Value>(&raw_payload) {
            if message_id.is_none() {
                message_id = mqtt_event_id(&json_payload);
            }
            if let Some(payload_text) = json_payload
                .get("text")
                .and_then(|value| value.as_str())
                .map(str::to_string)
            {
                text = payload_text;
            }

            if let Some(client_id) = json_payload
                .get("redirect_client_id")
                .or_else(|| json_payload.get("client_id"))
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
            {
                user_id = client_id.trim().to_string();
                if let Some(map) = metadata.as_object_mut() {
                    map.insert("client_id".into(), serde_json::json!(user_id.clone()));
                }
            }
        }

        metadata["client_id"] = user_id.clone().into();
        metadata["topic"] = topic.clone().into();

        if text.trim().is_empty() {
            return Err(RuaError::Runtime {
                message: "mqtt payload text is empty".into(),
            });
        }

        Ok(AgentChannelRequest {
            agent_id: native
                .get("agent_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            user_id,
            session_id: topic.clone(),
            input: vec![IncomingContentPart::Text { text }],
            channel_id,
            conversation_id: topic,
            message_id,
            metadata,
        })
    }

    async fn event_loop(
        self: Arc<Self>,
        mut event_loop: EventLoop,
        process: Arc<dyn Process>,
        client: AsyncClient,
        cancel: CancellationToken,
    ) {
        let mut tasks = tokio::task::JoinSet::new();
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::PlainText,
            self.render_options.clone(),
        ));
        loop {
            let event = tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tasks.join_next(), if !tasks.is_empty() => continue,
                event = event_loop.poll() => event,
            };
            match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    if let Err(error) = client
                        .subscribe(&self.config.subscribe_topic, self.config.qos.into())
                        .await
                    {
                        tracing::warn!(%error, "MQTT resubscribe failed");
                    }
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    let native = serde_json::json!({
                        "payload": String::from_utf8_lossy(&publish.payload),
                        "topic": publish.topic,
                    });
                    let mut req = match self.build_request_from_native(native) {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::debug!(%error, "MQTT message skipped");
                            continue;
                        }
                    };
                    if let (Some(acl), Some(metadata)) =
                        (self.acl_metadata.as_object(), req.metadata.as_object_mut())
                    {
                        metadata.extend(acl.clone());
                    }
                    let client = client.clone();
                    let topic = self.config.publish_topic.clone();
                    let qos = self.config.qos;
                    let send: pipeline::SendFn = Arc::new(move |to, message| {
                        let client = client.clone();
                        let topic = topic.replace(
                            "{client_id}",
                            to.metadata["client_id"]
                                .as_str()
                                .unwrap_or("unknown-client"),
                        );
                        Box::pin(async move {
                            let text = Self::render_content(&message.content);
                            if !text.is_empty() {
                                client
                                    .publish(topic, qos.into(), false, text.into_bytes())
                                    .await
                                    .map_err(|error| RuaError::Runtime {
                                        message: format!("MQTT reply: {error}"),
                                    })?;
                            }
                            Ok(())
                        })
                    });
                    let process = process.clone();
                    let renderer = renderer.clone();
                    tasks.spawn(async move {
                        if let Err(error) =
                            pipeline::run_inbound(process, req, None, renderer, send).await
                        {
                            tracing::warn!(%error, "MQTT processing failed");
                        }
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "MQTT reconnecting");
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
}

#[async_trait]
impl channel::Channel for MqttChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("mqtt channel already started");
            return Ok(());
        }

        let opts = self.build_mqtt_options()?;
        let (client, event_loop) = AsyncClient::new(opts, self.config.channel_capacity);

        let mut worker = Self::new(self.config.clone());
        worker.render_options = self.render_options.clone();
        worker.acl_metadata = self.acl_metadata.clone();
        worker.event_deduper = self.event_deduper.clone();
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(Arc::new(worker).event_loop(
            event_loop,
            process,
            client.clone(),
            cancel.clone(),
        ));
        *self.event_task.lock().await = Some((cancel, handle));

        *self.client.write().await = Some(client);
        *started = true;
        tracing::info!("mqtt channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            return Ok(());
        }

        if let Some(client) = self.client.write().await.take() {
            let _ = client.try_disconnect();
        }
        if let Some((cancel, mut task)) = self.event_task.lock().await.take() {
            cancel.cancel();
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }

        *started = false;
        tracing::info!("mqtt channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let text = Self::render_content(&msg.content);
        if text.is_empty() {
            tracing::debug!("empty content, skipping mqtt send");
            return Ok(());
        }
        let topic = self.config.publish_topic.replace(
            "{client_id}",
            to.metadata["client_id"]
                .as_str()
                .or(to.sender_id.as_deref())
                .unwrap_or("unknown-client"),
        );
        self.publish(&topic, text.as_bytes()).await
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let req = Self::build_request_from_native_value(self.channel_id.clone(), native)?;
        if mqtt_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate mqtt event".into(),
            });
        }
        Ok(req)
    }
}

fn mqtt_event_id(value: &serde_json::Value) -> Option<String> {
    ["message_id", "messageId", "event_id", "eventId", "id"]
        .iter()
        .find_map(|key| {
            value
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn mqtt_check_duplicate_request(
    deduper: &Arc<StdMutex<EventDeduper>>,
    req: &AgentRequest,
) -> Result<bool> {
    let Some(message_id) = req
        .message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    let mut deduper = deduper.lock().map_err(|_| rua_core::RuaError::Runtime {
        message: "mqtt event deduper lock poisoned".into(),
    })?;
    Ok(deduper.check_and_mark(format!("mqtt:message:{message_id}"))
        == EventDedupDecision::Duplicate)
}
