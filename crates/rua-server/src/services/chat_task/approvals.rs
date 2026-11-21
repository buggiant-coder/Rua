use super::super::device_command_bus::ConversationApprovalDecision;
use super::*;
use rua_security::approval_authority::{ApprovalDecision, ApprovalRequest};

impl ChatTaskService {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn decide_approval_at_authority<'a>(
        &'a self,
        owner: &'a TaskOwner,
        task_id: &'a str,
        attempt_no: u32,
        approval_id: &'a str,
        decision: ConversationApprovalDecision,
        authenticated_user_id: &'a str,
        authority_epoch: ConversationAuthorityEpoch,
    ) -> futures::future::BoxFuture<'a, Result<ApprovalRequest, String>> {
        Box::pin(async move {
            if authenticated_user_id.trim().is_empty() {
                return Err("A logged-in user must decide the Task approval".into());
            }
            let gate = self.authority.transition_gate(&owner.chat_address);
            let _transition = gate.lock().await;
            let scope = ChatStore::resolve(&self.root, owner.chat_address.clone());
            let placement =
                super::super::conversation_authority_target::load_chat_authority_placement_target(
                    scope.root(),
                    owner.chat_id(),
                )
                .await?;
            if placement.device_id != self.local_device_id
                || placement.authority_epoch != authority_epoch
            {
                return Err("Task approval must be decided by the current Chat Authority".into());
            }
            let device = self.execution_device(owner, task_id).await?;
            let ConversationApprovalDecision::AllowChat {
                decided_by,
                matcher,
            } = decision
            else {
                return Err("Only persistent Chat approval requires the Chat Authority".into());
            };
            if device == self.local_device_id {
                return response_value(
                    self.dispatch(
                        &device,
                        owner,
                        ChatTaskAction::ApprovalDecide {
                            task_id: task_id.into(),
                            attempt_no,
                            approval_id: approval_id.into(),
                            decision: ConversationApprovalDecision::AllowChat {
                                decided_by,
                                matcher,
                            },
                            authenticated_user_id: authenticated_user_id.into(),
                        },
                    )
                    .await?,
                )
                .and_then(|value| {
                    serde_json::from_value(value).map_err(|error| error.to_string())
                });
            }
            let snapshot = self.approval_snapshot(owner, task_id, attempt_no).await?;
            let original = snapshot
                .pending
                .into_iter()
                .chain(snapshot.resolved)
                .find(|request| request.id == approval_id)
                .ok_or("Task approval not found")?;
            let decided = self
                .target
                .security()
                .approval_authority()
                .decide_relayed_for_authenticated_user(
                    &original,
                    ApprovalDecision::AllowChat {
                        decided_by,
                        matcher,
                    },
                    authenticated_user_id,
                )
                .await
                .map_err(|error| error.to_string())?;
            if let Some(runtime) = self.publication.get() {
                let result = crate::sync_publication::publish_sync_fact_with_runtime(
                    runtime,
                    crate::sync_publication::SyncPublicationFact::StableConversationState {
                        scope_root: scope.root().to_owned(),
                        address: owner.chat_address.clone(),
                    },
                )
                .await;
                if !result.errors.is_empty() {
                    tracing::warn!(errors=?result.errors, "Task Chat approval settings synchronization remains pending");
                }
            }
            // The original target card is released only after its canonical Chat
            // permission has committed. The target must not write a second rule.
            let reply = self
                .dispatch(
                    &device,
                    owner,
                    ChatTaskAction::ApprovalApplyAtExecution {
                        task_id: task_id.into(),
                        attempt_no,
                        authority_device_id: self.local_device_id.clone(),
                        authority_epoch,
                        original: Box::new(original),
                        decided: Box::new(decided.clone()),
                    },
                )
                .await;
            match reply.and_then(response_value).and_then(|value| {
                serde_json::from_value::<ApprovalRequest>(value).map_err(|error| error.to_string())
            }) {
                Ok(acknowledged) if acknowledged == decided => Ok(decided),
                Ok(_) => Err("The execution Device acknowledged a different Task approval".into()),
                Err(error) => {
                    // Lost acknowledgement is reconciled by reading the same card;
                    // never repeat the permission mutation or create another card.
                    if self
                        .approval_snapshot(owner, task_id, attempt_no)
                        .await
                        .is_ok_and(|snapshot| {
                            snapshot.resolved.iter().any(|request| request == &decided)
                        })
                    {
                        Ok(decided)
                    } else {
                        Err(format!("Task approval delivery is unconfirmed: {error}"))
                    }
                }
            }
        })
    }
}
