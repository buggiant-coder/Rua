//! Whole installation replacement. Mutable Rua data never enters this transaction.
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

pub struct DirectorySwap {
    installed: PathBuf,
    backup: PathBuf,
    active: bool,
}
impl DirectorySwap {
    pub fn install(installed: &Path, candidate: &Path) -> Result<Self, String> {
        let parent = installed.parent().ok_or("Installation has no parent")?;
        #[cfg(target_os = "macos")]
        let backup = {
            // Atomic exchange leaves a complete App at the installed path even
            // if the process or machine stops between replacement and validation.
            exchange_directories(installed, candidate)?;
            candidate.to_path_buf()
        };
        #[cfg(not(target_os = "macos"))]
        let backup = {
            let backup = parent.join(format!(".rua-previous-{}", uuid::Uuid::new_v4()));
            std::fs::rename(installed, &backup)
                .map_err(|e| format!("Cannot preserve previous installation: {e}"))?;
            if let Err(error) = std::fs::rename(candidate, installed) {
                std::fs::rename(&backup, installed).map_err(|rollback| {
                    format!(
                        "Install failed: {error}; previous installation remains at {}: {rollback}",
                        backup.display()
                    )
                })?;
                sync_directory(parent)?;
                return Err(format!(
                    "Install failed; previous component set restored: {error}"
                ));
            }
            backup
        };
        let mut swap = Self {
            installed: installed.into(),
            backup,
            active: true,
        };
        if let Err(error) = sync_directory(parent) {
            swap.rollback()?;
            return Err(error);
        }
        Ok(swap)
    }
    pub fn rollback(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            exchange_directories(&self.installed, &self.backup)?;
            self.active = false;
            sync_directory(
                self.installed
                    .parent()
                    .ok_or("Installation has no parent")?,
            )?;
            let _ = std::fs::remove_dir_all(&self.backup);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let failed = self
                .installed
                .with_file_name(format!(".rua-failed-{}", uuid::Uuid::new_v4()));
            if self.installed.exists() {
                std::fs::rename(&self.installed, &failed)
                    .map_err(|e| format!("Cannot remove failed installation: {e}"))?;
            }
            if let Err(error) = std::fs::rename(&self.backup, &self.installed) {
                if failed.exists() {
                    let _ = std::fs::rename(&failed, &self.installed);
                }
                return Err(format!(
                    "Restore failed; intact previous installation is at {}: {error}",
                    self.backup.display()
                ));
            }
            self.active = false;
            sync_directory(self.installed.parent().expect("validated parent"))?;
            if failed.exists() {
                let _ = std::fs::remove_dir_all(failed);
            }
        }
        Ok(())
    }
    pub fn commit(mut self) -> Result<(), String> {
        self.active = false;
        if let Err(error) = std::fs::remove_dir_all(&self.backup) {
            eprintln!(
                "[Rua Desktop] Previous installation cleanup failed at {}: {error}",
                self.backup.display()
            );
        }
        Ok(())
    }
}
impl Drop for DirectorySwap {
    fn drop(&mut self) {
        if self.active {
            let _ = self.rollback();
        }
    }
}

