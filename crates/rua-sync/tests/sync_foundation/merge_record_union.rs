use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge::{record_union, TimestampedRecord};
use serde_json::json;

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn rec(id: &str, wall_ms: u64, data: serde_json::Value) -> TimestampedRecord {
    TimestampedRecord {
        id: id.to_string(),
        hlc: hlc(wall_ms),
        data,
    }
}

#[test]
fn disjoint_ids_union() {
    let local = vec![rec("a", 1, json!({"v": 1}))];
    let remote = vec![rec("b", 2, json!({"v": 2}))];
    let merged = record_union(&local, &remote);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].id, "a");
    assert_eq!(merged[1].id, "b");
}

#[test]
fn duplicate_id_keeps_later_hlc() {
    let local = vec![rec("x", 1, json!("old"))];
    let remote = vec![rec("x", 5, json!("new"))];
    let merged = record_union(&local, &remote);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].data, json!("new"));
    assert_eq!(merged[0].hlc.wall_ms, 5);
}

#[test]
fn duplicate_id_local_wins_when_later() {
    let local = vec![rec("x", 10, json!("local"))];
    let remote = vec![rec("x", 3, json!("remote"))];
    let merged = record_union(&local, &remote);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].data, json!("local"));
}

#[test]
fn result_sorted_by_hlc() {
    let local = vec![rec("c", 30, json!(3)), rec("a", 10, json!(1))];
    let remote = vec![rec("b", 20, json!(2))];
    let merged = record_union(&local, &remote);
    let ids: Vec<&str> = merged.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[test]
fn empty_inputs() {
    assert!(record_union(&[], &[]).is_empty());
    let local = vec![rec("a", 1, json!(1))];
    let merged = record_union(&local, &[]);
    assert_eq!(merged.len(), 1);
    let merged2 = record_union(&[], &local);
    assert_eq!(merged2.len(), 1);
}

#[test]
fn many_duplicates() {
    let local = vec![
        rec("a", 1, json!("l1")),
        rec("b", 2, json!("l2")),
        rec("c", 3, json!("l3")),
    ];
    let remote = vec![
        rec("a", 5, json!("r1")),
        rec("b", 1, json!("r2")),
        rec("d", 4, json!("r4")),
    ];
    let merged = record_union(&local, &remote);
    assert_eq!(merged.len(), 4);

    let a = merged.iter().find(|r| r.id == "a").unwrap();
    assert_eq!(a.data, json!("r1"));
    let b = merged.iter().find(|r| r.id == "b").unwrap();
    assert_eq!(b.data, json!("l2"));
}
