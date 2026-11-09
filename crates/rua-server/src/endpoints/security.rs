use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};

use rua_security::config_store::StoreError;
use rua_security::{builtin_structured_rules, AstRule, SecurityConfigStore, Severity};
use std::collections::HashSet;
use std::net::IpAddr;

use crate::endpoints::dto;
use crate::errors::{ApiError, ValidationError};
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/config/security/file-guard",
            get(get_file_guard).put(update_file_guard),
        )
        .route(
            "/config/security/skill-scanner",
            get(get_skill_scanner).put(update_skill_scanner),
        )
        .route(
            "/config/security/skill-scanner/blocked-history",
            get(get_blocked_history).delete(clear_blocked_history),
        )
        .route(
            "/config/security/skill-scanner/blocked-history/{index}",
            delete(delete_blocked_history_entry),
        )
        .route(
            "/config/security/skill-scanner/whitelist",
            post(add_whitelist),
        )
        .route(
            "/config/security/skill-scanner/whitelist/{skill_name}",
            delete(remove_whitelist),
        )
        .route(
            "/config/security/allow-no-auth-hosts",
            get(get_allow_no_auth_hosts).put(update_allow_no_auth_hosts),
        )
        .route(
            "/config/security/tool-guard",
            get(get_selected_tool_guard).put(update_selected_tool_guard),
        )
        .route(
            "/agents/{agent_id}/security/tool-guard",
            get(get_agent_tool_guard).put(update_agent_tool_guard),
        )
        .route(
            "/agents/{agent_id}/security/tool-guard/builtin-rules",
            get(get_agent_builtin_rules),
        )
        .route(
            "/agents/{agent_id}/security/tool-guard/reset-to-builtin",
            post(reset_agent_to_builtin),
        )
        .route(
            "/agents/{agent_id}/security/approval-mode",
            get(get_agent_approval_mode).put(update_agent_approval_mode),
        )
}

pub(crate) async fn build_store(state: &AppState) -> Result<SecurityConfigStore, ApiError> {
    Ok(SecurityConfigStore::new(state.rua_root.clone()))
}

pub(crate) fn map_store_error(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound { resource, id } => ApiError::NotFound(format!("{resource}: {id}")),
        other => ApiError::Internal(format!("security config store error: {other}")),
    }
}

pub(crate) fn normalize_allow_no_auth_hosts(hosts: Vec<String>) -> Result<Vec<String>, ApiError> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    let mut invalid = Vec::new();

    for raw in hosts {
        let host = raw.trim();
        if host.is_empty() {
            continue;
        }
        match host.parse::<IpAddr>() {
            Ok(ip) => {
                let host = ip.to_string();
                if seen.insert(host.clone()) {
                    normalized.push(host);
                }
            }
            Err(_) => invalid.push(host.to_string()),
        }
    }

    if invalid.is_empty() {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest(format!(
            "Invalid IP address(es): {}. Only literal IPv4/IPv6 addresses are allowed.",
            invalid.join(", ")
        )))
    }
}

pub(crate) fn allow_no_auth_hosts_to_dto(hosts: Vec<String>) -> dto::AllowNoAuthHostsConfig {
    dto::AllowNoAuthHostsConfig { hosts }
}

pub(crate) fn get_allow_no_auth_hosts_from_store(
    store: &SecurityConfigStore,
) -> Result<dto::AllowNoAuthHostsConfig, ApiError> {
    store
        .get_allow_no_auth_hosts()
        .map(allow_no_auth_hosts_to_dto)
        .map_err(map_store_error)
}

pub(crate) fn update_allow_no_auth_hosts_in_store(
    store: &SecurityConfigStore,
    body: dto::AllowNoAuthHostsConfig,
) -> Result<dto::AllowNoAuthHostsConfig, ApiError> {
    let hosts = normalize_allow_no_auth_hosts(body.hosts)?;
    store
        .update_allow_no_auth_hosts(hosts)
        .map(allow_no_auth_hosts_to_dto)
        .map_err(map_store_error)
}

pub(crate) async fn publish_process_security_config(state: &AppState, store: &SecurityConfigStore) {
    match store.get_security_config() {
        Ok(config) => {
            let _ = publish_sync_fact(
                state,
                SyncPublicationFact::RootProcessSecurityCommitted {
                    security: serde_json::to_value(config).unwrap_or_default(),
                },
            )
            .await;
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "[sync-publication] process security publication skipped: failed to load security config"
            );
        }
    }
}

