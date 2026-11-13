use std::path::Path;

use rua_config::dirs::{self, Paths};
use rua_config::types::{AgentProfileRef, ChannelConfig, JobsFile, SkillManifestFile};
use rua_doctor as doctor_svc;

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    #[command(subcommand)]
    pub command: Option<DoctorCommand>,

    #[arg(long, default_value_t = 5.0)]
    pub timeout: f64,

    #[arg(long)]
    pub deep: bool,
}

#[derive(Debug, clap::Subcommand)]
pub enum DoctorCommand {
    Fix(DoctorFixArgs),
}

#[derive(Debug, clap::Args)]
pub struct DoctorFixArgs {
    #[arg(long)]
    pub dry_run: bool,

    #[arg(long, short = 'y')]
    pub yes: bool,
}

pub fn run(args: DoctorArgs, paths: &Paths) -> anyhow::Result<()> {
    match args.command {
        Some(DoctorCommand::Fix(fix_args)) => run_fix(fix_args, paths),
        None => run_diagnostics(args.timeout, args.deep, paths),
    }
}

fn run_fix(args: DoctorFixArgs, paths: &Paths) -> anyhow::Result<()> {
    use rua_doctor::fix::{
        self, INTERACTIVE_FIX_IDS, NONINTERACTIVE_FIX_IDS, SAFE_FIX_IDS, SYNC_FIX_IDS,
    };

    println!("=== Rua Doctor Fix ===");
    println!();

    let fix_ids: Vec<&str> = SAFE_FIX_IDS
        .iter()
        .chain(SYNC_FIX_IDS.iter())
        .chain(INTERACTIVE_FIX_IDS.iter())
        .copied()
        .collect();

    if args.dry_run {
        println!("Planned operations (dry-run):");
        for id in &fix_ids {
            println!("  [{id}]");
        }
        println!();
        println!("(dry-run: no files modified)");
        return Ok(());
    }

    let interactive_ids: Vec<&str> = INTERACTIVE_FIX_IDS.to_vec();
    let noninteractive_ids: Vec<&str> = NONINTERACTIVE_FIX_IDS.to_vec();

    if !args.yes {
        println!("Running non-interactive fixes (use --yes to include interactive fixes):");
        println!();

        let mut applied = 0;
        for id in &noninteractive_ids {
            let result = fix::run_fix(id, &paths.rua_root);
            print_fix_result(&result);
            if result.success && !result.actions.is_empty() {
                applied += 1;
            }
        }

        if !interactive_ids.is_empty() {
            println!();
            println!(
                "Skipped interactive fixes (require --yes): {}",
                interactive_ids.join(", ")
            );
        }

        println!();
        println!("Done. Applied {applied} fix(es).");
        return Ok(());
    }

    println!("Running all fixes:");
    println!();

    let mut applied = 0;
    for id in &fix_ids {
        let result = fix::run_fix(id, &paths.rua_root);
        print_fix_result(&result);
        if result.success && !result.actions.is_empty() {
            applied += 1;
        }
    }

    println!();
    println!("Done. Applied {applied} fix(es).");
    Ok(())
}

fn print_fix_result(result: &rua_doctor::fix::FixResult) {
    if result.actions.is_empty() && result.success {
        println!("  [{}] OK — {}", result.fix_id, result.message);
        return;
    }
    if !result.success {
        println!("  [{}] FAIL — {}", result.fix_id, result.message);
    }
    for action in &result.actions {
        if action.applied {
            println!("    ✓ {}", action.description);
        } else if let Some(err) = &action.error {
            println!("    ✗ {} — {}", action.description, err);
        }
    }
}

