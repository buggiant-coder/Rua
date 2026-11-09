use std::path::Path;

use rua_agent::{MediaLimits, MediaProcessor, MediaType};

#[test]
fn media_type_detects_common_text_image_audio_video_inputs() {
    assert_eq!(MediaType::from_mime("text/plain"), MediaType::Text);
    assert_eq!(MediaType::from_mime("IMAGE/PNG"), MediaType::ImageUrl);
    assert_eq!(MediaType::from_mime("audio/wav"), MediaType::Audio);
    assert_eq!(MediaType::from_mime("video/mp4"), MediaType::Video);
    assert_eq!(
        MediaType::from_mime("application/octet-stream"),
        MediaType::Unknown
    );

    assert_eq!(MediaType::from_extension("md"), MediaType::Text);
    assert_eq!(MediaType::from_extension("JPG"), MediaType::ImageLocal);
    assert_eq!(MediaType::from_extension("m4a"), MediaType::Audio);
    assert_eq!(MediaType::from_extension("webm"), MediaType::Video);
    assert_eq!(MediaType::from_extension("bin"), MediaType::Unknown);
}

#[test]
fn workspace_path_validation_normalizes_dotdot_before_boundary_check() {
    let processor = MediaProcessor::new();
    let workspace = Path::new("/tmp/rua-workspace");

    let inside = processor
        .validate_workspace_path(Path::new("nested/../file.txt"), workspace)
        .expect("normalized relative path should stay inside workspace");
    assert_eq!(inside, Path::new("/tmp/rua-workspace/file.txt"));

    let err = processor
        .validate_workspace_path(Path::new("../outside.txt"), workspace)
        .expect_err("parent traversal should be rejected");
    assert!(err.to_string().contains("outside workspace"));
}

#[test]
fn absolute_workspace_path_is_accepted_after_normalization() {
    let processor = MediaProcessor::new();
    let workspace = Path::new("/tmp/rua-workspace");

    let path = processor
        .validate_workspace_path(Path::new("/tmp/rua-workspace/a/../b.txt"), workspace)
        .expect("absolute path that normalizes inside workspace should be accepted");

    assert_eq!(path, Path::new("/tmp/rua-workspace/b.txt"));
}

#[test]
fn base64_size_validation_uses_decoded_size_estimate() {
    let processor = MediaProcessor::with_limits(MediaLimits {
        max_base64_size: 3,
        ..MediaLimits::default()
    });

    processor
        .validate_base64_size("abcd")
        .expect("4 base64 bytes estimate to 3 decoded bytes");

    let err = processor
        .validate_base64_size("abcdefgh")
        .expect_err("8 base64 bytes estimate above 3 decoded bytes");
    assert!(err.to_string().contains("Base64 data too large"));
}

#[test]
fn image_url_validation_accepts_http_https_and_data_only() {
    let processor = MediaProcessor::new();

    for url in [
        "https://example.com/image.png",
        "http://example.com/image.png",
        "data:image/png;base64,abc",
    ] {
        processor
            .validate_image_url(url)
            .expect("supported image URL scheme");
    }

    assert!(processor.validate_image_url("").is_err());
    let err = processor
        .validate_image_url("file:///tmp/image.png")
        .expect_err("local file URLs are not image URLs at this boundary");
    assert!(err.to_string().contains("Invalid image URL scheme"));
}

#[test]
fn image_url_validation_rejects_non_image_data_urls() {
    let processor = MediaProcessor::new();

    for url in ["data:text/html;base64,PGgxPmhpPC9oMT4=", "data:;base64,abc"] {
        let err = processor
            .validate_image_url(url)
            .expect_err("image URL validation should reject non-image data URLs");
        assert!(err
            .to_string()
            .contains("Invalid image data URL media type"));
    }
}

#[test]
fn audio_and_video_support_follow_explicit_limits() {
    let disabled = MediaProcessor::new();
    assert!(disabled.validate_media_type(MediaType::Audio).is_err());
    assert!(disabled.validate_media_type(MediaType::Video).is_err());

    let enabled = MediaProcessor::with_limits(MediaLimits {
        audio_enabled: true,
        video_enabled: true,
        ..MediaLimits::default()
    });
    enabled
        .validate_media_type(MediaType::Audio)
        .expect("audio can be explicitly enabled");
    enabled
        .validate_media_type(MediaType::Video)
        .expect("video can be explicitly enabled");
}
