use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use rua_config::EnvStore;

use crate::endpoints::dto::{
    self,
    env::{env_map_to_list, list_to_env_map},
};
use crate::errors::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/envs", get(get_envs).put(set_envs))
        .route("/envs/{key}", delete(delete_env))
}

async fn build_store(state: &AppState) -> Result<EnvStore, ApiError> {
    let runtime = state.registry.get_or_create("default").await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for env store: {e}"
        ))
    })?;
    Ok(EnvStore::new(runtime.secret_root()))
}

fn load_envs(store: &EnvStore) -> Result<dto::EnvVarsList, ApiError> {
    store
        .list()
        .map(env_map_to_list)
        .map_err(|e| ApiError::Internal(format!("failed to read envs.json: {e}")))
}

async fn get_envs(State(state): State<AppState>) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match load_envs(&store) {
        Ok(envs) => (StatusCode::OK, Json(envs)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn set_envs(State(state): State<AppState>, Json(body): Json<serde_json::Value>) -> Response {
    let envs: dto::EnvVarsList = match &body {
        serde_json::Value::Array(_) => match serde_json::from_value(body) {
            Ok(v) => v,
            Err(e) => {
                return ApiError::BadRequest(format!("Invalid env array: {e}")).into_response();
            }
        },
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| dto::EnvVar {
                key: k.clone(),
                value: v.as_str().unwrap_or_default().to_string(),
                masked: None,
                extra: serde_json::Map::new(),
            })
            .collect(),
        _ => {
            return ApiError::BadRequest("Expected JSON object or array".into()).into_response();
        }
    };

    let env_store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    let new_map = list_to_env_map(&envs);
    if let Err(e) = env_store.save(new_map) {
        return ApiError::Internal(format!("write envs.json: {e}")).into_response();
    }

    match load_envs(&env_store) {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn delete_env(State(state): State<AppState>, Path(key): Path<String>) -> Response {
    let env_store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = env_store.delete(&key) {
        return ApiError::Internal(format!("delete env key: {e}")).into_response();
    }

    match load_envs(&env_store) {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => e.into_response(),
    }
}
