use rua_cli::setup_env::PROVIDERS;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

fn provider_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct ProviderEnvGuard {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl ProviderEnvGuard {
    fn cleared() -> Self {
        let previous = PROVIDERS
            .iter()
            .map(|provider| (provider.env_var, std::env::var_os(provider.env_var)))
            .collect();
        for provider in PROVIDERS {
            std::env::remove_var(provider.env_var);
        }
        Self { previous }
    }

    fn set(&self, key: &str, value: &str) {
        std::env::set_var(key, value);
    }
}

impl Drop for ProviderEnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn provider_display_names_match_python() {
    let names: Vec<&str> = PROVIDERS.iter().map(|p| p.display_name).collect();
    assert_eq!(
        names,
        vec![
            "OpenAI",
            "Anthropic",
            "OpenRouter",
            "DashScope",
            "Idealab",
            "DeepSeek",
            "Google Gemini",
        ],
    );
}

#[test]
fn provider_env_vars_match_python() {
    let vars: Vec<&str> = PROVIDERS.iter().map(|p| p.env_var).collect();
    assert_eq!(
        vars,
        vec![
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
            "DASHSCOPE_API_KEY",
            "IDEALAB_API_KEY",
            "DEEPSEEK_API_KEY",
            "GOOGLE_API_KEY",
        ],
    );
}

#[test]
fn non_interactive_no_keys_output() {
    let _lock = provider_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env = ProviderEnvGuard::cleared();
    let tmp = tempfile::tempdir().unwrap();
    let store = rua_config::EnvStore::new(tmp.path().to_path_buf());

    let mut count = 0u32;
    let mut lines: Vec<String> = Vec::new();
    for provider in PROVIDERS {
        if let Ok(val) = std::env::var(provider.env_var) {
            if !val.is_empty() {
                store.set(provider.env_var, val).unwrap();
                lines.push(format!("  ✓ {} configured", provider.display_name));
                count += 1;
            }
        }
    }
    if count == 0 {
        lines.push("No provider API keys found in environment variables.".into());
        lines.push("Set one or more of:".into());
        for p in PROVIDERS {
            lines.push(format!("  {}=<your-key>", p.env_var));
        }
    }

    let expected = "\
No provider API keys found in environment variables.
Set one or more of:
  OPENAI_API_KEY=<your-key>
  ANTHROPIC_API_KEY=<your-key>
  OPENROUTER_API_KEY=<your-key>
  DASHSCOPE_API_KEY=<your-key>
  IDEALAB_API_KEY=<your-key>
  DEEPSEEK_API_KEY=<your-key>
  GOOGLE_API_KEY=<your-key>";

    assert_eq!(lines.join("\n"), expected);
}

#[test]
fn non_interactive_with_key_output() {
    let _lock = provider_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let env = ProviderEnvGuard::cleared();
    let tmp = tempfile::tempdir().unwrap();
    let store = rua_config::EnvStore::new(tmp.path().to_path_buf());

    env.set("OPENAI_API_KEY", "sk-test-snapshot");

    let mut count = 0u32;
    let mut lines: Vec<String> = Vec::new();
    for provider in PROVIDERS {
        if let Ok(val) = std::env::var(provider.env_var) {
            if !val.is_empty() {
                store.set(provider.env_var, val).unwrap();
                lines.push(format!("  ✓ {} configured", provider.display_name));
                count += 1;
            }
        }
    }
    if count > 0 {
        lines.push(String::new());
        lines.push(format!("{count} key(s) imported into envs.json."));
    }

    assert!(
        lines.contains(&"  ✓ OpenAI configured".to_string()),
        "Expected OpenAI configured line, got: {:?}",
        lines,
    );
    assert!(
        lines.last().unwrap().contains("imported into envs.json"),
        "Expected import summary, got: {:?}",
        lines,
    );
}

#[test]
fn interactive_item_labels_include_env_var() {
    let items: Vec<String> = PROVIDERS
        .iter()
        .map(|p| format!("{} ({})", p.display_name, p.env_var))
        .collect();

    assert_eq!(items[0], "OpenAI (OPENAI_API_KEY)");
    assert_eq!(items[1], "Anthropic (ANTHROPIC_API_KEY)");
    assert_eq!(items[3], "DashScope (DASHSCOPE_API_KEY)");
    assert_eq!(items[6], "Google Gemini (GOOGLE_API_KEY)");
}
