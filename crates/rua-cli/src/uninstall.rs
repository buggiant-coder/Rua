use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::Args;
use rua_config::dirs::Paths;

#[derive(Debug, Args)]
pub struct UninstallArgs {
    #[arg(long)]
    pub purge: bool,

    #[arg(long)]
    pub yes: bool,
}

const INSTALLER_DIRS: &[&str] = &["venv", "bin"];

const SHELL_PROFILE_NAMES: &[&str] = &[".zshrc", ".bashrc", ".bash_profile"];

pub const PATH_BLOCK_PATTERN: &str = "# MyRua\nexport PATH=\"$HOME/.rua/bin:$PATH\"";

fn home_dir() -> PathBuf {
    rua_config::dirs::home_dir()
}

fn shell_profiles() -> Vec<PathBuf> {
    let home = home_dir();
    SHELL_PROFILE_NAMES
        .iter()
        .map(|name| home.join(name))
        .collect()
}

pub fn remove_path_entry(profile: &Path) -> io::Result<bool> {
    if !profile.is_file() {
        return Ok(false);
    }

    let text = fs::read_to_string(profile)?;

    let cleaned = text.replace(&format!("\n{PATH_BLOCK_PATTERN}\n"), "\n");
    let cleaned = cleaned.replace(&format!("\n{PATH_BLOCK_PATTERN}"), "\n");
    let cleaned = cleaned.replace(&format!("{PATH_BLOCK_PATTERN}\n"), "");
    let cleaned = cleaned.replace(PATH_BLOCK_PATTERN, "");

    if cleaned == text {
        return Ok(false);
    }

    fs::write(profile, cleaned)?;
    Ok(true)
}

fn confirm(prompt: &str) -> io::Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}

pub fn run(args: UninstallArgs, paths: &Paths) -> anyhow::Result<()> {
    run_inner(args, paths.rua_root.clone(), shell_profiles())
}

pub fn run_inner(args: UninstallArgs, wd: PathBuf, profiles: Vec<PathBuf>) -> anyhow::Result<()> {
    if args.purge {
        println!("This will remove ALL Rua data in {}", wd.display());
    } else {
        println!("This will remove the Rua Python environment and CLI wrapper.");
        println!(
            "Your configuration and data in {} will be preserved.",
            wd.display()
        );
    }

    if !args.yes && !confirm("Continue?")? {
        println!("Cancelled.");
        return Ok(());
    }

    for dirname in INSTALLER_DIRS {
        let d = wd.join(dirname);
        if d.exists() {
            if d.is_dir() && !d.is_symlink() {
                fs::remove_dir_all(&d)?;
            } else {
                fs::remove_file(&d)?;
            }
            println!("  Removed {}", d.display());
        }
    }

    if args.purge && wd.exists() {
        fs::remove_dir_all(&wd)?;
        println!("  Removed {}", wd.display());
    }

    for profile in &profiles {
        match remove_path_entry(profile) {
            Ok(true) => println!("  Cleaned {}", profile.display()),
            Ok(false) => {}
            Err(e) => {
                eprintln!("warning: failed to clean {}: {e}", profile.display());
            }
        }
    }

    println!();
    println!("Rua uninstalled. Please restart your terminal.");
    Ok(())
}
