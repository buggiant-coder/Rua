use std::io::Write;
use std::path::{Component, Path, PathBuf};

use clap::{Args, Subcommand};

use crate::chat::chat_endpoint;
use crate::http_client::{require_server, resolve_base_url, ApiClient};
use crate::output::{print_value, OutputFormat};

#[derive(Debug, Clone, clap::ValueEnum)]
enum ChatMode {
    Stream,
    Final,
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    #[command(subcommand)]
    command: AgentCommands,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateAgentBodyInput {
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    pub workspace_dir: Option<String>,
    pub template_id: Option<String>,
    pub language: Option<String>,
    pub skills: Vec<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentChatRequest {
    pub chat_id: String,
    pub body: serde_json::Value,
    pub prefix_added: bool,
}

#[derive(Debug, Subcommand)]
enum AgentCommands {
    List,

    Info {
        agent_id: String,
    },

    Create {
        #[arg(long)]
        id: Option<String>,

        #[arg(long)]
        name: String,

        #[arg(long, default_value = "")]
        description: String,

        #[arg(long)]
        workspace_dir: Option<String>,

        #[arg(long)]
        template_id: Option<String>,

        #[arg(long)]
        language: Option<String>,

        #[arg(long = "skill")]
        skills: Vec<String>,

        #[arg(long)]
        provider_id: Option<String>,

        #[arg(long)]
        model_id: Option<String>,
    },

    Delete {
        agent_id: String,

        #[arg(long)]
        remove_workspace: bool,

        #[arg(long)]
        yes: bool,
    },

    Chat {
        #[arg(id = "target_agent_id", value_name = "AGENT_ID")]
        agent_id: String,

        message: String,

        #[arg(long)]
        from_agent: Option<String>,

        #[arg(long)]
        chat_id: Option<String>,

        #[arg(long, value_enum, default_value_t = ChatMode::Final)]
        mode: ChatMode,

        #[arg(long, default_value_t = 300)]
        timeout: u64,

        #[arg(long)]
        json_output: bool,
    },
}

pub fn run(
    args: AgentArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
    working_dir: &Path,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id, working_dir))
}

async fn run_async(
    args: AgentArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
    working_dir: &Path,
) -> anyhow::Result<()> {
    let resolved_base_url = resolve_base_url(base_url);
    require_server(&resolved_base_url).await?;
    let client = ApiClient::new(&resolved_base_url, agent_id);

    match args.command {
        AgentCommands::List => {
            let resp = client.get(&agent_list_endpoint()).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => {
                    print!("{}", format_agent_list_human(&resp));
                }
            }
        }
        AgentCommands::Info { agent_id: aid } => {
            let resp = client.get(&agent_info_endpoint(&aid)).await?;
            print_value(&resp, format);
        }
        AgentCommands::Create {
            id,
            name,
            description,
            workspace_dir,
            template_id,
            language,
            skills,
            provider_id,
            model_id,
        } => {
            let body = build_create_agent_body(CreateAgentBodyInput {
                id,
                name,
                description,
                workspace_dir,
                template_id,
                language,
                skills,
                provider_id,
                model_id,
            });
            let resp = client.post("/api/agents", &body).await?;
            print_value(&resp, format);
        }
        AgentCommands::Delete {
            agent_id: aid,
            remove_workspace,
            yes,
        } => {
            if !yes {
                eprintln!("WARNING: You are about to delete agent '{aid}'.");
                eprintln!("WARNING: This will stop the agent and remove it from agent list.");
                if remove_workspace {
                    eprintln!(
                        "WARNING: The local workspace directory will also be \
                         permanently deleted. This action cannot be undone."
                    );
                }
                eprint!("Continue with deletion? [y/N] ");
                std::io::stderr().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !delete_confirmation_allows_proceed(&input) {
                    anyhow::bail!("Aborted.");
                }
            }

            let workspace_dir: Option<PathBuf> = if remove_workspace {
                let info = client.get(&agent_info_endpoint(&aid)).await?;
                workspace_dir_for_local_removal(&info, working_dir)?
            } else {
                None
            };

            let resp = client.delete(&agent_delete_endpoint(&aid)).await?;

            if remove_workspace {
                if let Some(wd) = &workspace_dir {
                    if wd.exists() {
                        std::fs::remove_dir_all(wd)?;
                        if matches!(format, OutputFormat::Human) {
                            println!("Removed workspace directory: {}", wd.display());
                        }
                    }
                }
            }

            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => println!("Agent '{aid}' deleted."),
            }
        }
        AgentCommands::Chat {
            agent_id: to_agent,
            message,
            from_agent,
            chat_id,
            mode,
            timeout,
            json_output,
        } => {
            let from = from_agent.as_deref().unwrap_or(agent_id);
            let request = build_agent_chat_request(&message, from, &to_agent, chat_id);

            eprintln!("INFO: Using chat_id: {}", request.chat_id);
            if request.prefix_added {
                eprintln!("INFO: Auto-added identity prefix: [Agent {from} requesting]");
            }

            let to_client = ApiClient::new(&resolved_base_url, &to_agent);

            match mode {
                ChatMode::Stream => {
                    handle_stream_mode(
                        &to_client,
                        &to_agent,
                        &request.chat_id,
                        &request.body,
                        timeout,
                    )
                    .await?;
                }
                ChatMode::Final => {
                    handle_final_mode(
                        &to_client,
                        &to_agent,
                        &request.body,
                        timeout,
                        json_output,
                        &request.chat_id,
                    )
                    .await?;
                }
            }
        }
    }

    Ok(())
}

