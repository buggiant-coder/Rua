//! A Desktop release is one Shell, resident and pinned driver set.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const TARGET: &str = env!("RUA_DESKTOP_TARGET");
const MANIFEST: &[u8] = include_bytes!("../automation/manifest.json");

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Preserve the exact PE image while allowing Authenticode to change its
/// checksum, certificate directory and appended certificate/alignment bytes.
/// Signature trust is checked separately by the Windows installer supervisor.
pub fn pe_image_sha256(bytes: &[u8]) -> Result<String, String> {
    let u32_at = |offset: usize| -> Result<usize, String> {
        let value = bytes.get(offset..offset + 4).ok_or("Truncated PE header")?;
        Ok(u32::from_le_bytes(value.try_into().expect("four bytes")) as usize)
    };
    if bytes.get(..2) != Some(b"MZ") {
        return Err("Missing PE DOS header".into());
    }
    let header = u32_at(0x3c)?;
    if bytes.get(header..header + 4) != Some(b"PE\0\0") {
        return Err("Missing PE header".into());
    }
    let optional = header + 24;
    let directory = match bytes.get(optional..optional + 2) {
        Some([0x0b, 0x01]) => optional + 128,
        Some([0x0b, 0x02]) => optional + 144,
        _ => return Err("Unsupported PE optional header".into()),
    };
    let certificate = u32_at(directory)?;
    let size = u32_at(directory + 4)?;
    let end = if certificate == 0 && size == 0 {
        bytes.len()
    } else if certificate >= directory + 8 && certificate.checked_add(size) == Some(bytes.len()) {
        certificate
    } else {
        return Err("Invalid PE certificate boundary".into());
    };
    if end < directory + 8 {
        return Err("Truncated PE image".into());
    }
    let mut image = bytes[..end].to_vec();
    image[optional + 64..optional + 68].fill(0);
    image[directory..directory + 8].fill(0);
    let length = image
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |i| i + 1);
    Ok(sha256(&image[..length]))
}

pub fn installed_root(executable: &Path) -> Result<PathBuf, String> {
    let directory = executable
        .parent()
        .ok_or("Desktop executable has no directory")?;
    #[cfg(target_os = "macos")]
    {
        if directory.file_name().is_none_or(|n| n != "MacOS") {
            return Err("Update requires an installed Rua.app bundle".into());
        }
        let root = directory
            .parent()
            .and_then(Path::parent)
            .ok_or("Invalid app bundle")?;
        if root.extension().is_none_or(|s| s != "app") {
            return Err("Invalid app bundle".into());
        }
        Ok(root.to_path_buf())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(directory.to_path_buf())
    }
}

pub fn binary_directory(root: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        root.join("Contents/MacOS")
    } else {
        root.to_path_buf()
    }
}
pub fn resource_directory(root: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        root.join("Contents/Resources")
    } else if cfg!(target_os = "linux") {
        root.parent().unwrap_or(root).join("lib/Rua")
    } else {
        root.to_path_buf()
    }
}

pub fn validate_receipt(
    receipt: &[u8],
    manifest: &[u8],
    version: &str,
    target: &str,
) -> Result<Value, String> {
    let value: Value =
        serde_json::from_slice(receipt).map_err(|e| format!("Invalid release receipt: {e}"))?;
    let source: Value = serde_json::from_slice(manifest).map_err(|e| e.to_string())?;
    if value["format"] != 1
        || value["target"] != target
        || value["product_version"] != version
        || value["manifest_sha256"] != sha256(manifest)
    {
        return Err(
            "Desktop release receipt does not match the product version, target or pinned manifest"
                .into(),
        );
    }
    for (name, expected) in [
        ("rua", version),
        (
            "agent-browser",
            source["agent_browser"]["version"]
                .as_str()
                .ok_or("Missing browser version")?,
        ),
        (
            "cua-driver",
            source["cua_driver"]["version"]
                .as_str()
                .ok_or("Missing Cua version")?,
        ),
    ] {
        let component = &value["components"][name];
        if component["version"] != expected
            || (target.ends_with("windows-msvc")
                && component["image_sha256"]
                    .as_str()
                    .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit())))
            || component["sha256"]
                .as_str()
                .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(format!(
                "Missing or mismatched {name} in Desktop release receipt"
            ));
        }
    }
    Ok(value)
}

