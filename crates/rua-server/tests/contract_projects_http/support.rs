use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use rua_channel::core::console::ConsoleChatFacade;
use rua_server::{router, AppState, ServerProfile};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    pub _temp: TempDir,
}

pub fn build_app() -> TestApp {
    build_app_with_console_facade(None)
}

pub fn build_app_with_console_facade(facade: Option<Arc<dyn ConsoleChatFacade>>) -> TestApp {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut state = AppState::from_tempdir(temp.path().join("data"), ServerProfile::Test);
    if let Some(facade) = facade {
        state.console_facade = facade;
    }
    let app = router::build_app(state.clone(), None);
    TestApp {
        app,
        state,
        _temp: temp,
    }
}

pub fn local_device_id(state: &AppState) -> String {
    state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("local Device")
}

pub async fn request(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("response")
}

pub async fn json(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
