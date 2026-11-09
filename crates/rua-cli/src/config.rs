use std::io::{self, BufRead, Write as IoWrite};
use std::path::Path;

use clap::{Args, Subcommand};
use rua_config::types::{ActiveHoursConfig, HeartbeatConfig};

use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: Option<ConfigCommands>,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommands {
    Path,

    Show,

    Get {
        key: String,
    },

    Set {
        key: String,

        value: String,
    },

    Init {
        #[arg(long)]
        force: bool,

        #[arg(long)]
        defaults: bool,

        #[arg(long)]
        accept_security: bool,
    },
}

pub fn run(args: ConfigArgs, format: OutputFormat) -> anyhow::Result<()> {
    match args.command {
        None | Some(ConfigCommands::Show) => show_config(format),
        Some(ConfigCommands::Path) => {
            println!(
                "{}",
                rua_config::get_config_path(&rua_config::dirs::rua_root()).display()
            );
            Ok(())
        }
        Some(ConfigCommands::Get { key }) => get_config(&key, format),
        Some(ConfigCommands::Set { key, value }) => set_config(&key, &value, format),
        Some(ConfigCommands::Init {
            force,
            defaults,
            accept_security,
        }) => run_init(force, defaults, accept_security),
    }
}

fn show_config(format: OutputFormat) -> anyhow::Result<()> {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());

    if !config_path.is_file() {
        eprintln!("Config file not found: {}", config_path.display());
        eprintln!("Run `rua doctor` to diagnose, or create the config manually.");
        std::process::exit(1);
    }

    let content = std::fs::read_to_string(&config_path)?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse {}: {e}", config_path.display()))?;
    match format {
        OutputFormat::Json => crate::output::print_json(&value),
        OutputFormat::Human => {
            let pretty = serde_json::to_string_pretty(&value)?;
            println!("{pretty}");
        }
    }
    Ok(())
}

fn get_config(key: &str, format: OutputFormat) -> anyhow::Result<()> {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());
    if !config_path.is_file() {
        anyhow::bail!("Config file not found: {}", config_path.display());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;

    let mut current = &root;
    for part in key.split('.') {
        match current.get(part) {
            Some(v) => current = v,
            None => {
                anyhow::bail!("Key '{key}' not found in config");
            }
        }
    }

    match format {
        OutputFormat::Json => crate::output::print_json(current),
        OutputFormat::Human => match current {
            serde_json::Value::String(s) => println!("{s}"),
            other => println!("{}", serde_json::to_string_pretty(other)?),
        },
    }
    Ok(())
}

fn set_config(key: &str, value: &str, format: OutputFormat) -> anyhow::Result<()> {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());
    if !config_path.is_file() {
        anyhow::bail!("Config file not found: {}", config_path.display());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut root: serde_json::Value = serde_json::from_str(&content)?;

    let parsed_value: serde_json::Value = serde_json::from_str(value)
        .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));

    let parts: Vec<&str> = key.split('.').collect();
    let mut current = &mut root;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            if let Some(obj) = current.as_object_mut() {
                obj.insert(part.to_string(), parsed_value.clone());
            } else {
                anyhow::bail!("Cannot set key '{key}': parent is not an object");
            }
        } else {
            if current.get(part).is_none() {
                if let Some(obj) = current.as_object_mut() {
                    obj.insert(part.to_string(), serde_json::json!({}));
                }
            }
            current = current
                .get_mut(part)
                .ok_or_else(|| anyhow::anyhow!("Cannot traverse key '{key}'"))?;
        }
    }

    let output = serde_json::to_string_pretty(&root)?;
    std::fs::write(&config_path, output)?;

    match format {
        OutputFormat::Json => {
            crate::output::print_json(&serde_json::json!({
                "key": key,
                "value": parsed_value,
            }));
        }
        OutputFormat::Human => println!("Set {key} = {parsed_value}"),
    }
    Ok(())
}

