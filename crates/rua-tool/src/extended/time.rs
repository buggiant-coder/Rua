use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use tracing::warn;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::RuaError;

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub struct FixedClock {
    time: DateTime<Utc>,
}

impl FixedClock {
    pub fn new(time: DateTime<Utc>) -> Self {
        Self { time }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.time
    }
}

#[derive(Debug, Deserialize)]
struct TimeArgs {
    #[serde(default)]
    timezone: Option<String>,
}

pub struct GetCurrentTimeTool;

impl GetCurrentTimeTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "time_now".to_string(),
            description: "Get the current date and time in the specified timezone.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "timezone": {
                        "type": "string",
                        "description": "IANA timezone name (e.g., 'Asia/Shanghai', 'America/New_York'). Defaults to user's configured timezone or UTC."
                    }
                }
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
                idempotent: false,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }

    pub fn get_time(clock: &dyn Clock, timezone: Option<&str>) -> String {
        let utc_now = clock.now();
        format_time(utc_now, timezone)
    }
}

impl Default for GetCurrentTimeTool {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_time(utc_now: DateTime<Utc>, timezone: Option<&str>) -> String {
    let tz_name = timezone.unwrap_or("UTC");

    match tz_name.parse::<chrono_tz::Tz>() {
        Ok(tz) => {
            let local_time = utc_now.with_timezone(&tz);
            format!(
                "{} {} ({})",
                local_time.format("%Y-%m-%d %H:%M:%S"),
                tz_name,
                local_time.format("%A"),
            )
        }
        Err(_) => {
            let human = utc_now.format("%Y-%m-%d %H:%M:%S");
            let day = utc_now.format("%A");
            format!(
                "{human} UTC ({day})\n\n\
                 Note: Timezone '{tz_name}' was not recognized. Using UTC instead.\n\
                 Use IANA timezone names like 'America/New_York', 'Asia/Shanghai', 'Europe/London'."
            )
        }
    }
}

fn configured_user_timezone() -> rua_core::Result<Option<String>> {
    let working_dir = rua_config::dirs::rua_root();
    let config_path = rua_config::get_config_path(&working_dir);
    Ok(rua_config::load_config(&config_path, &working_dir)
        .map_err(|error| RuaError::Config {
            message: error.to_string(),
        })?
        .user_timezone
        .filter(|timezone| !timezone.trim().is_empty()))
}

#[async_trait]
impl Tool for GetCurrentTimeTool {
    fn name(&self) -> &str {
        "time_now"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let args: TimeArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "time_now".into(),
                message: e.to_string(),
            })?;

        let configured_timezone =
            if args.timezone.is_some() || ctx.metadata.contains_key("user_timezone") {
                None
            } else {
                configured_user_timezone()?
            };
        let timezone = args
            .timezone
            .as_deref()
            .or_else(|| ctx.metadata.get("user_timezone").and_then(|v| v.as_str()))
            .or(configured_timezone.as_deref());

        let output = format_time(Utc::now(), timezone);

        Ok(ToolResult::text("time_now", output))
    }
}

#[derive(Debug, Deserialize)]
struct SetTimezoneArgs {
    timezone_name: String,
}

pub struct SetUserTimezoneTool;

impl SetUserTimezoneTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "set_timezone".to_string(),
            description: "Set the user's timezone. Only call this when the user \
                          explicitly asks to change their timezone."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "timezone_name": {
                        "type": "string",
                        "description": "IANA timezone name (e.g., 'Asia/Shanghai', 'America/New_York', 'Europe/London', 'UTC')."
                    }
                },
                "required": ["timezone_name"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: false,
                idempotent: true,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for SetUserTimezoneTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SetUserTimezoneTool {
    fn name(&self) -> &str {
        "set_timezone"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let args: SetTimezoneArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "set_timezone".into(),
                message: e.to_string(),
            })?;

        let tz_name = args.timezone_name.trim().to_string();

        if tz_name.is_empty() {
            return Ok(ToolResult::error(
                "set_timezone",
                "Error: timezone is empty.",
            ));
        }

        match tz_name.parse::<chrono_tz::Tz>() {
            Ok(tz) => {
                let wd = rua_config::dirs::rua_root();
                let config_path = rua_config::get_config_path(&wd);
                let mut config = rua_config::load_config(&config_path, &wd).map_err(|error| {
                    RuaError::Config {
                        message: error.to_string(),
                    }
                })?;
                config.user_timezone = Some(tz_name.clone());
                if let Err(e) = rua_config::save_config(&config, &config_path) {
                    warn!("Failed to persist timezone to config: {e}");
                }

                let now = Utc::now().with_timezone(&tz);
                let time_str = format!(
                    "{} {} ({})",
                    now.format("%Y-%m-%d %H:%M:%S"),
                    tz_name,
                    now.format("%A"),
                );
                Ok(ToolResult::text(
                    "set_timezone",
                    format!("Timezone set to {tz_name}. Current time: {time_str}"),
                ))
            }
            Err(_) => Ok(ToolResult::error(
                "set_timezone",
                format!("Error: invalid timezone '{tz_name}'."),
            )),
        }
    }
}