fn run_diagnostics(timeout: f64, deep: bool, paths: &Paths) -> anyhow::Result<()> {
    let report = doctor_svc::run_diagnostics_filtered(paths, deep);

    println!("=== Rua Doctor ===");
    println!();

    let version = report.version();
    println!("=== Version ===");
    println!("  rua version: {}", version.rua_version);
    println!();

    let runtime = report.runtime();
    println!("=== Runtime ===");
    println!("  platform: {} {}", runtime.platform, runtime.arch);
    println!("  working_dir: {}", runtime.working_dir);
    print!("  working_dir status: ");
    if !runtime.working_dir_exists {
        println!("MISSING");
    } else if !runtime.working_dir_writable {
        println!("NOT WRITABLE");
    } else {
        println!("OK");
    }
    println!();

    let config = report.config();
    println!("=== Config ===");
    print!("  config file: {} ... ", config.config_path);
    if !config.config_exists {
        println!("MISSING");
    } else if !config.config_parseable {
        println!(
            "FAIL — {}",
            config.parse_error.as_deref().unwrap_or("unknown")
        );
    } else {
        println!("OK");
        println!(
            "  agents: {} (active: {})",
            config.agent_count, config.active_agent
        );
    }
    println!();

    let providers = report.providers();
    println!("=== Providers ===");
    print!("  secret dir: {} ... ", providers.secret_dir);
    if !providers.secret_dir_exists {
        println!("MISSING");
    } else {
        println!("OK");
    }

    if providers.providers_dir_exists {
        if providers.providers.is_empty() {
            println!("  providers: (none found)");
        } else {
            for p in &providers.providers {
                let key_status = if p.has_api_key {
                    "key: set"
                } else {
                    "key: NOT SET"
                };
                let url_status = if p.has_base_url {
                    "url: set"
                } else {
                    "url: NOT SET"
                };
                println!(
                    "  {} ({}) — {}, {}, {} model(s)",
                    p.id, p.name, key_status, url_status, p.model_count
                );
            }
        }
    } else {
        println!("  providers dir: not found");
    }

    if let Some(am) = &providers.default_model {
        println!("  active model: {} / {}", am.provider_id, am.model);
    } else {
        println!("  active model: NOT SET");
    }
    println!();

    let workspace = report.workspace();
    println!("=== Workspaces ===");
    if workspace.agents.is_empty() {
        println!("  (no agent profiles found)");
    } else {
        for a in &workspace.agents {
            let enabled = if a.enabled { "" } else { " [disabled]" };
            let ws_status = if !a.workspace_exists {
                "MISSING"
            } else if !a.agent_json_exists {
                "agent.json MISSING"
            } else {
                "OK"
            };
            println!(
                "  {}{}: {} ... {}",
                a.agent_id, enabled, a.workspace_dir, ws_status
            );
        }
    }
    println!();

    println!("=== Plugins ===");
    let plugins_dir = rua_config::get_plugins_dir(&paths.rua_root);
    print!("  plugins dir: {} ... ", plugins_dir.display());
    if plugins_dir.is_dir() {
        let count = std::fs::read_dir(&plugins_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .count()
            })
            .unwrap_or(0);
        println!("OK ({count} plugins)");
    } else {
        println!("not found (no plugins installed)");
    }
    println!();

    println!("=== Cron ===");
    let root_jobs_path = rua_config::get_jobs_path(&paths.rua_root);
    print!("  root jobs file: {} ... ", root_jobs_path.display());
    if root_jobs_path.is_file() {
        print_jobs_file_status(&root_jobs_path);
    } else {
        println!("not found");
    }

    let config_path = rua_config::get_config_path(&paths.rua_root);
    if config_path.is_file() {
        let config = rua_config::load_config(&config_path, &paths.rua_root)?;
        for (agent_id, profile_val) in &config.agents.profiles {
            let profile_ref: Option<AgentProfileRef> =
                serde_json::from_value(profile_val.clone()).ok();
            if let Some(pr) = profile_ref {
                let ws = dirs::shellexpand_path(&pr.workspace_dir);
                let ws_jobs = ws.join("jobs.json");
                if ws_jobs.is_file() {
                    print!("  {agent_id} jobs: {} ... ", ws_jobs.display());
                    print_jobs_file_status(&ws_jobs);
                }
            }
        }
    }
    println!();

    println!("=== Channels ===");
    print_channels_status(&paths.rua_root);
    println!();

    println!("=== Skills ===");
    print_skills_status(&paths.rua_root);
    println!();

    println!("=== Heartbeat ===");
    print_heartbeat_status(&paths.rua_root);
    println!();

    println!("=== Environment ===");
    print_env_status();
    println!();

    println!("=== Logs ===");
    print_logs_status(&paths.rua_root);
    println!();

    if timeout > 0.0 {
        println!("=== Server ===");
        let host = std::env::var("RUA_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = std::env::var("RUA_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8765);
        let health_url = format!("http://{host}:{port}/healthz");
        print!("  server health ({health_url}): ");

        match std::net::TcpStream::connect_timeout(
            &format!("{host}:{port}")
                .parse()
                .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port))),
            std::time::Duration::from_secs_f64(timeout),
        ) {
            Ok(_) => println!("reachable"),
            Err(_) => println!("not reachable (server may not be running)"),
        }
        println!();
    }

    println!("=== Checks ===");
    for check in &report.checks {
        let icon = match check.status {
            doctor_svc::registry::CheckStatus::Pass => "OK",
            doctor_svc::registry::CheckStatus::Warn => "WARN",
            doctor_svc::registry::CheckStatus::Fail => "FAIL",
        };
        println!("  [{}] {} — {}", check.name, icon, check.message);
    }
    println!();

    if !deep {
        println!("Run with --deep to include network checks");
        println!();
    }

    println!("Doctor checks complete.");
    Ok(())
}

