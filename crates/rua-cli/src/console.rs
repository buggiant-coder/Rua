use clap::{Args, Subcommand};

use crate::http_client::{resolve_base_url, ApiClient};
use crate::output::{print_value, OutputFormat};

#[derive(Debug, Args)]
pub struct ConsoleArgs {
    #[command(subcommand)]
    command: ConsoleCommands,
}

#[derive(Debug, Subcommand)]
enum ConsoleCommands {
    List {
        #[arg(long)]
        user_id: Option<String>,

        #[arg(long)]
        channel: Option<String>,
    },

    Get {
        chat_id: String,
    },

    Create {
        #[arg(long)]
        user_id: String,

        #[arg(long, default_value = "New Chat")]
        name: String,

        #[arg(long, default_value = "console")]
        channel: String,
    },

    Update {
        chat_id: String,

        #[arg(long)]
        name: String,
    },

    Delete {
        chat_id: String,
    },
}

pub fn run(
    args: ConsoleArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id))
}

async fn run_async(
    args: ConsoleArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let client = ApiClient::new(&resolve_base_url(base_url), agent_id);

    match args.command {
        ConsoleCommands::List { user_id, channel } => {
            let path = chats_list_endpoint(agent_id, user_id.as_deref(), channel.as_deref());
            let resp = client.get(&path).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => {
                    let chats = resp
                        .get("chats")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    if chats.is_empty() {
                        println!("No chat sessions found.");
                    } else {
                        println!("Chat Sessions:");
                        for chat in &chats {
                            let id = chat.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                            let name = chat
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unnamed");
                            let ch = chat.get("channel").and_then(|v| v.as_str()).unwrap_or("?");
                            let uid = chat.get("user_id").and_then(|v| v.as_str()).unwrap_or("?");
                            println!("  {id}: {name} [channel={ch}, user={uid}]");
                        }
                    }
                }
            }
        }
        ConsoleCommands::Get { chat_id } => {
            let resp = client.get(&chat_endpoint(agent_id, &chat_id)).await?;
            print_value(&resp, format);
        }
        ConsoleCommands::Create {
            user_id,
            name,
            channel,
        } => {
            let body = build_create_chat_body(&name, &user_id, &channel);
            let resp = client
                .post(&chats_collection_endpoint(agent_id), &body)
                .await?;
            print_value(&resp, format);
        }
        ConsoleCommands::Update { chat_id, name } => {
            let body = serde_json::json!({ "name": name });
            let resp = client
                .put(&chat_endpoint(agent_id, &chat_id), &body)
                .await?;
            print_value(&resp, format);
        }
        ConsoleCommands::Delete { chat_id } => {
            let resp = client.delete(&chat_endpoint(agent_id, &chat_id)).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => println!("Chat session '{chat_id}' deleted."),
            }
        }
    }

    Ok(())
}

pub fn build_create_chat_body(name: &str, user_id: &str, channel: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "user_id": user_id,
        "channel": channel,
    })
}

pub fn chats_collection_endpoint(agent_id: &str) -> String {
    format!("/api/agents/{agent_id}/chats")
}

pub fn chats_list_endpoint(agent_id: &str, user_id: Option<&str>, channel: Option<&str>) -> String {
    let mut query_parts = Vec::new();
    if let Some(uid) = user_id {
        query_parts.push(format!("user_id={uid}"));
    }
    if let Some(ch) = channel {
        query_parts.push(format!("channel={ch}"));
    }

    let base = chats_collection_endpoint(agent_id);
    if query_parts.is_empty() {
        base
    } else {
        format!("{base}?{}", query_parts.join("&"))
    }
}

pub fn chat_endpoint(agent_id: &str, chat_id: &str) -> String {
    format!("/api/agents/{agent_id}/chats/{chat_id}")
}
