use tracing::warn;

use crate::state::AppState;

pub async fn reload_from_disk(state: &AppState, agent_id: &str) {
    if let Err(error) = apply_current(state, agent_id).await {
        warn!(agent_id, error = %error, "saved Channel configuration could not be applied");
    }
}

pub async fn apply_current(
    state: &AppState,
    agent_id: &str,
) -> Result<&'static str, rua_workspace::WorkspaceError> {
    apply_current_for(state, agent_id, None).await
}

pub async fn apply_current_for(
    state: &AppState,
    agent_id: &str,
    restart: Option<&str>,
) -> Result<&'static str, rua_workspace::WorkspaceError> {
    if agent_id == state.voice.agent_id() {
        state.voice_runtime.reload(state).await.map_err(|error| {
            rua_workspace::WorkspaceError::component_init_failed("voice", error.to_string())
        })?;
    }
    let Some(runtime) = state.registry.get(agent_id).await else {
        return Ok("deferred");
    };
    runtime
        .apply_channels_for(
            || async {
                state
                    .agent_cells
                    .section::<serde_json::Value>(agent_id, "channels")
                    .read()
                    .await
                    .map(|value| value.unwrap_or_else(|| serde_json::json!({})))
                    .map_err(|error| {
                        rua_workspace::WorkspaceError::config_invalid(error.to_string())
                    })
            },
            restart,
        )
        .await?;
    Ok("applied")
}
