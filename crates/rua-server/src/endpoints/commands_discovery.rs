use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEntry {
    pub name: String,
    pub scope: String,
    pub help: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args_hint: Option<String>,
    pub agent_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subsystem: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/commands", get(commands_handler))
}

async fn commands_handler(State(state): State<AppState>) -> Json<Vec<CommandEntry>> {
    Json(state.commands_metadata.as_ref().clone())
}

pub fn build_commands_metadata() -> Vec<CommandEntry> {
    let mut entries = Vec::new();

    let conv_registry = rua_agent::command_adapters::build_conversation_registry();
    for (name, help) in conv_registry.list() {
        let agent_required = conv_registry.is_agent_required(name);
        entries.push(CommandEntry {
            name: name.to_string(),
            scope: "conversation".to_string(),
            help: help.to_string(),
            args_hint: None,
            agent_required,
            alias_of: None,
            subsystem: None,
        });
    }

    let daemon_registry = rua_command::build_daemon_registry();
    for (name, help) in daemon_registry.list() {
        let is_router = name == "daemon";
        let alias_of = if !is_router && name != "reload_config" {
            Some(format!("/daemon {name}"))
        } else if name == "reload_config" {
            Some("/daemon reload-config".to_string())
        } else {
            None
        };
        entries.push(CommandEntry {
            name: name.to_string(),
            scope: "daemon".to_string(),
            help: help.to_string(),
            args_hint: None,
            agent_required: false,
            alias_of,
            subsystem: None,
        });
    }

    let control_registry = rua_command::build_control_registry();

    for (name, help) in control_registry.list() {
        entries.push(CommandEntry {
            name: name.to_string(),
            scope: "control".to_string(),
            help: help.to_string(),
            args_hint: None,
            agent_required: false,
            alias_of: None,
            subsystem: None,
        });
    }

    entries.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));

    entries
}