pub(crate) fn tool_guard_to_dto(
    cfg: &rua_config::types::security::ToolGuardConfig,
) -> dto::ToolGuardConfig {
    dto::ToolGuardConfig {
        enabled: cfg.enabled,
        custom_rules: cfg
            .custom_rules
            .iter()
            .map(|r| dto::CustomRule {
                id: r.id.clone(),
                tools: r.tools.clone(),
                category: r.category.clone(),
                severity: r.severity.clone(),
                description: r.description.clone(),
                remediation: r.remediation.clone(),
                command_set: r.command_set.clone(),
                arg_matchers: r
                    .arg_matchers
                    .iter()
                    .map(|m| dto::CustomArgMatcherDto {
                        command: m.command.clone(),
                        flag: m.flag.clone(),
                        value_contains: m.value_contains.clone(),
                        value_in: m.value_in.clone(),
                    })
                    .collect(),
                path_matchers: r
                    .path_matchers
                    .iter()
                    .map(|m| dto::CustomPathMatcherDto {
                        contains: m.contains.clone(),
                        prefix: m.prefix.clone(),
                        glob: m.glob.clone(),
                        on_redirect: m.on_redirect,
                    })
                    .collect(),
                lexical: r.lexical.clone(),
                structural: r.structural.clone(),
                action: r.action.clone(),
                extra: r.extra.clone(),
            })
            .collect(),
        disabled_rules: cfg.disabled_rules.clone(),
        approval_mode: None,
        file_guard: None,
        rule_guard: None,
        extra: strip_removed_tool_list_extra(&cfg.extra),
    }
}

pub(crate) fn dto_to_tool_guard(
    dto_cfg: &dto::ToolGuardConfig,
    existing: &rua_config::types::security::ToolGuardConfig,
) -> rua_config::types::security::ToolGuardConfig {
    rua_config::types::security::ToolGuardConfig {
        enabled: dto_cfg.enabled,
        custom_rules: dto_cfg
            .custom_rules
            .iter()
            .map(|r| rua_config::types::security::ToolGuardRuleConfig {
                id: r.id.clone(),
                tools: r.tools.clone(),
                category: r.category.clone(),
                severity: r.severity.clone(),
                description: r.description.clone(),
                remediation: r.remediation.clone(),
                command_set: r.command_set.clone(),
                arg_matchers: r
                    .arg_matchers
                    .iter()
                    .map(|m| rua_config::types::security::CustomArgMatcher {
                        command: m.command.clone(),
                        flag: m.flag.clone(),
                        value_contains: m.value_contains.clone(),
                        value_in: m.value_in.clone(),
                    })
                    .collect(),
                path_matchers: r
                    .path_matchers
                    .iter()
                    .map(|m| rua_config::types::security::CustomPathMatcher {
                        contains: m.contains.clone(),
                        prefix: m.prefix.clone(),
                        glob: m.glob.clone(),
                        on_redirect: m.on_redirect,
                    })
                    .collect(),
                lexical: r.lexical.clone(),
                structural: r.structural.clone(),
                action: r.action.clone(),
                extra: r.extra.clone(),
            })
            .collect(),
        disabled_rules: dto_cfg.disabled_rules.clone(),
        approval_mode: existing.approval_mode.clone(),
        extra: strip_removed_tool_list_extra(&merge_extra(&existing.extra, &dto_cfg.extra)),
    }
}

pub(crate) fn validate_tool_guard_config(_dto_cfg: &dto::ToolGuardConfig) -> Result<(), ApiError> {
    Ok(())
}

pub(crate) fn canonical_approval_mode(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ask_first" | "ask-first" => Some("ask_first"),
        "balanced" => Some("hands_off"),
        "hands_off" | "hands-off" => Some("hands_off"),
        "just_do_it" | "just-do-it" => Some("just_do_it"),
        "" | "strict" | "smart" | "custom" | "off" => Some("hands_off"),
        _ => None,
    }
}

pub(crate) fn approval_mode_to_dto(mode: &str) -> dto::ApprovalModeConfig {
    dto::ApprovalModeConfig {
        approval_mode: canonical_approval_mode(mode)
            .unwrap_or("hands_off")
            .to_string(),
    }
}

