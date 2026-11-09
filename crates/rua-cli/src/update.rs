use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Args;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(long)]
    yes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    CargoInstall,
    Git { repo_dir: PathBuf },
    Pip { installer: String },
    Unknown,
}

impl std::fmt::Display for InstallMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CargoInstall => write!(f, "cargo install"),
            Self::Git { repo_dir } => write!(f, "git ({})", repo_dir.display()),
            Self::Pip { installer } => write!(f, "{installer}"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

pub fn detect_install_method() -> InstallMethod {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return InstallMethod::Unknown,
    };

    let canonical = exe.canonicalize().unwrap_or_else(|_| exe.clone());

    if is_cargo_install_path(&canonical) {
        return InstallMethod::CargoInstall;
    }

    if let Some(repo) = find_git_repo(&canonical) {
        return InstallMethod::Git { repo_dir: repo };
    }

    if let Some(installer) = detect_pip_install() {
        return InstallMethod::Pip { installer };
    }

    InstallMethod::Unknown
}

pub fn is_cargo_install_path(exe: &Path) -> bool {
    let Some(parent) = exe.parent() else {
        return false;
    };
    let parent_str = parent.to_string_lossy();
    parent_str.contains(".cargo/bin") || parent_str.contains(".cargo\\bin")
}

fn find_git_repo(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent();
    while let Some(d) = dir {
        if d.join(".git").exists() && has_rua_cargo_toml(d) {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

pub fn has_rua_cargo_toml(dir: &Path) -> bool {
    let cargo_toml = dir.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(cargo_toml) {
        content.contains("rua")
    } else {
        false
    }
}

fn detect_pip_install() -> Option<String> {
    let output = Command::new("pip").args(["show", "rua"]).output().ok()?;
    if output.status.success() {
        let uv_output = Command::new("uv").arg("--version").output();
        if uv_output.is_ok_and(|o| o.status.success()) {
            return Some("uv pip".to_string());
        }
        return Some("pip".to_string());
    }
    None
}

fn run_command_streaming(cmd: &str, args: &[&str]) -> anyhow::Result<i32> {
    let mut child = Command::new(cmd)
        .args(args)
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to run `{cmd}`: {e}"))?;

    let status = child
        .wait()
        .map_err(|e| anyhow::anyhow!("Failed to wait for `{cmd}`: {e}"))?;

    Ok(status.code().unwrap_or(1))
}

fn update_cargo_install() -> anyhow::Result<()> {
    println!("Updating via cargo install...");
    println!();

    let exit_code = run_command_streaming("cargo", &["install", "rua-cli", "--force", "--locked"])?;

    if exit_code != 0 {
        anyhow::bail!(
            "cargo install failed with exit code {exit_code}. \
             Check network connectivity and try again."
        );
    }

    println!();
    println!("Update completed successfully.");
    Ok(())
}

fn update_git(repo_dir: &Path) -> anyhow::Result<()> {
    println!("Updating via git pull + cargo build...");
    println!();

    println!("[1/3] git pull...");
    let exit_code = run_command_streaming("git", &["-C", &repo_dir.to_string_lossy(), "pull"])?;
    if exit_code != 0 {
        anyhow::bail!(
            "git pull failed with exit code {exit_code}. \
             Check network connectivity and repository state."
        );
    }

    println!();
    println!("[2/3] cargo build --release...");
    let exit_code = run_command_streaming(
        "cargo",
        &[
            "build",
            "--release",
            "--manifest-path",
            &repo_dir.join("Cargo.toml").to_string_lossy(),
        ],
    )?;
    if exit_code != 0 {
        anyhow::bail!(
            "cargo build failed with exit code {exit_code}. \
             Check build output above for details."
        );
    }

    println!();
    println!("[3/3] Verifying new binary...");
    let release_bin = repo_dir.join("target/release/rua");
    if release_bin.exists() {
        let output = Command::new(&release_bin).arg("--version").output();
        match output {
            Ok(o) if o.status.success() => {
                let version = String::from_utf8_lossy(&o.stdout);
                println!("  New version: {}", version.trim());
            }
            _ => {
                println!("  Binary built but version check failed.");
            }
        }
    } else {
        println!(
            "  Warning: release binary not found at {}",
            release_bin.display()
        );
    }

    println!();
    println!("Update completed successfully.");
    println!("Note: you may need to copy the binary to your PATH or restart your shell.");
    Ok(())
}

fn update_pip(installer: &str) -> anyhow::Result<()> {
    println!("Updating via {installer}...");
    println!();

    let (cmd, args): (&str, Vec<&str>) = if installer.starts_with("uv") {
        (
            "uv",
            vec!["pip", "install", "--upgrade", "rua", "--prerelease=allow"],
        )
    } else {
        (
            "pip",
            vec!["install", "--upgrade", "rua", "--disable-pip-version-check"],
        )
    };

    let exit_code = run_command_streaming(cmd, &args)?;

    if exit_code != 0 {
        anyhow::bail!(
            "{installer} install failed with exit code {exit_code}. \
             Check network connectivity and try again."
        );
    }

    println!();
    println!("Update completed successfully.");
    println!("Please restart any running Rua service to use the new version.");
    Ok(())
}

pub fn run(args: UpdateArgs) -> anyhow::Result<()> {
    let method = detect_install_method();

    println!("=== Rua Update ===");
    println!();
    println!("Current version: {VERSION}");
    println!("Install method:  {method}");
    println!();

    if method == InstallMethod::Unknown {
        println!("Unable to detect how Rua was installed.");
        println!("You can update manually using one of the following methods:");
        println!();
        println!("  # If installed via cargo:");
        println!("  cargo install rua-cli --force");
        println!();
        println!("  # If installed via pip:");
        println!("  pip install --upgrade rua");
        println!();
        println!("  # If installed via uv:");
        println!("  uv pip install --upgrade rua");
        return Ok(());
    }

    if !args.yes {
        println!("This will update Rua using: {method}");
        print!("Continue? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            println!("Cancelled.");
            return Ok(());
        }
        println!();
    }

    match &method {
        InstallMethod::CargoInstall => update_cargo_install()?,
        InstallMethod::Git { repo_dir } => update_git(repo_dir)?,
        InstallMethod::Pip { installer } => update_pip(installer)?,
        InstallMethod::Unknown => unreachable!(),
    }

    Ok(())
}
