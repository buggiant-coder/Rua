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

fn hlc_dev(wall_ms: u64, device: u8) -> HlcTimestamp {
    let mut id = [0u8; 8];
    id[0] = device;
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: id,
    }
}

#[test]
fn disjoint_fields_both_kept() {
    let base = json!({});
    let local = json!({"a": 1});
    let remote = json!({"b": 2});
    let result = field_merge(&base, &local, &remote, hlc(1, 0), hlc(2, 0));
    assert_eq!(result, json!({"a": 1, "b": 2}));
}

#[test]
fn same_field_same_value() {
    let base = json!({"x": 0});
    let local = json!({"x": 1});
    let remote = json!({"x": 1});
    let result = field_merge(&base, &local, &remote, hlc(1, 0), hlc(2, 0));
    assert_eq!(result, json!({"x": 1}));
}

#[test]
fn same_field_different_value_lww_remote_wins() {
    let base = json!({"x": 0});
    let local = json!({"x": 1});
    let remote = json!({"x": 2});
    let result = field_merge(&base, &local, &remote, hlc(1, 0), hlc(2, 0));
    assert_eq!(result, json!({"x": 2}));
}

#[test]
fn same_field_different_value_lww_local_wins() {
    let base = json!({"x": 0});
    let local = json!({"x": 1});
    let remote = json!({"x": 2});
    let result = field_merge(&base, &local, &remote, hlc(3, 0), hlc(2, 0));
    assert_eq!(result, json!({"x": 1}));
}

#[test]
fn only_one_side_changed_field() {
    let base = json!({"x": 0, "y": 0});
    let local = json!({"x": 1, "y": 0});
    let remote = json!({"x": 0, "y": 2});
    let result = field_merge(&base, &local, &remote, hlc(1, 0), hlc(2, 0));
    assert_eq!(result, json!({"x": 1, "y": 2}));
}

#[test]
fn field_deleted_on_one_side_unchanged_other() {
    let base = json!({"x": 1, "y": 2});
    let local = json!({"x": 1});
    let remote = json!({"x": 1, "y": 2});
    let result = field_merge(&base, &local, &remote, hlc(2, 0), hlc(1, 0));

    assert_eq!(result, json!({"x": 1}));
}

#[test]
fn field_deleted_vs_edited_lww() {
    let base = json!({"x": 1});
    let local = json!({});
    let remote = json!({"x": 99});

    let result = field_merge(&base, &local, &remote, hlc(1, 0), hlc(2, 0));
    assert_eq!(result, json!({"x": 99}));
}

#[test]
fn non_object_values_lww() {
    let base = json!(42);
    let local = json!("hello");
    let remote = json!(true);
    let result = field_merge(&base, &local, &remote, hlc(1, 0), hlc(2, 0));
    assert_eq!(result, json!(true));
}

#[test]
fn empty_base_new_fields_from_both() {
    let base = json!({});
    let local = json!({"a": 1, "c": 3});
    let remote = json!({"b": 2, "c": 4});

    let result = field_merge(&base, &local, &remote, hlc(1, 0), hlc(2, 0));
    assert_eq!(result, json!({"a": 1, "b": 2, "c": 4}));
}

#[test]
fn device_id_breaks_tie() {
    let base = json!({"x": 0});
    let local = json!({"x": 1});
    let remote = json!({"x": 2});

    let result = field_merge(&base, &local, &remote, hlc_dev(100, 1), hlc_dev(100, 2));
    assert_eq!(result, json!({"x": 2}));
}
