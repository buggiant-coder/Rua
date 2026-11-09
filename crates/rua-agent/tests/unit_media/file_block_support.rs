use llm_multiplex::ModelInfo;
use rua_agent::{FileBlockSupportFormatter, MediaType};

fn model_info(
    supports_image: Option<bool>,
    supports_video: Option<bool>,
    supports_multimodal: Option<bool>,
) -> ModelInfo {
    ModelInfo {
        id: "media-contract-model".into(),
        name: "media-contract-model".into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal,
        supports_image,
        supports_video,
        probe_source: None,
        generate_kwargs: Default::default(),
    }
}

#[test]
fn text_only_formatter_accepts_text_and_rejects_all_media() {
    let formatter = FileBlockSupportFormatter::text_only("text-only-provider");

    assert_eq!(formatter.provider_id(), "text-only-provider");
    assert!(formatter.is_supported(MediaType::Text));

    for media_type in [
        MediaType::ImageUrl,
        MediaType::ImageLocal,
        MediaType::Video,
        MediaType::Audio,
        MediaType::Unknown,
    ] {
        assert!(
            !formatter.is_supported(media_type),
            "text-only formatter must reject {media_type:?}"
        );
        let err = formatter
            .validate(media_type)
            .expect_err("unsupported media should return an AgentError");
        assert!(
            err.to_string().contains("text-only-provider"),
            "error should name the provider: {err}"
        );
    }
}

#[test]
fn provider_formatter_uses_explicit_image_and_video_capabilities() {
    let info = model_info(Some(true), Some(false), Some(true));
    let formatter = FileBlockSupportFormatter::for_provider("vision-provider", &info);

    assert_eq!(formatter.provider_id(), "vision-provider");
    assert!(formatter.supports_image());
    assert!(!formatter.supports_video());
    assert!(formatter.is_supported(MediaType::ImageUrl));
    assert!(formatter.is_supported(MediaType::ImageLocal));
    assert!(!formatter.is_supported(MediaType::Video));
}

#[test]
fn missing_media_capabilities_default_to_text_only() {
    let info = model_info(None, None, None);
    let formatter = FileBlockSupportFormatter::for_provider("unknown-cap-provider", &info);

    assert!(formatter.is_supported(MediaType::Text));
    assert!(!formatter.supports_image());
    assert!(!formatter.supports_video());
    assert!(!formatter.is_supported(MediaType::ImageUrl));
    assert!(!formatter.is_supported(MediaType::Video));
}

#[test]
fn supports_multimodal_flag_alone_does_not_enable_media_without_specific_caps() {
    let info = model_info(None, None, Some(true));
    let formatter = FileBlockSupportFormatter::for_provider("coarse-flag-provider", &info);

    assert!(
        !formatter.is_supported(MediaType::ImageUrl),
        "Rust requires explicit image capability, not only supports_multimodal"
    );
    assert!(
        !formatter.is_supported(MediaType::Video),
        "Rust requires explicit video capability, not only supports_multimodal"
    );
}

#[test]
fn audio_is_rejected_even_when_other_media_is_supported() {
    let info = model_info(Some(true), Some(true), Some(true));
    let formatter = FileBlockSupportFormatter::for_provider("vision-video-provider", &info);

    assert!(formatter.is_supported(MediaType::ImageUrl));
    assert!(formatter.is_supported(MediaType::Video));
    assert!(
        !formatter.is_supported(MediaType::Audio),
        "audio is not a supported Rust provider media block today"
    );
}
