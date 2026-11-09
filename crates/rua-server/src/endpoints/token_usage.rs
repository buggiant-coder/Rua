use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
use rua_usage::{TokenUsageFilter, TokenUsageQuery as StoreQuery};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/token-usage", get(get_usage))
        .route("/token-usage/details", get(get_usage_details))
        .route("/token-usage/by-date", get(get_usage_by_date))
        .route("/agents/{agentId}/token-usage", get(get_usage_agent))
}

async fn build_query(state: &AppState) -> Result<StoreQuery, ApiError> {
    let runtime = state.registry.get_or_create("default").await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for token usage: {e}"
        ))
    })?;
    Ok(StoreQuery::new(runtime.rua_root()))
}

fn run_query(query: &StoreQuery, params: &dto::TokenUsageQuery) -> Response {
    let filter: TokenUsageFilter = params.into();
    run_query_with_filter(query, filter)
}

fn run_query_with_filter(query: &StoreQuery, filter: TokenUsageFilter) -> Response {
    match query.query(&filter) {
        Ok(summary) => {
            let response: dto::TokenUsageResponse = summary.into();
            let mut json = serde_json::to_value(&response).unwrap_or_default();
            if let Some(obj) = json.as_object_mut() {
                let total = response.total_prompt_tokens + response.total_completion_tokens;
                obj.insert("total_tokens".into(), serde_json::json!(total));
                obj.insert("count".into(), serde_json::json!(response.total_calls));
            }
            (StatusCode::OK, Json(json)).into_response()
        }
        Err(e) => ApiError::Internal(format!("failed to read token_usage_ledger.json: {e}"))
            .into_response(),
    }
}

async fn get_usage(
    State(state): State<AppState>,
    Query(query): Query<dto::TokenUsageQuery>,
) -> Response {
    let store_query = match build_query(&state).await {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    run_query(&store_query, &query)
}

async fn get_usage_details(
    State(state): State<AppState>,
    Query(query): Query<dto::TokenUsageQuery>,
) -> Response {
    let store_query = match build_query(&state).await {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    let filter: TokenUsageFilter = (&query).into();
    match store_query.query_details(&filter) {
        Ok(records) => {
            let response: Vec<dto::TokenUsageRecord> =
                records.into_iter().map(Into::into).collect();
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => ApiError::Internal(format!("failed to read token_usage_ledger.json: {e}"))
            .into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct ByDateParams {
    from: Option<String>,
    to: Option<String>,
}

async fn get_usage_by_date(
    State(state): State<AppState>,
    Query(params): Query<ByDateParams>,
) -> Response {
    let today = chrono::Utc::now().date_naive();

    let (from_date, to_date) = match (params.from.as_deref(), params.to.as_deref()) {
        (Some(f), Some(t)) => {
            let from = match NaiveDate::parse_from_str(f, "%Y-%m-%d") {
                Ok(d) => d,
                Err(_) => {
                    return ApiError::BadRequest(format!("invalid 'from' date: {f}"))
                        .into_response();
                }
            };
            let to = match NaiveDate::parse_from_str(t, "%Y-%m-%d") {
                Ok(d) => d,
                Err(_) => {
                    return ApiError::BadRequest(format!("invalid 'to' date: {t}")).into_response();
                }
            };
            if from > to {
                return ApiError::BadRequest("'from' must not be after 'to'".into())
                    .into_response();
            }
            (from, to)
        }
        (Some(f), None) => {
            let from = match NaiveDate::parse_from_str(f, "%Y-%m-%d") {
                Ok(d) => d,
                Err(_) => {
                    return ApiError::BadRequest(format!("invalid 'from' date: {f}"))
                        .into_response();
                }
            };
            (from, today)
        }
        (None, Some(t)) => {
            let to = match NaiveDate::parse_from_str(t, "%Y-%m-%d") {
                Ok(d) => d,
                Err(_) => {
                    return ApiError::BadRequest(format!("invalid 'to' date: {t}")).into_response();
                }
            };
            (to - chrono::Duration::days(30), to)
        }
        (None, None) => (today - chrono::Duration::days(30), today),
    };

    let store_query = match build_query(&state).await {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };

    let filter = TokenUsageFilter {
        start_date: Some(from_date.format("%Y-%m-%d").to_string()),
        end_date: Some(to_date.format("%Y-%m-%d").to_string()),
        ..Default::default()
    };

    match store_query.query(&filter) {
        Ok(summary) => {
            let mut items: Vec<dto::DailyUsageItem> = summary
                .by_date
                .into_iter()
                .map(|(date, stats)| dto::DailyUsageItem {
                    date,
                    prompt_tokens: stats.prompt_tokens,
                    completion_tokens: stats.completion_tokens,
                    total_cost: 0.0,
                })
                .collect();
            items.sort_by(|a, b| a.date.cmp(&b.date));
            (StatusCode::OK, Json(items)).into_response()
        }
        Err(e) => ApiError::Internal(format!("failed to read token_usage_ledger.json: {e}"))
            .into_response(),
    }
}

async fn get_usage_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<dto::TokenUsageQuery>,
) -> Response {
    let store_query = match build_query(&state).await {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    let mut filter: TokenUsageFilter = (&query).into();
    filter.agent_id = Some(agent_id);
    run_query_with_filter(&store_query, filter)
}
