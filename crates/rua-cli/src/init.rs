use std::fs;
use std::path::Path;

use clap::Args;

#[derive(Debug, Args)]
pub struct InitArgs {}

pub fn run(_args: InitArgs, rua_root: &Path) -> anyhow::Result<()> {
    init_rua_root(rua_root)
}

fn init_rua_root(target: &Path) -> anyhow::Result<()> {
    println!("Initializing Rua Root at: {}", target.display());
    println!();

    for d in &["workspaces", "local_models"] {
        let p = target.join(d);
        fs::create_dir_all(&p)?;
        println!("  Created: {}", p.display());
    }

    let secret_dir = {
        let parent = target.parent().unwrap_or(Path::new("/"));
        let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("rua");
        parent.join(format!("{name}.secret"))
    };
    fs::create_dir_all(&secret_dir)?;
    println!("  Created: {}", secret_dir.display());

    let config_path = target.join("config.json");
    if !config_path.exists() {
        rua_config::save_config(&rua_config::RootConfig::default(), &config_path)?;
    }
    let config = rua_config::load_config(&config_path, target)?;
    rua_config::save_config(&config, &config_path)?;
    let report = rua_server::seed::agents::ensure_builtin_agents_seeded(target);
    if report
        .outcomes
        .iter()
        .any(|outcome| matches!(outcome.status, rua_server::seed::SeedRunStatus::Failed))
    {
        anyhow::bail!("Agent initialization failed: {report:?}");
    }
    let templates = rua_server::seed::templates::ensure_builtin_templates_seeded(target);
    if templates
        .outcomes
        .iter()
        .any(|outcome| outcome.status == rua_server::seed::SeedRunStatus::Failed)
    {
        anyhow::bail!("Template initialization failed: {templates:?}");
    }

    let envs_json = target.join("envs.json");
    if !envs_json.exists() {
        fs::write(&envs_json, "{}\n")?;
        println!("  Created: {}", envs_json.display());
    } else {
        println!("  Exists:  {}", envs_json.display());
    }

    println!();
    println!("Next steps:");
    println!("  1. Configure a provider:  rua providers add <provider_id>");
    println!("  2. Start the server:      rua server start");
    println!("  3. Chat with the agent:   rua chat send 'hello'");

    Ok(())
}
