use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Read as _, Write as _};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand, ValueEnum};
use reqwest::StatusCode;
use serde_json::{Map, Value};

use crate::http_client::{error_status, resolve_base_url, ApiClient};
use crate::output::{print_value, OutputFormat};

#[derive(Debug, Args)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommands,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConflictMode {
    Fail,
    Skip,
    Overwrite,
}

impl ConflictMode {
    fn api_value(self) -> &'static str {
        match self {
            Self::Fail => "ask",
            Self::Skip => "skip",
            Self::Overwrite => "overwrite",
        }
    }
}

#[derive(Debug, Args)]
pub struct McpTransportArgs {
    #[arg(long)]
    pub url: Option<String>,

    #[arg(long)]
    pub bearer_token_env_var: Option<String>,

    #[arg(long = "env", value_parser = parse_key_value)]
    pub env: Vec<(String, String)>,

    #[arg(long = "header", value_parser = parse_key_value)]
    pub headers: Vec<(String, String)>,

    #[arg(long)]
    pub timeout: Option<u64>,

    #[arg(last = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum McpCommands {
    List {
        #[arg(long)]
        shared: bool,
    },

    Get {
        name: String,

        #[arg(long)]
        shared: bool,
    },

    #[command(alias = "add")]
    Install {
        name: String,

        #[command(flatten)]
        transport: McpTransportArgs,

        #[arg(long)]
        shared: bool,

        #[arg(long)]
        disabled: bool,
    },

    Import {
        file: String,

        #[arg(long)]
        shared: bool,

        #[arg(long)]
        replace: bool,
    },

    Update {
        name: String,

        #[command(flatten)]
        transport: McpTransportArgs,

        #[arg(long)]
        display_name: Option<String>,

        #[arg(long)]
        shared: bool,
    },

    Enable {
        name: String,
        #[arg(long)]
        shared: bool,
    },

    Disable {
        name: String,
        #[arg(long)]
        shared: bool,
    },

    Tools {
        name: String,
    },

    Call {
        name: String,

        tool: String,

        #[arg(long, default_value = "{}")]
        args: String,
    },

    SetSecret {
        name: String,

        path: String,

        #[arg(long, conflicts_with = "from_env")]
        stdin: bool,

        #[arg(long, value_name = "ENV_VAR", conflicts_with = "stdin")]
        from_env: Option<String>,

        #[arg(long)]
        shared: bool,
    },

    Login {
        name: String,

        #[arg(long, default_value = "")]
        scope: String,

        #[arg(long, default_value = "")]
        client_id: String,

        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },

    Logout {
        name: String,
    },

    Broadcast {
        names: Vec<String>,

        #[arg(long = "agent")]
        agents: Vec<String>,

        #[arg(long, conflicts_with = "names")]
        all: bool,

        #[arg(long)]
        all_agents: bool,

        #[arg(long, value_enum, default_value_t = ConflictMode::Fail)]
        on_conflict: ConflictMode,
    },

    Sync {
        names: Vec<String>,

        #[arg(long)]
        from_agent: String,

        #[arg(long, conflicts_with = "names")]
        all: bool,

        #[arg(long, value_enum, default_value_t = ConflictMode::Fail)]
        on_conflict: ConflictMode,
    },

    #[command(alias = "remove")]
    Uninstall {
        name: String,

        #[arg(long)]
        shared: bool,

        #[arg(long)]
        yes: bool,
    },
}

pub fn run(
    args: McpArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id))
}

