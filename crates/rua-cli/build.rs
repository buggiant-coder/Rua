fn main() {
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
