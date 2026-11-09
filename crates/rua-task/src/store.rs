use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::model::{
    TaskAttempt, TaskDependencyResult, TaskDeviceContext, TaskErrorInfo, TaskListFilter, TaskOwner,
    TaskRecord, TaskResult, TaskState,
};
use crate::runtime::TaskError;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tasks (
    task_id TEXT PRIMARY KEY NOT NULL,
    owner_key TEXT NOT NULL,
    runner_kind TEXT NOT NULL,
    spec_json TEXT NOT NULL,
    state TEXT NOT NULL,
    attempt_no INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    result_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_tasks_owner_created
    ON tasks(owner_key, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_owner_state
    ON tasks(owner_key, state);
CREATE TABLE IF NOT EXISTS task_attempts (
    task_id TEXT NOT NULL,
    attempt_no INTEGER NOT NULL,
    state TEXT NOT NULL,
    authorization_agent_id TEXT NOT NULL DEFAULT '',
    configuration_epoch TEXT,
    device_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    result_json TEXT,
    PRIMARY KEY(task_id, attempt_no),
    FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_task_attempts_task
    ON task_attempts(task_id, attempt_no ASC);
CREATE TABLE IF NOT EXISTS task_dependencies (
    task_id TEXT NOT NULL,
    depends_on_task_id TEXT NOT NULL,
    PRIMARY KEY(task_id, depends_on_task_id),
    FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE,
    FOREIGN KEY(depends_on_task_id) REFERENCES tasks(task_id) ON DELETE RESTRICT
);
CREATE INDEX IF NOT EXISTS idx_task_dependencies_upstream
    ON task_dependencies(depends_on_task_id, task_id);
CREATE TABLE IF NOT EXISTS task_terminal_deliveries (
    task_id TEXT NOT NULL,
    attempt_no INTEGER NOT NULL,
    owner_key TEXT NOT NULL,
    queued_at TEXT NOT NULL,
    PRIMARY KEY(task_id, attempt_no),
    FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_task_terminal_deliveries_owner
    ON task_terminal_deliveries(owner_key, queued_at ASC);
CREATE TABLE IF NOT EXISTS task_terminal_delivery_errors (
    task_id TEXT NOT NULL,
    attempt_no INTEGER NOT NULL,
    error TEXT NOT NULL,
    PRIMARY KEY(task_id, attempt_no),
    FOREIGN KEY(task_id, attempt_no) REFERENCES task_terminal_deliveries(task_id, attempt_no) ON DELETE CASCADE
);
"#;

#[derive(Clone)]
pub(crate) struct TaskStore {
    path: PathBuf,
}

impl TaskStore {
    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self, TaskError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(TaskError::io)?;
        }
        let connection = Connection::open(&path).map_err(TaskError::store)?;
        configure_connection(&connection)?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(TaskError::store)?;
        connection.execute_batch(SCHEMA).map_err(TaskError::store)?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn reconcile_non_terminal(
        &self,
        fallback_device: &TaskDeviceContext,
    ) -> Result<Vec<TaskRecord>, TaskError> {
        let mut connection = open_connection(&self.path)?;
        let candidates = {
            let mut statement = connection
                .prepare(
                    "SELECT task.task_id, task.spec_json, attempt.device_json, attempt.attempt_no
                     FROM tasks task
                     JOIN task_attempts attempt
                       ON attempt.task_id = task.task_id
                      AND attempt.attempt_no = task.attempt_no
                     WHERE task.state NOT IN (?1, ?2, ?3, ?4)",
                )
                .map_err(TaskError::store)?;
            let rows = statement
                .query_map(
                    params![
                        state_json(TaskState::Completed)?,
                        state_json(TaskState::Failed)?,
                        state_json(TaskState::Cancelled)?,
                        state_json(TaskState::Interrupted)?,
                    ],
                    |row| {
                        let spec_json = row.get::<_, String>(1)?;
                        let spec: crate::model::TaskSpec = parse_json(&spec_json, 1)?;
                        Ok((
                            row.get::<_, String>(0)?,
                            spec.owner,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)? as u32,
                        ))
                    },
                )
                .map_err(TaskError::store)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(TaskError::store)?
        };
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let tx = write_transaction(&mut connection)?;
        let now = Utc::now();
        for (task_id, owner, device_json, attempt_no) in &candidates {
            let mut device: TaskDeviceContext =
                serde_json::from_str(device_json).map_err(TaskError::serialize)?;
            if device == TaskDeviceContext::default() {
                device = fallback_device.clone();
            }
            let result = TaskResult {
                status: TaskState::Interrupted,
                output: "Outcome\nTask interrupted.\n\nKey Results\nThe previous process ended before this Attempt reached a terminal state.\n\nVerification\nStartup reconciliation found no live execution handle.\n\nRemaining Work\nInspect prior side effects before retrying."
                    .to_string(),
                full_result_ref: None,
                artifacts: Vec::new(),
                device,
                error: Some(TaskErrorInfo {
                    kind: crate::model::TaskErrorKind::Runner,
                    message: "task interrupted by runtime restart".to_string(),
                    retryable: false,
                    details: serde_json::json!({"provenance": "runtime_restart"}),
                }),
            };
            let result_json = serde_json::to_string(&result).map_err(TaskError::serialize)?;
            tx.execute(
                "UPDATE tasks
                 SET state = ?1, result_json = ?2, finished_at = ?3, updated_at = ?3
                 WHERE task_id = ?4",
                params![
                    state_json(TaskState::Interrupted)?,
                    result_json,
                    now.to_rfc3339(),
                    task_id,
                ],
            )
            .map_err(TaskError::store)?;
            tx.execute(
                "UPDATE task_attempts
                 SET state = ?1, result_json = ?2, finished_at = ?3, updated_at = ?3
                 WHERE task_id = ?4
                   AND attempt_no = (SELECT attempt_no FROM tasks WHERE task_id = ?4)",
                params![
                    state_json(TaskState::Interrupted)?,
                    result_json,
                    now.to_rfc3339(),
                    task_id,
                ],
            )
            .map_err(TaskError::store)?;
            queue_terminal_delivery(&tx, task_id, *attempt_no, &owner.identity_key(), now)?;
        }
        tx.commit().map_err(TaskError::store)?;

        let mut records = Vec::new();
        for (task_id, owner, _, _) in candidates {
            records.push(load_record(&connection, &owner, &task_id)?);
        }
        Ok(records)
    }

    pub(crate) async fn insert_with_limits(
        &self,
        mut record: TaskRecord,
        authorization: crate::model::TaskAttemptAuthorization,
        device: TaskDeviceContext,
        max_active: usize,
        max_dependencies: usize,
        max_dependency_depth: usize,
    ) -> Result<TaskRecord, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let tx = write_transaction(&mut connection)?;
            if record.spec.depends_on.len() > max_dependencies {
                return Err(TaskError::LimitExceeded(format!(
                    "Task has {} dependencies; maximum is {max_dependencies}",
                    record.spec.depends_on.len()
                )));
            }
            let active = tx
                .query_row(
                    "SELECT COUNT(*)
                     FROM tasks
                     WHERE owner_key = ?1
                       AND state NOT IN (?2, ?3, ?4, ?5)",
                    params![
                        record.spec.owner.identity_key(),
                        state_json(TaskState::Completed)?,
                        state_json(TaskState::Failed)?,
                        state_json(TaskState::Cancelled)?,
                        state_json(TaskState::Interrupted)?,
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(TaskError::store)? as usize;
            if active >= max_active {
                return Err(TaskError::LimitExceeded(format!(
                    "Chat already has {max_active} active Tasks"
                )));
            }
            record.state = derive_dependency_state(
                &tx,
                &record.spec.owner,
                &record.spec.depends_on,
                max_dependency_depth,
            )?;
            let attempt = TaskAttempt {
                attempt_no: record.attempt_no,
                state: record.state,
                authorization_agent_id: authorization.agent_id,
                configuration_epoch: authorization.configuration_epoch,
                device,
                created_at: record.created_at,
                updated_at: record.updated_at,
                started_at: record.started_at,
                finished_at: record.finished_at,
                result: record.result.clone(),
            };
            insert_record(&tx, &record, &attempt)?;
            record.attempts = vec![attempt];
            for dependency in &record.spec.depends_on {
                tx.execute(
                    "INSERT INTO task_dependencies(task_id, depends_on_task_id)
                     VALUES (?1, ?2)",
                    params![record.task_id, dependency],
                )
                .map_err(TaskError::store)?;
            }
            tx.commit().map_err(TaskError::store)?;
            Ok(record)
        })
        .await
    }

    pub(crate) async fn retry(
        &self,
        owner: TaskOwner,
        task_id: String,
        authorization: crate::model::TaskAttemptAuthorization,
        device: TaskDeviceContext,
        max_active: usize,
        max_attempts: usize,
    ) -> Result<TaskRecord, TaskError> {
        let path = self.path.clone();
        let owner_for_get = owner.clone();
        let task_id_for_get = task_id.clone();
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let tx = write_transaction(&mut connection)?;
            let (state, attempt_no) = tx
                .query_row(
                    "SELECT state, attempt_no
                     FROM tasks
                     WHERE task_id = ?1 AND owner_key = ?2",
                    params![task_id, owner.identity_key()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u32)),
                )
                .optional()
                .map_err(TaskError::store)?
                .ok_or_else(|| TaskError::NotFound("task".to_string()))?;
            let state: TaskState = serde_json::from_str(&state).map_err(TaskError::serialize)?;
            match state {
                TaskState::Failed | TaskState::Cancelled | TaskState::Interrupted => {}
                TaskState::Completed => {
                    return Err(TaskError::InvalidTransition(
                        "completed Tasks cannot be retried".to_string(),
                    ));
                }
                _ => {
                    return Err(TaskError::InvalidTransition(
                        "only failed, cancelled, or interrupted Tasks can be retried".to_string(),
                    ));
                }
            }
            let attempt_count = tx
                .query_row(
                    "SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1",
                    params![task_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(TaskError::store)? as usize;
            if attempt_count >= max_attempts {
                return Err(TaskError::LimitExceeded(format!(
                    "Task already has the maximum {max_attempts} Attempts"
                )));
            }
            let active = tx
                .query_row(
                    "SELECT COUNT(*)
                     FROM tasks
                     WHERE owner_key = ?1
                       AND state NOT IN (?2, ?3, ?4, ?5)",
                    params![
                        owner.identity_key(),
                        state_json(TaskState::Completed)?,
                        state_json(TaskState::Failed)?,
                        state_json(TaskState::Cancelled)?,
                        state_json(TaskState::Interrupted)?,
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(TaskError::store)? as usize;
            if active >= max_active {
                return Err(TaskError::LimitExceeded(format!(
                    "Chat already has {max_active} active Tasks"
                )));
            }

            let now = Utc::now();
            let next_attempt = attempt_no + 1;
            tx.execute(
                "UPDATE tasks
                 SET state = ?1, attempt_no = ?2, updated_at = ?3,
                     started_at = NULL, finished_at = NULL, result_json = NULL
                 WHERE task_id = ?4",
                params![
                    state_json(TaskState::Accepted)?,
                    next_attempt,
                    now.to_rfc3339(),
                    task_id,
                ],
            )
            .map_err(TaskError::store)?;
            insert_attempt(
                &tx,
                &task_id,
                &TaskAttempt {
                    attempt_no: next_attempt,
                    state: TaskState::Accepted,
                    authorization_agent_id: authorization.agent_id,
                    configuration_epoch: authorization.configuration_epoch,
                    device,
                    created_at: now,
                    updated_at: now,
                    started_at: None,
                    finished_at: None,
                    result: None,
                },
            )?;
            tx.commit().map_err(TaskError::store)
        })
        .await?;
        self.get(owner_for_get, task_id_for_get).await
    }

    pub(crate) async fn mark_running(
        &self,
        task_id: String,
        attempt_no: u32,
        started_at: DateTime<Utc>,
    ) -> Result<(), TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let tx = write_transaction(&mut connection)?;
            let changed = tx
                .execute(
                    "UPDATE tasks
                     SET state = ?1, started_at = ?2, updated_at = ?2
                     WHERE task_id = ?3 AND attempt_no = ?4 AND state IN (?5, ?6)",
                    params![
                        state_json(TaskState::Running)?,
                        started_at.to_rfc3339(),
                        task_id,
                        attempt_no,
                        state_json(TaskState::Accepted)?,
                        state_json(TaskState::Waiting)?,
                    ],
                )
                .map_err(TaskError::store)?;
            if changed != 1 {
                return Err(TaskError::InvalidTransition(
                    "task is no longer accepted".to_string(),
                ));
            }
            tx.execute(
                "UPDATE task_attempts
                 SET state = ?1, started_at = ?2, updated_at = ?2
                 WHERE task_id = ?3 AND attempt_no = ?4",
                params![
                    state_json(TaskState::Running)?,
                    started_at.to_rfc3339(),
                    task_id,
                    attempt_no,
                ],
            )
            .map_err(TaskError::store)?;
            tx.commit().map_err(TaskError::store)
        })
        .await
    }

    pub(crate) async fn set_waiting_approval(
        &self,
        task_id: String,
        attempt_no: u32,
        waiting: bool,
    ) -> Result<bool, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            let (before, after) = if waiting {
                (TaskState::Running, TaskState::WaitingApproval)
            } else { (TaskState::WaitingApproval, TaskState::Running) };
            let changed = connection.execute(
                "UPDATE tasks SET state = ?1, updated_at = ?2 WHERE task_id = ?3 AND attempt_no = ?4 AND state = ?5",
                params![state_json(after)?, Utc::now().to_rfc3339(), task_id, attempt_no, state_json(before)?],
            ).map_err(TaskError::store)?;
            Ok(changed == 1)
        }).await
    }

    pub(crate) async fn mark_waiting(
        &self,
        task_id: String,
        attempt_no: u32,
        updated_at: DateTime<Utc>,
    ) -> Result<(), TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let tx = write_transaction(&mut connection)?;
            let changed = tx
                .execute(
                    "UPDATE tasks
                     SET state = ?1, updated_at = ?2
                     WHERE task_id = ?3 AND attempt_no = ?4 AND state = ?5",
                    params![
                        state_json(TaskState::Waiting)?,
                        updated_at.to_rfc3339(),
                        task_id,
                        attempt_no,
                        state_json(TaskState::Accepted)?,
                    ],
                )
                .map_err(TaskError::store)?;
            if changed != 1 {
                return Err(TaskError::InvalidTransition(
                    "task is no longer accepted".to_string(),
                ));
            }
            tx.execute(
                "UPDATE task_attempts
                 SET state = ?1, updated_at = ?2
                 WHERE task_id = ?3 AND attempt_no = ?4",
                params![
                    state_json(TaskState::Waiting)?,
                    updated_at.to_rfc3339(),
                    task_id,
                    attempt_no,
                ],
            )
            .map_err(TaskError::store)?;
            tx.commit().map_err(TaskError::store)
        })
        .await
    }

    pub(crate) async fn finish(
        &self,
        task_id: String,
        attempt_no: u32,
        state: TaskState,
        result: TaskResult,
        finished_at: DateTime<Utc>,
    ) -> Result<TaskRecord, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let tx = write_transaction(&mut connection)?;
            let changed = tx
                .execute(
                    "UPDATE tasks
                     SET state = ?1, result_json = ?2, finished_at = ?3, updated_at = ?3
                     WHERE task_id = ?4 AND attempt_no = ?5
                       AND state NOT IN (?6, ?7, ?8, ?9)",
                    params![
                        state_json(state)?,
                        serde_json::to_string(&result).map_err(TaskError::serialize)?,
                        finished_at.to_rfc3339(),
                        task_id,
                        attempt_no,
                        state_json(TaskState::Completed)?,
                        state_json(TaskState::Failed)?,
                        state_json(TaskState::Cancelled)?,
                        state_json(TaskState::Interrupted)?,
                    ],
                )
                .map_err(TaskError::store)?;
            if changed != 1 {
                return Err(TaskError::InvalidTransition(
                    "task is no longer running".to_string(),
                ));
            }
            tx.execute(
                "UPDATE task_attempts
                 SET state = ?1, result_json = ?2, finished_at = ?3, updated_at = ?3
                 WHERE task_id = ?4 AND attempt_no = ?5",
                params![
                    state_json(state)?,
                    serde_json::to_string(&result).map_err(TaskError::serialize)?,
                    finished_at.to_rfc3339(),
                    task_id,
                    attempt_no,
                ],
            )
            .map_err(TaskError::store)?;
            let owner = tx
                .query_row(
                    "SELECT spec_json FROM tasks WHERE task_id = ?1",
                    params![task_id],
                    |row| {
                        let spec_json = row.get::<_, String>(0)?;
                        let spec: crate::model::TaskSpec = parse_json(&spec_json, 0)?;
                        Ok(spec.owner)
                    },
                )
                .map_err(TaskError::store)?;
            let record = load_record(&tx, &owner, &task_id)?;
            queue_terminal_delivery(
                &tx,
                &task_id,
                attempt_no,
                &owner.identity_key(),
                finished_at,
            )?;
            tx.commit().map_err(TaskError::store)?;
            Ok(record)
        })
        .await
    }

    pub(crate) async fn delete_owner(&self, owner: TaskOwner) -> Result<usize, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let tx = write_transaction(&mut connection)?;
            let deleted = tx
                .execute(
                    "DELETE FROM tasks WHERE owner_key = ?1",
                    params![owner.identity_key()],
                )
                .map_err(TaskError::store)?;
            tx.commit().map_err(TaskError::store)?;
            Ok(deleted)
        })
        .await
    }

    /// Enumerates Task Attempts whose terminal result has been persisted but not yet
    /// acknowledged by the owning source. `owner` restricts the enumeration to one Chat;
    /// `None` returns every outstanding delivery, which is what startup resumption needs.
    pub(crate) async fn list_pending_terminal(
        &self,
        owner: Option<TaskOwner>,
    ) -> Result<Vec<TaskRecord>, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            let queued = list_queued_deliveries(&connection, owner.as_ref())?;
            let mut records = Vec::with_capacity(queued.len());
            for (task_id, attempt_no) in queued {
                records.push(load_record_at_attempt(&connection, &task_id, attempt_no)?);
            }
            Ok(records)
        })
        .await
    }

    /// Clears the outstanding terminal delivery for one exact Task Attempt. Returns `true`
    /// when a delivery was outstanding. The `owner_key` predicate keeps one Chat from
    /// acknowledging another Chat's delivery.
    pub(crate) async fn terminal_delivery_error(
        &self,
        owner: TaskOwner,
        task_id: String,
        attempt_no: u32,
    ) -> Result<Option<String>, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            connection.query_row("SELECT error FROM task_terminal_delivery_errors error JOIN task_terminal_deliveries delivery USING(task_id, attempt_no) WHERE task_id=?1 AND attempt_no=?2 AND owner_key=?3", params![task_id, attempt_no, owner.identity_key()], |row| row.get(0)).optional().map_err(TaskError::store)
        }).await
    }

    pub(crate) async fn fail_terminal_delivery(
        &self,
        owner: TaskOwner,
        task_id: String,
        attempt_no: u32,
        error: String,
    ) -> Result<(), TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            connection.execute("INSERT INTO task_terminal_delivery_errors(task_id, attempt_no, error) SELECT task_id, attempt_no, ?4 FROM task_terminal_deliveries WHERE task_id=?1 AND attempt_no=?2 AND owner_key=?3 ON CONFLICT(task_id, attempt_no) DO UPDATE SET error=excluded.error", params![task_id, attempt_no, owner.identity_key(), error]).map_err(TaskError::store)?;
            Ok(())
        }).await
    }

    pub(crate) async fn ack_terminal_delivery(
        &self,
        owner: TaskOwner,
        task_id: String,
        attempt_no: u32,
    ) -> Result<bool, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let tx = write_transaction(&mut connection)?;
            let cleared = tx
                .execute(
                    "DELETE FROM task_terminal_deliveries
                     WHERE task_id = ?1 AND attempt_no = ?2 AND owner_key = ?3",
                    params![task_id, attempt_no, owner.identity_key()],
                )
                .map_err(TaskError::store)?;
            tx.commit().map_err(TaskError::store)?;
            Ok(cleared == 1)
        })
        .await
    }

    pub(crate) async fn get(
        &self,
        owner: TaskOwner,
        task_id: String,
    ) -> Result<TaskRecord, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut record = {
                let mut statement = connection
                    .prepare(
                        "SELECT task_id, spec_json, state, attempt_no, created_at, updated_at,
                                started_at, finished_at, result_json
                         FROM tasks
                         WHERE task_id = ?1 AND owner_key = ?2",
                    )
                    .map_err(TaskError::store)?;
                statement
                    .query_row(params![task_id, owner.identity_key()], row_to_record)
                    .optional()
                    .map_err(TaskError::store)?
                    .ok_or_else(|| TaskError::NotFound("task".to_string()))?
            };
            hydrate_record(&connection, &mut record)?;
            Ok(record)
        })
        .await
    }

    pub(crate) async fn list(
        &self,
        owner: TaskOwner,
        filter: TaskListFilter,
    ) -> Result<Vec<TaskRecord>, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            let limit = filter.limit.unwrap_or(100).clamp(1, 1000) as i64;
            let offset = i64::try_from(filter.offset)
                .map_err(|_| TaskError::Invalid("Task list offset is too large".into()))?;
            let mut records = Vec::new();
            if let Some(state) = filter.state {
                let mut statement = connection
                    .prepare(
                        "SELECT task_id, spec_json, state, attempt_no, created_at, updated_at,
                                started_at, finished_at, result_json
                         FROM tasks
                         WHERE owner_key = ?1 AND state = ?2
                         ORDER BY created_at DESC, task_id DESC
                         LIMIT ?3 OFFSET ?4",
                    )
                    .map_err(TaskError::store)?;
                let rows = statement
                    .query_map(
                        params![owner.identity_key(), state_json(state)?, limit, offset],
                        row_to_record,
                    )
                    .map_err(TaskError::store)?;
                for row in rows {
                    records.push(row.map_err(TaskError::store)?);
                }
            } else {
                let mut statement = connection
                    .prepare(
                        "SELECT task_id, spec_json, state, attempt_no, created_at, updated_at,
                                started_at, finished_at, result_json
                         FROM tasks
                         WHERE owner_key = ?1
                         ORDER BY created_at DESC, task_id DESC
                         LIMIT ?2 OFFSET ?3",
                    )
                    .map_err(TaskError::store)?;
                let rows = statement
                    .query_map(params![owner.identity_key(), limit, offset], row_to_record)
                    .map_err(TaskError::store)?;
                for row in rows {
                    records.push(row.map_err(TaskError::store)?);
                }
            }
            hydrate_records(&connection, &mut records)?;
            Ok(records)
        })
        .await
    }

    pub(crate) async fn list_non_terminal(
        &self,
        owner: TaskOwner,
    ) -> Result<Vec<TaskRecord>, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT task_id, spec_json, state, attempt_no, created_at, updated_at,
                            started_at, finished_at, result_json
                     FROM tasks
                     WHERE owner_key = ?1
                       AND state NOT IN (?2, ?3, ?4, ?5)
                     ORDER BY created_at ASC",
                )
                .map_err(TaskError::store)?;
            let rows = statement
                .query_map(
                    params![
                        owner.identity_key(),
                        state_json(TaskState::Completed)?,
                        state_json(TaskState::Failed)?,
                        state_json(TaskState::Cancelled)?,
                        state_json(TaskState::Interrupted)?,
                    ],
                    row_to_record,
                )
                .map_err(TaskError::store)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(TaskError::store)?);
            }
            hydrate_records(&connection, &mut records)?;
            Ok(records)
        })
        .await
    }

    pub(crate) async fn list_all_non_terminal(&self) -> Result<Vec<TaskRecord>, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT task_id, spec_json, state, attempt_no, created_at, updated_at,
                            started_at, finished_at, result_json
                     FROM tasks
                     WHERE state NOT IN (?1, ?2, ?3, ?4)
                     ORDER BY created_at ASC",
                )
                .map_err(TaskError::store)?;
            let rows = statement
                .query_map(
                    params![
                        state_json(TaskState::Completed)?,
                        state_json(TaskState::Failed)?,
                        state_json(TaskState::Cancelled)?,
                        state_json(TaskState::Interrupted)?,
                    ],
                    row_to_record,
                )
                .map_err(TaskError::store)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(TaskError::store)?);
            }
            hydrate_records(&connection, &mut records)?;
            Ok(records)
        })
        .await
    }

    pub(crate) async fn dependency_results(
        &self,
        task_id: String,
    ) -> Result<Vec<TaskDependencyResult>, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT upstream.task_id, upstream.attempt_no, upstream.state,
                            upstream.result_json
                     FROM task_dependencies dependency
                     JOIN tasks upstream ON upstream.task_id = dependency.depends_on_task_id
                     WHERE dependency.task_id = ?1
                     ORDER BY upstream.task_id ASC",
                )
                .map_err(TaskError::store)?;
            let rows = statement
                .query_map(params![task_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? as u32,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(TaskError::store)?;
            let mut results = Vec::new();
            for row in rows {
                let (task_id, attempt_no, state, result) = row.map_err(TaskError::store)?;
                let state: TaskState =
                    serde_json::from_str(&state).map_err(TaskError::serialize)?;
                let result: TaskResult = result
                    .as_deref()
                    .ok_or_else(|| {
                        TaskError::InvalidTransition(format!(
                            "dependency Task {task_id} has no result"
                        ))
                    })
                    .and_then(|value| serde_json::from_str(value).map_err(TaskError::serialize))?;
                results.push(TaskDependencyResult {
                    task_id,
                    attempt_no,
                    status: state,
                    output: result.output,
                    full_result_ref: result.full_result_ref,
                    artifacts: result.artifacts,
                    device: result.device,
                });
            }
            Ok(results)
        })
        .await
    }

    pub(crate) async fn refresh_dependents(
        &self,
        upstream_task_id: String,
        max_dependency_depth: usize,
    ) -> Result<Vec<TaskRecord>, TaskError> {
        let path = self.path.clone();
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let tx = write_transaction(&mut connection)?;
            let dependent_ids = {
                let mut statement = tx
                    .prepare(
                        "SELECT task_id FROM task_dependencies
                         WHERE depends_on_task_id = ?1
                         ORDER BY task_id ASC",
                    )
                    .map_err(TaskError::store)?;
                let rows = statement
                    .query_map(params![upstream_task_id], |row| row.get::<_, String>(0))
                    .map_err(TaskError::store)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(TaskError::store)?
            };
            let mut changed = Vec::new();
            let now = Utc::now();
            for dependent_id in dependent_ids {
                let (spec, current_state) = tx
                    .query_row(
                        "SELECT spec_json, state
                         FROM tasks WHERE task_id = ?1",
                        params![dependent_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .map_err(TaskError::store)?;
                let current_state: TaskState =
                    serde_json::from_str(&current_state).map_err(TaskError::serialize)?;
                if !matches!(current_state, TaskState::Waiting | TaskState::Blocked) {
                    continue;
                }
                let spec: crate::model::TaskSpec =
                    serde_json::from_str(&spec).map_err(TaskError::serialize)?;
                let owner = spec.owner.clone();
                let next_state =
                    derive_dependency_state(&tx, &owner, &spec.depends_on, max_dependency_depth)?;
                if next_state == current_state {
                    continue;
                }
                tx.execute(
                    "UPDATE tasks SET state = ?1, updated_at = ?2 WHERE task_id = ?3",
                    params![state_json(next_state)?, now.to_rfc3339(), dependent_id,],
                )
                .map_err(TaskError::store)?;
                tx.execute(
                    "UPDATE task_attempts
                     SET state = ?1, updated_at = ?2
                     WHERE task_id = ?3
                       AND attempt_no = (SELECT attempt_no FROM tasks WHERE task_id = ?3)",
                    params![state_json(next_state)?, now.to_rfc3339(), dependent_id,],
                )
                .map_err(TaskError::store)?;
                changed.push((dependent_id, owner));
            }
            tx.commit().map_err(TaskError::store)?;

            let mut records = Vec::new();
            for (task_id, owner) in changed {
                records.push(load_record(&connection, &owner, &task_id)?);
            }
            Ok(records)
        })
        .await
    }
}

