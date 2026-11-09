use serde::{Deserialize, Serialize};

use rua_config::dirs::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

pub struct CheckContext<'a> {
    pub paths: &'a Paths,
}

pub struct RegisteredCheck {
    pub name: &'static str,
    pub run: fn(&CheckContext) -> CheckResult,
    pub is_network: bool,
}

inventory::collect!(RegisteredCheck);

pub const fn register_check(
    name: &'static str,
    run: fn(&CheckContext) -> CheckResult,
) -> RegisteredCheck {
    RegisteredCheck {
        name,
        run,
        is_network: false,
    }
}

pub const fn register_network_check(
    name: &'static str,
    run: fn(&CheckContext) -> CheckResult,
) -> RegisteredCheck {
    RegisteredCheck {
        name,
        run,
        is_network: true,
    }
}

pub fn run_all_checks(ctx: &CheckContext) -> Vec<CheckResult> {
    inventory::iter::<RegisteredCheck>
        .into_iter()
        .map(|check| (check.run)(ctx))
        .collect()
}

pub fn run_all_checks_filtered(ctx: &CheckContext, deep: bool) -> (Vec<CheckResult>, usize) {
    let mut results = Vec::new();
    let mut skipped = 0;
    for check in inventory::iter::<RegisteredCheck> {
        if !deep && check.is_network {
            skipped += 1;
            continue;
        }
        results.push((check.run)(ctx));
    }
    (results, skipped)
}
