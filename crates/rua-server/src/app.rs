use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rua_contract::config::AppConfig;

use crate::lifecycle::{BackgroundTask, StartupContext, StartupTimer, WarmupContext};
use crate::profiles::ServerProfile;
use crate::router;
use crate::state::AppState;

fn resolve_rua_root(options: &ServerOptions) -> PathBuf {
    let cli_arg = options.rua_root.as_deref().and_then(|p| p.to_str());
    rua_config::dirs::resolve_rua_root(cli_arg)
}

fn resolve_secret_dir(options: &ServerOptions) -> PathBuf {
    let cli_arg = options.rua_root.as_deref().and_then(|p| p.to_str());
    rua_config::dirs::Paths::new(cli_arg).secret_dir
}

fn root_config_to_app_config(_root: &rua_config::types::RootConfig) -> AppConfig {
    AppConfig::default()
}

fn check_pid_lockfile(rua_root: &Path) -> anyhow::Result<()> {
    let pid_path = rua_root.join("server.pid");
    if pid_path.exists() {
        let content = std::fs::read_to_string(&pid_path).unwrap_or_default();
        if let Ok(pid) = content.trim().parse::<u32>() {
            if crate::process_liveness::is_process_alive(pid) {
                anyhow::bail!(
                    "Cannot start Rua: Rua Root '{}' is already in use by another \
                     Rua Runtime (PID {pid}). If the Rua Desktop app is running, quit it \
                     before starting a standalone server, or pass --rua-root <path> to use \
                     a different Rua Root.",
                    rua_root.display(),
                );
            }
        }

        let _ = std::fs::remove_file(&pid_path);
    }
    Ok(())
}

fn write_pid_lockfile(rua_root: &Path) {
    let pid_path = rua_root.join("server.pid");
    let _ = std::fs::write(&pid_path, std::process::id().to_string());
}

pub struct ServerOptions {
    pub host: IpAddr,

    pub port: u16,

    pub profile: ServerProfile,

    pub console_static_dir: Option<PathBuf>,

    pub no_auth: bool,

    pub debug_log: bool,

    pub rua_root: Option<PathBuf>,

    pub desktop_endpoint_file: Option<PathBuf>,

    pub desktop_host: Option<crate::desktop_host::DesktopHostConfig>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".parse().unwrap(),
            port: 8765,
            profile: ServerProfile::Desktop,
            console_static_dir: None,
            no_auth: false,
            debug_log: false,
            rua_root: None,
            desktop_endpoint_file: None,
            desktop_host: None,
        }
    }
}

pub fn build(options: &ServerOptions, state: AppState) -> axum::Router {
    let static_dir = resolve_static_dir(options);
    router::build_app_with_desktop_host(state, static_dir, options.desktop_host.clone())
}

fn resolve_static_dir(options: &ServerOptions) -> Option<PathBuf> {
    crate::static_assets::resolve_static_dir(options.console_static_dir.as_deref())
}

fn redirect_stderr_to_log(rua_root: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::fd::IntoRawFd;
        let log_dir = rua_root.join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("startup.log");
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let fd = file.into_raw_fd();
            unsafe {
                let _ = libc::dup2(fd, 2);

                libc::close(fd);
            }
        }
    }
    let _ = rua_root;
}

