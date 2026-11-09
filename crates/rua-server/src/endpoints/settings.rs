use std::path::Path;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rua_config::SettingsStore;

use crate::endpoints::dto::{self, settings::language_from_settings};
use crate::errors::ApiError;
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

const SUPPORTED_GLOBAL_LANGUAGES: &[&str] = &["en", "zh", "zh-CN", "ja", "ru"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/language", get(get_language).put(set_language))
        .route("/version", get(get_version))
        .route("/debug/logs", get(get_debug_logs))
}

async fn build_store(state: &AppState) -> Result<SettingsStore, ApiError> {
    let runtime = state.registry.get_or_create("default").await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for settings store: {e}"
        ))
    })?;
    Ok(SettingsStore::new(runtime.rua_root()))
}

async fn get_language(State(state): State<AppState>) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.load() {
        Ok(map) => (StatusCode::OK, Json(language_from_settings(&map))).into_response(),
        Err(e) => ApiError::Internal(format!("failed to read settings.json: {e}")).into_response(),
    }
}

async fn set_language(
    State(state): State<AppState>,
    Json(body): Json<dto::LanguageSetting>,
) -> Response {
    if !SUPPORTED_GLOBAL_LANGUAGES.contains(&body.language.as_str()) {
        return ApiError::BadRequest(format!(
            "Invalid language '{}'. Must be one of: {}",
            body.language,
            SUPPORTED_GLOBAL_LANGUAGES.join(", ")
        ))
        .into_response();
    }

    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let value = serde_json::Value::String(body.language.clone());
    if let Err(e) = store.set("language", value) {
        return ApiError::Internal(format!("failed to write settings.json: {e}")).into_response();
    }
    if let Ok(map) = store.load() {
        let settings_val = serde_json::to_value(&map).unwrap_or_default();
        let _ = publish_sync_fact(
            &state,
            SyncPublicationFact::RootSettingsCommitted {
                settings: settings_val,
            },
        )
        .await;
    }
    match store.load() {
        Ok(map) => (StatusCode::OK, Json(language_from_settings(&map))).into_response(),
        Err(e) => ApiError::Internal(format!("failed to read settings.json: {e}")).into_response(),
    }
}

async fn get_version() -> Response {
    let info = dto::VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build: None,
        commit: None,
        extra: serde_json::Map::new(),
    };
    (StatusCode::OK, Json(info)).into_response()
}

async fn get_debug_logs(State(state): State<AppState>) -> Response {
    let runtime = match state.registry.get_or_create("default").await {
        Ok(r) => r,
        Err(e) => {
            return ApiError::Internal(format!(
                "failed to obtain workspace runtime for debug logs: {e}"
            ))
            .into_response();
        }
    };
    let log_path = runtime.rua_root().join("logs").join("rua.log");
    let mut extra = serde_json::Map::new();
    extra.insert(
        "path".into(),
        serde_json::Value::String(log_path.display().to_string()),
    );

    match std::fs::metadata(&log_path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
            let content = tail_text_file(&log_path, 200, 512 * 1024);
            extra.insert("exists".into(), serde_json::Value::Bool(true));
            extra.insert("lines".into(), serde_json::json!(200));
            extra.insert(
                "updated_at".into(),
                mtime.map_or(serde_json::Value::Null, |v| serde_json::json!(v)),
            );
            extra.insert("size".into(), serde_json::json!(meta.len()));
            extra.insert("content".into(), serde_json::Value::String(content));
            let body = dto::DebugLogsResponse {
                logs: vec![],
                total: 0,
                extra,
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            extra.insert("exists".into(), serde_json::Value::Bool(false));
            extra.insert("lines".into(), serde_json::json!(200));
            extra.insert("updated_at".into(), serde_json::Value::Null);
            extra.insert("size".into(), serde_json::json!(0));
            extra.insert("content".into(), serde_json::Value::String(String::new()));
            let body = dto::DebugLogsResponse {
                logs: vec![],
                total: 0,
                extra,
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => ApiError::Internal(format!("Failed to stat log file: {e}")).into_response(),
    }
}

fn tail_text_file(path: &Path, lines: usize, max_bytes: usize) -> String {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len() as usize,
        Err(_) => return String::new(),
    };
    if size == 0 {
        return String::new();
    }
    let data = if size <= max_bytes {
        match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => return String::new(),
        }
    } else {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return String::new(),
        };
        let _ = f.seek(SeekFrom::Start((size - max_bytes) as u64));
        let mut buf = vec![0u8; max_bytes];
        let n = f.read(&mut buf).unwrap_or(0);
        buf.truncate(n);
        buf
    };
    let text = String::from_utf8_lossy(&data);
    let all_lines: Vec<&str> = text.lines().collect();
    let start = all_lines.len().saturating_sub(lines);
    all_lines[start..].join("\n")
}
