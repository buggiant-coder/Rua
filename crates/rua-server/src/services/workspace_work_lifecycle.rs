//! The process-composition owner of per-Agent Conversation and Task work.
//!
//! `rua-workspace` decides *when* an Agent stops but cannot enumerate its
//! turns or Task attempts: that state belongs to `rua-server`, which is why
//! this module implements the workspace work-authority seam. Every observation
//! reads live owner state — the Conversation Authority's Active Turn map, the
//! durable Turn Index, and the Task Runtime's Attempt store — never a mirrored
//! counter, and termination is confirmed from the same source that records it.

use std::sync::Arc;

use async_trait::async_trait;
use rua_core::{ChatAddress, RuaRoot};
use rua_workspace::{AgentWork, AgentWorkAuthority, WorkKind, WorkUnit, WorkspaceError};

use crate::services::console_chat::CONSOLE_CHANNEL_ID;
use crate::services::conversation_authority::ConversationAuthority;
use crate::streaming::ActiveStreamRegistry;

/// The process-composition work authority for Agent shutdown.
pub struct WorkspaceWorkAuthority {
    authority: Arc<ConversationAuthority>,
    streams: Arc<ActiveStreamRegistry>,
    task_runtime: Arc<rua_task::TaskRuntime>,
    root: RuaRoot,
}

impl WorkspaceWorkAuthority {
    pub fn new(
        authority: Arc<ConversationAuthority>,
        streams: Arc<ActiveStreamRegistry>,
        task_runtime: Arc<rua_task::TaskRuntime>,
        root: RuaRoot,
    ) -> Self {
        Self {
            authority,
            streams,
            task_runtime,
            root,
        }
    }

    /// Every Chat Address whose work this Agent currently owns.
    ///
    /// This is the union of what the live authority holds (an Active Turn or
    /// queued input needs no durable record to be real) and what the durable
    /// Turn Index remembers, so a Turn that outlived its in-memory entry and a
    /// leftover Task Attempt in an idle Chat are both reachable.
    async fn agent_addresses(&self, agent_id: &str) -> Result<Vec<ChatAddress>, WorkspaceError> {
        let mut addresses: Vec<ChatAddress> =
            self.authority.active_turn_addresses(Some(agent_id)).await;
        addresses.extend(self.authority.queued_input_addresses(agent_id).await);

        let workspace = self.root.chat_workspace();
        if workspace.chat_index_path().try_exists().map_err(|error| {
            WorkspaceError::internal(format!("Cannot inspect the Chat Index: {error}"))
        })? {
            let chats = crate::services::chat_index::list_all(workspace.as_path())
                .await
                .map_err(WorkspaceError::internal)?;
            addresses.extend(
                chats
                    .into_iter()
                    .filter(|chat| {
                        chat.meta
                            .get("agent_id")
                            .and_then(serde_json::Value::as_str)
                            == Some(agent_id)
                    })
                    .map(|chat| ChatAddress::new(chat.id)),
            );
        }

        addresses.sort_by_key(ChatAddress::identity_key);
        addresses.dedup();
        Ok(addresses)
    }

    /// Whether one Turn is genuinely terminal, read from the authority that
    /// records it: a committed terminal fact (the Active Turn is gone) or a
    /// durable non-`in_progress` lifecycle.
    async fn turn_is_terminal(&self, address: &ChatAddress, turn_id: &str) -> bool {
        // The in-memory Active Turn is the same map the turn lifecycle mutates.
        // While it still names this Turn, nothing has been committed yet.
        if let Some(active) = self.authority.active_turn(address).await {
            if active.turn_id == turn_id {
                return false;
            }
        }
        let store = rua_session::WorkspaceSessionStore::new(self.root.chat_workspace().as_path());
        match store
            .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
            .await
        {
            Ok(turns) => {
                turns
                    .into_iter()
                    .find(|turn| turn.turn_id == turn_id)
                    .map(|turn| {
                        rua_session::TurnLifecycleStatus::parse(&turn.lifecycle.status)
                            != rua_session::TurnLifecycleStatus::InProgress
                    })
                    // A Turn that is neither active nor present is not evidence of
                    // termination; it is simply unobservable.
                    .unwrap_or(false)
            }
            Err(error) => {
                tracing::warn!(%address, error = %error, "Chat Turn Index could not be read");
                false
            }
        }
    }

    /// All non-terminal Task Attempts across every owner, with their real owner.
    async fn outstanding_tasks(&self) -> Result<Vec<rua_task::TaskRecord>, WorkspaceError> {
        self.task_runtime
            .outstanding_attempts()
            .await
            .map_err(|error| WorkspaceError::internal(format!("task enumeration failed: {error}")))
    }