pub(crate) fn file_guard_to_dto(
    cfg: &rua_config::types::security::FileGuardConfig,
    secret_dir: Option<&std::path::Path>,
) -> dto::FileGuardConfig {
    let mut builtin: Vec<String> = rua_security::file_guardian::default_sensitive_patterns()
        .iter()
        .chain(rua_security::file_guardian::system_sensitive_paths().iter())
        .map(|s| (*s).to_string())
        .collect();
    if let Some(dir) = secret_dir {
        builtin.push(format!("{}/**", dir.display()));
    }
    dto::FileGuardConfig {
        enabled: cfg.enabled,
        paths: cfg.sensitive_files.clone(),
        builtin_paths: builtin,
        disabled_patterns: cfg.disabled_patterns.clone(),
        extra: cfg.extra.clone(),
    }
}

pub(crate) fn dto_to_file_guard(
    dto_cfg: &dto::FileGuardConfig,
    existing: &rua_config::types::security::FileGuardConfig,
) -> rua_config::types::security::FileGuardConfig {
    rua_config::types::security::FileGuardConfig {
        enabled: dto_cfg.enabled,
        sensitive_files: dto_cfg.paths.clone(),
        disabled_patterns: dto_cfg.disabled_patterns.clone(),
        extra: merge_extra(&existing.extra, &dto_cfg.extra),
    }
}

pub(crate) fn skill_scanner_to_dto(
    cfg: &rua_config::types::security::SkillScannerConfig,
) -> dto::SkillScannerConfig {
    dto::SkillScannerConfig {
        mode: cfg.mode.clone(),
        timeout: Some(u64::from(cfg.timeout)),
        whitelist: cfg
            .whitelist
            .iter()
            .map(|w| dto::SkillWhitelistEntry {
                skill_name: w.skill_name.clone(),
                content_hash: w.content_hash.clone(),
                added_at: w.added_at.clone(),
                extra: w.extra.clone(),
            })
            .collect(),
        extra: cfg.extra.clone(),
    }
}

pub(crate) fn dto_to_skill_scanner(
    dto_cfg: &dto::SkillScannerConfig,
    existing: &rua_config::types::security::SkillScannerConfig,
) -> rua_config::types::security::SkillScannerConfig {
    rua_config::types::security::SkillScannerConfig {
        mode: if dto_cfg.mode.is_empty() {
            existing.mode.clone()
        } else {
            dto_cfg.mode.clone()
        },
        timeout: dto_cfg
            .timeout
            .map(|t| t as u32)
            .unwrap_or(existing.timeout),
        whitelist: dto_cfg
            .whitelist
            .iter()
            .map(
                |w| rua_config::types::security::SkillScannerWhitelistEntry {
                    skill_name: w.skill_name.clone(),
                    content_hash: w.content_hash.clone(),
                    added_at: if w.added_at.is_empty() {
                        chrono::Utc::now().to_rfc3339()
                    } else {
                        w.added_at.clone()
                    },
                    extra: w.extra.clone(),
                },
            )
            .collect(),
        policy: existing.policy.clone(),
        extra: merge_extra(&existing.extra, &dto_cfg.extra),
    }
}

fn merge_extra(
    base: &serde_json::Map<String, serde_json::Value>,
    overlay: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut out = base.clone();
    for (k, v) in overlay {
        out.insert(k.clone(), v.clone());
    }
    out
}

fn strip_removed_tool_list_extra(
    input: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut out = input.clone();
    out.remove("guarded_tools");
    out.remove("denied_tools");
    out
}

fn severity_str(sev: Severity) -> &'static str {
    match sev {
        Severity::Critical => "CRITICAL",
        Severity::High => "HIGH",
        Severity::Medium => "MEDIUM",
        Severity::Low => "LOW",
        Severity::Info => "INFO",
    }
}

