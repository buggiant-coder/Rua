use clap::{CommandFactory, FromArgMatches, Parser};
use rua_cli::{Cli, Commands};
use rua_config::dirs::Paths;
use rua_core::runtime_layout::RuntimeLayout;
use rua_plugin_runtime::cli_registry::CliRegistry;
use rua_plugin_runtime::PluginCliCommand;

async fn wait_for_download_artifact(path: &std::path::Path) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if tokio::fs::metadata(path)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

fn main() -> anyhow::Result<()> {
    let server_owns_tracing =
        Cli::try_parse_from(std::env::args_os()).is_ok_and(|cli| cli.command.starts_server());
    let rua_root = pre_scan_rua_root();
    let paths = Paths::new(rua_root.as_deref());

    let runtime_layout = RuntimeLayout::from_current_process(None);
    if let Some(tmp) = runtime_layout.process_temp_dir_for_rua_root(&paths.rua_root) {
        let _ = std::fs::create_dir_all(&tmp);

        unsafe {
            std::env::set_var("TMPDIR", &tmp);
        }
    }

    let config_path = paths.rua_root.join("config.json");
    let diagnostics = match Cli::try_parse_from(std::env::args_os()) {
        Ok(cli) => matches!(cli.command, Commands::Init(_) | Commands::Doctor(_)),
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.exit()
        }
        Err(_) => false,
    };
    let observability = if diagnostics {
        // Initialization and diagnostics must be usable without loading broken configuration.
        rua_config::RootConfig::default().observability
    } else {
        match rua_config::loader::load_config(&config_path, &paths.rua_root) {
            Ok(config) => config.observability,
            // Logging also serves commands that do not require an initialized Rua root.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                rua_config::RootConfig::default().observability
            }
            Err(error) => return Err(error.into()),
        }
    };

    let verbose = std::env::args().any(|a| a == "--verbose" || a == "-v");
    let log_level = if verbose {
        Some("debug".to_string())
    } else {
        observability.log_level.clone()
    };
    let log_config = rua_observability::LogConfig {
        log_level,
        log_format: observability.log_format.clone(),
        rotate_size_mb: observability.rotate_size_mb.unwrap_or(5),
        rotate_keep: observability.rotate_keep.unwrap_or(3),
        log_file: None,
    };
    if !server_owns_tracing {
        rua_observability::init_tracing(&paths.rua_root, &log_config, vec![]);
    }
    rua_config::envs_sync::set_env_from_envs_json(&paths.rua_root);

    let plugins_dir = rua_config::get_plugins_dir(&paths.rua_root);
    let cli_registry = CliRegistry::from_plugins_dir(&plugins_dir);

    let builtin_cmd = Cli::command();
    let builtin_names: Vec<String> = builtin_cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();

    let mut cmd = builtin_cmd;
    for pc in cli_registry.commands() {
        if builtin_names.contains(&pc.spec.name) {
            eprintln!(
                "warning: plugin '{}' CLI command '{}' conflicts with built-in, skipping",
                pc.plugin_id, pc.spec.name
            );
            continue;
        }
        cmd = cmd.subcommand(plugin_cli_to_clap(pc));
    }

    let matches = cmd.get_matches();

    if let Some((sub_name, sub_matches)) = matches.subcommand() {
        if let Some(pc) = cli_registry.find_command(sub_name) {
            return dispatch_plugin_cli(&cli_registry, pc, sub_matches, &paths);
        }
    }

    let cli = Cli::from_arg_matches(&matches)?;
    let base_url = cli.base_url.as_deref();
    let agent_id = &cli.agent_id;
    let format = cli.output;

    match cli.command {
        Commands::Acp(args) => rua_cli::acp::run(args, &paths, base_url),
        Commands::Agent(args) => {
            rua_cli::agent::run(args, format, base_url, agent_id, &paths.rua_root)
        }
        Commands::Auth(args) => rua_cli::auth::run(args),
        Commands::Channels(args) => rua_cli::channels::run(args, base_url, agent_id),
        Commands::Chat(args) => rua_cli::chat::run(args, format, base_url, agent_id),
        Commands::Chats(args) => rua_cli::console::run(args, format, base_url, agent_id),
        Commands::Clean(args) => rua_cli::clean::run(args, &paths),
        Commands::Config(args) => rua_cli::config::run(args, format, &paths.rua_root),
        Commands::Cron(args) => rua_cli::cron::run(args, format, base_url, agent_id),
        Commands::Daemon(args) => rua_cli::daemon::run(args, &paths),
        Commands::Desktop(args) => rua_cli::desktop::run(args),
        Commands::Doctor(args) => rua_cli::doctor::run(args, &paths),
        Commands::Env(args) => rua_cli::env_cmd::run(args, &paths),
        Commands::Init(args) => rua_cli::init::run(args, &paths.rua_root),
        Commands::Mcp(args) => rua_cli::mcp::run(args, format, base_url, agent_id),
        Commands::Models(args) => rua_cli::models::run(args),
        Commands::Plugin(args) => rua_cli::plugin::run(args, base_url),
        Commands::Providers(args) => {
            rua_cli::providers::run(args, format, base_url, agent_id, &paths)
        }
        Commands::Project(args) => rua_cli::project::run(args, format, base_url, agent_id),
        Commands::Server(args) => {
            rua_cli::server::run(args, format, verbose, paths.rua_root.clone())
        }
        Commands::SetupEnv(args) => rua_cli::setup_env::run(args, &paths),
        Commands::Shutdown(args) => rua_cli::shutdown::run(args, &paths),
        Commands::Skills(args) => rua_cli::skills::run(args, format, base_url, agent_id),
        Commands::Uninstall(args) => rua_cli::uninstall::run(args, &paths),
        Commands::Update(args) => rua_cli::update::run(args),
        Commands::Usage(args) => rua_cli::usage::run(args, format, base_url, agent_id),
        Commands::Workspace(args) => rua_cli::workspace::run(args, format, base_url, agent_id),
        Commands::InternalDownloadTask { model_id } => {
            use indicatif::{ProgressBar, ProgressStyle};
            use rua_local_model::download_ipc::IpcMessage;
            use rua_local_model::LocalModelManager;
            use tokio::sync::broadcast::error::RecvError;

            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                LocalModelManager::init_instance(&paths.rua_root, false);
                let mgr = LocalModelManager::instance();
                let final_model_path = paths
                    .rua_root
                    .join("models")
                    .join(format!("{}.gguf", model_id.replace(['/', ':'], "_")));

                let _progress = mgr.download_model(model_id.clone()).await;
                let mut rx = mgr
                    .subscribe_download(&model_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to subscribe to download stream: {e}"))?;

                let pb = ProgressBar::new(0);
                pb.set_style(
                    ProgressStyle::with_template(
                        "{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
                    )
                    .unwrap()
                    .progress_chars("#>-"),
                );

                loop {
                    match rx.recv().await {
                        Ok(IpcMessage::Progress(p)) => {
                            if p.bytes_total > 0 {
                                pb.set_length(p.bytes_total);
                            }
                            pb.set_position(p.bytes_downloaded);
                        }
                        Ok(IpcMessage::Status(s)) => {
                            use rua_local_model::download_ipc::DownloadStatus;
                            match s.status {
                                DownloadStatus::Completed => {
                                    if !wait_for_download_artifact(&final_model_path).await {
                                        pb.abandon_with_message("failed");
                                        eprintln!(
                                            "download completed before final model artifact was written: {}",
                                            final_model_path.display()
                                        );
                                        std::process::exit(1);
                                    }
                                    pb.finish_with_message("done");
                                    return Ok(());
                                }
                                DownloadStatus::Cancelled => {
                                    pb.abandon_with_message("cancelled");
                                    eprintln!("download cancelled: {}", model_id);
                                    std::process::exit(1);
                                }
                                DownloadStatus::Started => {}
                            }
                        }
                        Ok(IpcMessage::Error(e)) => {
                            pb.abandon_with_message("failed");
                            eprintln!("download failed: {}", e.error);
                            std::process::exit(1);
                        }
                        Err(RecvError::Closed) => {
                            let status = mgr.model_download_progress().await;
                            match status {
                                Ok(dp) if dp.status == "completed" => {
                                    if !wait_for_download_artifact(&final_model_path).await {
                                        pb.abandon_with_message("failed");
                                        eprintln!(
                                            "download completed before final model artifact was written: {}",
                                            final_model_path.display()
                                        );
                                        std::process::exit(1);
                                    }
                                    pb.finish_with_message("done");
                                    return Ok(());
                                }
                                Ok(dp) if dp.error.is_some() => {
                                    pb.abandon_with_message("failed");
                                    eprintln!("download failed: {}", dp.error.unwrap());
                                    std::process::exit(1);
                                }
                                _ => {
                                    pb.finish_with_message("done");
                                    return Ok(());
                                }
                            }
                        }
                        Err(RecvError::Lagged(_)) => continue,
                    }
                }
            })
        }
    }
}

