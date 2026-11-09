use std::collections::HashSet;
use std::io::Write as _;

use clap::{Args, Subcommand};
use rua_core::AgentCollaborationMode;
use serde::{Deserialize, Serialize};

use crate::http_client::{require_server, resolve_base_url, ApiClient};
use crate::output::{print_value, OutputFormat};

#[derive(Debug, Args)]
pub struct ChatArgs {
    #[command(subcommand)]
    command: ChatCommands,
}

#[derive(Debug, Subcommand)]
enum ChatCommands {
    Send {
        message: String,

        #[arg(long)]
        chat_id: Option<String>,

        #[arg(long)]
        model: Option<String>,

        #[arg(long)]
        mode: Option<AgentCollaborationMode>,
    },

    Show {
        chat_id: String,
    },

    Stream {
        message: String,

        #[arg(long)]
        chat_id: Option<String>,

        #[arg(long)]
        model: Option<String>,

        #[arg(long)]
        mode: Option<AgentCollaborationMode>,
    },

    Update {
        chat_id: String,

        #[arg(long)]
        mode: AgentCollaborationMode,
    },
}

pub fn run(
    args: ChatArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id))
}

async fn run_async(
    args: ChatArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let base_url = resolve_base_url(base_url);
    require_server(&base_url).await?;
    let client = ApiClient::new(&base_url, agent_id);

    match args.command {
        ChatCommands::Show { chat_id } => {
            let resp = client
                .get(&format!("/api/agents/{agent_id}/chats/{chat_id}"))
                .await?;
            print_value(&resp, format);
        }
        ChatCommands::Send {
            message,
            chat_id,
            model,
            mode,
        } => {
            let chat_id = chat_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            eprintln!("INFO: Using chat_id: {chat_id}");
            let body = build_chat_body(&message, Some(&chat_id), model.as_deref(), mode);
            let events = consume_sse_stream(
                &client,
                &chat_stream_endpoint(agent_id, &chat_id),
                agent_id,
                &body,
            )
            .await?;

            let full_text = extract_completed_text(&events);
            match format {
                OutputFormat::Json => {
                    let resp = build_response_json(&events);
                    crate::output::print_json(&resp);
                }
                OutputFormat::Human => {
                    if let Some(checklist) = extract_latest_work_checklist(&events) {
                        println!("{}", format_work_checklist(&checklist));
                    }
                    if full_text.is_empty() {
                        println!("(no response)");
                    } else {
                        println!("{full_text}");
                    }
                }
            }
        }
        ChatCommands::Stream {
            message,
            chat_id,
            model,
            mode,
        } => {
            let chat_id = chat_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            eprintln!("INFO: Using chat_id: {chat_id}");
            let body = build_chat_body(&message, Some(&chat_id), model.as_deref(), mode);
            stream_sse(
                &client,
                &chat_stream_endpoint(agent_id, &chat_id),
                agent_id,
                format,
                &body,
            )
            .await?;
        }
        ChatCommands::Update { chat_id, mode } => {
            let response = client
                .put(
                    &chat_endpoint(agent_id, &chat_id),
                    &serde_json::json!({"collaboration_mode": mode}),
                )
                .await?;
            print_value(&response, format);
        }
    }

    Ok(())
}

pub fn build_chat_body(
    message: &str,
    chat_id: Option<&str>,
    model: Option<&str>,
    mode: Option<AgentCollaborationMode>,
) -> serde_json::Value {
    let mut body = serde_json::json!({ "message": message });
    if let Some(cid) = chat_id {
        body["chat_id"] = serde_json::Value::String(cid.to_string());
    }
    if let Some(m) = model {
        body["model"] = serde_json::Value::String(m.to_string());
    }
    if let Some(mode) = mode {
        body[rua_core::TURN_COLLABORATION_MODE_OVERRIDE_KEY] = serde_json::json!(mode);
    }
    body
}

pub fn chat_endpoint(agent_id: &str, chat_id: &str) -> String {
    let mut url = reqwest::Url::parse("http://localhost").expect("valid URL base");
    {
        let mut path = url.path_segments_mut().expect("URL supports path segments");
        path.clear();
        for segment in ["api", "agents", agent_id, "chats", chat_id] {
            path.push(segment);
        }
    }
    url.path().to_string()
}

pub fn chat_stream_endpoint(agent_id: &str, chat_id: &str) -> String {
    format!("{}/console/chat", chat_endpoint(agent_id, chat_id))
}