pub async fn serve(options: ServerOptions) -> anyhow::Result<()> {
    let total_start = Instant::now();

    if options.desktop_endpoint_file.is_some() {
        crate::desktop_endpoint::validate_desktop_host(options.host)?;
    }
    if options.profile == crate::profiles::ServerProfile::MobileApk {
        anyhow::ensure!(
            options.host.is_loopback(),
            "mobile-apk requires a loopback listener"
        );
    }

    let rua_root = resolve_rua_root(&options);

    check_pid_lockfile(&rua_root)?;
    if matches!(
        options.profile,
        crate::profiles::ServerProfile::Desktop | crate::profiles::ServerProfile::MobileApk
    ) && !rua_root.join("config.json").exists()
    {
        rua_config::save_config(
            &rua_config::RootConfig::default(),
            &rua_root.join("config.json"),
        )?;
    }

    redirect_stderr_to_log(&rua_root);
    crate::panic_report::install_panic_hook(&rua_root);
    install_android_crypto_provider();

    write_pid_lockfile(&rua_root);

    eprintln!("[boot] t=0ms serve_entry");
    let config_path = rua_root.join("config.json");
    let pre_config = rua_config::loader::load_config(&config_path, &rua_root)?;
    eprintln!(
        "[boot] t={}ms pre_config_loaded",
        total_start.elapsed().as_millis()
    );
    let log_level = if options.debug_log {
        Some("debug".to_string())
    } else {
        pre_config.observability.log_level.clone()
    };
    let log_config = rua_observability::LogConfig {
        log_level,
        log_format: pre_config.observability.log_format.clone(),
        rotate_size_mb: pre_config.observability.rotate_size_mb.unwrap_or(5),
        rotate_keep: pre_config.observability.rotate_keep.unwrap_or(3),
        log_file: None,
    };
    crate::telemetry::init_tracing(&rua_root, &log_config);
    eprintln!(
        "[boot] t={}ms tracing_inited",
        total_start.elapsed().as_millis()
    );

    crate::path_augment::augment_from_user_shell().await;
    eprintln!(
        "[boot] t={}ms path_augmented",
        total_start.elapsed().as_millis()
    );
    tracing::info!(
        profile = %options.profile,
        host = %options.host,
        port = options.port,
        "starting rua server"
    );

    #[cfg(feature = "metrics")]
    {
        if pre_config.observability.metrics.enabled {
            let addr: std::net::SocketAddr = pre_config
                .observability
                .metrics
                .bind_addr
                .parse()
                .unwrap_or_else(|_| "127.0.0.1:9090".parse().unwrap());
            if let Err(e) = crate::telemetry::init_metrics(addr) {
                tracing::error!(%e, "failed to start metrics exporter");
            }
        }
    }

    let config_timer = StartupTimer::new("config_loaded");
    let rua_root = resolve_rua_root(&options);
    let secret_dir = resolve_secret_dir(&options);
    let config_path = rua_root.join("config.json");
    tracing::info!(path = %config_path.display(), "loading config from disk");
    let root_config = rua_config::loader::load_config(&config_path, &rua_root)?;
    let config = root_config_to_app_config(&root_config);
    config_timer.finish();
    eprintln!(
        "[boot] t={}ms config_loaded",
        total_start.elapsed().as_millis()
    );

    if options.profile == crate::profiles::ServerProfile::MobileApk {
        if options.no_auth {
            anyhow::bail!("mobile-apk requires authenticated access; --no-auth is not supported");
        }
        rua_core::phone_use::initialize_from_host(&tokio_util::sync::CancellationToken::new())
            .await
            .map_err(anyhow::Error::msg)?;
    }

    let state_timer = StartupTimer::new("state_built");
    let profile = options.profile;
    let mut state =
        tokio::task::spawn_blocking(move || AppState::load(config, rua_root, secret_dir, profile))
            .await
            .map_err(|e| {
                anyhow::anyhow!("AppState construction panicked on blocking pool: {e}")
            })??;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.wait_platform_initialization(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!("Platform initialization exceeded its 30 second startup deadline")
    })??;

    if options.no_auth {
        tracing::warn!("auth disabled via --no-auth flag");
        state.auth = Arc::new(crate::auth::AuthService::new_disabled());
    }
    state_timer.finish();
    eprintln!(
        "[boot] t={}ms state_built",
        total_start.elapsed().as_millis()
    );

    let startup_ctx = StartupContext {
        profile: options.profile.to_string(),
    };
    state.lifecycle.start_all(&startup_ctx).await?;
    state
        .conversation_input_intake
        .resume_task_results(None)
        .await
        .map_err(anyhow::Error::msg)?;
    let collaboration_recovery = state
        .collaboration
        .reconcile_startup()
        .await
        .map_err(anyhow::Error::msg)?;
    tracing::info!(
        recovered_terminal = collaboration_recovery.recovered_terminal,
        interrupted = collaboration_recovery.interrupted,
        redelivered = collaboration_recovery.redelivered,
        failed = collaboration_recovery.failed,
        "collaboration startup reconciliation completed"
    );
    eprintln!(
        "[boot] t={}ms lifecycle_started",
        total_start.elapsed().as_millis()
    );

    let streams_for_cleanup = Arc::clone(&state.streams);
    let media_cleanup_state = state.clone();
    state.lifecycle.register_background_task(BackgroundTask {
        name: "staged_media_cleanup",
        interval: Duration::from_secs(15 * 60),
        task_fn: Box::new(move || {
            let state = media_cleanup_state.clone();
            Box::pin(async move {
                if let Err(error) = crate::services::chat_media::ChatMediaService.collect_staged_scopes(&state).await {
                    tracing::warn!(%error, "staged media cleanup failed; uncertain media retained");
                }
            })
        }),
    }).await;
    state
        .lifecycle
        .register_background_task(BackgroundTask {
            name: "stream_registry_cleanup",
            interval: Duration::from_secs(15 * 60),
            task_fn: Box::new(move || {
                let streams = Arc::clone(&streams_for_cleanup);
                Box::pin(async move {
                    let removed = streams.cleanup_stale(Duration::from_secs(3600)).await;
                    if removed > 0 {
                        tracing::warn!(removed, "stream_registry_cleanup swept stale entries");
                    }
                })
            }),
        })
        .await;

    let _extension_router = rua_skills_hub::lib_ext::install_default_router();

    let router_timer = StartupTimer::new("router_built");
    let app = build(&options, state.clone());
    router_timer.finish();
    eprintln!(
        "[boot] t={}ms router_built",
        total_start.elapsed().as_millis()
    );

    let listener_timer = StartupTimer::new("listener_ready");
    let listener = crate::desktop_endpoint::bind_listener(
        options.host,
        options.port,
        options.desktop_endpoint_file.as_deref(),
    )
    .await?;
    let local_addr = listener.local_addr()?;
    state.voice_runtime.set_port(local_addr.port());
    let voice_state = state.clone();
    let mut voice_shutdown = state.shutdown_rx();
    let voice_start = tokio::spawn(async move {
        tokio::select! {
            _ = voice_shutdown.changed() => {},
            result = voice_state.voice_runtime.reload(&voice_state) => {
                if let Err(error) = result {
                    tracing::warn!(%error, "voice channel startup failed");
                }
            }
        }
    });
    listener_timer.finish();
    eprintln!(
        "[boot] t={}ms listener_ready_serving_now",
        total_start.elapsed().as_millis()
    );

    if let Some(path) = options.desktop_endpoint_file.as_deref() {
        let endpoint = crate::desktop_endpoint::DesktopEndpoint::new(local_addr);
        crate::desktop_endpoint::publish(path, &endpoint)?;
    }

    if options.profile == crate::profiles::ServerProfile::MobileApk {
        rua_core::phone_use::bridge()
            .ok_or_else(|| anyhow::anyhow!("Private APK bridge is not initialized"))?
            .publish_http_port(
                local_addr.port(),
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            .map_err(anyhow::Error::msg)?;
    }

    let automation_cancel = tokio_util::sync::CancellationToken::new();
    let automation_task = state.desktop_automation.start(automation_cancel.clone());

    state.set_ready();
    let total_startup = total_start.elapsed();
    eprintln!("[boot] t={}ms TOTAL_READY", total_startup.as_millis());
    tracing::info!(
        "[startup-prof] server_ready_ms={}",
        total_startup.as_millis()
    );
    tracing::info!("server listening on http://{}", local_addr);

    let warmup_lifecycle = state.lifecycle.clone();
    let warmup_profile = options.profile.to_string();
    tokio::spawn(async move {
        let warmup_timer = StartupTimer::new("background_warmup");
        let ctx = WarmupContext {
            profile: warmup_profile,
        };
        warmup_lifecycle.warmup_all(&ctx).await;
        warmup_timer.finish();
    });

    let seed_data_dir = state.rua_root.clone();
    tokio::spawn(async move {
        let report = crate::seed::prompts::ensure_builtin_prompt_library_seeded(&seed_data_dir);
        for outcome in report.outcomes {
            tracing::info!(
                entry_id = %outcome.entry_id,
                status = ?outcome.status,
                "prompt seed completed"
            );
        }
    });

    let shutdown_lifecycle = state.lifecycle.clone();
    let shutdown_streams = state.streams.clone();
    let shutdown_plugins = Some(state.plugin_registry.clone());
    let shutdown_task_runtime = Some(Arc::clone(&state.task_runtime));
    let shutdown_tx = state.shutdown_tx.clone();
    let shutdown_rx = state.shutdown_rx();
    let shutdown_voice_state = state.clone();
    let shutdown_usage_stats = state.usage_stats.clone();
    let shutdown_pid_file = Some(state.rua_root.join("server.pid"));

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            crate::shutdown::shutdown_signal(shutdown_rx).await;
            crate::shutdown::arm_force_exit_watchdog(std::time::Duration::from_secs(8));
            crate::shutdown::arm_second_signal_force_exit();

            let _ = shutdown_tx.send(true);
            let _ = voice_start.await;
            shutdown_voice_state
                .voice_runtime
                .stop(&shutdown_voice_state)
                .await;

            // Each process owner must begin cleanup even if another owner stalls.
            let policy = rua_workspace::ShutdownPolicy::CancelActive {
                timeout: Duration::from_secs(5),
            };
            match tokio::time::timeout(Duration::from_secs(5), async {
                let (report, (), (), ()) = tokio::join!(
                    shutdown_voice_state.registry.shutdown_all(policy),
                    shutdown_voice_state.acp_chat_registry.close_all(),
                    shutdown_voice_state.active_plugin_registry.shutdown_all(),
                    shutdown_voice_state.terminal_authority.shutdown_all(),
                );
                report
            })
            .await
            {
                Ok(Ok(report)) if report.is_clean() => tracing::info!("Agent workspaces shut down"),
                Ok(Ok(report)) => tracing::warn!(?report, "Agent workspace cleanup incomplete"),
                Ok(Err(error)) => tracing::warn!(%error, "Agent workspace shutdown failed"),
                Err(_) => tracing::warn!("Agent workspace shutdown exceeded its deadline"),
            }

            shutdown_usage_stats.flush_if_dirty().await;
            crate::shutdown::graceful_shutdown(
                shutdown_lifecycle,
                shutdown_streams,
                shutdown_plugins,
                shutdown_task_runtime,
                shutdown_pid_file,
            )
            .await;
            automation_cancel.cancel();
            let _ = automation_task.await;
            tracing::info!("cleanup complete, exiting");
            std::process::exit(0);
        })
        .await?;

    tracing::info!("server stopped");
    Ok(())
}

#[cfg(target_os = "android")]
fn install_android_crypto_provider() {
    if let Err(err) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("[boot] rustls crypto provider already installed or unavailable: {err:?}");
    }
}

#[cfg(not(target_os = "android"))]
fn install_android_crypto_provider() {}
