use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use chrono::Utc;

use rua_cron::{run_session_cleanup, ChatInfo, SessionCleanupConfig, SessionCleanupStore};
use tempfile::TempDir;

enum StoreBehavior {
    Ok(Vec<ChatInfo>),
    Err(String),
}

struct MockChatStore {
    behavior: StoreBehavior,
    delete_calls: Mutex<Vec<Vec<String>>>,
}

impl MockChatStore {
    fn with_chats(chats: Vec<ChatInfo>) -> Self {
        Self {
            behavior: StoreBehavior::Ok(chats),
            delete_calls: Mutex::new(Vec::new()),
        }
    }

    fn with_error(msg: impl Into<String>) -> Self {
        Self {
            behavior: StoreBehavior::Err(msg.into()),
            delete_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SessionCleanupStore for MockChatStore {
    async fn list_all_chats(&self) -> Result<Vec<ChatInfo>, String> {
        match &self.behavior {
            StoreBehavior::Ok(v) => Ok(v.clone()),
            StoreBehavior::Err(e) => Err(e.clone()),
        }
    }

    async fn delete_chats(&self, ids: &[String]) -> Result<(), String> {
        self.delete_calls.lock().unwrap().push(ids.to_vec());
        Ok(())
    }
}

fn write_session_file(sessions_dir: &Path, filename: &str, age: Duration) -> PathBuf {
    let path = sessions_dir.join(filename);
    std::fs::write(&path, b"{\"messages\":[]}").unwrap();
    set_modified_age(&path, age);
    path
}

fn write_session_dir(sessions_dir: &Path, dirname: &str, age: Duration) -> PathBuf {
    let path = sessions_dir.join(dirname);
    std::fs::create_dir_all(path.join("messages")).unwrap();
    set_modified_age(&path, age);
    path
}

fn set_modified_age(path: &Path, age: Duration) {
    let new_mtime = SystemTime::now()
        .checked_sub(age)
        .expect("age within range");
    let f = std::fs::File::open(path).expect("open for set_times");
    let times = std::fs::FileTimes::new()
        .set_modified(new_mtime)
        .set_accessed(new_mtime);
    f.set_times(times).expect("set mtime");
}

fn live_chat_info(chat_id: &str) -> ChatInfo {
    ChatInfo {
        id: chat_id.to_string(),
        updated_at: Utc::now(),
    }
}

const SEVEN_DAYS: Duration = Duration::from_secs(7 * 24 * 3600);

#[tokio::test]
async fn case_a_live_chat_old_file_not_deleted() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let sid = "228ae543";
    let old_age = SEVEN_DAYS + Duration::from_secs(3600);
    let path = write_session_file(&sessions_dir, &format!("{sid}.json"), old_age);

    let store = MockChatStore::with_chats(vec![live_chat_info(sid)]);
    let config = SessionCleanupConfig::default();

    let result = run_session_cleanup(&sessions_dir, &config, &store).await;

    assert!(
        path.exists(),
        "live chat's old session file must NOT be deleted"
    );
    assert_eq!(
        result.files_removed, 0,
        "no files should be removed in case A"
    );
    assert!(
        store.delete_calls.lock().unwrap().is_empty(),
        "GC must not mutate chat records (delete_chats must not be called)"
    );
}

#[tokio::test]
async fn case_b_orphan_old_file_is_deleted() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let live_sid = "alive-001";
    let orphan_sid = "deleted-by-user-002";
    let old_age = SEVEN_DAYS + Duration::from_secs(3600);

    let live_path = write_session_file(&sessions_dir, &format!("{live_sid}.json"), old_age);
    let orphan_path = write_session_file(&sessions_dir, &format!("{orphan_sid}.json"), old_age);

    let store = MockChatStore::with_chats(vec![live_chat_info(live_sid)]);
    let config = SessionCleanupConfig::default();

    let result = run_session_cleanup(&sessions_dir, &config, &store).await;

    assert!(live_path.exists(), "live chat file must survive");
    assert!(!orphan_path.exists(), "orphan file must be deleted");
    assert_eq!(result.files_removed, 1);
}

#[tokio::test]
async fn case_b_orphan_old_directory_is_deleted() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let live_sid = "alive-001";
    let orphan_sid = "deleted-by-user-003";
    let old_age = SEVEN_DAYS + Duration::from_secs(3600);

    let live_path = write_session_dir(&sessions_dir, live_sid, old_age);
    let orphan_path = write_session_dir(&sessions_dir, orphan_sid, old_age);

    let store = MockChatStore::with_chats(vec![live_chat_info(live_sid)]);
    let config = SessionCleanupConfig::default();

    let result = run_session_cleanup(&sessions_dir, &config, &store).await;

    assert!(live_path.exists(), "live chat directory must survive");
    assert!(
        !orphan_path.exists(),
        "old orphan directory must be deleted via max_age"
    );
    assert_eq!(result.files_removed, 1);
}

#[tokio::test]
async fn case_c_store_error_skips_deletion() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let old_age = SEVEN_DAYS + Duration::from_secs(3600);
    let p1 = write_session_file(&sessions_dir, "one.json", old_age);
    let p2 = write_session_file(&sessions_dir, "two.json", old_age);

    let store = MockChatStore::with_error("io: broken pipe");
    let config = SessionCleanupConfig::default();

    let result = run_session_cleanup(&sessions_dir, &config, &store).await;

    assert!(
        p1.exists() && p2.exists(),
        "no files may be deleted when store errors"
    );
    assert_eq!(result.files_removed, 0);
    assert!(!result.errors.is_empty(), "error should be recorded");
}

#[tokio::test]
async fn case_c_empty_list_with_files_skips_deletion() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let old_age = SEVEN_DAYS + Duration::from_secs(3600);
    let path = write_session_file(&sessions_dir, "lonely.json", old_age);

    let store = MockChatStore::with_chats(vec![]);
    let config = SessionCleanupConfig::default();

    let result = run_session_cleanup(&sessions_dir, &config, &store).await;

    assert!(
        path.exists(),
        "must not delete when chat list is empty but files exist (fail-safe)"
    );
    assert_eq!(result.files_removed, 0);
}

#[tokio::test]
async fn empty_chats_and_empty_dir_is_noop() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let store = MockChatStore::with_chats(vec![]);
    let config = SessionCleanupConfig::default();

    let result = run_session_cleanup(&sessions_dir, &config, &store).await;

    assert_eq!(result.files_removed, 0);
    assert!(result.errors.is_empty());
}

#[tokio::test]
async fn recent_orphan_file_not_deleted() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let fresh_age = Duration::from_secs(60);
    let path = write_session_file(&sessions_dir, "fresh-orphan.json", fresh_age);

    let store = MockChatStore::with_chats(vec![live_chat_info("some-other-chat")]);
    let config = SessionCleanupConfig::default();

    let result = run_session_cleanup(&sessions_dir, &config, &store).await;

    assert!(path.exists(), "recent files (< max_age) must survive");
    assert_eq!(result.files_removed, 0);
}
