use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, read_json, request};

fn env_value<'a>(items: &'a [serde_json::Value], key: &str) -> Option<&'a str> {
    items
        .iter()
        .find(|item| item["key"] == key)
        .and_then(|item| item["value"].as_str())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_language_defaults_to_zh_roundtrips_and_rejects_invalid_values() {
    let app = build_app();

    let initial = request(&app.app, Method::GET, "/api/settings/language", None, None).await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(read_json(initial).await, json!({"language": "zh"}));

    let update = request(
        &app.app,
        Method::PUT,
        "/api/settings/language",
        Some(json!({"language": "zh"})),
        None,
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(read_json(update).await, json!({"language": "zh"}));

    let after = request(&app.app, Method::GET, "/api/settings/language", None, None).await;
    assert_eq!(read_json(after).await, json!({"language": "zh"}));

    let invalid = request(
        &app.app,
        Method::PUT,
        "/api/settings/language",
        Some(json!({"language": "xx"})),
        None,
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid_body = read_json(invalid).await;
    assert!(
        invalid_body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Invalid language")),
        "expected readable invalid language detail, got {invalid_body:?}"
    );

    let unchanged = request(&app.app, Method::GET, "/api/settings/language", None, None).await;
    assert_eq!(read_json(unchanged).await, json!({"language": "zh"}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_language_accepts_rust_supported_values_and_rejects_empty_or_missing() {
    let app = build_app();

    for language in ["en", "zh", "zh-CN", "ja", "ru"] {
        let resp = request(
            &app.app,
            Method::PUT,
            "/api/settings/language",
            Some(json!({ "language": language })),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(read_json(resp).await, json!({ "language": language }));
    }

    for body in [json!({ "language": "" }), json!({ "lang": "zh" })] {
        let resp = request(
            &app.app,
            Method::PUT,
            "/api/settings/language",
            Some(body),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(read_json(resp).await["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Invalid language")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_language_rejects_qwenpaw_values_without_rust_locale_bundles() {
    let app = build_app();

    let seed = request(
        &app.app,
        Method::PUT,
        "/api/settings/language",
        Some(json!({ "language": "ja" })),
        None,
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);

    for language in ["pt-BR", "id", "vi"] {
        let resp = request(
            &app.app,
            Method::PUT,
            "/api/settings/language",
            Some(json!({ "language": language })),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{language}");
        let body = read_json(resp).await;
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("Invalid language")),
            "expected invalid language detail for {language}, got {body:?}"
        );

        let after = request(&app.app, Method::GET, "/api/settings/language", None, None).await;
        assert_eq!(
            read_json(after).await,
            json!({"language": "ja"}),
            "unsupported QwenPaw language {language} must not mutate Rust settings"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_language_update_preserves_other_settings_keys() {
    let app = build_app();
    let settings_path = app.data_dir.join("settings.json");
    rua_config::atomic_write_json(
        &settings_path,
        &json!({
            "language": "en",
            "theme": "dark",
            "telemetry": false
        }),
    )
    .expect("seed settings");

    let resp = request(
        &app.app,
        Method::PUT,
        "/api/settings/language",
        Some(json!({ "language": "zh-CN" })),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(read_json(resp).await, json!({"language": "zh-CN"}));

    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(settings_path).expect("settings"))
            .expect("settings json");
    assert_eq!(stored["language"], "zh-CN");
    assert_eq!(stored["theme"], "dark");
    assert_eq!(stored["telemetry"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn envs_map_put_get_and_delete_roundtrip() {
    let app = build_app();

    let empty = request(&app.app, Method::GET, "/api/envs", None, None).await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(read_json(empty).await, json!([]));

    let put = request(
        &app.app,
        Method::PUT,
        "/api/envs",
        Some(json!({
            "INTEGRATION_TEST_KEY": "value_1",
            "ANOTHER_KEY": "value_2"
        })),
        None,
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    let put_body = read_json(put).await;
    let put_items = put_body.as_array().expect("env list");
    assert_eq!(
        env_value(put_items, "INTEGRATION_TEST_KEY"),
        Some("value_1")
    );
    assert_eq!(env_value(put_items, "ANOTHER_KEY"), Some("value_2"));

    let get = request(&app.app, Method::GET, "/api/envs", None, None).await;
    assert_eq!(get.status(), StatusCode::OK);
    let get_body = read_json(get).await;
    let get_items = get_body.as_array().expect("env list");
    assert_eq!(
        env_value(get_items, "INTEGRATION_TEST_KEY"),
        Some("value_1")
    );
    assert_eq!(env_value(get_items, "ANOTHER_KEY"), Some("value_2"));

    let delete = request(
        &app.app,
        Method::DELETE,
        "/api/envs/INTEGRATION_TEST_KEY",
        None,
        None,
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    let delete_body = read_json(delete).await;
    let delete_items = delete_body.as_array().expect("env list");
    assert_eq!(env_value(delete_items, "INTEGRATION_TEST_KEY"), None);
    assert_eq!(env_value(delete_items, "ANOTHER_KEY"), Some("value_2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn envs_accept_array_payload_and_sort_response_by_key() {
    let app = build_app();

    let put = request(
        &app.app,
        Method::PUT,
        "/api/envs",
        Some(json!([
            {
                "key": "Z_LAST",
                "value": "z",
                "masked": true,
                "ui_only": "not-persisted"
            },
            {
                "key": "A_FIRST",
                "value": "a"
            }
        ])),
        None,
    )
    .await;

    assert_eq!(put.status(), StatusCode::OK);
    let put_body = read_json(put).await;
    assert_eq!(
        put_body,
        json!([
            {"key": "A_FIRST", "value": "a", "masked": null},
            {"key": "Z_LAST", "value": "z", "masked": null}
        ])
    );

    let get = request(&app.app, Method::GET, "/api/envs", None, None).await;
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(read_json(get).await, put_body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn envs_reject_non_collection_payload_without_mutating_existing_values() {
    let app = build_app();

    let seed = request(
        &app.app,
        Method::PUT,
        "/api/envs",
        Some(json!({ "KEEP_ME": "value" })),
        None,
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);

    let invalid = request(
        &app.app,
        Method::PUT,
        "/api/envs",
        Some(json!("KEY=value")),
        None,
    )
    .await;

    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(invalid).await,
        json!({"detail": "Expected JSON object or array"})
    );

    let get = request(&app.app, Method::GET, "/api/envs", None, None).await;
    let get_body = read_json(get).await;
    let items = get_body.as_array().expect("env list");
    assert_eq!(env_value(items, "KEEP_ME"), Some("value"));
}