pub(crate) async fn send_chat_request(
    client: &ApiClient,
    path: &str,
    agent_id: &str,
    body: &serde_json::Value,
) -> anyhow::Result<reqwest::Response> {
    let url = client.streaming_url(path);
    let resp = client
        .inner()
        .post(&url)
        .header("X-Agent-Id", agent_id)
        .header("Accept", "text/event-stream")
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

pub(crate) async fn consume_sse_stream(
    client: &ApiClient,
    path: &str,
    agent_id: &str,
    body: &serde_json::Value,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let resp = send_chat_request(client, path, agent_id, body).await?;
    let mut events = Vec::new();
    let mut line = String::new();

    use futures::StreamExt;
    use tokio::io::AsyncBufReadExt;
    let stream = resp
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let reader = tokio_util::io::StreamReader::new(stream);
    let mut buf_reader = tokio::io::BufReader::new(reader);

    loop {
        line.clear();
        let n = buf_reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        if let Some(event) = parse_sse_line(&line) {
            let is_done = event.get("type").and_then(|v| v.as_str()) == Some("turn_terminal");
            if let Some(error) = event_error_message(&event) {
                anyhow::bail!("Chat failed: {error}");
            }
            events.push(event);
            if is_done {
                break;
            }
        }
    }

    Ok(events)
}

pub(crate) async fn stream_sse(
    client: &ApiClient,
    path: &str,
    agent_id: &str,
    format: OutputFormat,
    body: &serde_json::Value,
) -> anyhow::Result<()> {
    let resp = send_chat_request(client, path, agent_id, body).await?;

    use futures::StreamExt;
    use tokio::io::AsyncBufReadExt;
    let stream = resp
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let reader = tokio_util::io::StreamReader::new(stream);
    let mut buf_reader = tokio::io::BufReader::new(reader);

    let mut line = String::new();
    let mut printed_text_len = 0usize;
    let mut assistant_text_message_ids = HashSet::new();
    let mut latest_work_checklist = None;

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
        record_assistant_text_message(&event, &mut assistant_text_message_ids);
        if matches!(format, OutputFormat::Human) {
            if let Some(checklist) = work_checklist_from_event(&event) {
                if latest_work_checklist.as_ref() != Some(&checklist) {
                    if printed_text_len > 0 {
                        println!();
                    }
                    println!("{}", format_work_checklist(&checklist));
                    latest_work_checklist = Some(checklist);
                }
            }
        }

        match (obj, event_type) {
            (_, "text_delta") => match format {
                OutputFormat::Human => {
                    if let Some(delta) = event.get("delta").and_then(|value| value.as_str()) {
                        print!("{delta}");
                        std::io::stdout().flush()?;
                        printed_text_len += delta.len();
                    }
                }
                OutputFormat::Json => crate::output::print_json(&event),
            },
            ("content", "text")
                if event.get("delta").and_then(serde_json::Value::as_bool) == Some(true)
                    && event
                        .get("msg_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| assistant_text_message_ids.contains(id)) =>
            {
                match format {
                    OutputFormat::Human => {
                        if let Some(delta) = event.get("text").and_then(serde_json::Value::as_str) {
                            print!("{delta}");
                            std::io::stdout().flush()?;
                            printed_text_len += delta.len();
                        }
                    }
                    OutputFormat::Json => crate::output::print_json(&event),
                }
            }
            (_, "turn_terminal") => {
                match format {
                    OutputFormat::Human if printed_text_len > 0 => println!(),
                    OutputFormat::Human => {}
                    OutputFormat::Json => crate::output::print_json(&event),
                }
                break;
            }
            ("message", _) => match format {
                OutputFormat::Human => {
                    let status = event.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    if status == "completed" || status == "in_progress" {
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
                OutputFormat::Json => crate::output::print_json(&event),
            },
            ("response", _) => {
                if matches!(format, OutputFormat::Json) {
                    crate::output::print_json(&event);
                }
            }
            _ => {
                if let Some(error) = event_error_message(&event) {
                    if matches!(format, OutputFormat::Json) {
                        crate::output::print_json(&event);
                    }
                    anyhow::bail!("Chat failed: {error}");
                }
                if matches!(format, OutputFormat::Json) {
                    crate::output::print_json(&event);
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn event_error_message(event: &serde_json::Value) -> Option<&str> {
    if event.get("type").and_then(serde_json::Value::as_str) != Some("error") {
        return None;
    }
    event
        .get("message")
        .or_else(|| event.get("error"))
        .and_then(serde_json::Value::as_str)
}

fn parse_sse_line(line: &str) -> Option<serde_json::Value> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let data = trimmed.strip_prefix("data: ")?;
    if data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

fn extract_text_from_content(event: &serde_json::Value) -> Option<String> {
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

fn extract_completed_text(events: &[serde_json::Value]) -> String {
    for event in events.iter().rev() {
        let obj = event.get("object").and_then(|v| v.as_str()).unwrap_or("");
        let msg_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let status = event.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if obj == "message" && msg_type == "message" && status == "completed" {
            if let Some(text) = extract_text_from_content(event) {
                return text;
            }
        }
    }
    let mut assistant_text_message_ids = HashSet::new();
    let mut text = String::new();
    for event in events {
        record_assistant_text_message(event, &mut assistant_text_message_ids);
        if event.get("type").and_then(serde_json::Value::as_str) == Some("text_delta") {
            if let Some(delta) = event.get("delta").and_then(serde_json::Value::as_str) {
                text.push_str(delta);
            }
            continue;
        }
        if event.get("object").and_then(serde_json::Value::as_str) == Some("content")
            && event.get("type").and_then(serde_json::Value::as_str) == Some("text")
            && event.get("delta").and_then(serde_json::Value::as_bool) == Some(true)
            && event
                .get("msg_id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| assistant_text_message_ids.contains(id))
        {
            if let Some(delta) = event.get("text").and_then(serde_json::Value::as_str) {
                text.push_str(delta);
            }
        }
    }
    text
}

fn record_assistant_text_message(event: &serde_json::Value, message_ids: &mut HashSet<String>) {
    if event.get("object").and_then(serde_json::Value::as_str) != Some("message")
        || event.get("type").and_then(serde_json::Value::as_str) != Some("message")
        || event.get("role").and_then(serde_json::Value::as_str) != Some("assistant")
    {
        return;
    }
    if let Some(id) = event.get("id").and_then(serde_json::Value::as_str) {
        message_ids.insert(id.to_string());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct WorkChecklistStep {
    step: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct WorkChecklist {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    explanation: Option<String>,
    plan: Vec<WorkChecklistStep>,
}

fn parse_work_checklist(arguments: &serde_json::Value) -> Option<serde_json::Value> {
    let value = match arguments {
        serde_json::Value::String(raw) => serde_json::from_str(raw).ok()?,
        value => value.clone(),
    };
    let mut checklist: WorkChecklist = serde_json::from_value(value).ok()?;
    let mut in_progress = 0usize;
    for step in &mut checklist.plan {
        step.step = step.step.trim().to_string();
        if step.step.is_empty()
            || !matches!(
                step.status.as_str(),
                "pending" | "in_progress" | "completed"
            )
        {
            return None;
        }
        if step.status == "in_progress" {
            in_progress += 1;
        }
    }
    if in_progress > 1 {
        return None;
    }
    checklist.explanation = checklist
        .explanation
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    serde_json::to_value(checklist).ok()
}

fn work_checklist_from_record(record: &serde_json::Value) -> Option<serde_json::Value> {
    if record.get("name").and_then(serde_json::Value::as_str) != Some("update_plan") {
        return None;
    }
    parse_work_checklist(record.get("arguments")?)
}

fn work_checklist_from_event(event: &serde_json::Value) -> Option<serde_json::Value> {
    if event.get("type").and_then(serde_json::Value::as_str) == Some("tool_call") {
        return work_checklist_from_record(event);
    }
    if event.get("object").and_then(serde_json::Value::as_str) == Some("content")
        && event.get("type").and_then(serde_json::Value::as_str) == Some("data")
    {
        return work_checklist_from_record(event.get("data")?);
    }
    if event.get("object").and_then(serde_json::Value::as_str) == Some("message")
        && matches!(
            event.get("type").and_then(serde_json::Value::as_str),
            Some("plugin_call" | "mcp_call")
        )
    {
        return event
            .get("content")?
            .as_array()?
            .iter()
            .filter_map(|part| part.get("data").or(Some(part)))
            .find_map(work_checklist_from_record);
    }
    None
}

fn extract_latest_work_checklist(events: &[serde_json::Value]) -> Option<serde_json::Value> {
    events
        .iter()
        .filter_map(work_checklist_from_event)
        .next_back()
}

pub fn format_work_checklist(checklist: &serde_json::Value) -> String {
    let Ok(checklist) = serde_json::from_value::<WorkChecklist>(checklist.clone()) else {
        return String::new();
    };
    let mut lines = vec!["Plan".to_string()];
    if let Some(explanation) = checklist.explanation {
        lines.push(format!("  {explanation}"));
    }
    lines.extend(checklist.plan.into_iter().map(|step| {
        let marker = match step.status.as_str() {
            "completed" => "x",
            "in_progress" => ">",
            _ => " ",
        };
        format!("  [{marker}] {}", step.step)
    }));
    lines.join("\n")
}

pub fn build_response_json(events: &[serde_json::Value]) -> serde_json::Value {
    let text = extract_completed_text(events);
    let chat_id = events
        .iter()
        .rev()
        .find_map(|e| {
            if e.get("object").and_then(|v| v.as_str()) == Some("response")
                || e.get("type").and_then(|v| v.as_str()) == Some("turn_terminal")
            {
                e.get("chat_id").cloned()
            } else {
                None
            }
        })
        .unwrap_or(serde_json::Value::Null);

    let mut response = serde_json::json!({
        "text": text,
        "chat_id": chat_id,
    });
    if let Some(checklist) = extract_latest_work_checklist(events) {
        response["work_checklist"] = checklist;
    }
    response
}