fn ast_rule_to_dto(rule: &AstRule) -> dto::BuiltinRule {
    dto::BuiltinRule {
        id: rule.id.clone(),
        tools: vec!["bash".to_string()],
        category: if rule.category.is_empty() {
            "ast_analysis".to_string()
        } else {
            rule.category.clone()
        },
        severity: severity_str(rule.severity).to_string(),
        description: rule.id.clone(),
        remediation: "Confirm with user before proceeding".to_string(),
        rule_type: "ast".to_string(),
        command_set: rule.command_set.clone(),
        arg_matchers: rule
            .arg_matchers
            .iter()
            .filter_map(|m| serde_json::to_value(m).ok())
            .collect(),
        path_matchers: rule
            .path_matchers
            .iter()
            .filter_map(|m| serde_json::to_value(m).ok())
            .collect(),
        lexical: rule.lexical.iter().map(|l| format!("{:?}", l)).collect(),
        structural: rule.structural.iter().map(|f| format!("{:?}", f)).collect(),
        action: format!("{:?}", rule.action),
        extra: serde_json::Map::new(),
    }
}

pub(crate) fn builtin_rules_payload() -> Vec<dto::BuiltinRule> {
    builtin_structured_rules()
        .iter()
        .map(ast_rule_to_dto)
        .collect()
}