    async fn observe(&self, agent_id: &str) -> Result<AgentWork, WorkspaceError> {
        let addresses = self.agent_addresses(agent_id).await?;
        let mut work = AgentWork {
            units: Vec::new(),
            queued_inputs: 0,
        };

        for address in &addresses {
            work.queued_inputs += self.authority.queue_snapshot(address).await.len();
            if let Some(active) = self.authority.active_turn(address).await {
                work.units.push(WorkUnit {
                    attempt_no: None,
                    kind: WorkKind::Turn,
                    chat: address.clone(),
                    id: active.turn_id,
                });
            }
        }

        // A Task keeps its authorizing Agent even if its Chat selects another.
        for record in self.outstanding_tasks().await? {
            let owner = record.spec.owner.chat_address.clone();
            if record.attempts.is_empty() {
                return Err(WorkspaceError::internal(
                    "Outstanding Task has no authorization Attempt",
                ));
            }
            if record
                .attempts
                .last()
                .is_none_or(|attempt| attempt.authorization_agent_id != agent_id)
            {
                continue;
            }
            work.units.push(WorkUnit {
                attempt_no: record.attempts.last().map(|attempt| attempt.attempt_no),
                kind: WorkKind::Task,
                chat: owner,
                id: record.task_id,
            });
        }

        work.units.sort_by(|left, right| {
            left.chat
                .identity_key()
                .cmp(&right.chat.identity_key())
                .then_with(|| left.id.cmp(&right.id))
        });
        work.units.dedup();
        Ok(work)
    }
}

#[async_trait]
impl AgentWorkAuthority for WorkspaceWorkAuthority {
    async fn resume(&self, agent_id: &str) -> Result<(), WorkspaceError> {
        let addresses = self.agent_addresses(agent_id).await?;
        self.authority.resume_agent(agent_id, &addresses).await;
        self.task_runtime.resume_agent_admission(agent_id).await;
        Ok(())
    }

    async fn begin_shutdown(&self, agent_id: &str) -> Result<(), WorkspaceError> {
        self.task_runtime.pause_agent_admission(agent_id).await;
        self.authority.begin_draining(agent_id).await;
        for address in self.agent_addresses(agent_id).await? {
            self.authority.begin_chat_shutdown(&address).await;
        }
        Ok(())
    }

    async fn outstanding_work(&self, agent_id: &str) -> Result<AgentWork, WorkspaceError> {
        self.observe(agent_id).await
    }

    async fn cancel_work(&self, agent_id: &str) -> Result<AgentWork, WorkspaceError> {
        // Close admission before looking at anything. A submission racing this
        // call would otherwise be able to create a Turn after the queue scan and
        // before the cancellation, and the report would then be wrong.
        self.authority.begin_draining(agent_id).await;

        let addresses = self.agent_addresses(agent_id).await?;

        // Queued input goes first. An accepted submission that has not started is
        // the only work that can turn into a *new* Turn, so discarding it before
        // stopping anything is what prevents a Turn from appearing after the
        // Agent was reported stopped.
        for address in &addresses {
            self.authority.discard_unclaimed_inputs(address).await;
        }

        // Task Attempts next: interrupting an Attempt is what lets its Turn reach
        // a terminal state instead of being cancelled out from under it.
        for record in self.outstanding_tasks().await? {
            if record
                .attempts
                .last()
                .is_none_or(|attempt| attempt.authorization_agent_id != agent_id)
            {
                continue;
            }
            self.task_runtime
                .cancel(&record.spec.owner, &record.task_id)
                .await
                .map_err(|error| {
                    WorkspaceError::internal(format!("Task cancellation failed: {error}"))
                })?;
        }

        for address in &addresses {
            if self.authority.active_turn(address).await.is_none() {
                continue;
            }
            self.streams.cancel_address(address).await;
            self.authority
                .request_stop(Some(self.streams.as_ref()), CONSOLE_CHANNEL_ID, address)
                .await;
        }

        // Report what the owner still holds rather than what was requested.
        self.observe(agent_id).await
    }

    async fn terminated_units(&self, units: &[WorkUnit]) -> Result<Vec<WorkUnit>, WorkspaceError> {
        let mut terminated = Vec::new();
        for unit in units {
            let is_terminal = match unit.kind {
                WorkKind::Turn => self.turn_is_terminal(&unit.chat, &unit.id).await,
                WorkKind::Task => {
                    let owner = rua_task::TaskOwner::from_address(unit.chat.clone());
                    match self.task_runtime.get(&owner, &unit.id).await {
                        Ok(record) => record.attempts.iter().any(|attempt| {
                            Some(attempt.attempt_no) == unit.attempt_no
                                && attempt.state.is_terminal()
                                && !self.task_runtime.attempt_execution_active(
                                    &owner,
                                    &unit.id,
                                    attempt.attempt_no,
                                )
                        }),
                        Err(_) => false,
                    }
                }
            };
            if is_terminal {
                terminated.push(unit.clone());
            }
        }
        Ok(terminated)
    }
}