async fn run_async(
    args: McpArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let client = ApiClient::new(&resolve_base_url(base_url), agent_id);

    match args.command {
        McpCommands::List { shared } => list_clients(&client, format, shared).await,
        McpCommands::Get { name, shared } => {
            let resp = client.get(&mcp_config_path(shared, &name)).await?;
            print_value(&resp, format);
            Ok(())
        }
        McpCommands::Install {
            name,
            transport,
            shared,
            disabled,
        } => {
            let body = build_install_body(&name, transport, !disabled)?;
            let resp = client.post(mcp_collection_path(shared), &body).await?;
            print_value(&resp, format);
            Ok(())
        }
        McpCommands::Import {
            file,
            shared,
            replace,
        } => import_clients(&client, format, &file, shared, replace).await,
        McpCommands::Update {
            name,
            transport,
            display_name,
            shared,
        } => {
            let body = build_update_body(transport, display_name)?;
            let resp = client.put(&mcp_config_path(shared, &name), &body).await?;
            print_value(&resp, format);
            Ok(())
        }
        McpCommands::Enable { name, shared } => {
            set_enabled(&client, format, &name, shared, true).await
        }
        McpCommands::Disable { name, shared } => {
            set_enabled(&client, format, &name, shared, false).await
        }
        McpCommands::Tools { name } => {
            let resp = client.get(&format!("/api/mcp/{name}/tools")).await?;
            print_value(&resp, format);
            Ok(())
        }
        McpCommands::Call {
            name,
            tool,
            args: tool_args,
        } => {
            let parsed_args: Value = serde_json::from_str(&tool_args)
                .map_err(|e| anyhow::anyhow!("invalid JSON for --args: {e}"))?;
            let body = build_tool_call_body(&tool, parsed_args);
            let resp = client.post(&format!("/api/mcp/{name}/call"), &body).await?;
            print_value(&resp, format);
            Ok(())
        }
        McpCommands::SetSecret {
            name,
            path,
            stdin,
            from_env,
            shared,
        } => {
            let secret = read_secret(stdin, from_env.as_deref())?;
            let endpoint = mcp_config_path(shared, &name);
            let current = client.get(&endpoint).await?;
            let body = build_secret_update_body(&current, &path, Value::String(secret))?;
            let resp = client.put(&endpoint, &body).await?;
            print_value(&resp, format);
            Ok(())
        }
        McpCommands::Login {
            name,
            scope,
            client_id,
            timeout,
        } => login(&client, format, &name, &scope, &client_id, timeout).await,
        McpCommands::Logout { name } => {
            let resp = match client.delete(&format!("/api/mcp/oauth/{name}")).await {
                Ok(resp) => resp,
                Err(error) if error_status(&error) == Some(StatusCode::NOT_FOUND) => {
                    serde_json::json!({"changed": false, "message": "MCP client not found"})
                }
                Err(error) => return Err(error),
            };
            print_value(&resp, format);
            Ok(())
        }
        McpCommands::Broadcast {
            names,
            agents,
            all,
            all_agents,
            on_conflict,
        } => {
            if names.is_empty() && !all {
                anyhow::bail!("specify one or more MCP names, or pass --all");
            }
            if agents.is_empty() && !all_agents {
                anyhow::bail!("specify one or more --agent values, or pass --all-agents");
            }
            let body = serde_json::json!({
                "client_keys": if all { Vec::<String>::new() } else { names },
                "agent_ids": if all_agents { Vec::<String>::new() } else { agents },
                "conflict_mode": on_conflict.api_value(),
            });
            let resp = client.post("/api/shared-mcp/broadcast", &body).await?;
            let failed = response_has_failures(&resp);
            print_value(&resp, format);
            if failed {
                anyhow::bail!("one or more MCP broadcasts failed");
            }
            Ok(())
        }
        McpCommands::Sync {
            names,
            from_agent,
            all,
            on_conflict,
        } => {
            if names.is_empty() && !all {
                anyhow::bail!("specify one or more MCP names, or pass --all");
            }
            let body = serde_json::json!({
                "agent_id": from_agent,
                "client_keys": if all { Vec::<String>::new() } else { names },
                "conflict_mode": on_conflict.api_value(),
            });
            let resp = client.post("/api/shared-mcp/sync", &body).await?;
            let failed = response_has_failures(&resp);
            print_value(&resp, format);
            if failed {
                anyhow::bail!("one or more MCP sync operations failed");
            }
            Ok(())
        }
        McpCommands::Uninstall { name, shared, yes } => {
            uninstall(&client, format, &name, shared, yes).await
        }
    }
}

pub fn build_tool_call_body(tool: &str, arguments: Value) -> Value {
    serde_json::json!({
        "tool": tool,
        "arguments": arguments,
    })
}

async fn list_clients(
    client: &ApiClient,
    format: OutputFormat,
    shared: bool,
) -> anyhow::Result<()> {
    let resp = client.get(mcp_collection_path(shared)).await?;
    match format {
        OutputFormat::Json => crate::output::print_json(&resp),
        OutputFormat::Human => {
            let clients = resp.as_array().cloned().unwrap_or_else(|| {
                resp.get("clients")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            });
            if clients.is_empty() {
                println!("No MCP clients configured.");
            } else {
                println!("MCP Clients:");
                for item in &clients {
                    let key = item.get("key").and_then(Value::as_str).unwrap_or("?");
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(key);
                    let enabled = item
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let transport = item.get("transport").and_then(Value::as_str).unwrap_or("?");
                    let status = if enabled { "enabled" } else { "disabled" };
                    println!("  {key}: {name} ({transport}) [{status}]");
                }
            }
        }
    }
    Ok(())
}

