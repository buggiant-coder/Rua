use std::fs;
use std::path::{Path, PathBuf};

use clap::Args;
use rua_config::dirs::Paths;

#[derive(Debug, Args)]
pub struct CleanArgs {
    #[arg(long)]
    pub dry_run: bool,

    #[arg(long)]
    pub all: bool,
}

const DEFAULT_TARGETS: &[&str] = &["logs", "cache", "workspaces", "tmp", "media"];

fn is_log_file(name: &str) -> bool {
    name.starts_with("rua.log")
}

pub fn collect_targets(wd: &Path, all: bool) -> Vec<PathBuf> {
    let mut targets = Vec::new();

    let children = match fs::read_dir(wd) {
        Ok(rd) => rd,
        Err(_) => return targets,
    };

    for entry in children.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let dominated = all || DEFAULT_TARGETS.contains(&name.as_str()) || is_log_file(&name);
        if dominated {
            targets.push(path);
        }
    }

    targets.sort();
    targets
}

fn remove_entry(path: &Path) -> anyhow::Result<()> {
    if path.is_dir() && !path.is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn run(args: CleanArgs, paths: &Paths) -> anyhow::Result<()> {
    run_with_dir(paths.rua_root.clone(), args.dry_run, args.all)
}

pub fn run_with_dir(wd: PathBuf, dry_run: bool, all: bool) -> anyhow::Result<()> {
    if !wd.exists() {
        println!("WORKING_DIR does not exist: {}", wd.display());
        return Ok(());
    }

    let targets = collect_targets(&wd, all);

    if targets.is_empty() {
        if all {
            println!("WORKING_DIR is already empty: {}", wd.display());
        } else {
            println!("Nothing to clean in: {}", wd.display());
        }
        return Ok(());
    }

    println!("WORKING_DIR: {}", wd.display());
    println!("Will remove:");
    for t in &targets {
        println!("  - {}", t.display());
    }

    if dry_run {
        println!("dry-run: nothing deleted.");
        return Ok(());
    }

    for t in &targets {
        match remove_entry(t) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("warning: failed to remove {}: {e}", t.display());
            }
        }
    }

    println!("Done.");
    Ok(())
}