pub const SECURITY_WARNING: &str = "\
Security warning -- please read.

Rua is a personal assistant that runs in your own environment. It can connect to
channels (DingTalk, Feishu, QQ, Discord, iMessage, etc.) and run skills that read
files, run commands, and call external APIs. By default it is a single-operator
boundary: one trusted user. A malicious or confused prompt can lead the agent to
do unsafe things if tools are enabled.

If multiple people can message the same Rua instance with tools enabled, they
share the same delegated authority (files, commands, secrets the agent can use).

If you are not comfortable with access control and hardening, do not run Rua with
tools or expose it to untrusted users. Get help from someone experienced before
enabling powerful skills or exposing the bot to the internet.

Recommended baseline:
- Restrict which channels and users can trigger the agent; use allowlists where possible.
- Multi-user or shared inbox: use separate config/credentials and ideally separate
  OS users or hosts per trust boundary.
- Run skills with least privilege; sandbox where you can.
- Keep secrets out of the agent's working directory and skill-accessible paths.
- Use a capable model when the agent has tools or handles untrusted input.

Review your config and skills regularly; limit tool scope to what you need.";

const DEFAULT_HEARTBEAT_EVERY: &str = "6h";

pub fn default_heartbeat_md(lang: &str) -> &'static str {
    match lang {
        "en" => {
            "# Heartbeat checklist\n\
             - Scan inbox for urgent email\n\
             - Check calendar for next 2h\n\
             - Check tasks for blockers\n\
             - Light check-in if quiet for 8h\n"
        }
        "ru" => {
            "# Heartbeat checklist\n\
             - \u{041f}\u{0440}\u{043e}\u{0432}\u{0435}\u{0440}\u{0438}\u{0442}\u{044c} \u{0432}\u{0445}\u{043e}\u{0434}\u{044f}\u{0449}\u{0438}\u{0435} \u{043d}\u{0430} \u{0441}\u{0440}\u{043e}\u{0447}\u{043d}\u{044b}\u{0435} \u{043f}\u{0438}\u{0441}\u{044c}\u{043c}\u{0430}\n\
             - \u{041f}\u{0440}\u{043e}\u{0441}\u{043c}\u{043e}\u{0442}\u{0440}\u{0435}\u{0442}\u{044c} \u{043a}\u{0430}\u{043b}\u{0435}\u{043d}\u{0434}\u{0430}\u{0440}\u{044c} \u{043d}\u{0430} \u{0431}\u{043b}\u{0438}\u{0436}\u{0430}\u{0439}\u{0448}\u{0438}\u{0435} 2 \u{0447}\u{0430}\u{0441}\u{0430}\n\
             - \u{041f}\u{0440}\u{043e}\u{0432}\u{0435}\u{0440}\u{0438}\u{0442}\u{044c} \u{0437}\u{0430}\u{0434}\u{0430}\u{0447}\u{0438} \u{043d}\u{0430} \u{043d}\u{0430}\u{043b}\u{0438}\u{0447}\u{0438}\u{0435} \u{0431}\u{043b}\u{043e}\u{043a}\u{0438}\u{0440}\u{043e}\u{0432}\u{043e}\u{043a}\n\
             - \u{041b}\u{0451}\u{0433}\u{043a}\u{0430}\u{044f} \u{043f}\u{0440}\u{043e}\u{0432}\u{0435}\u{0440}\u{043a}\u{0430} \u{043f}\u{0440}\u{0438} \u{043e}\u{0442}\u{0441}\u{0443}\u{0442}\u{0441}\u{0442}\u{0432}\u{0438}\u{0438} \u{0430}\u{043a}\u{0442}\u{0438}\u{0432}\u{043d}\u{043e}\u{0441}\u{0442}\u{0438} \u{0431}\u{043e}\u{043b}\u{0435}\u{0435} 8 \u{0447}\u{0430}\u{0441}\u{043e}\u{0432}\n"
        }
        _ => {
            "# Heartbeat checklist\n\
             - \u{626b}\u{63cf}\u{6536}\u{4ef6}\u{7bb1}\u{7d27}\u{6025}\u{90ae}\u{4ef6}\n\
             - \u{67e5}\u{770b}\u{672a}\u{6765} 2h \u{7684}\u{65e5}\u{5386}\n\
             - \u{68c0}\u{67e5}\u{5f85}\u{529e}\u{662f}\u{5426}\u{5361}\u{4f4f}\n\
             - \u{82e5}\u{5b89}\u{9759}\u{8d85}\u{8fc7} 8h\u{ff0c}\u{8f7b}\u{91cf} check-in\n"
        }
    }
}

