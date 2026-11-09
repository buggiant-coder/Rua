use sha2::{Digest, Sha256};

fn main() {
    println!(
        "cargo:rustc-env=RUA_DESKTOP_TARGET={}",
        std::env::var("TARGET").expect("Cargo target")
    );
    println!("cargo:rerun-if-changed=automation/manifest.json");
    println!("cargo:rerun-if-changed=binaries/automation/release-set.json");
    let receipt_hash = std::fs::read("binaries/automation/release-set.json")
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .unwrap_or_default();
    println!("cargo:rustc-env=RUA_DESKTOP_SET_SHA256={receipt_hash}");
    tauri_build::build()
}
