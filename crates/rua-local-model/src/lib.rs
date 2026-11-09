pub mod cuda;
pub mod download_ipc;
pub mod download_subprocess;
pub mod manager;
pub mod model_source;
pub mod process;
pub mod robustness;
pub mod server_binary;
pub mod source_custom;
pub mod source_hf;
pub mod source_modelscope;
pub mod source_ollama;

pub use cuda::{
    detect_arch, detect_cuda, detect_os, detect_ram_mb, detect_system_info, get_recommended_models,
    CudaInfo, DetectedArch, DetectedOs, RecommendedModel, SystemInfo,
};
pub use download_ipc::{DownloadStatus, ErrorUpdate, IpcMessage, ProgressUpdate, StatusUpdate};
pub use download_subprocess::DownloadSubprocess;
pub use manager::{
    DownloadProgress, LocalModelInfo, LocalModelManager, ScanResult, ServerStatus,
    ServerUpdateInfo, StartServerRequest,
};
pub use model_source::{ModelSource, ProgressTx};
pub use process::ProcessState;
pub use server_binary::ServerBinaryDownloader;
pub use source_custom::CustomUrlSource;
pub use source_hf::HfSource;
pub use source_modelscope::ModelScopeSource;
pub use source_ollama::OllamaSource;