pub fn build_create_agent_body(input: CreateAgentBodyInput) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": input.name,
        "description": input.description,
    });
    if let Some(id) = input.id.filter(|value| !value.trim().is_empty()) {
        body["id"] = serde_json::Value::String(id);
    }
    if let Some(wd) = input.workspace_dir {
        body["workspace_dir"] = serde_json::Value::String(wd);
    }
    if let Some(tid) = input.template_id {
        body["template_id"] = serde_json::Value::String(tid);
    }
    if let Some(lang) = input.language {
        body["language"] = serde_json::Value::String(lang);
    }
    if !input.skills.is_empty() {
        body["skill_names"] = serde_json::json!(input.skills);
    }
    if let Some(pid) = input.provider_id {
        body["provider_id"] = serde_json::Value::String(pid);
    }
    if let Some(mid) = input.model_id {
        body["model_id"] = serde_json::Value::String(mid);
    }
    body
}

pub fn agent_info_endpoint(agent_id: &str) -> String {
    format!("/api/agents/{agent_id}")
}

pub fn agent_list_endpoint() -> String {
    "/api/agents".to_string()
}

pub fn agent_delete_endpoint(agent_id: &str) -> String {
    format!("/api/agents/{agent_id}")
}

pub fn format_agent_list_human(resp: &serde_json::Value) -> String {
    let agents = resp
        .get("agents")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));

    let Some(arr) = agents.as_array() else {
        return String::new();
    };

    if arr.is_empty() {
        return "No agents found.\n".to_string();
    }

    let mut output = String::from("Agents:\n");
    for a in arr {
        let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
        let enabled = a.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let status = if enabled { "" } else { " [disabled]" };
        output.push_str(&format!("  {id}: {name}{status}\n"));
    }
    output
}

pub fn delete_confirmation_allows_proceed(input: &str) -> bool {
    let answer = input.trim().to_lowercase();
    answer == "y" || answer == "yes"
}

pub fn workspace_dir_from_agent_info(info: &serde_json::Value) -> Option<PathBuf> {
    info.get("workspace_dir")
        .and_then(|v| v.as_str())
        .map(expand_tilde)
}

pub fn workspace_dir_for_local_removal(
    info: &serde_json::Value,
    working_dir: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(workspace_dir) = workspace_dir_from_agent_info(info) else {
        return Ok(None);
    };

    let normalized_working_dir = normalize_existing_or_lexical(working_dir);
    let resolved_workspace = resolve_workspace_dir(&workspace_dir, &normalized_working_dir);
    let normalized_workspace = normalize_existing_or_lexical(&resolved_workspace);

    if !is_workspace_dir_inside_working_dir(&normalized_workspace, &normalized_working_dir) {
        anyhow::bail!(
            "Refusing to remove workspace directory outside Rua working directory: {}",
            resolved_workspace.display()
        );
    }

    Ok(Some(normalized_workspace))
}

pub fn is_workspace_dir_inside_working_dir(workspace_dir: &Path, working_dir: &Path) -> bool {
    let workspace_dir = normalize_existing_or_lexical(workspace_dir);
    let working_dir = normalize_existing_or_lexical(working_dir);

    workspace_dir.starts_with(&working_dir) && workspace_dir != working_dir
}

pub fn agent_chat_endpoint(agent_id: &str, chat_id: &str) -> String {
    format!("{}/console/chat", chat_endpoint(agent_id, chat_id))
}

pub fn build_agent_chat_request(
    message: &str,
    from_agent: &str,
    _to_agent: &str,
    chat_id: Option<String>,
) -> AgentChatRequest {
    let final_chat_id = chat_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let final_message = ensure_identity_prefix(message, from_agent);
    let prefix_added = final_message != message;

    let body = serde_json::json!({
        "chat_id": final_chat_id,
        "input": [{
            "role": "user",
            "content": [{"type": "text", "text": final_message}],
        }],
    });

    AgentChatRequest {
        chat_id: final_chat_id,
        body,
        prefix_added,
    }
}

pub fn ensure_identity_prefix(text: &str, from_agent: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with("[Agent ") || trimmed.starts_with("[来自智能体 ") {
        return text.to_string();
    }
    format!("[Agent {from_agent} requesting] {text}")
}

async fn send_agent_chat_request(
    client: &ApiClient,
    to_agent: &str,
    chat_id: &str,
    body: &serde_json::Value,
    timeout_secs: u64,
) -> anyhow::Result<reqwest::Response> {
    let url = client.streaming_url(&agent_chat_endpoint(to_agent, chat_id));
    let resp = client
        .inner()
        .post(&url)
        .header("X-Agent-Id", to_agent)
        .header("Accept", "text/event-stream")
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .json(body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Chat request failed ({status}): {text}");
    }
    Ok(resp)
}

