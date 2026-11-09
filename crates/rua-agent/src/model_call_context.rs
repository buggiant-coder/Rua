use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct ModelCallIdentityContext {
    chat_address: Value,
}

impl ModelCallIdentityContext {
    pub fn from_chat_address(address: &rua_core::ChatAddress) -> Self {
        Self {
            chat_address: serde_json::to_value(address).expect("ChatAddress must serialize"),
        }
    }

    pub fn from_metadata(metadata: &std::collections::HashMap<String, Value>) -> Option<Self> {
        Some(Self {
            chat_address: metadata
                .get(crate::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)?
                .clone(),
        })
    }

    pub fn from_json_map(metadata: &serde_json::Map<String, Value>) -> Option<Self> {
        Some(Self {
            chat_address: metadata
                .get(crate::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)?
                .clone(),
        })
    }

    pub fn model_call(&self) -> ModelCallContext {
        ModelCallContext {
            chat_address: self.chat_address.clone(),
            model_call_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelCallContext {
    chat_address: Value,
    model_call_id: String,
}

impl ModelCallContext {
    pub fn apply_to_metadata(&self, metadata: &mut serde_json::Map<String, Value>) {
        metadata.insert(
            crate::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.to_string(),
            self.chat_address.clone(),
        );
        metadata.insert(
            llm_multiplex::debug_capture::MODEL_CALL_ID_METADATA_KEY.to_string(),
            Value::String(self.model_call_id.clone()),
        );
    }
}
