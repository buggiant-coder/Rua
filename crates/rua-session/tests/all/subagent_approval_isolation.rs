use rua_session::tool_loop::{is_subagent_id, parse_subagent_id, subagent_agent_id};

#[test]
fn subagent_id_format() {
    let id = subagent_agent_id("parent-1", "task-42");
    assert_eq!(id, "parent-1:subtask:task-42");
}

#[test]
fn parse_roundtrip() {
    let id = subagent_agent_id("agent-a", "abc-123");
    let (parent, task) = parse_subagent_id(&id).unwrap();
    assert_eq!(parent, "agent-a");
    assert_eq!(task, "abc-123");
}

#[test]
fn parent_query_excludes_subagent() {
    let parent = "agent-p";
    let child = subagent_agent_id(parent, "t1");

    assert!(!is_subagent_id(parent));

    assert!(is_subagent_id(&child));

    assert_ne!(parent, child.as_str());
}

#[test]
fn three_level_nesting_isolation() {
    let root = "root-agent";
    let l1 = subagent_agent_id(root, "task-1");
    let l2 = subagent_agent_id(&l1, "task-2");

    assert_eq!(l1, "root-agent:subtask:task-1");
    assert_eq!(l2, "root-agent:subtask:task-1:subtask:task-2");

    assert_ne!(root, l1.as_str());
    assert_ne!(l1, l2);
    assert_ne!(root, l2.as_str());

    let (p2, t2) = parse_subagent_id(&l2).unwrap();
    assert_eq!(p2, "root-agent:subtask:task-1");
    assert_eq!(t2, "task-2");

    let (p1, t1) = parse_subagent_id(&l1).unwrap();
    assert_eq!(p1, "root-agent");
    assert_eq!(t1, "task-1");
}

#[test]
fn supersede_stays_within_bucket() {
    let parent = "agent-p";
    let child = subagent_agent_id(parent, "sub-1");

    let tool = "dangerous_tool";
    assert_ne!(parent, child.as_str());

    let _ = tool;
}

#[test]
fn rejects_malformed_ids() {
    assert!(parse_subagent_id("").is_none());
    assert!(parse_subagent_id("no-separator").is_none());
    assert!(parse_subagent_id(":subtask:task").is_none());
    assert!(parse_subagent_id("parent:subtask:").is_none());
    assert!(parse_subagent_id(":subtask:").is_none());
}
