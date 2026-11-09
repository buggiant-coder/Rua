use rua_sync::power_policy::{
    LoadedSyncPowerPolicy, SyncPowerPolicyConfig, SyncPowerPolicyMode, SyncRuntimeHostProfile,
};

#[test]
fn missing_mobile_apk_policy_defaults_to_power_saver() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loaded =
        LoadedSyncPowerPolicy::load(tmp.path(), SyncRuntimeHostProfile::MobileApk).unwrap();

    assert!(loaded.stored_policy.is_none());
    assert_eq!(loaded.host_profile, SyncRuntimeHostProfile::MobileApk);
    assert_eq!(loaded.effective_policy.mode.as_str(), "power_saver");
    assert_eq!(loaded.effective_policy.periodic_interval_secs, 600);
    assert_eq!(loaded.effective_policy.background_flush_grace_secs, 60);
    assert!(!loaded.effective_policy.allow_background_runtime);
    assert!(!loaded.effective_policy.allow_background_scheduled_sync);
    assert!(!tmp.path().join("sync").join("power_policy.json").exists());
}

#[test]
fn desktop_and_headless_defaults_preserve_process_running_sync() {
    for host in [
        SyncRuntimeHostProfile::Desktop,
        SyncRuntimeHostProfile::Headless,
    ] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let loaded = LoadedSyncPowerPolicy::load(tmp.path(), host).unwrap();

        assert!(loaded.stored_policy.is_none());
        assert_eq!(loaded.host_profile, host);
        assert_eq!(loaded.effective_policy.mode, SyncPowerPolicyMode::Realtime);
        assert_eq!(loaded.effective_policy.periodic_interval_secs, 600);
        assert_eq!(loaded.effective_policy.background_flush_grace_secs, 0);
        assert!(loaded.effective_policy.allow_background_runtime);
        assert!(loaded.effective_policy.allow_background_scheduled_sync);
    }
}

#[test]
fn custom_policy_requires_explicit_parameters() {
    let policy = SyncPowerPolicyConfig {
        mode: SyncPowerPolicyMode::Custom,
        periodic_interval_secs: Some(120),
        background_flush_grace_secs: None,
        allow_background_runtime: Some(false),
        allow_background_scheduled_sync: Some(true),
    };

    let err = policy.validate().unwrap_err().to_string();
    assert!(err.contains("background_flush_grace_secs"));
}

#[test]
fn update_persists_raw_policy_and_returns_effective_policy() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let policy = SyncPowerPolicyConfig {
        mode: SyncPowerPolicyMode::Balanced,
        periodic_interval_secs: Some(300),
        background_flush_grace_secs: None,
        allow_background_runtime: None,
        allow_background_scheduled_sync: None,
    };

    let saved = LoadedSyncPowerPolicy::save(
        tmp.path(),
        SyncRuntimeHostProfile::MobileApk,
        policy.clone(),
    )
    .unwrap();
    assert_eq!(saved.stored_policy.as_ref(), Some(&policy));
    assert_eq!(saved.effective_policy.mode, SyncPowerPolicyMode::Balanced);
    assert_eq!(saved.effective_policy.periodic_interval_secs, 300);
    assert_eq!(saved.effective_policy.background_flush_grace_secs, 60);
    assert!(!saved.effective_policy.allow_background_runtime);
    assert!(saved.effective_policy.allow_background_scheduled_sync);

    let loaded = LoadedSyncPowerPolicy::load(tmp.path(), SyncRuntimeHostProfile::MobileApk)
        .expect("reload saved policy");
    assert_eq!(loaded, saved);
}

#[test]
fn saving_policy_creates_only_device_local_power_policy_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    LoadedSyncPowerPolicy::save(
        tmp.path(),
        SyncRuntimeHostProfile::MobileApk,
        SyncPowerPolicyConfig {
            mode: SyncPowerPolicyMode::PowerSaver,
            periodic_interval_secs: None,
            background_flush_grace_secs: None,
            allow_background_runtime: None,
            allow_background_scheduled_sync: None,
        },
    )
    .unwrap();

    let sync_dir = tmp.path().join("sync");
    let mut entries = std::fs::read_dir(&sync_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(entries, vec!["power_policy.json"]);
}