fn build_install_body(
    name: &str,
    transport: McpTransportArgs,
    enabled: bool,
) -> anyhow::Result<Value> {
    let mut body = transport_body(transport, true)?;
    let object = body
        .as_object_mut()
        .expect("transport_body always returns object");
    object.insert("key".into(), Value::String(name.to_string()));
    object.insert("name".into(), Value::String(name.to_string()));
    object.insert("enabled".into(), Value::Bool(enabled));
    Ok(body)
}

fn build_update_body(
    transport: McpTransportArgs,
    display_name: Option<String>,
) -> anyhow::Result<Value> {
    let mut body = transport_body(transport, false)?;
    let object = body
        .as_object_mut()
        .expect("transport_body always returns object");
    if let Some(display_name) = display_name {
        object.insert("name".into(), Value::String(display_name));
    }
    if object.is_empty() {
        anyhow::bail!("no MCP update fields were provided");
    }
    Ok(body)
}

fn transport_body(transport: McpTransportArgs, require_transport: bool) -> anyhow::Result<Value> {
    let McpTransportArgs {
        url,
        bearer_token_env_var,
        env,
        headers,
        timeout,
        command,
    } = transport;

    if url.is_some() && !command.is_empty() {
        anyhow::bail!("use either --url or a command after --, not both");
    }
    if require_transport && url.is_none() && command.is_empty() {
        anyhow::bail!("MCP install requires --url URL or -- COMMAND...");
    }
    if require_transport && !env.is_empty() && command.is_empty() {
        anyhow::bail!("--env is only valid for stdio MCP commands");
    }
    if require_transport && (!headers.is_empty() || bearer_token_env_var.is_some()) && url.is_none()
    {
        anyhow::bail!("--header and --bearer-token-env-var require --url");
    }

    let mut body = Map::new();
    if let Some(url) = url {
        body.insert("transport".into(), Value::String("streamable_http".into()));
        body.insert("url".into(), Value::String(url));
        if !headers.is_empty() {
            body.insert("headers".into(), map_value(headers));
        }
        if let Some(env_var) = bearer_token_env_var {
            body.insert("bearer_token_env_var".into(), Value::String(env_var));
        }
    } else if !command.is_empty() {
        body.insert("transport".into(), Value::String("stdio".into()));
        body.insert("command".into(), Value::String(command[0].clone()));
        body.insert(
            "args".into(),
            Value::Array(command[1..].iter().cloned().map(Value::String).collect()),
        );
        if !env.is_empty() {
            body.insert("env".into(), map_value(env));
        }
    } else {
        if !env.is_empty() {
            body.insert("env".into(), map_value(env));
        }
        if !headers.is_empty() {
            body.insert("headers".into(), map_value(headers));
        }
        if let Some(env_var) = bearer_token_env_var {
            body.insert("bearer_token_env_var".into(), Value::String(env_var));
        }
    }
    if let Some(timeout) = timeout {
        body.insert("timeout_secs".into(), Value::from(timeout));
    }
    Ok(Value::Object(body))
}

fn map_value(entries: Vec<(String, String)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )
}

async fn import_clients(
    client: &ApiClient,
    format: OutputFormat,
    file: &str,
    shared: bool,
    replace: bool,
) -> anyhow::Result<()> {
    let raw = read_text_input(file)?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("invalid MCP JSON: {e}"))?;
    let imported = parse_mcp_import(&value)?;
    if imported.is_empty() {
        anyhow::bail!("MCP import contains no clients");
    }

    let existing = configured_client_names(client, shared).await?;
    let conflicts: Vec<_> = imported
        .keys()
        .filter(|name| existing.contains(*name))
        .cloned()
        .collect();
    if !replace && !conflicts.is_empty() {
        anyhow::bail!(
            "MCP client(s) already exist: {}; pass --replace to overwrite",
            conflicts.join(", ")
        );
    }

    let mut results = Vec::new();
    for (name, config) in imported {
        let body = normalized_import_body(&name, config)?;
        let replaced = existing.contains(&name);
        let response = if replaced {
            let mut update = body.clone();
            if let Some(obj) = update.as_object_mut() {
                obj.remove("key");
            }
            client.put(&mcp_config_path(shared, &name), &update).await?
        } else {
            client.post(mcp_collection_path(shared), &body).await?
        };
        results.push(serde_json::json!({
            "name": name,
            "changed": true,
            "replaced": replaced,
            "client": response,
        }));
    }
    let output = Value::Array(results);
    print_value(&output, format);
    Ok(())
}