pub fn parse_sse_line(line: &str) -> Option<serde_json::Value> {
    let data = line.trim().strip_prefix("data: ")?;
    if data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

pub fn extract_text_from_content(event: &serde_json::Value) -> Option<String> {
    let content = event.get("content")?.as_array()?;
    for item in content {
        if item.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                return Some(text.to_string());
            }
        }
    }
    None
}

pub fn extract_final_agent_chat_text(events: &[serde_json::Value]) -> String {
    let mut text = String::new();
    for event in events.iter().rev() {
        let obj = event.get("object").and_then(|v| v.as_str()).unwrap_or("");
        let msg_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let status = event.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if obj == "message" && msg_type == "message" && status == "completed" {
            if let Some(t) = extract_text_from_content(event) {
                text = t;
                break;
            }
        }
    }
    if !text.is_empty() {
        return text;
    }
    events
        .iter()
        .filter(|event| event.get("type").and_then(|value| value.as_str()) == Some("text_delta"))
        .filter_map(|event| event.get("delta").and_then(|value| value.as_str()))
        .collect()
}

async fn handle_stream_mode(
    client: &ApiClient,
    to_agent: &str,
    chat_id: &str,
    body: &serde_json::Value,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    let resp = send_agent_chat_request(client, to_agent, chat_id, body, timeout_secs).await?;

    use futures::StreamExt;
    use tokio::io::AsyncBufReadExt;
    let stream = resp
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let reader = tokio_util::io::StreamReader::new(stream);
    let mut buf_reader = tokio::io::BufReader::new(reader);

    let mut line = String::new();
    let mut printed_text_len = 0usize;

    loop {
        line.clear();
        let n = buf_reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let Some(event) = parse_sse_line(&line) else {
            continue;
        };

        let obj = event.get("object").and_then(|v| v.as_str()).unwrap_or("");
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match (obj, event_type) {
            (_, "text_delta") => {
                if let Some(delta) = event.get("delta").and_then(|value| value.as_str()) {
                    print!("{delta}");
                    std::io::stdout().flush()?;
                    printed_text_len += delta.len();
                }
            }
            (_, "turn_terminal") => {
                if printed_text_len > 0 {
                    println!();
                }
                break;
            }
            ("message", _) => {
                let msg_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let status = event.get("status").and_then(|v| v.as_str()).unwrap_or("");

                if msg_type == "message" && (status == "completed" || status == "in_progress") {
                    if let Some(text) = extract_text_from_content(&event) {
                        if text.len() >= printed_text_len {
                            let new_text = &text[printed_text_len..];
                            if !new_text.is_empty() {
                                print!("{new_text}");
                                std::io::stdout().flush()?;
                                printed_text_len = text.len();
                            }
                        }
                    }
                }
            }
            ("response", _) => {}
            _ => {
                if let Some(error) = crate::chat::event_error_message(&event) {
                    anyhow::bail!("Chat failed: {error}");
                }
            }
        }
    }

    Ok(())
}

async fn handle_final_mode(
    client: &ApiClient,
    to_agent: &str,
    body: &serde_json::Value,
    timeout_secs: u64,
    json_output: bool,
    chat_id: &str,
) -> anyhow::Result<()> {
    let resp = send_agent_chat_request(client, to_agent, chat_id, body, timeout_secs).await?;

    use futures::StreamExt;
    use tokio::io::AsyncBufReadExt;
    let stream = resp
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let reader = tokio_util::io::StreamReader::new(stream);
    let mut buf_reader = tokio::io::BufReader::new(reader);

    let mut events = Vec::new();
    let mut line = String::new();

    loop {
        line.clear();
        let n = buf_reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        if let Some(event) = parse_sse_line(&line) {
            let is_done = event.get("type").and_then(|v| v.as_str()) == Some("turn_terminal");
            if let Some(error) = crate::chat::event_error_message(&event) {
                anyhow::bail!("Chat failed: {error}");
            }
            events.push(event);
            if is_done {
                break;
            }
        }
    }

    let full_text = extract_final_agent_chat_text(&events);

    if json_output {
        let resp_json = serde_json::json!({
            "text": full_text,
            "chat_id": chat_id,
        });
        crate::output::print_json(&resp_json);
    } else {
        println!("[CHAT: {chat_id}]");
        println!();
        if full_text.is_empty() {
            eprintln!("(No text content in response)");
        } else {
            println!("{full_text}");
        }
    }

    Ok(())
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        home_dir().join(rest)
    } else if s == "~" {
        home_dir()
    } else {
        PathBuf::from(s)
    }
}

fn home_dir() -> PathBuf {
    rua_config::dirs::home_dir()
}

fn resolve_workspace_dir(workspace_dir: &Path, working_dir: &Path) -> PathBuf {
    if workspace_dir.is_absolute() {
        workspace_dir.to_path_buf()
    } else {
        working_dir.join(workspace_dir)
    }
}

fn normalize_existing_or_lexical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| lexical_normalize(path))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
        }
    }

    normalized
}
