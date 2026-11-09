#[path = "../voice_test_helpers.rs"]
mod voice_test_helpers;

use rua_voice::{AsrConfig, TtsConfig, VoiceManager};
use std::sync::Arc;
use tempfile::TempDir;

mod stub_runtime {
    use async_trait::async_trait;
    use futures::stream;
    use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
    use rua_core::event::RuntimeEvent;
    use rua_core::{Result, RuaError};
    use std::pin::Pin;

    pub struct StubRuntime;

    #[async_trait]
    impl AgentRuntime for StubRuntime {
        async fn stream(
            &self,
            _req: AgentRequest,
        ) -> Result<
            Pin<
                Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>,
            >,
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
}

#[tokio::test]
async fn hundred_tts_reads_zero_disk_access() {
    let tmp = TempDir::new().unwrap();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).unwrap();

    let tts = TtsConfig {
        provider_type: "openai".into(),
        provider_id: "test-provider".into(),
        model: "tts-1".into(),
        voice: "alloy".into(),
        speed: 1.0,
        language: None,
    };
    let asr = AsrConfig {
        provider_type: "disabled".into(),
        provider_id: String::new(),
        model: "whisper-1".into(),
        language: None,
    };

    let runtime: Arc<dyn rua_contract::agent::AgentRuntime> = Arc::new(stub_runtime::StubRuntime);
    let registry =
        voice_test_helpers::StubRegistry::with_runtime("default", tmp.path(), runtime).await;
    let mgr =
        VoiceManager::with_configs("s".into(), tts, asr, secret_dir, registry, "default".into());

    for i in 0..100 {
        let cfg = mgr.get_tts_config();
        assert_eq!(cfg.provider_id, "test-provider", "iter {i}");
        assert_eq!(cfg.model, "tts-1", "iter {i}");
    }
}

#[tokio::test]
async fn hundred_asr_reads_zero_disk_access() {
    let tmp = TempDir::new().unwrap();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).unwrap();

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
        provider_id: "asr-prov".into(),
        model: "whisper-1".into(),
        language: Some("en".into()),
    };

    let runtime: Arc<dyn rua_contract::agent::AgentRuntime> = Arc::new(stub_runtime::StubRuntime);
    let registry =
        voice_test_helpers::StubRegistry::with_runtime("default", tmp.path(), runtime).await;
    let mgr =
        VoiceManager::with_configs("s".into(), tts, asr, secret_dir, registry, "default".into());

    for i in 0..100 {
        let cfg = mgr.get_asr_config();
        assert_eq!(cfg.provider_id, "asr-prov", "iter {i}");
        assert_eq!(cfg.language.as_deref(), Some("en"), "iter {i}");
    }
}

#[tokio::test]
async fn with_dirs_loads_once_then_no_disk() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&data_dir).unwrap();
    rua_config::save_config(
        &rua_config::RootConfig::default(),
        &data_dir.join("config.json"),
    )
    .unwrap();
    std::fs::create_dir_all(&secret_dir).unwrap();

    let config = r#"{"agents":{"tts_provider_id":"from-disk","tts_model":"tts-2"}}"#;
    std::fs::write(data_dir.join("config.json"), config).unwrap();

    let runtime: Arc<dyn rua_contract::agent::AgentRuntime> = Arc::new(stub_runtime::StubRuntime);
    let registry =
        voice_test_helpers::StubRegistry::with_runtime("default", tmp.path(), runtime).await;
    let mgr = VoiceManager::with_dirs(
        "s".into(),
        data_dir.clone(),
        secret_dir,
        registry,
        "default".into(),
    )
    .expect("valid test configuration");

    std::fs::remove_file(data_dir.join("config.json")).unwrap();

    for i in 0..100 {
        let cfg = mgr.get_tts_config();
        assert_eq!(cfg.provider_id, "from-disk", "iter {i}");
        assert_eq!(cfg.model, "tts-2", "iter {i}");
    }
}