async fn get_allow_no_auth_hosts(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match get_allow_no_auth_hosts_from_store(&store) {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_allow_no_auth_hosts(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::AllowNoAuthHostsConfig>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match update_allow_no_auth_hosts_in_store(&store, body) {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn get_file_guard(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let secret_dir = state.security.secret_dir();
    match store.get_file_guard() {
        Ok(cfg) => (StatusCode::OK, Json(file_guard_to_dto(&cfg, secret_dir))).into_response(),
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn update_file_guard(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::FileGuardConfig>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let existing = match store.get_file_guard() {
        Ok(c) => c,
        Err(e) => return map_store_error(e).into_response(),
    };
    let merged = dto_to_file_guard(&body, &existing);
    let secret_dir = state.security.secret_dir();
    match store.update_file_guard(merged) {
        Ok(saved) => {
            publish_process_security_config(&state, &store).await;
            (StatusCode::OK, Json(file_guard_to_dto(&saved, secret_dir))).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn get_skill_scanner(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.get_skill_scanner() {
        Ok(cfg) => (StatusCode::OK, Json(skill_scanner_to_dto(&cfg))).into_response(),
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn update_skill_scanner(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::SkillScannerConfig>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let existing = match store.get_skill_scanner() {
        Ok(c) => c,
        Err(e) => return map_store_error(e).into_response(),
    };
    let merged = dto_to_skill_scanner(&body, &existing);
    match store.update_skill_scanner(merged) {
        Ok(saved) => {
            publish_process_security_config(&state, &store).await;
            (StatusCode::OK, Json(skill_scanner_to_dto(&saved))).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn get_blocked_history(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.get_blocked_history() {
        Ok(values) => {
            let entries: Vec<dto::BlockedHistoryEntry> = values
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();
            (StatusCode::OK, Json(entries)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn clear_blocked_history(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.clear_blocked_history() {
        Ok(()) => {
            let resp = serde_json::json!({ "cleared": true });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn delete_blocked_history_entry(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(index): Path<usize>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.delete_blocked_history_entry(index) {
        Ok(()) => {
            let resp = serde_json::json!({ "removed": true });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn add_whitelist(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::SkillWhitelistEntry>,
) -> Response {
    if body.skill_name.is_empty() {
        return ApiError::BadRequest("skill_name is required".into()).into_response();
    }
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let entry = rua_config::types::security::SkillScannerWhitelistEntry {
        skill_name: body.skill_name.clone(),
        content_hash: body.content_hash.clone(),
        added_at: chrono::Utc::now().to_rfc3339(),
        extra: body.extra.clone(),
    };
    match store.add_whitelist(entry) {
        Ok(saved) => {
            publish_process_security_config(&state, &store).await;
            let resp = serde_json::json!({ "whitelisted": true, "skill_name": saved.skill_name });
            (StatusCode::CREATED, Json(resp)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn remove_whitelist(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Path(skill_name): Path<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.remove_whitelist(&skill_name) {
        Ok(()) => {
            publish_process_security_config(&state, &store).await;
            let resp = serde_json::json!({ "removed": true, "skill_name": skill_name });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn get_selected_tool_guard(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    get_tool_guard_for_agent(&state, &ctx.agent_id)
}

async fn update_selected_tool_guard(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::ToolGuardConfig>,
) -> Response {
    update_tool_guard_for_agent(&state, &ctx.agent_id, body).await
}

async fn get_agent_tool_guard(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    get_tool_guard_for_agent(&state, &agent_id)
}

fn get_tool_guard_for_agent(state: &AppState, agent_id: &str) -> Response {
    let store = state.security.config_store();
    match store.load(agent_id) {
        Ok(cfg) => (StatusCode::OK, Json(tool_guard_to_dto(&cfg.tool_guard))).into_response(),
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn update_agent_tool_guard(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::ToolGuardConfig>,
) -> Response {
    update_tool_guard_for_agent(&state, &agent_id, body).await
}

async fn update_tool_guard_for_agent(
    state: &AppState,
    agent_id: &str,
    body: dto::ToolGuardConfig,
) -> Response {
    if let Err(e) = validate_tool_guard_config(&body) {
        return e.into_response();
    }

    let store = state.security.config_store();
    let mut cfg = match store.load(agent_id) {
        Ok(c) => c,
        Err(e) => return map_store_error(e).into_response(),
    };
    cfg.tool_guard = dto_to_tool_guard(&body, &cfg.tool_guard);
    if let Err(e) = state.security.save_and_invalidate(agent_id, &cfg) {
        return map_store_error(e).into_response();
    }
    crate::sync_publication::publish_agent_security_committed(
        state,
        agent_id,
        serde_json::to_value(&cfg).unwrap_or_default(),
    )
    .await;
    (StatusCode::OK, Json(tool_guard_to_dto(&cfg.tool_guard))).into_response()
}

async fn get_agent_builtin_rules(
    State(_state): State<AppState>,
    Path(_agent_id): Path<String>,
) -> Response {
    (StatusCode::OK, Json(builtin_rules_payload())).into_response()
}

async fn reset_agent_to_builtin(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let builtin = SecurityConfigStore::builtin_template();
    if let Err(e) = state.security.save_and_invalidate(&agent_id, &builtin) {
        return map_store_error(e).into_response();
    }
    crate::sync_publication::publish_agent_security_committed(
        &state,
        &agent_id,
        serde_json::to_value(&builtin).unwrap_or_default(),
    )
    .await;
    (StatusCode::OK, Json(tool_guard_to_dto(&builtin.tool_guard))).into_response()
}

async fn get_agent_approval_mode(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    get_approval_mode_for_agent(&state, &agent_id)
}

fn get_approval_mode_for_agent(state: &AppState, agent_id: &str) -> Response {
    let store = state.security.config_store();
    let mut cfg = match store.load(agent_id) {
        Ok(cfg) => cfg,
        Err(e) => return map_store_error(e).into_response(),
    };

    let canonical = canonical_approval_mode(&cfg.tool_guard.approval_mode)
        .unwrap_or("balanced")
        .to_string();
    if cfg.tool_guard.approval_mode != canonical {
        cfg.tool_guard.approval_mode = canonical.clone();
        if let Err(e) = state.security.save_and_invalidate(agent_id, &cfg) {
            return map_store_error(e).into_response();
        }
    }

    (
        StatusCode::OK,
        Json(dto::ApprovalModeConfig {
            approval_mode: canonical,
        }),
    )
        .into_response()
}

async fn update_agent_approval_mode(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::ApprovalModeConfig>,
) -> Response {
    update_approval_mode_for_agent(&state, &agent_id, body).await
}

async fn update_approval_mode_for_agent(
    state: &AppState,
    agent_id: &str,
    body: dto::ApprovalModeConfig,
) -> Response {
    let Some(mode) = canonical_approval_mode(&body.approval_mode) else {
        return ApiError::Validation(vec![ValidationError {
            loc: vec!["body".to_string(), "approval_mode".to_string()],
            msg: "approval_mode must be one of ask_first, hands_off, just_do_it".to_string(),
            error_type: "value_error".to_string(),
        }])
        .into_response();
    };

    let store = state.security.config_store();
    let mut cfg = match store.load(agent_id) {
        Ok(cfg) => cfg,
        Err(e) => return map_store_error(e).into_response(),
    };
    cfg.tool_guard.approval_mode = mode.to_string();
    if let Err(e) = state.security.save_and_invalidate(agent_id, &cfg) {
        return map_store_error(e).into_response();
    }
    crate::sync_publication::publish_agent_security_committed(
        state,
        agent_id,
        serde_json::to_value(&cfg).unwrap_or_default(),
    )
    .await;
    (StatusCode::OK, Json(approval_mode_to_dto(mode))).into_response()
}
