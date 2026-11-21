use super::{
    ApprovalAuthority, ApprovalReuseRule, ApprovalReuseScope, ApprovalStatus,
    AuthenticatedApprovalDecision, PermissionRuleEffect, PermissionToolCall,
};
use crate::ToolCallContext;
use serde_json::Value;

impl ApprovalAuthority {
    /// Return the path from an authenticated local AllowChat or AllowAgent decision whose exact rule is
    /// still in effect. The caller must separately check the current file policy and object.
    pub async fn locally_approved_chat_read(
        &self,
        ctx: &ToolCallContext,
        local_device_id: &str,
    ) -> Result<Option<String>, String> {
        let Some(address) = ctx.approval_owner.as_chat_address() else {
            return Ok(None);
        };
        let call = PermissionToolCall::new(&ctx.tool_name, &ctx.tool_input, Some(local_device_id));
        let Some(args) = call.bound_read.as_deref() else {
            return Ok(None);
        };
        if ctx.agent_id.trim().is_empty()
            || address.chat_id().as_str().trim().is_empty()
            || ctx
                .local_device_id
                .as_deref()
                .is_some_and(|id| id != local_device_id)
            || !single_local_read(args, local_device_id)
        {
            return Ok(None);
        }

        let mut live_rules = self.reuse_rules.write().await;
        let mut current: Vec<_> = match &self.chat_rule_store {
            Some(store) => store.list_chat_rules(address)?,
            None => live_rules
                .values()
                .map(|entry| &entry.rule)
                .filter(|rule| {
                    rule.scope == ApprovalReuseScope::Chat
                        && rule.owner.as_ref() == Some(&ctx.approval_owner)
                })
                .cloned()
                .collect(),
        };
        current.extend(self.current_agent_rules(&ctx.agent_id, &mut live_rules)?);
        let is_deny = |rule: &ApprovalReuseRule| {
            rule.enabled
                && rule.effect == PermissionRuleEffect::Deny
                && rule.pattern().matches(&call)
        };
        if current.iter().any(is_deny) {
            return Ok(None);
        }
        let requests = self.requests.read().await;
        let path = current.iter().find_map(|rule| {
            let entry = live_rules.get(&rule.id)?;
            let local = &entry.rule;
            let request = requests.get(&local.source_approval_id)?;
            let original = PermissionToolCall::new(
                &request.tool_name,
                &request.tool_call.args,
                Some(local_device_id),
            );
            let decision = match rule.scope {
                ApprovalReuseScope::Chat
                    if rule.owner.as_ref() == Some(&ctx.approval_owner)
                        && request.owner == ctx.approval_owner
                        && (rule.agent_id.is_empty() || rule.agent_id == ctx.agent_id) =>
                {
                    AuthenticatedApprovalDecision::AllowChat
                }
                ApprovalReuseScope::Agent
                    if rule.owner.is_none() && rule.agent_id == ctx.agent_id =>
                {
                    AuthenticatedApprovalDecision::AllowAgent
                }
                _ => return None,
            };
            if !entry.locally_decided_read
                || local.agent_id != ctx.agent_id
                || local.scope != rule.scope
                || local.owner != rule.owner
                || !rule.enabled
                || rule.source_approval_id != request.id
                || rule.matcher != local.matcher
                || rule.pattern().approved_file_args.as_ref() != Some(args)
                || original.bound_read.as_deref() != Some(args)
                || request.agent_id != ctx.agent_id
                || !request
                    .reuse_options
                    .iter()
                    .any(|option| option.scope == rule.scope && option.matcher == local.matcher)
                || request.status != ApprovalStatus::Approved
                || request.authenticated_user_decision.as_ref()?.decision != decision
            {
                return None;
            }
            // The exact matcher was selected from this request's options. In particular, an
            // unqualified historical read without a recorded Device cannot acquire one here.
            request.tool_call.args["file_path"]
                .as_str()
                .map(str::to_owned)
        });
        Ok(path)
    }
}

fn single_local_read(args: &Value, local_device_id: &str) -> bool {
    let Some(fields) = args.as_object() else {
        return false;
    };
    let Some(path) = args["file_path"].as_str() else {
        return false;
    };
    args["device_id"].as_str() == Some(local_device_id)
        && !path.starts_with("//")
        && !path.starts_with(r"\\")
        && !path.ends_with(['/', '\\'])
        && !path
            .split(['/', '\\'])
            .any(|part| part == "." || part == "..")
        && !path.chars().any(char::is_control)
        && !(fields.contains_key("byte_offset")
            && (fields.contains_key("start_line") || fields.contains_key("limit")))
        && fields.iter().all(|(key, value)| match key.as_str() {
            "device_id" | "file_path" => true,
            "byte_offset" => value.as_u64().is_some(),
            "start_line" | "limit" => value.as_u64().is_some_and(|n| n > 0),
            _ => false,
        })
}
