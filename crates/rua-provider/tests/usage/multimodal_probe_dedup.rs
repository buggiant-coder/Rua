use llm_multiplex::ProbeResult;

#[test]
fn probe_result_roundtrip_full() {
    let result = ProbeResult {
        supports_image: true,
        supports_video: false,
        supports_multimodal: true,
        image_message: "Multimodal supported".into(),
        stage1_passed: true,
        stage2_passed: true,
        error: None,
    };

    let json = serde_json::to_string(&result).unwrap();
    let back: ProbeResult = serde_json::from_str(&json).unwrap();

    assert!(back.supports_image);
    assert!(!back.supports_video);
    assert!(back.supports_multimodal);
    assert_eq!(back.image_message, "Multimodal supported");
    assert!(back.stage1_passed);
    assert!(back.stage2_passed);
    assert!(back.error.is_none());
}

#[test]
fn probe_result_roundtrip_unsupported() {
    let result = ProbeResult {
        supports_image: false,
        supports_video: false,
        supports_multimodal: false,
        image_message: "Multimodal not supported".into(),
        stage1_passed: false,
        stage2_passed: false,
        error: Some("timeout".into()),
    };

    let json = serde_json::to_string(&result).unwrap();
    let back: ProbeResult = serde_json::from_str(&json).unwrap();

    assert!(!back.supports_image);
    assert!(!back.supports_multimodal);
    assert_eq!(back.error, Some("timeout".into()));
}

#[test]
fn probe_result_deserialize_historical_4field_json() {
    let historical = r#"{
        "supports_image": true,
        "supports_video": false,
        "supports_multimodal": true,
        "image_message": "Multimodal supported"
    }"#;

    let result: ProbeResult = serde_json::from_str(historical).unwrap();
    assert!(result.supports_image);
    assert!(result.supports_multimodal);
    assert!(!result.stage1_passed);
    assert!(!result.stage2_passed);
    assert!(result.error.is_none());
}

#[test]
fn probe_result_serialization_schema_keys() {
    let result = ProbeResult {
        supports_image: true,
        supports_video: false,
        supports_multimodal: true,
        image_message: "Multimodal supported".into(),
        stage1_passed: true,
        stage2_passed: true,
        error: None,
    };

    let json = serde_json::to_value(&result).unwrap();
    let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
    assert!(keys.contains(&&"supports_image".to_string()));
    assert!(keys.contains(&&"supports_video".to_string()));
    assert!(keys.contains(&&"supports_multimodal".to_string()));
    assert!(keys.contains(&&"image_message".to_string()));
    assert!(keys.contains(&&"stage1_passed".to_string()));
    assert!(keys.contains(&&"stage2_passed".to_string()));
    assert!(
        !keys.contains(&&"error".to_string()),
        "error=None should be skipped"
    );
}