pub fn parse_mcp_import(value: &Value) -> anyhow::Result<BTreeMap<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("MCP import root must be a JSON object"))?;
    let clients = object
        .get("mcpServers")
        .or_else(|| object.get("mcp_servers"))
        .unwrap_or(value);
    let clients = clients
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("MCP client map must be a JSON object"))?;

    let mut result = BTreeMap::new();
    for (name, config) in clients {
        if !config.is_object() {
            anyhow::bail!("MCP config for '{name}' must be an object");
        }
        result.insert(name.clone(), config.clone());
    }
    Ok(result)
}

fn normalized_import_body(name: &str, config: Value) -> anyhow::Result<Value> {
    let mut object = config
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("MCP config for '{name}' must be an object"))?;

    if let Some(Value::Object(mut transport)) = object.remove("transport") {
        if let Some(kind) = transport.remove("type") {
            object.insert("transport".into(), kind);
        }
        for (key, value) in transport {
            object.entry(key).or_insert(value);
        }
    }
    if !object.contains_key("transport") {
        if let Some(kind) = object.remove("type") {
            object.insert("transport".into(), kind);
        } else if object.contains_key("url") {
            object.insert("transport".into(), Value::String("streamable_http".into()));
        } else if object.contains_key("command") {
            object.insert("transport".into(), Value::String("stdio".into()));
        }
    }
    object.insert("key".into(), Value::String(name.to_string()));
    object
        .entry("name")
        .or_insert_with(|| Value::String(name.to_string()));
    object.entry("enabled").or_insert(Value::Bool(true));
    Ok(Value::Object(object))
}

async fn configured_client_names(
    client: &ApiClient,
    shared: bool,
) -> anyhow::Result<BTreeSet<String>> {
    let response = client.get(mcp_collection_path(shared)).await?;
    Ok(response
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("key").and_then(Value::as_str))
        .map(str::to_string)
        .collect())
}

async fn set_enabled(
    client: &ApiClient,
    format: OutputFormat,
    name: &str,
    shared: bool,
    enabled: bool,
) -> anyhow::Result<()> {
    let endpoint = mcp_config_path(shared, name);
    let current = client.get(&endpoint).await?;
    if current.get("enabled").and_then(Value::as_bool) == Some(enabled) {
        print_value(
            &serde_json::json!({"name": name, "enabled": enabled, "changed": false}),
            format,
        );
        return Ok(());
    }

    let response = if shared {
        client
            .put(&endpoint, &serde_json::json!({"enabled": enabled}))
            .await?
    } else {
        client
            .patch(&format!("/api/mcp/{name}/toggle"), &serde_json::json!({}))
            .await?
    };
    print_value(
        &serde_json::json!({
            "name": name,
            "enabled": enabled,
            "changed": true,
            "client": response,
        }),
        format,
    );
    Ok(())
}

