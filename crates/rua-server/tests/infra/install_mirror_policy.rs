use rua_core::runtime_layout::{HostProfile, RuntimeLayout};
use rua_server::install_mirror::{
    decide_install_region, ip_geo_probe_enabled, locale_indicates_china, region_from_override,
    timezone_indicates_china, InstallMirrorConfig, InstallRegion,
};

#[test]
fn install_region_override_wins() {
    assert_eq!(region_from_override(Some("cn")), Some(InstallRegion::China));
    assert_eq!(
        region_from_override(Some("global")),
        Some(InstallRegion::Global)
    );
    assert_eq!(region_from_override(Some("auto")), None);

    assert_eq!(
        decide_install_region(Some("global"), true, Some("CN")),
        InstallRegion::Global,
        "explicit override must beat local and IP signals"
    );
}

#[test]
fn ip_country_is_authoritative_when_available() {
    assert_eq!(
        decide_install_region(None, false, Some("CN")),
        InstallRegion::China
    );
    assert_eq!(
        decide_install_region(None, true, Some("US")),
        InstallRegion::Global
    );
}

#[test]
fn local_language_and_timezone_are_fallback_china_signals() {
    assert!(locale_indicates_china("zh_CN.UTF-8"));
    assert!(locale_indicates_china("zh-Hans"));
    assert!(timezone_indicates_china("Asia/Shanghai"));

    assert_eq!(
        decide_install_region(None, true, None),
        InstallRegion::China
    );
    assert_eq!(
        decide_install_region(None, false, None),
        InstallRegion::Global
    );
}

#[test]
fn mobile_runtime_skips_ip_geo_probe() {
    let mobile = RuntimeLayout::android(
        "/data/app/com.rua.mobile-test/lib/arm64",
        "/data/data/com.rua.mobile/files",
        "/data/data/com.rua.mobile/files/home",
    );
    let android = RuntimeLayout {
        host_profile: HostProfile::Desktop,
        os_family: "android".to_string(),
        linux_compat: None,
    };
    let desktop = RuntimeLayout {
        host_profile: HostProfile::Desktop,
        os_family: "linux".to_string(),
        linux_compat: None,
    };

    assert!(!ip_geo_probe_enabled(&mobile));
    assert!(!ip_geo_probe_enabled(&android));
    assert!(ip_geo_probe_enabled(&desktop));
}

#[test]
fn china_mirror_config_sets_npm_node_and_python_indexes() {
    let config = InstallMirrorConfig {
        region: InstallRegion::China,
        npm_registry: Some("https://registry.npmmirror.com".to_string()),
        node_dist_base_url: Some("https://mirrors.ustc.edu.cn/node".to_string()),
        pypi_index_url: Some("https://mirrors.ustc.edu.cn/pypi/simple".to_string()),
    };

    assert_eq!(
        config.npm_registry.as_deref(),
        Some("https://registry.npmmirror.com")
    );
    assert_eq!(
        config.pypi_index_url.as_deref(),
        Some("https://mirrors.ustc.edu.cn/pypi/simple")
    );
    assert_eq!(
        config.node_archive_url("24.18.0", "node-v24.18.0-linux-arm64.tar.xz", "default"),
        "https://mirrors.ustc.edu.cn/node/v24.18.0/node-v24.18.0-linux-arm64.tar.xz"
    );
}
