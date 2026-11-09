use std::path::Path;

use crate::{SyncError, SyncResult};

pub(crate) fn is_untouched_seed_file(data_dir: &Path, relative_path: &str) -> SyncResult<bool> {
    rua_core::seed_manifest::is_file_untouched_seed(data_dir, relative_path)
        .map_err(|e| SyncError::Store(e.to_string()))
}

pub(crate) fn is_untouched_seed_entity(data_dir: &Path, relative_dir: &str) -> SyncResult<bool> {
    rua_core::seed_manifest::is_entity_untouched_seed(data_dir, relative_dir)
        .map_err(|e| SyncError::Store(e.to_string()))
}
