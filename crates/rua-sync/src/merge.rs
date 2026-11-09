use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hlc::HlcTimestamp;

pub fn is_text(data: &[u8]) -> bool {
    !data.contains(&0) && std::str::from_utf8(data).is_ok()
}

pub fn field_merge(
    base: &Value,
    local: &Value,
    remote: &Value,
    local_hlc: HlcTimestamp,
    remote_hlc: HlcTimestamp,
) -> Value {
    let base_obj = base.as_object();
    let local_obj = local.as_object();
    let remote_obj = remote.as_object();

    let (Some(base_obj), Some(local_obj), Some(remote_obj)) = (base_obj, local_obj, remote_obj)
    else {
        return if remote_hlc >= local_hlc {
            remote.clone()
        } else {
            local.clone()
        };
    };

    let mut result = serde_json::Map::new();

    let mut all_keys: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
    all_keys.extend(base_obj.keys());
    all_keys.extend(local_obj.keys());
    all_keys.extend(remote_obj.keys());

    for key in all_keys {
        let in_base = base_obj.get(key);
        let in_local = local_obj.get(key);
        let in_remote = remote_obj.get(key);

        match (in_base, in_local, in_remote) {
            (_, Some(lv), Some(rv)) => {
                if lv == rv {
                    result.insert(key.clone(), lv.clone());
                } else {
                    let local_changed = !in_base.is_some_and(|bv| lv == bv);
                    let remote_changed = !in_base.is_some_and(|bv| rv == bv);
                    match (local_changed, remote_changed) {
                        (true, false) => {
                            result.insert(key.clone(), lv.clone());
                        }
                        (false, true) => {
                            result.insert(key.clone(), rv.clone());
                        }
                        _ => {
                            if let (Some(la), Some(ra)) = (lv.as_array(), rv.as_array()) {
                                if can_merge_array_by_id(la) && can_merge_array_by_id(ra) {
                                    result.insert(
                                        key.clone(),
                                        merge_array_by_id(la, ra, local_hlc, remote_hlc),
                                    );
                                    continue;
                                }
                            }
                            if remote_hlc >= local_hlc {
                                result.insert(key.clone(), rv.clone());
                            } else {
                                result.insert(key.clone(), lv.clone());
                            }
                        }
                    }
                }
            }

            (bv, Some(lv), None) => {
                let remote_deleted = bv.is_some();
                if remote_deleted {
                    let local_changed = !bv.is_some_and(|b| lv == b);
                    if local_changed && local_hlc > remote_hlc {
                        result.insert(key.clone(), lv.clone());
                    }
                } else {
                    result.insert(key.clone(), lv.clone());
                }
            }

            (bv, None, Some(rv)) => {
                let local_deleted = bv.is_some();
                if local_deleted {
                    let remote_changed = !bv.is_some_and(|b| rv == b);
                    if remote_changed && remote_hlc > local_hlc {
                        result.insert(key.clone(), rv.clone());
                    }
                } else {
                    result.insert(key.clone(), rv.clone());
                }
            }

            (_, None, None) => {}
        }
    }

    Value::Object(result)
}

fn can_merge_array_by_id(arr: &[Value]) -> bool {
    arr.iter().all(|v| {
        v.as_object()
            .and_then(|o| o.get("id"))
            .and_then(|v| v.as_str())
            .is_some()
    })
}

