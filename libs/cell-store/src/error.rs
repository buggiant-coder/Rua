use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CellStoreError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("io error: {0}")]
    IoBare(#[from] std::io::Error),

    #[error("json error at {path}: {source}")]
    Serde {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("json error: {0}")]
    SerdeBare(#[from] serde_json::Error),

    #[error("invalid json path: {0}")]
    InvalidJsonPath(String),

    #[error("json path not found: {path} in {file}", file = file.display())]
    JsonPathNotFound { file: PathBuf, path: String },
}

impl CellStoreError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn serde(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Serde {
            path: path.into(),
            source,
        }
    }
}