fn pre_scan_rua_root() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--rua-root" {
            return args.get(i + 1).cloned();
        }
        if let Some(val) = arg.strip_prefix("--rua-root=") {
            return Some(val.to_string());
        }
    }
    None
}

fn plugin_cli_to_clap(pc: &PluginCliCommand) -> clap::Command {
    let mut cmd = clap::Command::new(pc.spec.name.clone()).about(pc.spec.description.clone());
    for arg_spec in &pc.spec.args {
        let mut arg = clap::Arg::new(arg_spec.name.clone())
            .long(arg_spec.name.clone())
            .help(arg_spec.description.clone());
        if arg_spec.required {
            arg = arg.required(true);
        }
        cmd = cmd.arg(arg);
    }
    cmd
}

fn dispatch_plugin_cli(
    registry: &CliRegistry,
    pc: &PluginCliCommand,
    matches: &clap::ArgMatches,
    paths: &Paths,
) -> anyhow::Result<()> {
    let mut args = serde_json::Map::new();
    for arg_spec in &pc.spec.args {
        if let Some(val) = matches.get_one::<String>(&arg_spec.name) {
            args.insert(
                arg_spec.name.clone(),
                serde_json::Value::String(val.clone()),
            );
        }
    }

    let rt = tokio::runtime::Runtime::new()?;
    let output = rt.block_on(registry.dispatch(
        pc,
        serde_json::Value::Object(args),
        &paths.rua_root,
        &paths.secret_dir,
        &paths.cache_dir,
    ))?;

    if !output.is_empty() {
        println!("{output}");
    }

    Ok(())
}
