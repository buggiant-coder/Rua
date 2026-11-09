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

async fn make_manager() -> (VoiceManager, TempDir) {
    let tmp = TempDir::new().unwrap();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).unwrap();

    let tts = TtsConfig {
        provider_type: "openai".into(),
        provider_id: "old-provider".into(),
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
    (mgr, tmp)
}

#[tokio::test]
async fn store_tts_config_swaps_snapshot() {
    let (mgr, _tmp) = make_manager().await;
    assert_eq!(mgr.get_tts_config().provider_id, "old-provider");

    mgr.store_tts_config(TtsConfig {
        provider_type: "openai".into(),
        provider_id: "new-provider".into(),
        model: "tts-2".into(),
        voice: "nova".into(),
        speed: 1.5,
        language: Some("ja-JP".into()),
    });

    let cfg = mgr.get_tts_config();
    assert_eq!(cfg.provider_id, "new-provider");
    assert_eq!(cfg.model, "tts-2");
    assert_eq!(cfg.voice, "nova");
    assert!((cfg.speed - 1.5).abs() < f64::EPSILON);
    assert_eq!(cfg.language.as_deref(), Some("ja-JP"));
}

#[tokio::test]
async fn store_asr_config_swaps_snapshot() {
    let (mgr, _tmp) = make_manager().await;
    assert_eq!(mgr.get_asr_config().provider_type, "disabled");

    mgr.store_asr_config(AsrConfig {
        provider_type: "openai".into(),
        provider_id: "asr-new".into(),
        model: "whisper-2".into(),
        language: Some("zh-CN".into()),
    });

    let cfg = mgr.get_asr_config();
    assert_eq!(cfg.provider_type, "openai");
    assert_eq!(cfg.provider_id, "asr-new");
    assert_eq!(cfg.model, "whisper-2");
    assert_eq!(cfg.language.as_deref(), Some("zh-CN"));
}

#[tokio::test]
async fn update_config_swaps_both() {
    let (mgr, _tmp) = make_manager().await;
    assert_eq!(mgr.get_tts_config().provider_id, "old-provider");
    assert_eq!(mgr.get_asr_config().provider_type, "disabled");

    let mut root = rua_config::types::RootConfig::default();
    root.agents
        .extra
        .insert("tts_provider_id".into(), "bulk-new".into());
    root.agents.extra.insert("tts_model".into(), "tts-3".into());
    root.agents
        .extra
        .insert("transcription_provider_type".into(), "azure".into());
    root.agents
        .extra
        .insert("transcription_provider_id".into(), "az-1".into());

    mgr.update_config(&root);

    assert_eq!(mgr.get_tts_config().provider_id, "bulk-new");
    assert_eq!(mgr.get_tts_config().model, "tts-3");
    assert_eq!(mgr.get_asr_config().provider_type, "azure");
    assert_eq!(mgr.get_asr_config().provider_id, "az-1");
}