fn open_connection(path: &Path) -> Result<Connection, TaskError> {
    let connection = Connection::open(path).map_err(TaskError::store)?;
    configure_connection(&connection)?;
    Ok(connection)
}

fn configure_connection(connection: &Connection) -> Result<(), TaskError> {
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(TaskError::store)?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(TaskError::store)?;
    Ok(())
}

fn write_transaction(connection: &mut Connection) -> Result<Transaction<'_>, TaskError> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(TaskError::store)
}

fn insert_record(
    tx: &Transaction<'_>,
    record: &TaskRecord,
    attempt: &TaskAttempt,
) -> Result<(), TaskError> {
    tx.execute(
        "INSERT INTO tasks (
             task_id, owner_key, runner_kind, spec_json, state,
             attempt_no, created_at, updated_at, started_at, finished_at, result_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            record.task_id,
            record.spec.owner.identity_key(),
            record.spec.runner.kind().to_string(),
            serde_json::to_string(&record.spec).map_err(TaskError::serialize)?,
            state_json(record.state)?,
            record.attempt_no,
            record.created_at.to_rfc3339(),
            record.updated_at.to_rfc3339(),
            record.started_at.map(|value| value.to_rfc3339()),
            record.finished_at.map(|value| value.to_rfc3339()),
            record
                .result
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(TaskError::serialize)?,
        ],
    )
    .map_err(TaskError::store)?;
    insert_attempt(tx, &record.task_id, attempt)?;
    Ok(())
}

