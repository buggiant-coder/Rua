use crate::diagnostics::RepairDiagnostic;

#[derive(Debug, thiserror::Error)]
pub enum RepairError {
    #[error("empty input")]
    EmptyInput,

    #[error("input too large: {bytes} bytes (limit: {limit})")]
    InputTooLarge { bytes: usize, limit: usize },

    #[error("output too large: {bytes} bytes (limit: {limit})")]
    OutputTooLarge { bytes: usize, limit: usize },

    #[error("max depth exceeded: {depth} (limit: {limit})")]
    MaxDepthExceeded { depth: usize, limit: usize },

    #[error("unrecoverable: {reason}")]
    Unrecoverable {
        reason: String,
        diagnostics: Vec<RepairDiagnostic>,
    },

    #[error("parse error after repair: {source}")]
    Parse {
        #[from]
        source: serde_json::Error,
    },
}
