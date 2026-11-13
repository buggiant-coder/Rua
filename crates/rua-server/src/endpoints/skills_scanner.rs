use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use tracing::warn;

use rua_skills_scanner::history::{
    blocked_history_path, clear_blocked_history, get_blocked_history, remove_blocked_entry,
};
use rua_skills_scanner::whitelist::compute_skill_content_hash;

use crate::state::AppState;

pub fn register_scanner_routes() -> Router<AppState> {
    Router::new()
        .route("/skills/scanner/blocked", get(scanner_blocked_list_handler))
        .route(
            "/skills/scanner/blocked/clear",
            post(scanner_blocked_clear_handler),
        )
        .route(
            "/skills/scanner/blocked/{name}/{hash}",
            delete(scanner_blocked_remove_handler),
        )
        .route("/skills/scanner/scan", post(scanner_manual_scan_handler))
}

pub async fn scanner_blocked_list_handler() -> Response {
    let records = get_blocked_history();
    (StatusCode::OK, Json(records)).into_response()
}

pub async fn scanner_blocked_clear_handler() -> Response {
    clear_blocked_history();
    (StatusCode::OK, Json(json!({ "cleared": true }))).into_response()
}

pub async fn scanner_blocked_remove_handler(
    Path((name, hash)): Path<(String, String)>,
) -> Response {
    if !blocked_history_path().is_file() {
        let body = json!({
            "detail": "blocked history file not found",
        });
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }
    if remove_blocked_entry(&name, &hash) {
        (StatusCode::OK, Json(json!({ "removed": true }))).into_response()
    } else {
        let body = json!({
            "detail": format!("blocked entry not found for name={name} hash={hash}"),
        });
        (StatusCode::NOT_FOUND, Json(body)).into_response()
    }
}

pub async fn scanner_manual_scan_handler(State(state): State<AppState>) -> Response {
    let data_dir = &state.rua_root;
    let store = state.security.config_store();
    // Capture the current process settings once for this scan batch.
    let config = match store.get_skill_scanner() {
        Ok(config) => config,
        Err(error) => return super::security::map_store_error(error).into_response(),
    };

    let mut skill_dirs: Vec<(String, std::path::PathBuf, String)> = Vec::new();

    let workspaces_root = data_dir.join("workspaces");
    if let Ok(entries) = std::fs::read_dir(&workspaces_root) {
        for entry in entries.filter_map(|e| e.ok()) {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let ws = entry.path();
            let agent_id = ws
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let source = format!("workspace:{agent_id}");
            let skills_dir = {
                let preferred = ws.join("skills");
                if preferred.is_dir() {
                    preferred
                } else {
                    let legacy = ws.join("skill");
                    if legacy.is_dir() {
                        legacy
                    } else {
                        continue;
                    }
                }
            };
            if let Ok(skill_entries) = std::fs::read_dir(&skills_dir) {
                for se in skill_entries.filter_map(|e| e.ok()) {
                    if se.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        if let Some(name) = se.file_name().to_str() {
                            skill_dirs.push((name.to_string(), se.path(), source.clone()));
                        }
                    }
                }
            }
        }
    }

    let pool_dir = data_dir.join("skill_pool");
    if let Ok(entries) = std::fs::read_dir(&pool_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    skill_dirs.push((name.to_string(), entry.path(), "pool".to_string()));
                }
            }
        }
    }

    let scanned_count = skill_dirs.len();

    let scan_futures = skill_dirs.iter().map(|(skill_name, skill_dir, source)| {
        let name = skill_name.clone();
        let dir = skill_dir.clone();
        let src = source.clone();
        let config = &config;
        async move {
            let outcome = rua_skills_scanner::scan_skill_directory_with_config(
                &dir,
                &name,
                None,
                None,
                Some(config),
            )
            .await;
            (name, dir, src, outcome)
        }
    });
    let scan_results = futures::future::join_all(scan_futures).await;

    let mut results = Vec::new();
    let mut total_issues: usize = 0;
    let mut new_entries: Vec<serde_json::Value> = Vec::new();

    for (skill_name, skill_dir, source, outcome) in scan_results {
        match outcome {
            Ok(Some(sr)) => {
                let fc = sr.findings.len();
                if fc > 0 {
                    let content_hash = compute_skill_content_hash(&skill_dir).unwrap_or_default();
                    let findings_json: Vec<serde_json::Value> = sr
                        .findings
                        .iter()
                        .map(|f| {
                            json!({
                                "severity": f.severity.as_str(),
                                "title": f.title,
                                "description": f.description,
                                "file_path": f.file_path.as_ref().map(|p| format!("{}/{}", skill_name, p)),
                                "line_number": f.line_number,
                                "rule_id": f.rule_id,
                            })
                        })
                        .collect();
                    new_entries.push(json!({
                        "skill_name": sr.skill_name,
                        "blocked_at": chrono::Utc::now().to_rfc3339(),
                        "max_severity": sr.max_severity.as_str(),
                        "findings": findings_json,
                        "content_hash": content_hash,
                        "action": "warned",
                        "source": source,
                    }));
                    total_issues += fc;
                    results.push(json!({
                        "skill_name": sr.skill_name,
                        "severity": sr.max_severity.as_str(),
                        "findings_count": fc,
                        "action": "warned",
                        "source": source,
                    }));
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!(skill = skill_name.as_str(), error = %e, "manual scan error");
                let fc = e.findings.len();
                total_issues += fc;
                results.push(json!({
                    "skill_name": skill_name,
                    "severity": "UNKNOWN",
                    "findings_count": fc,
                    "action": "blocked",
                    "source": source,
                }));
            }
        }
    }

    if !new_entries.is_empty() {
        if let Err(e) = store.append_blocked_entries(&new_entries) {
            warn!(error = %e, "failed to persist blocked history via SecurityConfigStore");
        }
    }

    let body = json!({
        "scanned": scanned_count,
        "issues": total_issues,
        "results": results,
    });
    (StatusCode::OK, Json(body)).into_response()
}
