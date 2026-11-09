use rua_core::RuaError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("vecstore error: {0}")]
    VecStore(#[from] sqlite_vecstore::VecStoreError),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("not found: {resource} {id}")]
    NotFound { resource: String, id: String },

    #[error("invalid: {0}")]
    Invalid(String),

    #[error("migration error: {0}")]
    Migration(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("path denied: {0}")]
    PathDenied(String),
}

pub type MemoryResult<T> = std::result::Result<T, MemoryError>;

impl From<MemoryError> for RuaError {
    fn from(e: MemoryError) -> Self {
        match e {
            MemoryError::NotFound { resource, id } => RuaError::NotFound { resource, id },
            MemoryError::PathDenied(msg) => RuaError::Security { message: msg },
            other => RuaError::Memory {
                message: other.to_string(),
            },
        }
    }
}