pub fn verify_directory(root: &Path, version: &str, target: &str) -> Result<Value, String> {
    let resources = resource_directory(root);
    let automation = resources.join("automation");
    let manifest = std::fs::read(automation.join("manifest.json"))
        .map_err(|e| format!("Missing automation manifest: {e}"))?;
    let receipt = validate_receipt(
        &std::fs::read(automation.join("release-set.json")).map_err(|e| e.to_string())?,
        &manifest,
        version,
        target,
    )?;
    for name in [
        "agent-browser-LICENSE",
        "cua-driver-LICENSE",
        "NOTICE.md",
        "agent-browser-rua.patch",
    ] {
        if !automation.join(name).is_file() {
            return Err(format!("Missing bundled automation resource: {name}"));
        }
    }
    if !resources.join("console/index.html").is_file() {
        return Err("Missing bundled Console".into());
    }
    let manifest: Value = serde_json::from_slice(&manifest).map_err(|e| e.to_string())?;
    let binaries = binary_directory(root);
    for name in ["rua", "agent-browser", "cua-driver"] {
        let binary = binaries.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        });
        if !binary.is_file() {
            return Err(format!("Missing bundled component: {name}"));
        }
        #[cfg(windows)]
        if pe_image_sha256(&std::fs::read(&binary).map_err(|e| e.to_string())?)?
            != receipt["components"][name]["image_sha256"]
                .as_str()
                .unwrap_or("")
        {
            return Err(format!("Bundled {name} image does not match this release"));
        }
        // Cua's pinned macOS artifact requires 13+. Its signed sealed file is
        // still checked above; an older OS can continue using Browser Use.
        if name == "cua-driver" && !rua_core::desktop_automation::computer_os_supported() {
            continue;
        }
        let output = probe(&binary, "--version", 5)?;
        let expected = receipt["components"][name]["version"]
            .as_str()
            .ok_or("Missing component version")?;
        if !output
            .split_whitespace()
            .any(|word| word.trim_start_matches('v') == expected)
        {
            return Err(format!(
                "Running {name} version does not match {expected}: {output}"
            ));
        }
        if name == "agent-browser"
            && probe(&binary, "--rua-automation-contract", 5)?.trim()
                != manifest["agent_browser"]["adapter"]["contract"]
                    .as_str()
                    .unwrap_or("")
        {
            return Err("Bundled Browser Use adapter contract mismatch".into());
        }
    }
    Ok(receipt)
}

fn probe(binary: &Path, argument: &str, seconds: u64) -> Result<String, String> {
    // Tiny version responses are bounded by process lifetime. Never start a browser/daemon.
    let mut child = std::process::Command::new(binary)
        .arg(argument)
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .env("CUA_DRIVER_RS_UPDATE_CHECK", "false")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Cannot run bundled {}: {e}", binary.display()))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if !status.success() => {
                return Err(format!("Bundled {} {argument} failed", binary.display()))
            }
            Some(_) => break,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Bundled {} version probe timed out",
                    binary.display()
                ));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

pub fn verify_shell(root: &Path) -> Result<(), String> {
    let executable = binary_directory(root).join(if cfg!(windows) {
        "rua-desktop.exe"
    } else {
        "rua-desktop"
    });
    // The candidate Shell validates its own compiled manifest and release receipt.
    probe(&executable, "--verify-desktop-bundle", 30)?;
    Ok(())
}

pub fn verify_current(executable: &Path) -> Result<Value, String> {
    let root = installed_root(executable)?;
    let manifest = std::fs::read(resource_directory(&root).join("automation/manifest.json"))
        .map_err(|e| e.to_string())?;
    if manifest != MANIFEST {
        return Err("Automation manifest belongs to a different Rua build".into());
    }
    let receipt = std::fs::read(resource_directory(&root).join("automation/release-set.json"))
        .map_err(|e| e.to_string())?;
    if sha256(&receipt) != env!("RUA_DESKTOP_SET_SHA256") {
        return Err("Desktop component receipt belongs to a different Shell build".into());
    }
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&root)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("Desktop bundle signature or sealed component integrity is invalid".into());
        }
    }
    verify_directory(&root, env!("CARGO_PKG_VERSION"), TARGET)
}
