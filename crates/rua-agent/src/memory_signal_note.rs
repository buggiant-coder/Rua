use rua_session::{
    ChatSessionHeader, MemorySignalRuntimeNote, RuntimeNote, RuntimeNotePayload,
    WorkspaceSessionStore,
};

use crate::memory_signal::{detect_memory_signal, MemorySignal, MemorySignalState};

const HEADER_MEMORY_SIGNAL_STATE_KEY: &str = "memory_signal_state";
pub const MEMORY_SIGNAL_MARKER: &str = "memory_signal";

pub fn render_memory_signal_for_model(signal: &MemorySignal) -> String {
    memory_signal_runtime_note("", "", signal).render_for_model()
}

pub fn memory_signal_runtime_note(
    runtime_note_id: impl Into<String>,
    created_at: impl Into<String>,
    signal: &MemorySignal,
) -> RuntimeNote {
    RuntimeNote {
        runtime_note_id: runtime_note_id.into(),
        created_at: created_at.into(),
        payload: RuntimeNotePayload::MemorySignal(MemorySignalRuntimeNote {
            signal_kind: signal.kind.as_str().to_string(),
            confidence: signal.confidence.as_str().to_string(),
            suggested_memory_kind: signal.suggested_memory_kind.clone(),
            suggested_scope: signal.suggested_scope.clone(),
            evidence: signal.evidence.clone(),
        }),
    }
}

fn memory_signal_state_from_chat_header(header: &ChatSessionHeader) -> MemorySignalState {
    header
        .extra
        .get(HEADER_MEMORY_SIGNAL_STATE_KEY)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

fn save_memory_signal_state_to_chat_header(
    header: &mut ChatSessionHeader,
    state: &MemorySignalState,
) {
    header.extra.insert(
        HEADER_MEMORY_SIGNAL_STATE_KEY.to_string(),
        serde_json::to_value(state).unwrap_or_default(),
    );
}

pub async fn detect_memory_signal_for_user_input(
    store: &WorkspaceSessionStore,
    chat_id: &str,
    user_input: &str,
    memory_write_available: bool,
) -> rua_core::Result<Option<MemorySignal>> {
    let mut signal = None;
    store
        .update_chat_header(chat_id, |header| {
            let mut state = memory_signal_state_from_chat_header(header);
            signal = detect_memory_signal(user_input, memory_write_available, &mut state);
            save_memory_signal_state_to_chat_header(header, &state);
        })
        .await?;
    Ok(signal)
}
