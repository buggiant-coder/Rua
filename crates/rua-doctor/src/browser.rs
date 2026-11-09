use std::process::Command;

use serde::Serialize;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

const BROWSER_CANDIDATES: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
    "chrome",
    "firefox",
];

#[derive(Debug, Clone, Serialize)]
pub struct BrowserDetails {
    pub found: Vec<BrowserEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserEntry {
    pub name: String,
    pub path: String,
}

fn find_browser(name: &str) -> Option<String> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let output = Command::new(cmd).arg(name).output().ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            None
        } else {
            Some(path)
        }
    } else {
        None
    }
}

fn check_browser(_ctx: &CheckContext) -> CheckResult {
    let mut found = Vec::new();

    for &candidate in BROWSER_CANDIDATES {
        if let Some(path) = find_browser(candidate) {
            found.push(BrowserEntry {
                name: candidate.to_string(),
                path,
            });
        }
    }

    let (status, message) = if found.is_empty() {
        (CheckStatus::Warn, "no browser found in PATH".to_string())
    } else {
        let names: Vec<&str> = found.iter().map(|b| b.name.as_str()).collect();
        (CheckStatus::Pass, format!("found: {}", names.join(", ")))
    };

    CheckResult {
        name: "browser".into(),
        status,
        message,
        details: Some(serde_json::to_value(&BrowserDetails { found }).unwrap()),
    }
}

inventory::submit! {
    register_check("browser", check_browser)
}
