use crate::lifecycle::{LifecycleManager, ShutdownContext};
use crate::streaming::ActiveStreamRegistry;
use rua_plugin_runtime::PluginRegistry;
use std::path::PathBuf;
use std::sync::Arc;

pub async fn shutdown_signal(mut app_shutdown: tokio::sync::watch::Receiver<bool>) {
    if *app_shutdown.borrow() {
        tracing::info!("received application shutdown request");
        return;
    }

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to register SIGINT");
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("received SIGTERM"),
            _ = sigint.recv() => tracing::info!("received SIGINT"),
            _ = wait_for_app_shutdown(&mut app_shutdown) => {
                tracing::info!("received application shutdown request")
            },
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.expect("failed to listen for Ctrl+C");
                tracing::info!("received Ctrl+C");
            },
            _ = wait_for_app_shutdown(&mut app_shutdown) => {
                tracing::info!("received application shutdown request")
            },
        }
    }
}

async fn wait_for_app_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

pub async fn graceful_shutdown(
    lifecycle: Arc<LifecycleManager>,
    streams: Arc<ActiveStreamRegistry>,
    plugin_registry: Option<Arc<PluginRegistry>>,
    task_runtime: Option<Arc<rua_task::TaskRuntime>>,
    pid_file: Option<PathBuf>,
) {
    tracing::info!("starting graceful shutdown");

    if let Some(ref registry) = plugin_registry {
        match registry
            .dispatch_hook(rua_plugin_runtime::HookPoint::Shutdown, "{}")
            .await
        {
            Ok(_) => tracing::info!("plugin Shutdown hooks dispatched"),
            Err(e) => tracing::warn!(error = %e, "plugin Shutdown hook dispatch failed"),
        }
    }

    if let Some(task_runtime) = task_runtime {
        match task_runtime.shutdown().await {
            Ok(interrupted) => {
                tracing::info!(interrupted, "Task Runtime shutdown completed");
            }
            Err(error) => {
                tracing::error!(error = %error, "Task Runtime shutdown failed");
            }
        }
    }

    let active_count = streams.active_count().await;
    if active_count > 0 {
        tracing::info!("cleaning up {} active streams", active_count);
        streams
            .cleanup_stale(std::time::Duration::from_secs(0))
            .await;
    }

    let ctx = ShutdownContext { graceful: true };
    lifecycle.shutdown_all(&ctx).await;

    // Diagnostic artifacts are queued during model calls. Drain them only after
    // normal producers stop, without blocking a Tokio worker on filesystem fsyncs.
    if let Err(error) =
        llm_multiplex::debug_capture::shutdown_capture_writers(std::time::Duration::from_secs(5))
            .await
    {
        tracing::warn!(%error, "LLM debug capture shutdown drain incomplete");
    }

    if let Some(path) = pid_file {
        let _ = std::fs::remove_file(&path);
    }

    tracing::info!("graceful shutdown complete");
}

pub fn arm_force_exit_watchdog(timeout: std::time::Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        eprintln!(
            "shutdown watchdog expired after {:?}, force exiting",
            timeout.as_secs()
        );
        std::process::exit(0);
    });
}

pub fn arm_second_signal_force_exit() {
    #[cfg(unix)]
    {
        unsafe extern "C" fn force_exit_handler(_sig: libc::c_int) {
            let msg = b"received second signal, force exiting\n";
            libc::write(2, msg.as_ptr() as *const libc::c_void, msg.len());
            libc::_exit(130);
        }

        unsafe {
            libc::signal(
                libc::SIGINT,
                force_exit_handler as *const () as libc::sighandler_t,
            );
            libc::signal(
                libc::SIGTERM,
                force_exit_handler as *const () as libc::sighandler_t,
            );
        }
    }

    #[cfg(not(unix))]
    {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(30));
            std::process::exit(130);
        });
    }
}

pub fn android_on_foreground(trigger: Option<&rua_sync::trigger::SyncTriggerHandle>) {
    tracing::info!("android: foreground");
    if let Some(handle) = trigger {
        handle.notify_resume();
    }
}

pub fn android_on_background() {
    tracing::info!("android: background");
}

pub async fn android_shutdown(
    lifecycle: Arc<LifecycleManager>,
    streams: Arc<ActiveStreamRegistry>,
    plugin_registry: Option<Arc<PluginRegistry>>,
    task_runtime: Option<Arc<rua_task::TaskRuntime>>,
    pid_file: Option<PathBuf>,
) {
    tracing::info!("android: shutdown requested");
    graceful_shutdown(lifecycle, streams, plugin_registry, task_runtime, pid_file).await;
}