async fn uninstall(
    client: &ApiClient,
    format: OutputFormat,
    name: &str,
    shared: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let endpoint = mcp_config_path(shared, name);
    match client.get(&endpoint).await {
        Ok(_) => {}
        Err(error) if error_status(&error) == Some(StatusCode::NOT_FOUND) => {
            print_value(
                &serde_json::json!({"name": name, "changed": false, "removed": false}),
                format,
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    }

    if !yes && !confirm_destructive(&format!("Uninstall MCP client '{name}'?"))? {
        anyhow::bail!("aborted");
    }
    client.delete(&endpoint).await?;
    print_value(
        &serde_json::json!({"name": name, "changed": true, "removed": true}),
        format,
    );
    Ok(())
}

async fn login(
    client: &ApiClient,
    format: OutputFormat,
    name: &str,
    scope: &str,
    client_id: &str,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    let started = client
        .post(
            &format!("/api/mcp/oauth/start/{name}"),
            &serde_json::json!({
                "scope": scope,
                "client_id": client_id,
            }),
        )
        .await?;
    let auth_url = started
        .get("auth_url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("OAuth start response did not include auth_url"))?;
    eprintln!("Open this URL to authorize MCP '{name}':\n{auth_url}");

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let status = client.get(&format!("/api/mcp/oauth/status/{name}")).await?;
        if status
            .get("authorized")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            print_value(&status, format);
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("OAuth login timed out after {timeout_secs} seconds");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn response_has_failures(response: &Value) -> bool {
    response
        .get("results")
        .and_then(Value::as_array)
        .is_some_and(|results| {
            results.iter().any(|result| {
                result.get("success").and_then(Value::as_bool) == Some(false)
                    || matches!(
                        result.get("status").and_then(Value::as_str),
                        Some("error" | "conflict")
                    )
            })
        })
}

fn read_secret(stdin_flag: bool, from_env: Option<&str>) -> anyhow::Result<String> {
    if let Some(name) = from_env {
        return std::env::var(name)
            .map_err(|_| anyhow::anyhow!("environment variable '{name}' is not set"));
    }
    if stdin_flag {
        let mut value = String::new();
        io::stdin().read_to_string(&mut value)?;
        return Ok(value
            .strip_suffix("\r\n")
            .or_else(|| value.strip_suffix('\n'))
            .unwrap_or(&value)
            .to_string());
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!("secret input requires a terminal, --stdin, or --from-env");
    }
    rpassword::prompt_password_stderr("Secret value: ")
        .map_err(|e| anyhow::anyhow!("failed to read secret: {e}"))
}

fn read_text_input(path: &str) -> anyhow::Result<String> {
    if path == "-" {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        Ok(input)
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read MCP import file '{path}': {e}"))
    }
}

fn confirm_destructive(prompt: &str) -> anyhow::Result<bool> {
    if !io::stdin().is_terminal() {
        anyhow::bail!("confirmation requires a terminal; pass --yes for non-interactive use");
    }
    eprint!("{prompt} [y/N] ");
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn parse_key_value(raw: &str) -> Result<(String, String), String> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| "expected KEY=VALUE".to_string())?;
    let key = key.trim();
    if key.is_empty() {
        return Err("key must not be empty".to_string());
    }
    Ok((key.to_string(), value.to_string()))
}

fn mcp_collection_path(shared: bool) -> &'static str {
    if shared {
        "/api/shared-mcp"
    } else {
        "/api/mcp"
    }
}

pub fn mcp_config_path(shared: bool, client_key: &str) -> String {
    if shared {
        format!("/api/shared-mcp/{client_key}")
    } else {
        format!("/api/mcp/{client_key}")
    }
}

pub fn build_secret_update_body(
    current: &Value,
    logical_path: &str,
    value: Value,
) -> anyhow::Result<Value> {
    let path = normalize_secret_path(logical_path)?;
    let Some(parent_key) = path.first() else {
        anyhow::bail!("secret path cannot be empty");
    };

    let mut parent = current
        .get(parent_key)
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    let mut body = Map::new();
    if path.len() == 1 {
        body.insert(parent_key.clone(), value);
    } else {
        set_json_path(&mut parent, &path[1..], value)?;
        body.insert(parent_key.clone(), parent);
    }
    Ok(Value::Object(body))
}

fn normalize_secret_path(logical_path: &str) -> anyhow::Result<Vec<String>> {
    let mut parts: Vec<String> = logical_path
        .split('.')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        anyhow::bail!("secret path cannot be empty");
    }

    if parts.first().map(String::as_str) == Some("transport") {
        parts.remove(0);
    }

    let current_key = parts.last().map(String::as_str);
    let supported = rua_mcp::secret_field_io::should_protect_string(&parts, current_key);

    if supported {
        Ok(parts)
    } else {
        match parts.first().map(String::as_str) {
            Some("env" | "headers" | "oauth") => anyhow::bail!(
                "unsupported MCP secret path '{logical_path}': path must include a supported secret field"
            ),
            Some(other) => anyhow::bail!(
                "unsupported MCP secret path '{logical_path}': expected env.*, headers.*, oauth.*, raw secret fields like extra_token, or nested secret fields like nested.database_password; got top-level '{other}'"
            ),
            None => anyhow::bail!("secret path cannot be empty"),
        }
    }
}

fn set_json_path(target: &mut Value, path: &[String], value: Value) -> anyhow::Result<()> {
    if path.is_empty() {
        anyhow::bail!("secret path must include a field name");
    }

    let mut cursor = target;
    for part in &path[..path.len() - 1] {
        if !cursor.is_object() {
            *cursor = Value::Object(Map::new());
        }
        cursor = cursor
            .as_object_mut()
            .expect("cursor is an object")
            .entry(part.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }

    if !cursor.is_object() {
        *cursor = Value::Object(Map::new());
    }
    cursor
        .as_object_mut()
        .expect("cursor is an object")
        .insert(path[path.len() - 1].clone(), value);
    Ok(())
}
