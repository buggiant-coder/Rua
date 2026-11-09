use std::env;
use std::io;
use std::path::Path;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=proto/feishu/lark_ws.proto");
    // This crate also links test executables using LiveKit's Objective-C categories.
    if env::var_os("CARGO_FEATURE_SIP_LIVEKIT").is_some()
        && matches!(
            env::var("CARGO_CFG_TARGET_OS").as_deref(),
            Ok("macos" | "ios")
        )
    {
        println!("cargo:rustc-link-arg=-ObjC");
    }

    if env::consts::OS == "android" {
        return Err(io::Error::other(
            "Cross-compile the Android APK from macOS/Linux with scripts/android/build-android-native-runtime.sh; protobuf generation requires a host protoc executable.",
        ));
    }

    let proto = Path::new("proto/feishu/lark_ws.proto");
    let includes = &[Path::new("proto")];
    prost_build::Config::new()
        .compile_protos(&[proto], includes)
        .map_err(|err| {
            io::Error::other(format!(
                "failed to generate Feishu protobuf bindings: {err}\nInstall protoc with scripts/install-protoc.sh, or set PROTOC to a protoc executable."
            ))
        })
}