pub fn print_jobs_file_status(path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<JobsFile>(&content) {
            Ok(jf) => {
                let enabled = jf.jobs.values().filter(|j| j.enabled).count();
                println!("OK ({} job(s), {} enabled)", jf.jobs.len(), enabled);
            }
            Err(e) => println!("FAIL — parse error: {e}"),
        },
        Err(e) => println!("FAIL — read error: {e}"),
    }
}

fn print_channels_status(wd: &Path) {
    let config_path = rua_config::get_config_path(wd);
    if !config_path.is_file() {
        println!("  (config not found — skipping channel checks)");
        return;
    }
    let config = match rua_config::load_config(&config_path, wd) {
        Ok(config) => config,
        Err(error) => {
            println!("  Configuration error: {error}");
            return;
        }
    };

    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<AgentProfileRef> = serde_json::from_value(profile_val.clone()).ok();
        if let Some(pr) = profile_ref {
            let ws = dirs::shellexpand_path(&pr.workspace_dir);
            let agent_json = ws.join("agent.json");
            if !agent_json.is_file() {
                continue;
            }
            let raw = match std::fs::read_to_string(&agent_json) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let agent_val: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(ch_val) = agent_val.get("channels") {
                if let Ok(ch) = serde_json::from_value::<ChannelConfig>(ch_val.clone()) {
                    let agent_enabled = collect_enabled_channels(&ch);
                    if !agent_enabled.is_empty() {
                        println!("  {agent_id} channels: {}", agent_enabled.join(", "));
                    }
                    if ch.voice.enabled {
                        check_voice_credentials(&ch.voice);
                    }
                }
            }
        }
    }
}

pub fn collect_enabled_channels(ch: &ChannelConfig) -> Vec<String> {
    let mut enabled = Vec::new();
    let checks: &[(&str, &rua_config::BaseChannelConfig)] = &[
        ("imessage", &ch.imessage),
        ("discord", &ch.discord),
        ("dingtalk", &ch.dingtalk),
        ("feishu", &ch.feishu),
        ("qq", &ch.qq),
        ("telegram", &ch.telegram),
        ("mattermost", &ch.mattermost),
        ("mqtt", &ch.mqtt),
        ("console", &ch.console),
        ("matrix", &ch.matrix),
        ("voice", &ch.voice),
        ("wecom", &ch.wecom),
        ("xiaoyi", &ch.xiaoyi),
        ("weixin", &ch.weixin),
        ("onebot", &ch.onebot),
    ];
    for (name, cfg) in checks {
        if cfg.enabled {
            enabled.push(name.to_string());
        }
    }
    for (key, val) in &ch.extra {
        let is_enabled = if let Some(obj) = val.as_object() {
            obj.get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        } else {
            false
        };
        if is_enabled {
            enabled.push(format!("{key} (plugin)"));
        }
    }
    enabled
}

fn check_voice_credentials(voice: &rua_config::BaseChannelConfig) {
    let required = ["twilio_account_sid", "twilio_auth_token", "phone_number"];
    let mut missing = Vec::new();
    for key in &required {
        let present = voice
            .extra
            .get(*key)
            .map(|v| match v {
                serde_json::Value::String(s) => !s.is_empty(),
                serde_json::Value::Null => false,
                _ => true,
            })
            .unwrap_or(false);
        if !present {
            missing.push(*key);
        }
    }
    if missing.is_empty() {
        println!("  voice credentials: OK");
    } else {
        println!(
            "  WARN: voice enabled but missing credentials: {}",
            missing.join(", ")
        );
    }
}

