#[test]
fn chat_prompt_state_writes_publish_stable_conversation_fact() {
    let src = include_str!("../../src/endpoints/chat_prompt.rs");

    assert!(
        src.contains("publish_sync_fact") && src.contains("SyncPublicationFact::StableConversationState"),
        "prompt-state writes must publish StableConversationState through the Replication Publication Seam"
    );
    assert!(
        !src.contains("sync_ops::emit_evergreen_chat_v4_snapshot"),
        "prompt-state writes must not call the legacy endpoint-level Evergreen emit helper"
    );
    for name in [
        "put_prompt_state_core",
        "add_file_core",
        "remove_file_core",
        "reset_files_core",
        "reorder_files_core",
    ] {
        let marker = format!("async fn {name}");
        let body = src
            .split(&marker)
            .nth(1)
            .and_then(|tail| tail.split("\nasync fn ").next())
            .expect("prompt-state mutator should exist");
        assert!(
            body.contains("publish_prompt_state_sync_fact"),
            "{name} must publish a StableConversationState fact after updating prompt-state"
        );
    }
}

#[test]
fn console_chat_message_persistence_publishes_a_model_call_boundary() {
    let src = include_str!("../../src/services/console_chat.rs");
    let save_messages = src
        .split("async fn save_messages_inner")
        .nth(1)
        .and_then(|tail| tail.split("async fn finalize_turn_inner").next())
        .expect("WorkspaceSessionPersistence::save_messages_inner should exist");
    let append_pos = save_messages
        .find("append_output")
        .expect("save_messages should persist Evergreen chat messages");
    let publish_pos = save_messages
        .find("after_turn_write")
        .expect("save_messages should publish after message persistence");

    assert!(
        publish_pos > append_pos,
        "runtime chat message persistence must publish its Model Call Boundary after writing messages"
    );
}

#[test]
fn complete_model_call_publication_advances_watermark_only_after_sync_publication() {
    let src = include_str!("../../src/services/console_chat.rs");
    let save_messages = src
        .split("async fn save_messages_inner")
        .nth(1)
        .and_then(|tail| tail.split("async fn finalize_turn_inner").next())
        .expect("WorkspaceSessionPersistence::save_messages_inner should exist");
    assert!(
        save_messages.contains("ConversationPublicationBoundary::ModelCall"),
        "a complete model-call write must publish even while its Turn remains in_progress"
    );
    assert!(
        !save_messages.contains("is_syncable_conversation_state"),
        "model-call publication must not retain the terminal-only syncability gate"
    );

    let publish_boundary = src
        .split("async fn publish_conversation_boundary")
        .nth(1)
        .and_then(|tail| tail.split("async fn emit_chat_index_upsert_json").next())
        .expect("SessionSyncEmitter::publish_conversation_boundary should exist");
    let publish_pos = publish_boundary
        .find("publish_sync_fact_with_runtime")
        .expect("boundary must publish the replication fact");
    let status_pos = publish_boundary
        .find("PublicationStatus::Published")
        .expect("boundary must require actual publication");
    let watermark_pos = publish_boundary
        .find("watermarks.advance")
        .expect("published boundary must advance its watermark");
    assert!(
        publish_pos < status_pos && status_pos < watermark_pos,
        "watermark advancement must happen only after successful publication"
    );
}
