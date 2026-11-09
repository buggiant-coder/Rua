use std::time::Duration;

use serde::Serialize;

use crate::registry::{register_network_check, CheckContext, CheckResult, CheckStatus};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

pub const SERVICES: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com/v1/models"),
    ("anthropic", "https://api.anthropic.com/v1/messages"),
    (
        "gemini",
        "https://generativelanguage.googleapis.com/v1/models",
    ),
    (
        "dashscope",
        "https://dashscope.aliyuncs.com/api/v1/services/aigc/text-generation/generation",
    ),
    ("dingtalk_webhook", "https://oapi.dingtalk.com/robot/send"),
    ("twilio", "https://api.twilio.com/"),
    ("ollama", "http://localhost:11434/api/tags"),
];

#[derive(Debug, Clone, Serialize)]
pub struct ConnectivityDetails {
    pub services: Vec<ServiceReachability>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceReachability {
    pub name: String,
    pub url: String,
    pub reachable: bool,
    pub status_code: Option<u16>,
    pub error: Option<String>,
}

pub fn is_reachable_status(code: u16) -> bool {
    matches!(code, 200 | 401 | 403)
}

fn check_connectivity(_ctx: &CheckContext) -> CheckResult {
    let mut services = Vec::new();
    let mut all_reachable = true;

    for &(name, url) in SERVICES {
        let (reachable, status_code, error) =
            match crate::http_proxy::blocking_client_for_url(url, PROBE_TIMEOUT) {
                Ok(client) => match client.head(url).send() {
                    Ok(resp) => {
                        let code = resp.status().as_u16();
                        (is_reachable_status(code), Some(code), None)
                    }
                    Err(e) => (false, None, Some(e.to_string())),
                },
                Err(e) => (
                    false,
                    None,
                    Some(format!("failed to build HTTP client: {e}")),
                ),
            };

        if !reachable {
            all_reachable = false;
        }

        services.push(ServiceReachability {
            name: name.to_string(),
            url: url.to_string(),
            reachable,
            status_code,
            error,
        });
    }

    let reachable_count = services.iter().filter(|s| s.reachable).count();
    let status = if all_reachable {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };

    CheckResult {
        name: "connectivity".into(),
        status,
        message: format!("{}/{} services reachable", reachable_count, services.len()),
        details: Some(serde_json::to_value(&ConnectivityDetails { services }).unwrap()),
    }
}

inventory::submit! {
    register_network_check("connectivity", check_connectivity)
}
