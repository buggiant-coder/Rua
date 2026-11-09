use tokio_util::sync::CancellationToken;

use crate::HubError;

pub fn ensure_not_cancelled(token: Option<&CancellationToken>) -> Result<(), HubError> {
    if let Some(t) = token {
        if t.is_cancelled() {
            return Err(HubError::Cancelled);
        }
    }
    Ok(())
}
