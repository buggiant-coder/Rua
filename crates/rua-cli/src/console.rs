use clap::{Args, Subcommand};

use crate::chat::chat_endpoint;
use crate::http_client::{api_url, relative_url, resolve_base_url, ApiClient};
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
    selected_agent: Option<&str>,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, selected_agent))
}

async fn run_async(
    args: ConsoleArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    selected_agent: Option<&str>,
) -> anyhow::Result<()> {
    let client = ApiClient::new(
        &resolve_base_url(base_url),
        selected_agent.unwrap_or("default"),
    );

    match args.command {
        ConsoleCommands::List { user_id, channel } => {
            let path = chats_list_endpoint(selected_agent, user_id.as_deref(), channel.as_deref());
            let resp = client.get(&path).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => {
                    let chats = resp
                        .as_array()
                        .ok_or_else(|| anyhow::anyhow!("expected a Chat list"))?;
                    if chats.is_empty() {
                        println!("No chat sessions found.");
                    } else {
                        println!("Chat Sessions:");
                        for chat in chats {
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
            let resp = client.get(&chat_endpoint(&chat_id)).await?;
            print_value(&resp, format);
        }
        ConsoleCommands::Create {
            user_id,
            name,
            channel,
        } => {
            let body = build_create_chat_body(&name, client.agent_id(), &user_id, &channel);
            let resp = client.post("/api/chats", &body).await?;
            print_value(&resp, format);
        }
        ConsoleCommands::Update { chat_id, name } => {
            let body = serde_json::json!({ "name": name });
            let resp = client.put(&chat_endpoint(&chat_id), &body).await?;
            print_value(&resp, format);
        }
        ConsoleCommands::Delete { chat_id } => {
            let resp = client.delete(&chat_endpoint(&chat_id)).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => println!("Chat session '{chat_id}' deleted."),
            }
        }
    }

    Ok(())
}

pub fn build_create_chat_body(
    name: &str,
    agent_id: &str,
    user_id: &str,
    channel: &str,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "agent_id": agent_id,
        "user_id": user_id,
        "channel": channel,
    })
}

pub fn chats_list_endpoint(
    agent_id: Option<&str>,
    user_id: Option<&str>,
    channel: Option<&str>,
) -> String {
    let mut url = api_url(&["chats"]);
    let mut query = url.query_pairs_mut();
    if let Some(id) = agent_id {
        query.append_pair("agent_id", id);
    }
    if let Some(id) = user_id {
        query.append_pair("user_id", id);
    }
    if let Some(channel) = channel {
        query.append_pair("channel", channel);
    }
    drop(query);
    relative_url(url)
}
