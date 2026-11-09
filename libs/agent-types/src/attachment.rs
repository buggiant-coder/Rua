use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub url: String,
    pub name: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}
