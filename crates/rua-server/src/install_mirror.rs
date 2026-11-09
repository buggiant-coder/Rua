use std::env;
use std::time::Duration;

use rua_core::runtime_layout::RuntimeLayout;
use tokio::process::Command;
use tokio::sync::OnceCell;

const REGION_OVERRIDE_ENV: &str = "RUA_INSTALL_REGION";
const CN_NPM_REGISTRY_ENV: &str = "RUA_CN_NPM_REGISTRY";
const CN_NODE_DIST_BASE_ENV: &str = "RUA_CN_NODE_DIST_BASE_URL";
const CN_PYPI_INDEX_ENV: &str = "RUA_CN_PYPI_INDEX_URL";

const DEFAULT_CN_NPM_REGISTRY: &str = "https://registry.npmmirror.com";
const DEFAULT_CN_NODE_DIST_BASE: &str = "https://mirrors.ustc.edu.cn/node";
const DEFAULT_CN_PYPI_INDEX: &str = "https://mirrors.ustc.edu.cn/pypi/simple";
const IP_COUNTRY_URL: &str = "https://ifconfig.co/country-iso";
const IP_GEO_TIMEOUT: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallRegion {
    China,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallMirrorConfig {
    pub region: InstallRegion,
    pub npm_registry: Option<String>,
    pub node_dist_base_url: Option<String>,
    pub pypi_index_url: Option<String>,
}

impl InstallMirrorConfig {
    pub fn for_region(region: InstallRegion) -> Self {
        match region {
            InstallRegion::China => Self {
                region,
                npm_registry: Some(
                    env::var(CN_NPM_REGISTRY_ENV)
                        .unwrap_or_else(|_| DEFAULT_CN_NPM_REGISTRY.to_string()),
                ),
                node_dist_base_url: Some(
                    env::var(CN_NODE_DIST_BASE_ENV)
                        .unwrap_or_else(|_| DEFAULT_CN_NODE_DIST_BASE.to_string()),
                ),
                pypi_index_url: Some(
                    env::var(CN_PYPI_INDEX_ENV)
                        .unwrap_or_else(|_| DEFAULT_CN_PYPI_INDEX.to_string()),
                ),
            },
            InstallRegion::Global => Self {
                region,
                npm_registry: None,
                node_dist_base_url: None,
                pypi_index_url: None,
            },
        }
    }

    pub fn apply_to_command(&self, cmd: &mut Command) {
        let Some(npm_registry) = self.npm_registry.as_deref() else {
            return;
        };
        cmd.env("npm_config_registry", npm_registry)
            .env("NPM_CONFIG_REGISTRY", npm_registry);

        if let Some(node_dist_base) = self.node_dist_base_url.as_deref() {
            cmd.env("npm_config_disturl", node_dist_base)
                .env("NPM_CONFIG_DISTURL", node_dist_base);
        }
        if let Some(pypi_index) = self.pypi_index_url.as_deref() {
            cmd.env("PIP_INDEX_URL", pypi_index)
                .env("UV_DEFAULT_INDEX", pypi_index)
                .env("UV_INDEX_URL", pypi_index);
        }
    }

    pub fn apply_to_env_args(&self, env_args: &mut Vec<String>) {
        let Some(npm_registry) = self.npm_registry.as_deref() else {
            return;
        };
        env_args.push(format!("npm_config_registry={npm_registry}"));
        env_args.push(format!("NPM_CONFIG_REGISTRY={npm_registry}"));

        if let Some(node_dist_base) = self.node_dist_base_url.as_deref() {
            env_args.push(format!("npm_config_disturl={node_dist_base}"));
            env_args.push(format!("NPM_CONFIG_DISTURL={node_dist_base}"));
        }
        if let Some(pypi_index) = self.pypi_index_url.as_deref() {
            env_args.push(format!("PIP_INDEX_URL={pypi_index}"));
            env_args.push(format!("UV_DEFAULT_INDEX={pypi_index}"));
            env_args.push(format!("UV_INDEX_URL={pypi_index}"));
        }
    }

    pub fn node_archive_url(&self, version: &str, archive: &str, default_url: &str) -> String {
        match self.node_dist_base_url.as_deref() {
            Some(base) => {
                let base = base.trim_end_matches('/');
                format!("{base}/v{version}/{archive}")
            }
            None => default_url.to_string(),
        }
    }
}

static DETECTED_IP_COUNTRY: OnceCell<Option<String>> = OnceCell::const_new();

pub async fn detect_install_region() -> InstallRegion {
    detect_install_region_with_locale_values(&[]).await
}

pub async fn detect_install_region_with_locale_values(locale_values: &[String]) -> InstallRegion {
    if let Some(region) = region_from_override(env::var(REGION_OVERRIDE_ENV).ok().as_deref()) {
        return region;
    }

    let local_signals = local_signals_indicate_china()
        || locale_values
            .iter()
            .any(|value| locale_indicates_china(value));
    let ip_country = if current_ip_geo_probe_enabled() {
        DETECTED_IP_COUNTRY
            .get_or_init(|| async { detect_ip_country_iso().await })
            .await
            .clone()
    } else {
        None
    };
    decide_install_region(None, local_signals, ip_country.as_deref())
}

pub async fn detect_install_mirror_config() -> InstallMirrorConfig {
    InstallMirrorConfig::for_region(detect_install_region().await)
}

pub async fn detect_install_mirror_config_with_locale_values(
    locale_values: &[String],
) -> InstallMirrorConfig {
    InstallMirrorConfig::for_region(detect_install_region_with_locale_values(locale_values).await)
}

pub fn decide_install_region(
    override_value: Option<&str>,
    local_signals_indicate_china: bool,
    ip_country_iso: Option<&str>,
) -> InstallRegion {
    if let Some(region) = region_from_override(override_value) {
        return region;
    }
    if let Some(country) = ip_country_iso {
        let country = country.trim();
        if !country.is_empty() {
            return if country.eq_ignore_ascii_case("CN") {
                InstallRegion::China
            } else {
                InstallRegion::Global
            };
        }
    }
    if local_signals_indicate_china {
        InstallRegion::China
    } else {
        InstallRegion::Global
    }
}

pub fn region_from_override(value: Option<&str>) -> Option<InstallRegion> {
    let value = value?.trim().to_ascii_lowercase();
    match value.as_str() {
        "cn" | "china" | "mainland" | "true" | "1" => Some(InstallRegion::China),
        "global" | "intl" | "international" | "false" | "0" => Some(InstallRegion::Global),
        "auto" | "" => None,
        _ => None,
    }
}

pub fn local_signals_indicate_china() -> bool {
    let locale_hit = ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"]
        .into_iter()
        .filter_map(|key| env::var(key).ok())
        .any(|value| locale_indicates_china(&value));

    let timezone = env::var("TZ")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| iana_time_zone::get_timezone().ok());

    locale_hit || timezone.as_deref().is_some_and(timezone_indicates_china)
}

pub fn locale_indicates_china(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("zh_cn")
        || value.contains("zh-cn")
        || value.contains("zh_hans")
        || value.contains("zh-hans")
}

pub fn timezone_indicates_china(value: &str) -> bool {
    matches!(
        value.trim(),
        "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Harbin" | "Asia/Urumqi"
    )
}

pub fn ip_geo_probe_enabled(runtime_layout: &RuntimeLayout) -> bool {
    !runtime_layout.host_profile.is_mobile() && runtime_layout.os_family != "android"
}

fn current_ip_geo_probe_enabled() -> bool {
    ip_geo_probe_enabled(&RuntimeLayout::from_current_process(None))
}

async fn detect_ip_country_iso() -> Option<String> {
    if !current_ip_geo_probe_enabled() {
        return None;
    }
    let client = rua_http::build_client(
        reqwest::Client::builder()
            .timeout(IP_GEO_TIMEOUT)
            .user_agent("rua-install-region"),
    )
    .ok()?;
    let response = client.get(IP_COUNTRY_URL).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let text = response.text().await.ok()?;
    let country = text.trim().to_ascii_uppercase();
    if country.len() == 2 {
        Some(country)
    } else {
        None
    }
}