fn merge_array_by_id(
    local: &[Value],
    remote: &[Value],
    local_hlc: HlcTimestamp,
    remote_hlc: HlcTimestamp,
) -> Value {
    let local_map: HashMap<&str, &Value> = local
        .iter()
        .filter_map(|v| Some((v.as_object()?.get("id")?.as_str()?, v)))
        .collect();

    let mut result = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for rv in remote {
        if let Some(id) = rv
            .as_object()
            .and_then(|o| o.get("id"))
            .and_then(|v| v.as_str())
        {
            seen_ids.insert(id);
            if let Some(&lv) = local_map.get(id) {
                if remote_hlc >= local_hlc {
                    result.push(rv.clone());
                } else {
                    result.push(lv.clone());
                }
            } else {
                result.push(rv.clone());
            }
        }
    }

    for lv in local {
        if let Some(id) = lv
            .as_object()
            .and_then(|o| o.get("id"))
            .and_then(|v| v.as_str())
        {
            if !seen_ids.contains(id) {
                result.push(lv.clone());
            }
        }
    }

    Value::Array(result)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampedRecord {
    pub id: String,
    pub hlc: HlcTimestamp,
    pub data: Value,
}

pub fn record_union(
    local: &[TimestampedRecord],
    remote: &[TimestampedRecord],
) -> Vec<TimestampedRecord> {
    let mut by_id: HashMap<&str, &TimestampedRecord> = HashMap::new();

    for r in local {
        by_id.insert(&r.id, r);
    }
    for r in remote {
        match by_id.get(r.id.as_str()) {
            Some(existing) if existing.hlc >= r.hlc => {}
            _ => {
                by_id.insert(&r.id, r);
            }
        }
    }

    let mut merged: Vec<TimestampedRecord> = by_id.into_values().cloned().collect();
    merged.sort_by_key(|a| a.hlc);
    merged
}

#[derive(Debug, Clone)]
pub struct ThreeWayResult {
    pub merged: String,

    pub had_conflict: bool,

    pub defeated_hunks: Vec<DefeatedHunk>,
}

#[derive(Debug, Clone)]
pub struct DefeatedHunk {
    pub winner: Side,
    pub loser_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

pub fn three_way_merge(
    base: &str,
    local: &str,
    remote: &str,
    local_hlc: HlcTimestamp,
    remote_hlc: HlcTimestamp,
) -> ThreeWayResult {
    if local == remote {
        return ThreeWayResult {
            merged: local.to_string(),
            had_conflict: false,
            defeated_hunks: Vec::new(),
        };
    }
    if local == base {
        return ThreeWayResult {
            merged: remote.to_string(),
            had_conflict: false,
            defeated_hunks: Vec::new(),
        };
    }
    if remote == base {
        return ThreeWayResult {
            merged: local.to_string(),
            had_conflict: false,
            defeated_hunks: Vec::new(),
        };
    }

    let mut opts = diffy::DiffOptions::new();
    opts.set_context_len(0);
    let local_patch = opts.create_patch(base, local);
    let remote_patch = opts.create_patch(base, remote);

    let local_hunks: Vec<HunkRange> = local_patch.hunks().iter().map(hunk_range).collect();
    let remote_hunks: Vec<HunkRange> = remote_patch.hunks().iter().map(hunk_range).collect();

    let base_lines: Vec<&str> = base.lines().collect();
    let local_lines: Vec<&str> = local.lines().collect();
    let remote_lines: Vec<&str> = remote.lines().collect();

    let mut result_lines: Vec<String> = Vec::new();
    let mut had_conflict = false;
    let mut defeated_hunks: Vec<DefeatedHunk> = Vec::new();

    let mut base_pos: usize = 0;

    let mut events = build_merge_events(&local_hunks, &remote_hunks);
    events.sort_by_key(|e| e.base_start);

    for event in &events {
        while base_pos < event.base_start && base_pos < base_lines.len() {
            result_lines.push(base_lines[base_pos].to_string());
            base_pos += 1;
        }

        match event.kind {
            MergeEventKind::LocalOnly(idx) => {
                let h = &local_patch.hunks()[idx];
                for line in h.lines() {
                    match line {
                        diffy::Line::Context(s) | diffy::Line::Insert(s) => {
                            result_lines.push(s.trim_end_matches('\n').to_string());
                        }
                        diffy::Line::Delete(_) => {}
                    }
                }
                base_pos = event.base_end;
            }
            MergeEventKind::RemoteOnly(idx) => {
                let h = &remote_patch.hunks()[idx];
                for line in h.lines() {
                    match line {
                        diffy::Line::Context(s) | diffy::Line::Insert(s) => {
                            result_lines.push(s.trim_end_matches('\n').to_string());
                        }
                        diffy::Line::Delete(_) => {}
                    }
                }
                base_pos = event.base_end;
            }
            MergeEventKind::Conflict(li, ri) => {
                had_conflict = true;
                let local_winner = local_hlc > remote_hlc;
                let (winner_lines, loser_lines) = if local_winner {
                    (&local_lines, &remote_lines)
                } else {
                    (&remote_lines, &local_lines)
                };

                let winner_range = if local_winner {
                    &local_hunks[li]
                } else {
                    &remote_hunks[ri]
                };
                let loser_range = if local_winner {
                    &remote_hunks[ri]
                } else {
                    &local_hunks[li]
                };

                for i in winner_range.new_start..winner_range.new_end {
                    if i < winner_lines.len() {
                        result_lines.push(winner_lines[i].to_string());
                    }
                }

                let mut loser_text = String::new();
                for i in loser_range.new_start..loser_range.new_end {
                    if i < loser_lines.len() {
                        if !loser_text.is_empty() {
                            loser_text.push('\n');
                        }
                        loser_text.push_str(loser_lines[i]);
                    }
                }

                defeated_hunks.push(DefeatedHunk {
                    winner: if local_winner {
                        Side::Local
                    } else {
                        Side::Remote
                    },
                    loser_text,
                });

                base_pos = event.base_end;
            }
        }
    }

    while base_pos < base_lines.len() {
        result_lines.push(base_lines[base_pos].to_string());
        base_pos += 1;
    }

    let mut merged = result_lines.join("\n");

    let winner_text = if remote_hlc >= local_hlc {
        remote
    } else {
        local
    };
    if winner_text.ends_with('\n') && !merged.ends_with('\n') {
        merged.push('\n');
    }

    ThreeWayResult {
        merged,
        had_conflict,
        defeated_hunks,
    }
}

#[derive(Debug, Clone)]
struct HunkRange {
    old_start: usize,

    old_end: usize,

    new_start: usize,

    new_end: usize,
}

fn hunk_range(hunk: &diffy::Hunk<'_, str>) -> HunkRange {
    let r = hunk.old_range();
    let old_start = r.start().saturating_sub(1);
    let old_end = old_start + r.len();
    let rn = hunk.new_range();
    let new_start = rn.start().saturating_sub(1);
    let new_end = new_start + rn.len();
    HunkRange {
        old_start,
        old_end,
        new_start,
        new_end,
    }
}

fn ranges_overlap(a: &HunkRange, b: &HunkRange) -> bool {
    if a.old_start == a.old_end && b.old_start == b.old_end {
        return a.old_start == b.old_start;
    }
    a.old_start < b.old_end && b.old_start < a.old_end
}

#[derive(Debug)]
enum MergeEventKind {
    LocalOnly(usize),
    RemoteOnly(usize),
    Conflict(usize, usize),
}

#[derive(Debug)]
struct MergeEvent {
    base_start: usize,
    base_end: usize,
    kind: MergeEventKind,
}

fn build_merge_events(local_hunks: &[HunkRange], remote_hunks: &[HunkRange]) -> Vec<MergeEvent> {
    let mut events = Vec::new();
    let mut used_remote: Vec<bool> = vec![false; remote_hunks.len()];

    for (li, lh) in local_hunks.iter().enumerate() {
        let mut conflicting = Vec::new();
        for (ri, rh) in remote_hunks.iter().enumerate() {
            if ranges_overlap(lh, rh) {
                conflicting.push(ri);
            }
        }

        if conflicting.is_empty() {
            events.push(MergeEvent {
                base_start: lh.old_start,
                base_end: lh.old_end,
                kind: MergeEventKind::LocalOnly(li),
            });
        } else {
            for &ri in &conflicting {
                if !used_remote[ri] {
                    used_remote[ri] = true;
                    let combined_start = lh.old_start.min(remote_hunks[ri].old_start);
                    let combined_end = lh.old_end.max(remote_hunks[ri].old_end);
                    events.push(MergeEvent {
                        base_start: combined_start,
                        base_end: combined_end,
                        kind: MergeEventKind::Conflict(li, ri),
                    });
                }
            }
        }
    }

    for (ri, rh) in remote_hunks.iter().enumerate() {
        if !used_remote[ri] {
            events.push(MergeEvent {
                base_start: rh.old_start,
                base_end: rh.old_end,
                kind: MergeEventKind::RemoteOnly(ri),
            });
        }
    }

    events
}
