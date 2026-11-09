use clap::Args;
use rua_config::dirs::Paths;
use rua_config::EnvStore;

#[derive(Debug, Args)]
pub struct SetupEnvArgs {
    #[arg(long)]
    pub non_interactive: bool,
}

pub struct ProviderEntry {
    pub display_name: &'static str,
    pub env_var: &'static str,
}

pub const PROVIDERS: &[ProviderEntry] = &[
    ProviderEntry {
        display_name: "OpenAI",
        env_var: "OPENAI_API_KEY",
    },
    ProviderEntry {
        display_name: "Anthropic",
        env_var: "ANTHROPIC_API_KEY",
    },
    ProviderEntry {
        display_name: "OpenRouter",
        env_var: "OPENROUTER_API_KEY",
    },
    ProviderEntry {
        display_name: "DashScope",
        env_var: "DASHSCOPE_API_KEY",
    },
    ProviderEntry {
        display_name: "Idealab",
        env_var: "IDEALAB_API_KEY",
    },
    ProviderEntry {
        display_name: "DeepSeek",
        env_var: "DEEPSEEK_API_KEY",
    },
    ProviderEntry {
        display_name: "Google Gemini",
        env_var: "GOOGLE_API_KEY",
    },
];

pub fn run(args: SetupEnvArgs, paths: &Paths) -> anyhow::Result<()> {
    let store = EnvStore::new(paths.secret_dir.clone());

    if args.non_interactive {
        run_non_interactive(&store)
    } else {
        run_interactive(&store)
    }
}

fn run_non_interactive(store: &EnvStore) -> anyhow::Result<()> {
    let mut count = 0u32;
    for provider in PROVIDERS {
        if let Ok(val) = std::env::var(provider.env_var) {
            if !val.is_empty() {
                store.set(provider.env_var, val)?;
                println!("  \u{2713} {} configured", provider.display_name);
                count += 1;
            }
        }
    }
    if count == 0 {
        println!("No provider API keys found in environment variables.");
        println!("Set one or more of:");
        for p in PROVIDERS {
            println!("  {}=<your-key>", p.env_var);
        }
    } else {
        println!("\n{count} key(s) imported into envs.json.");
    }
    Ok(())
}

fn run_interactive(store: &EnvStore) -> anyhow::Result<()> {
    use dialoguer::{theme::ColorfulTheme, MultiSelect, Password};

    let theme = ColorfulTheme::default();
    let items: Vec<String> = PROVIDERS
        .iter()
        .map(|p| format!("{} ({})", p.display_name, p.env_var))
        .collect();

    let selections = MultiSelect::with_theme(&theme)
        .with_prompt("Select providers to configure API keys")
        .items(&items)
        .interact()?;

    if selections.is_empty() {
        println!("No providers selected.");
        return Ok(());
    }

    for idx in selections {
        let provider = &PROVIDERS[idx];
        let key = Password::with_theme(&theme)
            .with_prompt(format!("API key for {}", provider.display_name))
            .interact()?;

        if !key.is_empty() {
            store.set(provider.env_var, key.clone())?;
            println!(
                "  \u{2713} {} \u{2014} API key saved",
                provider.display_name
            );
        }
    }

    println!("\nEnvironment variable configuration complete.");
    Ok(())
}
