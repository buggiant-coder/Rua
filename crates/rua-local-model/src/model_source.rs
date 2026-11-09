use std::path::Path;

use async_trait::async_trait;
use rua_core::Result;
use tokio::sync::mpsc;

use crate::download_ipc::IpcMessage;

pub type ProgressTx = mpsc::UnboundedSender<IpcMessage>;

#[async_trait]
pub trait ModelSource: Send + Sync {
    fn name(&self) -> &str;

    async fn fetch_to(&self, dest_path: &Path, progress_tx: ProgressTx) -> Result<()>;
}