fn insert_attempt(
    tx: &Transaction<'_>,
    task_id: &str,
    attempt: &TaskAttempt,
) -> Result<(), TaskError> {
    tx.execute(
        "INSERT INTO task_attempts (
             task_id, attempt_no, state, authorization_agent_id, device_json, created_at, updated_at,
             started_at, finished_at, result_json, configuration_epoch
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            task_id,
            attempt.attempt_no,
            state_json(attempt.state)?,
            attempt.authorization_agent_id,
            serde_json::to_string(&attempt.device).map_err(TaskError::serialize)?,
            attempt.created_at.to_rfc3339(),
            attempt.updated_at.to_rfc3339(),
            attempt.started_at.map(|value| value.to_rfc3339()),
            attempt.finished_at.map(|value| value.to_rfc3339()),
            attempt
                .result
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(TaskError::serialize)?,
            attempt.configuration_epoch,
        ],
    )
    .map_err(TaskError::store)?;
    Ok(())
}

/// Records an outstanding terminal delivery inside the caller's write transaction, so a
/// persisted terminal result can never become invisible to the owning source. Re-queueing
/// the same Task Attempt is idempotent.
fn queue_terminal_delivery(
    tx: &Transaction<'_>,
    task_id: &str,
    attempt_no: u32,
    owner_key: &str,
    queued_at: DateTime<Utc>,
) -> Result<(), TaskError> {
    tx.execute(
        "INSERT INTO task_terminal_deliveries (task_id, attempt_no, owner_key, queued_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(task_id, attempt_no) DO NOTHING",
        params![task_id, attempt_no, owner_key, queued_at.to_rfc3339()],
    )
    .map_err(TaskError::store)?;
    Ok(())
}