#[cfg(target_os = "macos")]
fn exchange_directories(installed: &Path, candidate: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let left =
        std::ffi::CString::new(installed.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let right =
        std::ffi::CString::new(candidate.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // Both paths are private validated directories on the same installation volume.
    if unsafe { libc::renamex_np(left.as_ptr(), right.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(format!(
            "Cannot atomically exchange Desktop installations: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn extract_macos(bytes: &[u8], directory: &Path) -> Result<PathBuf, String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(bytes)));
    let mut root = None;
    let mut count = 0;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if !path.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err("Update archive contains an unsafe path".into());
        }
        let first = path
            .components()
            .next()
            .ok_or("Empty update path")?
            .as_os_str();
        if Path::new(first).extension().is_none_or(|s| s != "app")
            || root
                .as_ref()
                .is_some_and(|r: &PathBuf| r != Path::new(first))
        {
            return Err("Update must contain exactly one complete .app bundle".into());
        }
        root = Some(PathBuf::from(first));
        if entry.header().entry_type().is_symlink() {
            let link = entry
                .link_name()
                .map_err(|e| e.to_string())?
                .ok_or("Empty bundle symlink")?;
            // Framework links may use '..', but must resolve within this app root.
            let mut depth = path.components().count().saturating_sub(1);
            for component in link.components() {
                match component {
                    Component::Normal(_) => depth += 1,
                    Component::CurDir => {}
                    Component::ParentDir if depth > 1 => depth -= 1,
                    _ => return Err("Update symlink escapes its app bundle".into()),
                }
            }
        } else if !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir()) {
            return Err("Update archive contains an unsupported file type".into());
        }
        if !entry.unpack_in(directory).map_err(|e| e.to_string())? {
            return Err("Update entry could not be extracted safely".into());
        }
        let unpacked = directory.join(&path);
        if entry.header().entry_type().is_file() {
            std::fs::File::open(&unpacked)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
        }
        if let Some(parent) = unpacked.parent() {
            sync_directory(parent)?;
        }
        count += 1;
    }
    if count == 0 {
        return Err("Empty update archive".into());
    }
    Ok(directory.join(root.ok_or("Missing app bundle")?))
}

pub fn extract_windows_installer(bytes: &[u8], directory: &Path) -> Result<PathBuf, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut selected = None;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        if file.is_dir() {
            continue;
        }
        let path = file
            .enclosed_name()
            .ok_or("Unsafe installer archive path")?;
        if path.components().count() != 1
            || path.extension().is_none_or(|s| s != "exe")
            || selected.is_some()
        {
            return Err("Windows update must contain exactly one NSIS executable".into());
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content).map_err(|e| e.to_string())?;
        if !content.starts_with(b"MZ") {
            return Err("Invalid Windows installer executable".into());
        }
        let output = directory.join(path);
        crate::preferences::atomic_write(&output, &content)?;
        selected = Some(output);
    }
    selected.ok_or_else(|| "Missing Windows installer".into())
}

#[cfg(target_os = "macos")]
pub struct MacUpdate {
    _directory: tempfile::TempDir,
    candidate: PathBuf,
    installed: PathBuf,
    version: String,
}
#[cfg(target_os = "macos")]
impl MacUpdate {
    pub fn prepare(bytes: &[u8], executable: &Path, version: &str) -> Result<Self, String> {
        let installed = crate::bundle::installed_root(executable)?;
        let directory=tempfile::Builder::new().prefix(".rua-update-").tempdir_in(installed.parent().ok_or("Invalid app path")?)
            .map_err(|e|format!("Cannot stage a complete update beside Rua.app; install into a user-writable location: {e}"))?;
        let candidate = extract_macos(bytes, directory.path())?;
        let signature = std::process::Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&candidate)
            .output()
            .map_err(|e| e.to_string())?;
        if !signature.status.success() {
            return Err(format!(
                "Updated App signature is invalid: {}",
                String::from_utf8_lossy(&signature.stderr)
            ));
        }
        let identity = |app: &Path| -> Result<String, String> {
            let out = std::process::Command::new("/usr/bin/codesign")
                .args(["-dv", "--verbose=4"])
                .arg(app)
                .output()
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stderr);
            let team = text
                .lines()
                .find(|line| line.starts_with("TeamIdentifier="))
                .filter(|line| !line.ends_with("not set"))
                .ok_or("Desktop update requires a signed publisher identity")?;
            Ok(team.into())
        };
        if identity(&installed)? != identity(&candidate)? {
            return Err("Updated App publisher does not match the installed Rua".into());
        }
        crate::bundle::verify_directory(&candidate, version, crate::bundle::TARGET)?;
        crate::bundle::verify_shell(&candidate)?;
        Ok(Self {
            _directory: directory,
            candidate,
            installed,
            version: version.into(),
        })
    }
    pub fn install(self) -> Result<(), String> {
        let mut swap = DirectorySwap::install(&self.installed, &self.candidate)?;
        if let Err(error) =
            crate::bundle::verify_directory(&self.installed, &self.version, crate::bundle::TARGET)
                .and_then(|_| crate::bundle::verify_shell(&self.installed))
        {
            swap.rollback()?;
            return Err(format!(
                "Update validation failed; previous component set restored: {error}"
            ));
        }
        swap.commit()
    }
}

#[cfg(windows)]
pub struct WindowsUpdate {
    directory: tempfile::TempDir,
    plan: PathBuf,
    script: PathBuf,
}
#[cfg(windows)]
impl WindowsUpdate {
    pub fn prepare(
        bytes: &[u8],
        executable: &Path,
        version: &str,
        result: &Path,
    ) -> Result<Self, String> {
        let installed = crate::bundle::installed_root(executable)?;
        let directory = tempfile::Builder::new()
            .prefix("rua-update-")
            .tempdir_in(installed.parent().ok_or("Invalid installation path")?)
            .map_err(|e| e.to_string())?;
        let installer = extract_windows_installer(bytes, directory.path())?;
        let script = directory.path().join("windows-update.ps1");
        crate::preferences::atomic_write(
            &script,
            include_bytes!("../../../scripts/release/windows-update.ps1"),
        )?;
        let plan = directory.path().join("plan.json");
        let value = serde_json::json!({"ready":directory.path().join("ready"),"installed":installed,"desktop":executable,"parent_pid":std::process::id(),"parent_started":crate::owner::capture_process_identity(std::process::id()).ok_or("Cannot capture update owner")?.start_time,"installer":installer,"installer_sha256":crate::bundle::sha256(&std::fs::read(&installer).map_err(|e|e.to_string())?),"version":version,"previous_version":env!("CARGO_PKG_VERSION"),"result":result});
        crate::preferences::atomic_write(
            &plan,
            &serde_json::to_vec(&value).map_err(|e| e.to_string())?,
        )?;
        Ok(Self {
            directory,
            plan,
            script,
        })
    }
    pub fn handoff(self) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        let system_root = std::env::var_os("SystemRoot").ok_or("Windows SystemRoot is missing")?;
        let powershell =
            PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child = std::process::Command::new(powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&self.script)
            .arg("-Plan")
            .arg(&self.plan)
            .creation_flags(0x08000000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start the Desktop update supervisor: {e}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !self.directory.path().join("ready").is_file() {
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("Desktop update supervisor exited before accepting the update".into());
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Desktop update supervisor did not accept the update".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        // The supervisor owns this private directory after the Shell exits.
        let _ = self.directory.keep();
        Ok(())
    }
}
