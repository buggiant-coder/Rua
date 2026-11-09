use clap::{Args, Subcommand};
use rua_core::AgentCollaborationMode;

use crate::chat::{build_response_json, consume_sse_stream, stream_sse};
use crate::http_client::{require_server, resolve_base_url, ApiClient};
use crate::output::{print_value, OutputFormat};

#[derive(Debug, Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    command: ProjectCommands,
}

#[derive(Debug, Subcommand)]
enum ProjectCommands {
    Chat(ProjectChatArgs),
}

#[derive(Debug, Args)]
struct ProjectChatArgs {
    #[command(subcommand)]
    command: ProjectChatCommands,
}

#[derive(Debug, Subcommand)]
enum ProjectChatCommands {
    List {
        project_id: String,
        #[arg(long, default_value_t = 0)]
        offset: u64,
        #[arg(long, default_value_t = 50)]
        limit: u64,
        #[arg(long)]
        search: Option<String>,
    },

    Show {
        project_id: String,
        chat_id: String,
        #[arg(long)]
        turn_limit: Option<u64>,
        #[arg(long)]
        before_turn_ordinal: Option<u64>,
    },

    Send {
        project_id: String,
        message: String,
        #[arg(long)]
        chat_id: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        mode: Option<AgentCollaborationMode>,
    },

    Stream {
        project_id: String,
        message: String,
        #[arg(long)]
        chat_id: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        mode: Option<AgentCollaborationMode>,
    },

    Update {
        project_id: String,
        chat_id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, conflicts_with = "unpin")]
        pin: bool,
        #[arg(long, conflicts_with = "pin")]
        unpin: bool,
        #[arg(long)]
        pin_order: Option<i64>,
        #[arg(long)]
        mode: Option<AgentCollaborationMode>,
    },

    Delete {
        project_id: String,
        chat_id: String,
        #[arg(long)]
        force: bool,
    },

    Stop {
        project_id: String,
        chat_id: String,
    },
}

pub struct ProjectChatRequest {
    pub chat_id: String,
    pub body: serde_json::Value,
}

pub fn run(
    args: ProjectArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id))
}

async fn run_async(
    args: ProjectArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let base_url = resolve_base_url(base_url);
    require_server(&base_url).await?;
    let client = ApiClient::new(&base_url, agent_id);

    match args.command {
        ProjectCommands::Chat(args) => {
            run_chat(args, format, &client, agent_id).await?;
        }
    }
    Ok(())
}

async fn run_chat(
    args: ProjectChatArgs,
    format: OutputFormat,
    client: &ApiClient,
    agent_id: &str,
) -> anyhow::Result<()> {
    match args.command {
        ProjectChatCommands::List {
            project_id,
            offset,
            limit,
            search,
        } => {
            let response = client
                .get(&project_chats_endpoint(
                    &project_id,
                    offset,
                    limit,
                    search.as_deref(),
                ))
                .await?;
            print_value(&response, format);
        }
        ProjectChatCommands::Show {
            project_id,
            chat_id,
            turn_limit,
            before_turn_ordinal,
        } => {
            let response = client
                .get(&project_chat_history_endpoint(
                    &project_id,
                    &chat_id,
                    turn_limit,
                    before_turn_ordinal,
                ))
                .await?;
            print_value(&response, format);
        }
        ProjectChatCommands::Send {
            project_id,
            message,
            chat_id,
            model,
            mode,
        } => {
            let request =
                build_project_chat_request(&message, agent_id, chat_id, model.as_deref(), mode);
            eprintln!("INFO: Using chat_id: {}", request.chat_id);
            let events = consume_sse_stream(
                client,
                &project_chat_stream_endpoint(&project_id, &request.chat_id),
                agent_id,
                &request.body,
            )
            .await?;
            let response = build_response_json(&events);
            match format {
                OutputFormat::Json => crate::output::print_json(&response),
                OutputFormat::Human => {
                    let text = response
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if text.is_empty() {
                        println!("(no response)");
                    } else {
                        println!("{text}");
                    }
                }
            }
        }
        ProjectChatCommands::Stream {
            project_id,
            message,
            chat_id,
            model,
            mode,
        } => {
            let request =
                build_project_chat_request(&message, agent_id, chat_id, model.as_deref(), mode);
            eprintln!("INFO: Using chat_id: {}", request.chat_id);
            stream_sse(
                client,
                &project_chat_stream_endpoint(&project_id, &request.chat_id),
                agent_id,
                format,
                &request.body,
            )
            .await?;
        }
        ProjectChatCommands::Update {
            project_id,
            chat_id,
            name,
            pin,
            unpin,
            pin_order,
            mode,
        } => {
            let mut body = serde_json::Map::new();
            if let Some(name) = name {
                body.insert("name".to_string(), serde_json::Value::String(name));
            }
            if pin {
                body.insert("pinned".to_string(), serde_json::Value::Bool(true));
            } else if unpin {
                body.insert("pinned".to_string(), serde_json::Value::Bool(false));
            }
            if let Some(pin_order) = pin_order {
                body.insert("pin_order".to_string(), serde_json::json!(pin_order));
            }
            if let Some(mode) = mode {
                body.insert("collaboration_mode".to_string(), serde_json::json!(mode));
            }
            if body.is_empty() {
                anyhow::bail!(
                    "Project Chat update requires --name, --pin, --unpin, --pin-order, or --mode"
                );
            }
            let response = client
                .patch(
                    &project_chat_endpoint(&project_id, &chat_id),
                    &serde_json::Value::Object(body),
                )
                .await?;
            print_value(&response, format);
        }
        ProjectChatCommands::Delete {
            project_id,
            chat_id,
            force,
        } => {
            let response = client
                .delete(&format!(
                    "{}?force={force}",
                    project_chat_endpoint(&project_id, &chat_id)
                ))
                .await?;
            print_value(&response, format);
        }
        ProjectChatCommands::Stop {
            project_id,
            chat_id,
        } => {
            let response = client
                .post(
                    &project_chat_stop_endpoint(&project_id, &chat_id),
                    &serde_json::json!({}),
                )
                .await?;
            print_value(&response, format);
        }
    }
    Ok(())
}

