//! Filesystem durability primitives shared by canonical stores.

use std::io;
use std::path::Path;

/// Flush a directory entry after an atomic rename or removal on Unix.
///
/// `std::fs::File` cannot open directories on Windows. File contents must still
/// be flushed before replacement there; attempting the Unix directory flush
/// reports AccessDenied after the canonical write has already succeeded.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        if path.as_os_str().is_empty() {
            return Ok(());
        }
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}
