use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge::field_merge;
use serde_json::json;

fn hlc(wall_ms: u64, counter: u32) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter,
        device_id: [0; 8],
    }
}

#[test]
fn both_sides_same_id_remote_wins() {
    let base = json!({
        "models": [{"id": "m1", "name": "original"}]
    });
    let local = json!({
        "models": [{"id": "m1", "name": "local-edit"}]
    });
    let remote = json!({
        "models": [{"id": "m1", "name": "remote-edit"}]
    });

    let merged = field_merge(&base, &local, &remote, hlc(100, 0), hlc(200, 0));
    let models = merged["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["name"], "remote-edit");
}

#[test]
fn both_sides_same_id_local_wins() {
    let base = json!({
        "models": [{"id": "m1", "name": "original"}]
    });
    let local = json!({
        "models": [{"id": "m1", "name": "local-edit"}]
    });
    let remote = json!({
        "models": [{"id": "m1", "name": "remote-edit"}]
    });

    let merged = field_merge(&base, &local, &remote, hlc(200, 0), hlc(100, 0));
    let models = merged["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["name"], "local-edit");
}

#[test]
fn local_only_element_preserved() {
    let base = json!({
        "models": [{"id": "m1", "name": "original"}]
    });

    let local = json!({
        "models": [
            {"id": "m1", "name": "original"},
            {"id": "m2", "name": "local-new"}
        ]
    });

    let remote = json!({
        "models": [{"id": "m1", "name": "remote-renamed"}]
    });

    let merged = field_merge(&base, &local, &remote, hlc(100, 0), hlc(200, 0));
    let models = merged["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);

    let m1 = models.iter().find(|m| m["id"] == "m1").unwrap();
    assert_eq!(m1["name"], "remote-renamed");

    assert!(models
        .iter()
        .any(|m| m["id"] == "m2" && m["name"] == "local-new"));
}

#[test]
fn remote_only_element_added() {
    let base = json!({
        "models": [{"id": "m1", "name": "original"}]
    });

    let local = json!({
        "models": [{"id": "m1", "name": "local-renamed"}]
    });

    let remote = json!({
        "models": [
            {"id": "m1", "name": "original"},
            {"id": "m3", "name": "remote-new"}
        ]
    });

    let merged = field_merge(&base, &local, &remote, hlc(200, 0), hlc(100, 0));
    let models = merged["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);

    let m1 = models.iter().find(|m| m["id"] == "m1").unwrap();
    assert_eq!(m1["name"], "local-renamed");

    assert!(models
        .iter()
        .any(|m| m["id"] == "m3" && m["name"] == "remote-new"));
}

#[test]
fn no_id_falls_back_to_lww() {
    let base = json!({
        "tags": ["a", "b"]
    });
    let local = json!({
        "tags": ["a", "b", "c"]
    });
    let remote = json!({
        "tags": ["a", "b", "d"]
    });

    let merged = field_merge(&base, &local, &remote, hlc(100, 0), hlc(200, 0));

    assert_eq!(merged["tags"], json!(["a", "b", "d"]));
}

#[test]
fn combined_scenario_preserves_all_and_orders_correctly() {
    let base = json!({
        "models": [
            {"id": "m1", "name": "base-m1"},
            {"id": "m2", "name": "base-m2"},
            {"id": "m3", "name": "base-m3"}
        ]
    });
    let local = json!({
        "models": [
            {"id": "m1", "name": "local-m1"},
            {"id": "m2", "name": "base-m2"},
            {"id": "m4", "name": "local-new"}
        ]
    });
    let remote = json!({
        "models": [
            {"id": "m1", "name": "remote-m1"},
            {"id": "m3", "name": "remote-m3"},
            {"id": "m5", "name": "remote-new"}
        ]
    });

    let merged = field_merge(&base, &local, &remote, hlc(100, 0), hlc(200, 0));
    let models = merged["models"].as_array().unwrap();

    assert_eq!(models.len(), 5);

    assert_eq!(
        models.iter().find(|m| m["id"] == "m1").unwrap()["name"],
        "remote-m1"
    );

    assert!(models.iter().any(|m| m["id"] == "m2"));

    assert!(models.iter().any(|m| m["id"] == "m3"));

    assert!(models.iter().any(|m| m["id"] == "m4"));

    assert!(models.iter().any(|m| m["id"] == "m5"));

    let ids: Vec<&str> = models.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["m1", "m3", "m5", "m2", "m4"]);
}
