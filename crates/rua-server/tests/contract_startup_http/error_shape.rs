use axum::http::StatusCode;
use axum::response::IntoResponse;
use rua_server::errors::ValidationError;
use rua_server::ApiError;

#[tokio::test]
async fn validation_error_returns_fastapi_detail_array() {
    let err = ApiError::Validation(vec![
        ValidationError {
            loc: vec!["body".into(), "name".into()],
            msg: "field required".into(),
            error_type: "value_error.missing".into(),
        },
        ValidationError {
            loc: vec!["body".into(), "age".into()],
            msg: "must be positive".into(),
            error_type: "value_error".into(),
        },
    ]);

    let resp = err.into_response();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let detail = parsed["detail"].as_array().expect("detail array");

    assert_eq!(detail.len(), 2);
    assert_eq!(detail[0]["loc"], serde_json::json!(["body", "name"]));
    assert_eq!(detail[0]["msg"], "field required");
    assert_eq!(detail[0]["type"], "value_error.missing");
    assert_eq!(detail[1]["loc"], serde_json::json!(["body", "age"]));
    assert_eq!(detail[1]["msg"], "must be positive");
    assert_eq!(detail[1]["type"], "value_error");
}

#[tokio::test]
async fn model_retry_deadline_returns_service_unavailable_with_detail() {
    let error = llm_multiplex::LlmError::RetryDeadlineExceeded {
        provider: "test".into(),
        timeout: std::time::Duration::from_secs(20),
    };
    let response = ApiError::from(error).into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        parsed["detail"],
        "model call retry deadline exceeded after 20s"
    );
}
