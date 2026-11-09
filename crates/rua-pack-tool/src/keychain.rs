use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::process::Command;

pub fn read_privkey_from_keychain(key_id: &str) -> Result<Vec<u8>, String> {
    let service = format!("rua-pack-signing-{key_id}");
    let output = Command::new("security")
        .args(["find-generic-password", "-s", &service, "-w"])
        .output()
        .map_err(|e| format!("cannot run `security`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("keychain lookup failed for '{service}': {stderr}"));
    }

    let b64 = String::from_utf8_lossy(&output.stdout).trim().to_string();
    B64.decode(&b64)
        .map_err(|e| format!("invalid base64 from keychain: {e}"))
}
