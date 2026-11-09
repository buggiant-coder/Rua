pub fn subagent_agent_id(parent: &str, task_id: &str) -> String {
    format!("{parent}:subtask:{task_id}")
}

pub fn parse_subagent_id(agent_id: &str) -> Option<(&str, &str)> {
    const SEP: &str = ":subtask:";
    let idx = agent_id.rfind(SEP)?;
    let parent = &agent_id[..idx];
    let task_id = &agent_id[idx + SEP.len()..];
    if parent.is_empty() || task_id.is_empty() {
        return None;
    }
    Some((parent, task_id))
}

pub fn is_subagent_id(agent_id: &str) -> bool {
    parse_subagent_id(agent_id).is_some()
}