fn print_skills_status(wd: &Path) {
    let config_path = rua_config::get_config_path(wd);
    if !config_path.is_file() {
        println!("  (config not found — skipping skill checks)");
        return;
    }
    let config = match rua_config::load_config(&config_path, wd) {
        Ok(config) => config,
        Err(error) => {
            println!("  Configuration error: {error}");
            return;
        }
    };

    let mut any_found = false;
    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<AgentProfileRef> = serde_json::from_value(profile_val.clone()).ok();
        if let Some(pr) = profile_ref {
            let ws = dirs::shellexpand_path(&pr.workspace_dir);
            let skill_json = ws.join("skill.json");
            if !skill_json.is_file() {
                continue;
            }
            let content = match std::fs::read_to_string(&skill_json) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let manifest: SkillManifestFile = match serde_json::from_str(&content) {
                Ok(m) => m,
                Err(e) => {
                    println!("  {agent_id}: skill.json FAIL — {e}");
                    any_found = true;
                    continue;
                }
            };
            let total = manifest.skills.len();
            let enabled: Vec<&String> = manifest
                .skills
                .iter()
                .filter(|(_, e)| e.enabled)
                .map(|(k, _)| k)
                .collect();

            println!(
                "  {agent_id}: {} skill(s), {} enabled",
                total,
                enabled.len()
            );

            let skills_dir = ws.join("skills");
            for name in &enabled {
                let skill_dir = skills_dir.join(name);
                if !skill_dir.is_dir() {
                    println!(
                        "    WARN: skill {name:?} enabled but directory missing: {}",
                        skill_dir.display()
                    );
                }
            }
            any_found = true;
        }
    }
    if !any_found {
        println!("  (no skill manifests found)");
    }
}

fn print_heartbeat_status(wd: &Path) {
    let heartbeat_path = rua_config::get_heartbeat_query_path(wd);
    print!("  heartbeat file: {} ... ", heartbeat_path.display());
    if heartbeat_path.is_file() {
        match std::fs::metadata(&heartbeat_path) {
            Ok(meta) => {
                let size = meta.len();
                println!("OK ({size} bytes)");
            }
            Err(_) => println!("OK"),
        }
    } else {
        println!("not found");
    }

    let config_path = rua_config::get_config_path(wd);
    if config_path.is_file() {
        let config = match rua_config::load_config(&config_path, wd) {
            Ok(config) => config,
            Err(error) => {
                println!("  Configuration error: {error}");
                return;
            }
        };
        for (agent_id, profile_val) in &config.agents.profiles {
            let profile_ref: Option<AgentProfileRef> =
                serde_json::from_value(profile_val.clone()).ok();
            if let Some(pr) = profile_ref {
                let ws = dirs::shellexpand_path(&pr.workspace_dir);
                let hb_file = ws.join("HEARTBEAT.md");
                if hb_file.is_file() {
                    if let Ok(meta) = std::fs::metadata(&hb_file) {
                        println!(
                            "  {agent_id}: workspace HEARTBEAT.md ({} bytes)",
                            meta.len()
                        );
                    }
                }
            }
        }
    }
}

pub fn print_env_status() {
    let env_checks: &[(&str, &str)] = &[
        ("RUA_SECRET_DIR", "secret directory override"),
        ("RUA_CONFIG_FILE", "config file override"),
        ("RUA_HOST", "server host"),
        ("RUA_PORT", "server port"),
        ("RUA_JOBS_FILE", "jobs file override"),
        ("RUA_HEARTBEAT_FILE", "heartbeat file override"),
        ("RUA_LOG_LEVEL", "log level"),
        ("RUA_CONSOLE_STATIC_DIR", "console static dir"),
    ];

    for (var, desc) in env_checks {
        match std::env::var(var) {
            Ok(val) => println!("  {var}: {val} ({desc})"),
            Err(_) => println!("  {var}: (not set) ({desc})"),
        }
    }
}

pub fn print_logs_status(wd: &Path) {
    let log_path = wd.join("rua.log");
    print!("  log file: {} ... ", log_path.display());
    if log_path.is_file() {
        match std::fs::metadata(&log_path) {
            Ok(meta) => {
                let size_kb = meta.len() / 1024;
                if is_file_writable(&log_path) {
                    println!("OK ({size_kb} KiB, writable)");
                } else {
                    println!("WARN — exists but NOT WRITABLE ({size_kb} KiB)");
                }
            }
            Err(e) => println!("FAIL — stat error: {e}"),
        }
    } else if wd.is_dir() {
        if is_dir_writable_check(wd) {
            println!("not present (parent dir is writable — OK)");
        } else {
            println!("not present (parent dir NOT WRITABLE — logs may fail)");
        }
    } else {
        println!("not present (working dir missing)");
    }

    let log1 = wd.join("rua.log.1");
    if log1.is_file() {
        if let Ok(meta) = std::fs::metadata(&log1) {
            let size_kb = meta.len() / 1024;
            println!("  rotated log: {} ({size_kb} KiB)", log1.display());
        }
    }
}

fn is_file_writable(path: &Path) -> bool {
    std::fs::OpenOptions::new().append(true).open(path).is_ok()
}

fn is_dir_writable_check(path: &Path) -> bool {
    let probe = path.join(".rua_doctor_probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
