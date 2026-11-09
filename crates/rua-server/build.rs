fn main() {
    println!("cargo:rerun-if-changed=../../console/src-tauri/automation/manifest.json");
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read("../../console/src-tauri/automation/manifest.json")
            .expect("automation manifest"),
    )
    .expect("automation manifest JSON");
    for (path, source) in manifest["cua_driver"]["host_sources"]
        .as_object()
        .expect("pinned Cua host sources")
    {
        let path = std::path::Path::new("../../console/src-tauri").join(path);
        println!("cargo:rerun-if-changed={}", path.display());
        use sha2::Digest;
        let actual = format!(
            "{:x}",
            sha2::Sha256::digest(std::fs::read(&path).expect("Cua host source"))
        );
        assert_eq!(
            Some(actual.as_str()),
            source["sha256"].as_str(),
            "Cua host source differs from pinned manifest: {}",
            path.display()
        );
    }

    // LiveKit's static WebRTC archive contains Objective-C categories. Cargo
    // does not propagate a dependency's rustc-link-arg to final executables.
    if std::env::var_os("CARGO_FEATURE_SIP_LIVEKIT").is_some()
        && matches!(
            std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
            Ok("macos" | "ios")
        )
    {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
