use chrono::{Duration, Utc};
use rua_server::usage_stats::{spawn_flush_task, UsageStats};
use serde_json::json;

fn read_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn missing_or_corrupt_usage_stats_start_empty() {
    let tmp = tempfile::tempdir().unwrap();

    let missing = UsageStats::load(tmp.path());
    assert!(
        missing.agents_to_preheat(1, 30, "default").await.is_empty(),
        "missing usage_stats.json should be an empty startup state"
    );

    std::fs::write(tmp.path().join("usage_stats.json"), "{not json").unwrap();
    let corrupt = UsageStats::load(tmp.path());

    assert!(
        corrupt.agents_to_preheat(1, 30, "default").await.is_empty(),
        "corrupt usage_stats.json should not poison server startup"
    );
}

#[tokio::test]
async fn bump_and_flush_persist_usage_stats_atomically() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("usage_stats.json");
    let stats = UsageStats::load(tmp.path());

    stats.bump_agent("agent-a").await;
    stats.bump_agent("agent-a").await;
    stats.bump_mcp("agent-a", "memory").await;
    stats.flush_if_dirty().await;
    stats.flush_if_dirty().await;

    let saved = read_json(&path);
    assert_eq!(saved["agents"]["agent-a"]["calls"], 2);
    assert!(saved["agents"]["agent-a"]["last_used"].is_string());
    assert_eq!(saved["mcp"]["agent-a/memory"]["calls"], 1);
    assert!(saved["mcp"]["agent-a/memory"]["last_used"].is_string());

    let temp_siblings: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".usage_stats.json.tmp")
        })
        .collect();
    assert!(
        temp_siblings.is_empty(),
        "atomic usage_stats flush should not leave temp siblings"
    );
}

#[tokio::test]
async fn concurrent_bump_and_flush_do_not_deadlock_or_drop_future_flushes() {
    let tmp = tempfile::tempdir().unwrap();
    let stats = UsageStats::load(tmp.path());

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut tasks = Vec::new();
        for index in 0..20 {
            let stats = stats.clone();
            tasks.push(tokio::spawn(async move {
                stats.bump_agent(&format!("agent-{index}")).await;
                stats.flush_if_dirty().await;
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        stats.flush_if_dirty().await;
    })
    .await;

    assert!(
        result.is_ok(),
        "usage_stats bump/flush lock ordering should not deadlock"
    );

    let saved = read_json(&tmp.path().join("usage_stats.json"));
    assert_eq!(saved["agents"].as_object().unwrap().len(), 20);
}

#[tokio::test]
async fn flush_task_flushes_and_exits_when_shutdown_sender_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let stats = UsageStats::load(tmp.path());
    stats.bump_agent("agent-a").await;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let task = spawn_flush_task(stats, shutdown_rx);

    drop(shutdown_tx);

    tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .expect("closed shutdown channel must stop the flush task")
        .expect("flush task must not panic");
    assert_eq!(
        read_json(&tmp.path().join("usage_stats.json"))["agents"]["agent-a"]["calls"],
        1
    );
}

#[tokio::test]
async fn agents_to_preheat_filters_and_sorts_usage_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let recent = (now - Duration::days(1)).to_rfc3339();
    let old = (now - Duration::days(90)).to_rfc3339();
    std::fs::write(
        tmp.path().join("usage_stats.json"),
        serde_json::to_vec_pretty(&json!({
            "agents": {
                "active": {"calls": 99, "last_used": recent},
                "heavy": {"calls": 10, "last_used": recent},
                "medium": {"calls": 5, "last_used": recent},
                "below-threshold": {"calls": 2, "last_used": recent},
                "stale": {"calls": 50, "last_used": old},
                "never-used": {"calls": 50}
            },
            "mcp": {
                "heavy/aone": {"calls": 99, "last_used": recent}
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let stats = UsageStats::load(tmp.path());

    assert_eq!(
        stats.agents_to_preheat(3, 30, "active").await,
        vec!["heavy".to_string(), "medium".to_string()]
    );
}