fn prompt_line(prompt: &str) -> io::Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut buf = String::new();
    io::stdin().lock().read_line(&mut buf)?;
    Ok(buf.trim().to_string())
}

fn prompt_confirm(prompt: &str, default: bool) -> io::Result<bool> {
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    let input = prompt_line(&format!("{prompt} {hint} "))?;
    if input.is_empty() {
        return Ok(default);
    }
    Ok(matches!(input.to_lowercase().as_str(), "y" | "yes"))
}

fn prompt_with_default(prompt: &str, default: &str) -> io::Result<String> {
    let input = prompt_line(&format!("{prompt} [{default}]: "))?;
    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input)
    }
}

fn prompt_choice(prompt: &str, options: &[&str], default: &str) -> io::Result<String> {
    eprintln!("{prompt}");
    for (i, opt) in options.iter().enumerate() {
        let marker = if *opt == default { " (default)" } else { "" };
        eprintln!("  {}: {opt}{marker}", i + 1);
    }
    let input = prompt_line(&format!("Choice [{}]: ", default))?;
    if input.is_empty() {
        return Ok(default.to_string());
    }
    if let Ok(idx) = input.parse::<usize>() {
        if idx >= 1 && idx <= options.len() {
            return Ok(options[idx - 1].to_string());
        }
    }
    if options.contains(&input.as_str()) {
        return Ok(input);
    }
    Ok(default.to_string())
}

fn run_init(force: bool, use_defaults: bool, accept_security: bool) -> anyhow::Result<()> {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());
    let working_dir = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let heartbeat_path = rua_config::get_heartbeat_query_path(&rua_config::dirs::rua_root());

    println!("Working dir: {}", working_dir.display());

    println!();
    println!("{SECURITY_WARNING}");
    println!();
    if use_defaults && accept_security {
        println!("Security acceptance assumed (--accept-security with --defaults).");
    } else {
        let accepted = prompt_confirm(
            "Have you read and accepted the security notice above? (yes to continue, no to abort)",
            true,
        )?;
        if !accepted {
            println!("Initialization aborted. Read the security notice and run again when ready.");
            std::process::exit(1);
        }
    }
    std::fs::create_dir_all(&working_dir)?;

    let mut write_config = true;
    if config_path.is_file() && !force && !use_defaults {
        write_config = prompt_confirm(
            &format!(
                "{} exists. Do you want to overwrite it? (\"no\" to skip configuration)",
                config_path.display()
            ),
            false,
        )?;
    }

    if !write_config {
        println!("Skipping configuration.");
    } else {
        let mut config = if config_path.is_file() {
            rua_config::load_config(&config_path, &working_dir)?
        } else {
            rua_config::RootConfig::default()
        };

        if use_defaults {
            configure_defaults(&mut config);
        } else {
            configure_interactive(&mut config)?;
        }

        rua_config::save_config(&config, &config_path)?;
        println!(
            "\n\u{2713} Configuration saved to {}",
            config_path.display()
        );
    }

    let current_lang = {
        let cfg = if config_path.is_file() {
            rua_config::load_config(&config_path, &working_dir)?
        } else {
            rua_config::RootConfig::default()
        };
        cfg.agents.language.unwrap_or_else(|| "zh".to_string())
    };
    let default_hb_content = default_heartbeat_md(&current_lang);

    let mut write_heartbeat = true;
    if heartbeat_path.is_file() && !force {
        if use_defaults {
            println!("\u{2713} HEARTBEAT.md already present, skipped.");
            write_heartbeat = false;
        } else {
            write_heartbeat = prompt_confirm(
                &format!("{} exists. Overwrite?", heartbeat_path.display()),
                false,
            )?;
        }
    }
    if !write_heartbeat {
        if !use_defaults {
            println!("Skipped HEARTBEAT.md.");
        }
    } else {
        let content = if use_defaults {
            default_hb_content.to_string()
        } else {
            println!("\n=== Heartbeat Query Configuration ===");
            let use_default_hb = prompt_confirm("Use default heartbeat checklist?", true)?;
            if use_default_hb {
                default_hb_content.to_string()
            } else {
                println!("Enter heartbeat checklist (end with an empty line):");
                let mut lines = Vec::new();
                loop {
                    let line = prompt_line("")?;
                    if line.is_empty() {
                        break;
                    }
                    lines.push(line);
                }
                if lines.is_empty() {
                    default_hb_content.to_string()
                } else {
                    lines.join("\n") + "\n"
                }
            }
        };
        if let Some(parent) = heartbeat_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&heartbeat_path, content.trim())?;
        println!(
            "\u{2713} Heartbeat query saved to {}",
            heartbeat_path.display()
        );
    }

    println!("\n\u{2713} Initialization complete!");
    Ok(())
}

