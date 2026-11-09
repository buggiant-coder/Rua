use clap::{Args, Subcommand};

use crate::http_client::{resolve_base_url, ApiClient};
use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct UsageArgs {
    #[command(subcommand)]
    command: UsageCommands,
}

#[derive(Debug, Subcommand)]
enum UsageCommands {
    Query {
        #[arg(long, default_value_t = 30)]
        days: u32,

        #[arg(long)]
        model: Option<String>,

        #[arg(long)]
        provider: Option<String>,
    },

    Summary {
        #[arg(long, default_value_t = 30)]
        days: u32,
    },
}

pub fn run(
    args: UsageArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id))
}

async fn run_async(
    args: UsageArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let client = ApiClient::new(&resolve_base_url(base_url), agent_id);

    match args.command {
        UsageCommands::Query {
            days,
            model,
            provider,
        } => {
            let mut query_parts = vec![format!("days={days}")];
            if let Some(m) = &model {
                query_parts.push(format!("model_name={m}"));
            }
            if let Some(p) = &provider {
                query_parts.push(format!("provider_id={p}"));
            }
            let query_string = query_parts.join("&");
            let resp = client
                .get(&format!("/api/token-usage?{query_string}"))
                .await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => print_usage_human(&resp),
            }
        }
        UsageCommands::Summary { days } => {
            let resp = client.get(&format!("/api/token-usage?days={days}")).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => print_usage_human(&resp),
            }
        }
    }

    Ok(())
}

fn print_usage_human(resp: &serde_json::Value) {
    let prompt = resp
        .get("total_prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion = resp
        .get("total_completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let calls = resp
        .get("total_calls")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    println!("Token Usage Summary:");
    println!("  Total prompt tokens:     {prompt}");
    println!("  Total completion tokens: {completion}");
    println!("  Total API calls:         {calls}");

    if let Some(by_model) = resp.get("by_model").and_then(|v| v.as_object()) {
        if !by_model.is_empty() {
            println!();
            println!("  By Model:");
            for (key, val) in by_model {
                let p = val
                    .get("prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let c = val
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let n = val.get("call_count").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("    {key}: {p} prompt + {c} completion ({n} calls)");
            }
        }
    }
}