fn derive_dependency_state(
    tx: &Transaction<'_>,
    owner: &TaskOwner,
    dependencies: &[String],
    max_depth: usize,
) -> Result<TaskState, TaskError> {
    let mut unfinished = false;
    let mut blocked = false;
    for dependency in dependencies {
        let state = tx
            .query_row(
                "SELECT state
                 FROM tasks
                 WHERE task_id = ?1 AND owner_key = ?2",
                params![dependency, owner.identity_key()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(TaskError::store)?
            .ok_or_else(|| TaskError::NotFound(format!("dependency Task {dependency}")))?;
        let state: TaskState = serde_json::from_str(&state).map_err(TaskError::serialize)?;
        match state {
            TaskState::Completed => {}
            TaskState::Failed | TaskState::Cancelled | TaskState::Interrupted => blocked = true,
            _ => unfinished = true,
        }

        let depth = tx
            .query_row(
                "WITH RECURSIVE ancestors(task_id, depth) AS (
                     SELECT ?1, 1
                     UNION ALL
                     SELECT dependency.depends_on_task_id, ancestors.depth + 1
                     FROM task_dependencies dependency
                     JOIN ancestors ON dependency.task_id = ancestors.task_id
                     WHERE ancestors.depth <= ?2
                 )
                 SELECT COALESCE(MAX(depth), 0) FROM ancestors",
                params![dependency, max_depth as i64],
                |row| row.get::<_, i64>(0),
            )
            .map_err(TaskError::store)? as usize;
        if depth > max_depth {
            return Err(TaskError::LimitExceeded(format!(
                "dependency graph depth exceeds maximum {max_depth}"
            )));
        }
    }
    Ok(if blocked {
        TaskState::Blocked
    } else if unfinished {
        TaskState::Waiting
    } else {
        TaskState::Accepted
    })
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    let spec_json: String = row.get(1)?;
    let state_json: String = row.get(2)?;
    let result_json: Option<String> = row.get(8)?;
    Ok(TaskRecord {
        task_id: row.get(0)?,
        spec: parse_json(&spec_json, 1)?,
        state: parse_json(&state_json, 2)?,
        attempt_no: row.get::<_, i64>(3)? as u32,
        input_capability: crate::session::TaskInputCapability::None,
        watch_active: false,
        created_at: parse_time(row.get::<_, String>(4)?, 4)?,
        updated_at: parse_time(row.get::<_, String>(5)?, 5)?,
        started_at: row
            .get::<_, Option<String>>(6)?
            .map(|value| parse_time(value, 6))
            .transpose()?,
        finished_at: row
            .get::<_, Option<String>>(7)?
            .map(|value| parse_time(value, 7))
            .transpose()?,
        result: result_json.map(|value| parse_json(&value, 8)).transpose()?,
        attempts: Vec::new(),
        waiting_on: Vec::new(),
        blocked_by: Vec::new(),
    })
}

fn load_record(
    connection: &Connection,
    owner: &TaskOwner,
    task_id: &str,
) -> Result<TaskRecord, TaskError> {
    let mut record = {
        let mut statement = connection
            .prepare(
                "SELECT task_id, spec_json, state, attempt_no, created_at, updated_at,
                        started_at, finished_at, result_json
                 FROM tasks
                 WHERE task_id = ?1 AND owner_key = ?2",
            )
            .map_err(TaskError::store)?;
        statement
            .query_row(params![task_id, owner.identity_key()], row_to_record)
            .optional()
            .map_err(TaskError::store)?
            .ok_or_else(|| TaskError::NotFound("task".to_string()))?
    };
    hydrate_record(connection, &mut record)?;
    Ok(record)
}

/// Projects the `tasks` row for a *specific* Attempt, so a terminal result queued for an
/// earlier Attempt is still reported accurately after `retry` advanced the Task.
fn load_record_at_attempt(
    connection: &Connection,
    task_id: &str,
    attempt_no: u32,
) -> Result<TaskRecord, TaskError> {
    let mut statement = connection
        .prepare(
            "SELECT spec_json
             FROM tasks
             WHERE task_id = ?1",
        )
        .map_err(TaskError::store)?;
    let spec_json = statement
        .query_row(params![task_id], |row| row.get::<_, String>(0))
        .optional()
        .map_err(TaskError::store)?
        .ok_or_else(|| TaskError::NotFound("task".to_string()))?;
    let spec: crate::model::TaskSpec = parse_json(&spec_json, 0).map_err(TaskError::store)?;
    let owner = spec.owner.clone();
    let mut record = load_record(connection, &owner, task_id)?;
    let attempt = record
        .attempts
        .iter()
        .find(|attempt| attempt.attempt_no == attempt_no)
        .ok_or_else(|| TaskError::NotFound(format!("Task Attempt {attempt_no}")))?;
    record.state = attempt.state;
    record.attempt_no = attempt.attempt_no;
    record.updated_at = attempt.updated_at;
    record.started_at = attempt.started_at;
    record.finished_at = attempt.finished_at;
    record.result = attempt.result.clone();
    Ok(record)
}

/// Lists queued `(task_id, attempt_no)` deliveries in FIFO order, optionally scoped to one
/// owning Chat.
fn list_queued_deliveries(
    connection: &Connection,
    owner: Option<&TaskOwner>,
) -> Result<Vec<(String, u32)>, TaskError> {
    let rows = match owner {
        Some(owner) => {
            let mut statement = connection
                .prepare(
                    "SELECT task_id, attempt_no
                     FROM task_terminal_deliveries
                     WHERE owner_key = ?1
                     ORDER BY queued_at ASC, task_id ASC",
                )
                .map_err(TaskError::store)?;
            let rows = statement
                .query_map(params![owner.identity_key()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u32))
                })
                .map_err(TaskError::store)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(TaskError::store)?
        }
        None => {
            let mut statement = connection
                .prepare(
                    "SELECT task_id, attempt_no
                     FROM task_terminal_deliveries
                     ORDER BY queued_at ASC, task_id ASC",
                )
                .map_err(TaskError::store)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u32))
                })
                .map_err(TaskError::store)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(TaskError::store)?
        }
    };
    Ok(rows)
}

