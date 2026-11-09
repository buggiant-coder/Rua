use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge::three_way_merge;

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

#[test]
fn identical_local_and_remote() {
    let r = three_way_merge("base", "same", "same", hlc(1), hlc(2));
    assert_eq!(r.merged, "same");
    assert!(!r.had_conflict);
    assert!(r.defeated_hunks.is_empty());
}

#[test]
fn only_local_changed() {
    let r = three_way_merge("base\n", "local\n", "base\n", hlc(1), hlc(2));
    assert_eq!(r.merged, "local\n");
    assert!(!r.had_conflict);
}

#[test]
fn only_remote_changed() {
    let r = three_way_merge("base\n", "base\n", "remote\n", hlc(1), hlc(2));
    assert_eq!(r.merged, "remote\n");
    assert!(!r.had_conflict);
}

#[test]
fn non_overlapping_changes_auto_merge() {
    let base = "line1\nline2\nline3\nline4\nline5\n";
    let local = "LINE1\nline2\nline3\nline4\nline5\n";
    let remote = "line1\nline2\nline3\nline4\nLINE5\n";
    let r = three_way_merge(base, local, remote, hlc(1), hlc(2));
    assert!(!r.had_conflict);
    assert!(r.merged.contains("LINE1"));
    assert!(r.merged.contains("LINE5"));
}

#[test]
fn overlapping_changes_lww_remote_wins() {
    let base = "aaa\nbbb\nccc\n";
    let local = "aaa\nLOCAL\nccc\n";
    let remote = "aaa\nREMOTE\nccc\n";
    let r = three_way_merge(base, local, remote, hlc(1), hlc(2));
    assert!(r.had_conflict);
    assert!(r.merged.contains("REMOTE"));
    assert!(!r.merged.contains("LOCAL"));
    assert_eq!(r.defeated_hunks.len(), 1);
    assert!(r.defeated_hunks[0].loser_text.contains("LOCAL"));
}

#[test]
fn overlapping_changes_lww_local_wins() {
    let base = "aaa\nbbb\nccc\n";
    let local = "aaa\nLOCAL\nccc\n";
    let remote = "aaa\nREMOTE\nccc\n";
    let r = three_way_merge(base, local, remote, hlc(5), hlc(2));
    assert!(r.had_conflict);
    assert!(r.merged.contains("LOCAL"));
    assert!(!r.merged.contains("REMOTE"));
    assert_eq!(r.defeated_hunks.len(), 1);
    assert!(r.defeated_hunks[0].loser_text.contains("REMOTE"));
}

#[test]
fn empty_base_both_add_different_content() {
    let r = three_way_merge("", "hello\n", "world\n", hlc(1), hlc(2));

    assert!(r.had_conflict);
    assert!(r.merged.contains("world"));
}
