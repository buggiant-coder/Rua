use std::path::PathBuf;
use std::time::Duration;

use rua_command::{is_acp_reserved_command, AcpChatSessionManager, ACP_RESERVED_COMMANDS};

#[tokio::test]
async fn register_then_get_returns_chat() {
    let mgr = AcpChatSessionManager::new(Duration::from_secs(60));
    mgr.register("chat-1".into(), "codex".into(), PathBuf::from("/tmp"))
        .await;

    let chat = mgr.get("chat-1").await.expect("chat should be present");
    assert_eq!(chat.chat_id, "chat-1");
    assert_eq!(chat.runner, "codex");
    assert_eq!(chat.cwd, PathBuf::from("/tmp"));
    assert!(mgr.contains_chat("chat-1").await);
    assert_eq!(mgr.active_count().await, 1);
}

#[tokio::test]
async fn unregister_removes_chat() {
    let mgr = AcpChatSessionManager::new(Duration::from_secs(60));
    mgr.register("chat-1".into(), "codex".into(), PathBuf::from("/tmp"))
        .await;
    mgr.unregister("chat-1").await;
    assert!(mgr.get("chat-1").await.is_none());
    assert!(!mgr.contains_chat("chat-1").await);
}

#[tokio::test]
async fn touch_updates_last_active() {
    let mgr = AcpChatSessionManager::new(Duration::from_millis(200));
    mgr.register("chat-1".into(), "codex".into(), PathBuf::from("/tmp"))
        .await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    mgr.touch("chat-1").await;
    tokio::time::sleep(Duration::from_millis(120)).await;

    assert!(mgr.contains_chat("chat-1").await);
}

#[tokio::test]
async fn chat_remains_active_after_idle_timeout_until_evicted() {
    let mgr = AcpChatSessionManager::new(Duration::from_millis(50));
    mgr.register("chat-1".into(), "codex".into(), PathBuf::from("/tmp"))
        .await;

    tokio::time::sleep(Duration::from_millis(80)).await;

    assert!(mgr.contains_chat("chat-1").await);
    assert!(mgr.get("chat-1").await.is_some());
    assert_eq!(mgr.active_count().await, 1);
}

#[tokio::test]
async fn evict_idle_drops_expired_sessions() {
    let mgr = AcpChatSessionManager::new(Duration::from_millis(50));
    mgr.register("chat-1".into(), "codex".into(), PathBuf::from("/tmp"))
        .await;
    mgr.register("chat-2".into(), "opencode".into(), PathBuf::from("/tmp"))
        .await;

    tokio::time::sleep(Duration::from_millis(80)).await;
    mgr.evict_idle().await;

    assert!(mgr.get("chat-1").await.is_none());
    assert!(mgr.get("chat-2").await.is_none());
    assert_eq!(mgr.active_count().await, 0);
}

#[tokio::test]
async fn take_resume_hint_fires_once_then_false() {
    let mgr = AcpChatSessionManager::new(Duration::from_secs(60));
    mgr.register_resumed("chat-1".into(), "codex".into(), PathBuf::from("/tmp"))
        .await;
    assert!(mgr.take_resume_hint("chat-1").await);
    assert!(!mgr.take_resume_hint("chat-1").await);
}

#[tokio::test]
async fn take_resume_hint_false_for_plain_register() {
    let mgr = AcpChatSessionManager::new(Duration::from_secs(60));
    mgr.register("chat-1".into(), "codex".into(), PathBuf::from("/tmp"))
        .await;
    assert!(!mgr.take_resume_hint("chat-1").await);
}

#[test]
fn reserved_command_allowlist_matches_expected_set() {
    assert_eq!(ACP_RESERVED_COMMANDS, &["stop"]);
    for cmd in ACP_RESERVED_COMMANDS {
        assert!(is_acp_reserved_command(cmd), "{cmd} should be reserved");
    }
    for cmd in [
        "acp", "noacp", "acph", "acpr", "help", "model", "new", "clear", "compact", "mode", "modes",
    ] {
        assert!(!is_acp_reserved_command(cmd), "{cmd} should be piped");
    }
}

#[tokio::test]
async fn take_resume_hint_missing_chat_returns_false() {
    let mgr = AcpChatSessionManager::new(Duration::from_secs(60));
    assert!(!mgr.take_resume_hint("nope").await);
}

#[tokio::test]
async fn different_runners_share_manager_but_are_keyed_by_chat() {
    let mgr = AcpChatSessionManager::new(Duration::from_secs(60));
    mgr.register("chat-a".into(), "codex".into(), PathBuf::from("/proj-a"))
        .await;
    mgr.register("chat-b".into(), "claude".into(), PathBuf::from("/proj-b"))
        .await;

    assert_eq!(mgr.get("chat-a").await.unwrap().runner, "codex");
    assert_eq!(mgr.get("chat-b").await.unwrap().runner, "claude");
    assert_eq!(mgr.active_count().await, 2);
}