fn hydrate_records(connection: &Connection, records: &mut [TaskRecord]) -> Result<(), TaskError> {
    for record in records {
        hydrate_record(connection, record)?;
    }
    Ok(())
}

fn hydrate_record(connection: &Connection, record: &mut TaskRecord) -> Result<(), TaskError> {
    record.attempts = load_attempts(connection, &record.task_id)?;
    let (waiting_on, blocked_by) = load_dependency_state(connection, &record.task_id)?;
    record.waiting_on = waiting_on;
    record.blocked_by = blocked_by;
    Ok(())
}

fn load_dependency_state(
    connection: &Connection,
    task_id: &str,
) -> Result<(Vec<String>, Vec<String>), TaskError> {
    let mut statement = connection
        .prepare(
            "SELECT dependency.depends_on_task_id, upstream.state
             FROM task_dependencies dependency
             JOIN tasks upstream ON upstream.task_id = dependency.depends_on_task_id
             WHERE dependency.task_id = ?1
             ORDER BY dependency.depends_on_task_id ASC",
        )
        .map_err(TaskError::store)?;
    let rows = statement
        .query_map(params![task_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(TaskError::store)?;
    let mut waiting_on = Vec::new();
    let mut blocked_by = Vec::new();
    for row in rows {
        let (dependency, state) = row.map_err(TaskError::store)?;
        let state: TaskState = serde_json::from_str(&state).map_err(TaskError::serialize)?;
        match state {
            TaskState::Completed => {}
            TaskState::Failed | TaskState::Cancelled | TaskState::Interrupted => {
                blocked_by.push(dependency)
            }
            _ => waiting_on.push(dependency),
        }
    }
    Ok((waiting_on, blocked_by))
}

fn load_attempts(connection: &Connection, task_id: &str) -> Result<Vec<TaskAttempt>, TaskError> {
    let mut statement = connection
        .prepare(
            "SELECT attempt_no, state, authorization_agent_id, device_json, created_at, updated_at,
                    started_at, finished_at, result_json, configuration_epoch
             FROM task_attempts
             WHERE task_id = ?1
             ORDER BY attempt_no ASC",
        )
        .map_err(TaskError::store)?;
    let rows = statement
        .query_map(params![task_id], |row| {
            let state: String = row.get(1)?;
            let device: String = row.get(3)?;
            let result: Option<String> = row.get(8)?;
            Ok(TaskAttempt {
                attempt_no: row.get::<_, i64>(0)? as u32,
                state: parse_json(&state, 1)?,
                authorization_agent_id: row.get(2)?,
                configuration_epoch: row.get(9)?,
                device: parse_json(&device, 3)?,
                created_at: parse_time(row.get(4)?, 4)?,
                updated_at: parse_time(row.get(5)?, 5)?,
                started_at: row
                    .get::<_, Option<String>>(6)?
                    .map(|value| parse_time(value, 6))
                    .transpose()?,
                finished_at: row
                    .get::<_, Option<String>>(7)?
                    .map(|value| parse_time(value, 7))
                    .transpose()?,
                result: result.map(|value| parse_json(&value, 8)).transpose()?,
            })
        })
        .map_err(TaskError::store)?;
    let mut attempts = Vec::new();
    for row in rows {
        attempts.push(row.map_err(TaskError::store)?);
    }
    Ok(attempts)
}

fn state_json(state: TaskState) -> Result<String, TaskError> {
    serde_json::to_string(&state).map_err(TaskError::serialize)
}

fn parse_json<T: serde::de::DeserializeOwned>(value: &str, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn parse_time(value: String, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, TaskError> + Send + 'static,
) -> Result<T, TaskError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| TaskError::Store(format!("task store worker failed: {error}")))?
}
