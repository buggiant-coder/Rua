use clap::{Args, Subcommand};

use crate::http_client::{resolve_base_url, ApiClient};

#[derive(Debug, Args)]
pub struct ChannelsArgs {
    #[command(subcommand)]
    pub command: ChannelsCommands,
}

#[derive(Debug, Subcommand)]
pub enum ChannelsCommands {
    List,

    Enable {
        name: String,
    },

    Disable {
        name: String,
    },

    Send {
        #[arg(long = "sender-agent-id")]
        sender_agent_id: Option<String>,

        #[arg(long)]
        channel: String,

        #[arg(long)]
        target_user: String,

        #[arg(long)]
        target_session: String,

        #[arg(long)]
        text: String,
    },
}

pub fn run(args: ChannelsArgs, base_url: Option<&str>, agent_id: &str) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, base_url, agent_id))
}

async fn run_async(
    args: ChannelsArgs,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let base_url = resolve_base_url(base_url);
    let client = ApiClient::new(&base_url, agent_id);

    match args.command {
        ChannelsCommands::List => {
            let resp = client.get(&channels_config_endpoint()).await?;
            print_channels_table(&resp);
        }
        ChannelsCommands::Enable { name } => {
            set_channel_enabled(&client, &name, true).await?;
            println!("Channel '{}' enabled.", name);
        }
        ChannelsCommands::Disable { name } => {
            set_channel_enabled(&client, &name, false).await?;
            println!("Channel '{}' disabled.", name);
        }
        ChannelsCommands::Send {
            sender_agent_id,
            channel,
            target_user,
            target_session,
            text,
        } => {
            let send_agent_id = sender_agent_id.as_deref().unwrap_or(agent_id);
            let send_client = ApiClient::new(&base_url, send_agent_id);
            let payload =
                build_channel_send_payload(&channel, &target_user, &target_session, &text);
            let resp = send_client
                .post(&messages_send_endpoint(), &payload)
                .await?;
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
    }
    Ok(())
}

const SECRET_FIELDS: &[&str] = &[
    "bot_token",
    "client_secret",
    "app_secret",
    "http_proxy_auth",
    "twilio_auth_token",
];

fn mask_value(value: &str) -> String {
    if value.is_empty() {
        return "(empty)".to_string();
    }
    if value.len() <= 4 {
        return "****".to_string();
    }
    format!("{}****", &value[..4])
}

fn format_field_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "(null)".to_string(),
        other => other.to_string(),
    }
}

fn print_channels_table(resp: &serde_json::Value) {
    let channels = match resp.as_object() {
        Some(obj) => obj,
        None => {
            println!("No channels configured.");
            return;
        }
    };

    if channels.is_empty() {
        println!("No channels configured.");
        return;
    }

    for (name, config) in channels {
        let enabled = config
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let status = if enabled { "enabled" } else { "disabled" };

        println!();
        println!("{}", "─".repeat(40));
        println!("  {}  [{}]", name, status);
        println!("{}", "─".repeat(40));

        if let Some(obj) = config.as_object() {
            for (field, value) in obj {
                if field == "enabled" {
                    continue;
                }
                let display = if SECRET_FIELDS.contains(&field.as_str()) {
                    mask_value(&format_field_value(value))
                } else {
                    format_field_value(value)
                };
                println!("  {:<20}: {}", field, display);
            }
        }
    }

    println!();
}

async fn set_channel_enabled(client: &ApiClient, name: &str, enabled: bool) -> anyhow::Result<()> {
    let current = client.get(&channel_config_endpoint(name)).await?;

    let mut updated = match current {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            if let serde_json::Value::Null = other {
            } else {
                map.insert("_original".to_string(), other);
            }
            map
        }
    };

    updated.insert("enabled".to_string(), serde_json::Value::Bool(enabled));

    client
        .put(
            &channel_config_endpoint(name),
            &serde_json::Value::Object(updated),
        )
        .await?;

    Ok(())
}

pub fn channels_config_endpoint() -> String {
    "/api/config/channels".to_string()
}

pub fn channel_config_endpoint(name: &str) -> String {
    format!("/api/config/channels/{name}")
}

pub fn messages_send_endpoint() -> String {
    "/api/messages/send".to_string()
}

pub fn build_channel_send_payload(
    channel: &str,
    target_user: &str,
    target_session: &str,
    text: &str,
) -> serde_json::Value {
    serde_json::json!({
        "channel": channel,
        "target_user": target_user,
        "target_session": target_session,
        "text": text,
    })
}
