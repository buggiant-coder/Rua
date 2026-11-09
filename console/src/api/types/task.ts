export type TaskState =
  | "accepted"
  | "waiting"
  | "blocked"
  | "running"
  | "waiting_approval"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted";

export type TaskInputCapability = "none" | "stdin" | "steering_message";

export type TaskRunnerSpec = {
  type: "tool";
  tool_name: string;
  arguments: Record<string, unknown>;
};

export type LegacyTaskRunnerSpec =
  | { type: "rua_agent"; agent_id: string }
  | { type: "sub_agent" }
  | { type: "acp"; runner: string };

export type TaskScope =
  | {
      kind: "agent";
      agent_id: string;
      [key: string]: unknown;
    }
  | {
      kind: "project";
      project_id: string;
      [key: string]: unknown;
    };

export interface TaskChatAddress {
  scope: TaskScope;
  chat_id: string;
  [key: string]: unknown;
}

export interface TaskOwner {
  chat_address: TaskChatAddress;
}

export interface TaskOrigin {
  turn_id?: string | null;
  tool_call_id?: string | null;
}

export interface TaskSpec {
  owner: TaskOwner;
  instruction: string;
  runner: TaskRunnerSpec | LegacyTaskRunnerSpec;
  context_refs: TaskResourceRef[];
  depends_on: string[];
  origin: TaskOrigin;
}

export interface TaskDeviceContext {
  device_id?: string | null;
  device_name?: string | null;
  host_profile?: string | null;
  authority_epoch?: string | null;
}

export interface TaskResourceRef {
  uri: string;
  ref_type?: string;
  source_device_id?: string | null;
}

export interface TaskArtifact {
  id: string;
  name: string;
  media_type: string;
  resource: TaskResourceRef;
  metadata?: Record<string, unknown> | null;
}

export interface TaskErrorInfo {
  kind: "runner" | "budget_exceeded";
  message: string;
  retryable: boolean;
  details?: Record<string, unknown> | null;
}

export interface TaskResult {
  status: TaskState;
  output: string;
  full_result_ref?: TaskResourceRef | null;
  artifacts: TaskArtifact[];
  device: TaskDeviceContext;
  error?: TaskErrorInfo | null;
}

export interface TaskAttempt {
  attempt_no: number;
  state: TaskState;
  authorization_agent_id: string;
  device: TaskDeviceContext;
  created_at: string;
  updated_at: string;
  started_at?: string | null;
  finished_at?: string | null;
  result?: TaskResult | null;
}

export interface TaskRecord {
  task_id: string;
  spec: TaskSpec;
  state: TaskState;
  attempt_no: number;
  input_capability: TaskInputCapability;
  watch_active: boolean;
  created_at: string;
  updated_at: string;
  started_at?: string | null;
  finished_at?: string | null;
  result?: TaskResult | null;
  attempts: TaskAttempt[];
  waiting_on: string[];
  blocked_by: string[];
}

export interface TaskEvent {
  sequence: number;
  owner: TaskOwner;
  task_id: string;
  attempt_no: number;
  kind: string;
  state: TaskState;
  created_at: string;
}
