#[path = "../voice_test_helpers.rs"]
mod voice_test_helpers;

use async_trait::async_trait;
use futures::stream;
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::{Result, RuaError};
use rua_voice::{AsrConfig, TtsConfig, VoiceManager};
use std::pin::Pin;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

struct StubRuntime;

#[async_trait]
impl AgentRuntime for StubRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        Ok(Box::pin(stream::empty()))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> Result<()> {
        Ok(())
    }

    async fn warmup(&self, _ctx: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

async fn make_manager(
    base_url: String,
    api_key: &str,
    provider_id: &str,
) -> (VoiceManager, TempDir) {
    let tmp = TempDir::new().unwrap();
    let secret_dir = tmp.path().join("secret");
    let provider_dir = secret_dir.join("providers/providers");
    std::fs::create_dir_all(&provider_dir).unwrap();
    std::fs::write(
        provider_dir.join(format!("{provider_id}.json")),
        serde_json::json!({
            "base_url": base_url,
            "api_key": api_key,
        })
        .to_string(),
    )
    .unwrap();

    let tts = TtsConfig {
        provider_type: "disabled".into(),
        provider_id: String::new(),
        model: "tts-1".into(),
        voice: "alloy".into(),
        speed: 1.0,
        language: None,
    };
    let asr = AsrConfig {
        provider_type: "openai".into(),
        provider_id: provider_id.into(),
        model: "whisper-1".into(),
        language: None,
    };
    let runtime: Arc<dyn AgentRuntime> = Arc::new(StubRuntime);
    let registry =
        voice_test_helpers::StubRegistry::with_runtime("default", tmp.path(), runtime).await;
    let mgr =
        VoiceManager::with_configs("s".into(), tts, asr, secret_dir, registry, "default".into());
    (mgr, tmp)
}

async fn make_manager_with_asr_config(asr: AsrConfig) -> (VoiceManager, TempDir) {
    let tmp = TempDir::new().unwrap();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(secret_dir.join("providers/providers")).unwrap();

    let tts = TtsConfig {
        provider_type: "disabled".into(),
        provider_id: String::new(),
        model: "tts-1".into(),
        voice: "alloy".into(),
        speed: 1.0,
        language: None,
    };
    let runtime: Arc<dyn AgentRuntime> = Arc::new(StubRuntime);
    let registry =
        voice_test_helpers::StubRegistry::with_runtime("default", tmp.path(), runtime).await;
    let mgr =
        VoiceManager::with_configs("s".into(), tts, asr, secret_dir, registry, "default".into());
    (mgr, tmp)
}

async fn spawn_asr_server(body: &'static str) -> (String, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0; 8192];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = tx.send(request);

        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        let _ = socket.shutdown().await;
    });

    (format!("http://{addr}"), rx)
}

#[tokio::test]
async fn asr_request_uses_openai_compatible_endpoint_auth_and_trims_text() {
    let (base_url, request_rx) = spawn_asr_server(r#"{"text":"  hello world  "}"#).await;
    let (manager, _tmp) = make_manager(base_url, "sk-test", "asr-provider").await;

    let text = manager
        .transcribe_audio(b"fake audio".to_vec(), "clip.wav", None)
        .await
        .expect("asr should succeed");

    assert_eq!(text, "hello world");

    let request = request_rx.await.expect("server should capture request");
    assert!(
        request.starts_with("POST /audio/transcriptions HTTP/1.1"),
        "unexpected request: {request}"
    );
    assert!(
        request.contains("authorization: Bearer sk-test"),
        "missing auth header: {request}"
    );
    assert!(
        request.contains("name=\"model\""),
        "missing model part: {request}"
    );
    assert!(
        request.contains("whisper-1"),
        "missing configured model: {request}"
    );
    assert!(
        request.contains("filename=\"clip.wav\""),
        "missing file part: {request}"
    );
}

#[tokio::test]
async fn asr_request_omits_auth_for_blank_key_and_honors_model_override() {
    let (base_url, request_rx) = spawn_asr_server(r#"{"text":"override transcript"}"#).await;
    let (manager, _tmp) = make_manager(format!("{base_url}/"), "", "local-asr").await;

    let text = manager
        .transcribe_audio(b"local audio".to_vec(), "local.wav", Some("custom-whisper"))
        .await
        .expect("blank-key ASR provider should be usable");

    assert_eq!(text, "override transcript");

    let request = request_rx.await.expect("server should capture request");
    assert!(
        request.starts_with("POST /audio/transcriptions HTTP/1.1"),
        "base_url trailing slash should not duplicate path separators: {request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("authorization:"),
        "blank API keys must not produce an Authorization header: {request}"
    );
    assert!(
        request.contains("custom-whisper"),
        "call-time model override should win over configured ASR model: {request}"
    );
}

#[tokio::test]
async fn asr_disabled_local_and_missing_provider_return_clear_configuration_errors() {
    let (disabled_manager, _tmp) = make_manager_with_asr_config(AsrConfig {
        provider_type: "disabled".into(),
        provider_id: String::new(),
        model: "whisper-1".into(),
        language: None,
    })
    .await;

    let disabled_err = disabled_manager
        .transcribe_audio(b"audio".to_vec(), "clip.wav", None)
        .await
        .expect_err("disabled ASR should not silently succeed");
    assert!(
        disabled_err
            .to_string()
            .contains("audio transcription is disabled"),
        "unexpected disabled ASR error: {disabled_err}"
    );

    let (local_manager, _tmp) = make_manager_with_asr_config(AsrConfig {
        provider_type: "local_whisper".into(),
        provider_id: String::new(),
        model: "whisper-1".into(),
        language: None,
    })
    .await;

    let local_err = local_manager
        .transcribe_audio(b"audio".to_vec(), "clip.wav", None)
        .await
        .expect_err("unavailable local whisper should not fall through to provider creds");
    assert!(
        local_err
            .to_string()
            .contains("local whisper transcription is not available"),
        "unexpected local whisper error: {local_err}"
    );

    let (missing_manager, _tmp) = make_manager_with_asr_config(AsrConfig {
        provider_type: "openai".into(),
        provider_id: "missing-asr-provider".into(),
        model: "whisper-1".into(),
        language: None,
    })
    .await;

    let missing_err = missing_manager
        .transcribe_audio(b"audio".to_vec(), "clip.wav", None)
        .await
        .expect_err("missing provider config should fail clearly");
    assert!(
        missing_err
            .to_string()
            .contains("missing-asr-provider.json"),
        "unexpected missing provider error: {missing_err}"
    );
}
