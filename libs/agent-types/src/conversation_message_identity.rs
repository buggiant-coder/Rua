pub const MODEL_DIALOGUE_ITEM_ID_METADATA_KEY: &str = "rua_model_dialogue_item_id";

pub const TEXT_MESSAGE_ID_METADATA_KEY: &str = "rua_text_message_id";

pub const REASONING_MESSAGE_ID_METADATA_KEY: &str = "rua_reasoning_message_id";

pub const CANONICAL_MESSAGE_ID_METADATA_KEY: &str = "rua_message_id";

pub fn text_message_id(model_call_id: &str) -> String {
    format!("msg-{model_call_id}-text")
}

pub fn reasoning_message_id(model_call_id: &str) -> String {
    format!("msg-{model_call_id}-reasoning")
}

pub fn tool_use_message_id(tool_call_id: &str) -> String {
    format!("msg-tc-{tool_call_id}")
}

pub fn tool_result_message_id(tool_call_id: &str) -> String {
    format!("msg-tr-{tool_call_id}")
}

pub fn tool_result_attachment_message_id(tool_call_id: &str) -> String {
    format!("msg-tr-{tool_call_id}-attachment")
}