pub fn configure_defaults(config: &mut rua_config::RootConfig) {
    let hb = HeartbeatConfig {
        enabled: true,
        every: DEFAULT_HEARTBEAT_EVERY.to_string(),
        target: "main".to_string(),
        active_hours: None,
        extra: serde_json::Map::new(),
    };
    config.agents.heartbeat = Some(hb);
    config.show_tool_details = Some(true);
}

fn configure_interactive(config: &mut rua_config::RootConfig) -> io::Result<()> {
    println!("\n=== Heartbeat Configuration ===");
    let every = prompt_with_default("Heartbeat interval (e.g. 30m, 1h)", DEFAULT_HEARTBEAT_EVERY)?;
    let target = prompt_choice("Heartbeat target:", &["main", "last"], "main")?;

    let use_active = prompt_confirm("Set active hours for heartbeat? (skip = run 24h)", false)?;
    let active_hours = if use_active {
        let start = prompt_with_default("Active start (HH:MM)", "08:00")?;
        let end = prompt_with_default("Active end (HH:MM)", "22:00")?;
        Some(ActiveHoursConfig {
            start,
            end,
            extra: serde_json::Map::new(),
        })
    } else {
        None
    };

    let hb = HeartbeatConfig {
        enabled: true,
        every,
        target,
        active_hours,
        extra: serde_json::Map::new(),
    };
    config.agents.heartbeat = Some(hb);

    config.show_tool_details = Some(prompt_confirm(
        "Show tool call/result details in channel messages?",
        true,
    )?);

    let current_lang = config.agents.language.as_deref().unwrap_or("zh");
    let language = prompt_choice(
        "Select language for MD files:",
        &["zh", "en", "ru"],
        current_lang,
    )?;
    config.agents.language = Some(language);

    let configure_ch = prompt_confirm(
        "Configure channels? (iMessage/Discord/DingTalk/Feishu/QQ/Console)",
        false,
    )?;
    if configure_ch {
        println!("  (Channel configuration is available via `rua channels` commands)");
    }

    let configure_env = prompt_confirm("Configure environment variables?", false)?;
    if configure_env {
        println!("  (Environment variables can be managed via `rua env` commands)");
    }

    Ok(())
}