pub fn build_project_chat_request(
    message: &str,
    agent_id: &str,
    chat_id: Option<String>,
    model: Option<&str>,
    mode: Option<AgentCollaborationMode>,
) -> ProjectChatRequest {
    let chat_id = chat_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut body = serde_json::json!({
        "agent_id": agent_id,
        "chat_id": chat_id,
        "message": message,
    });
    if let Some(model) = model {
        body["model"] = serde_json::Value::String(model.to_string());
    }
    if let Some(mode) = mode {
        body[rua_core::TURN_COLLABORATION_MODE_OVERRIDE_KEY] = serde_json::json!(mode);
    }
    ProjectChatRequest { chat_id, body }
}

pub fn project_chats_endpoint(
    project_id: &str,
    offset: u64,
    limit: u64,
    search: Option<&str>,
) -> String {
    let mut url = api_url(&["projects", project_id, "chats"]);
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("offset", &offset.to_string());
        query.append_pair("limit", &limit.to_string());
        if let Some(search) = search.filter(|value| !value.trim().is_empty()) {
            query.append_pair("search", search.trim());
        }
    }
    relative_url(url)
}

pub fn project_chat_endpoint(project_id: &str, chat_id: &str) -> String {
    relative_url(api_url(&["projects", project_id, "chats", chat_id]))
}

pub fn project_chat_history_endpoint(
    project_id: &str,
    chat_id: &str,
    turn_limit: Option<u64>,
    before_turn_ordinal: Option<u64>,
) -> String {
    let mut url = api_url(&["projects", project_id, "chats", chat_id]);
    {
        let mut query = url.query_pairs_mut();
        if let Some(turn_limit) = turn_limit {
            query.append_pair("turn_limit", &turn_limit.to_string());
        }
        if let Some(before) = before_turn_ordinal {
            query.append_pair("before_turn_ordinal", &before.to_string());
        }
    }
    relative_url(url)
}

pub fn project_chat_stream_endpoint(project_id: &str, chat_id: &str) -> String {
    relative_url(api_url(&[
        "projects", project_id, "chats", chat_id, "console", "chat",
    ]))
}

pub fn project_chat_stop_endpoint(project_id: &str, chat_id: &str) -> String {
    relative_url(api_url(&[
        "projects", project_id, "chats", chat_id, "console", "stop",
    ]))
}

fn api_url(segments: &[&str]) -> reqwest::Url {
    let mut url = reqwest::Url::parse("http://localhost").expect("valid URL base");
    {
        let mut path = url.path_segments_mut().expect("URL supports path segments");
        path.clear();
        path.push("api");
        for segment in segments {
            path.push(segment);
        }
    }
    url
}

fn relative_url(url: reqwest::Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}
