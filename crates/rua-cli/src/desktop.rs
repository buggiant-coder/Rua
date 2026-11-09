use std::path::PathBuf;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;

use clap::Args;

#[derive(Debug, Args)]
pub struct DesktopArgs {}

const APP_NAME: &str = "Rua.app";
const WINDOWS_EXE_NAME: &str = "Rua.exe";

pub fn candidate_paths() -> Vec<PathBuf> {
    candidate_paths_for(
        std::env::consts::OS,
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        std::env::current_exe().ok(),
    )
}

pub fn find_app() -> Option<PathBuf> {
    find_app_in(std::env::consts::OS, candidate_paths())
}

pub fn run(_args: DesktopArgs) -> anyhow::Result<()> {
    if !matches!(std::env::consts::OS, "macos" | "windows") {
        anyhow::bail!("The desktop command is only supported on macOS and Windows.");
    }

    let app_path = match find_app() {
        Some(p) => p,
        None => {
            let searched: Vec<String> = candidate_paths()
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect();
            eprintln!("Rua desktop application not found.");
            eprintln!("Searched locations:");
            for loc in &searched {
                eprintln!("{loc}");
            }
            eprintln!();
            eprintln!("Install the Rua desktop app first, then retry.");
            std::process::exit(1);
        }
    };

    println!("Launching {}...", app_path.display());
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg(&app_path)
            .status()
            .map_err(|e| anyhow::anyhow!("Failed to execute 'open': {e}"))?;
        if !status.success() {
            anyhow::bail!(
                "'open {}' exited with status {}",
                app_path.display(),
                status
            );
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new(&app_path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| {
                anyhow::anyhow!(
                    "Failed to launch desktop application {}: {error}",
                    app_path.display()
                )
            })?;
    }

    Ok(())
}

#[doc(hidden)]
pub fn candidate_paths_for(
    platform: &str,
    home: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    current_exe: Option<PathBuf>,
) -> Vec<PathBuf> {
    match platform {
        "macos" => {
            let mut paths = Vec::new();
            if let Some(home) = home {
                paths.push(home.join("Applications").join(APP_NAME));
            }
            paths.push(PathBuf::from("/Applications").join(APP_NAME));
            paths
        }
        "windows" => {
            let mut paths = Vec::new();
            if let Some(parent) = current_exe.as_deref().and_then(|path| path.parent()) {
                paths.push(parent.join(WINDOWS_EXE_NAME));
            }
            if let Some(local_app_data) = local_app_data {
                paths.push(local_app_data.join("Rua").join(WINDOWS_EXE_NAME));
                paths.push(
                    local_app_data
                        .join("Programs")
                        .join("Rua")
                        .join(WINDOWS_EXE_NAME),
                );
            }
            paths.dedup();
            paths
        }
        _ => Vec::new(),
    }
}

#[doc(hidden)]
pub fn find_app_in(
    platform: &str,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    candidates.into_iter().find(|path| match platform {
        "macos" => path.is_dir(),
        "windows" => path.is_file(),
        _ => false,
    })
}
