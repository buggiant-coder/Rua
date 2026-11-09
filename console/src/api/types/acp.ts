export type ToolParseMode = "call_title" | "update_detail" | "call_detail";

export interface ACPRunnerInfo {
  key: string;
  enabled: boolean;
  command: string;
  args: string[];
  env: Record<string, string>;
  trusted: boolean;
  tool_parse_mode: ToolParseMode;
  stdio_buffer_limit_bytes: number | null;
  is_builtin: boolean;
  check_result?: ACPRunnerCheckResult;
}

export interface ACPRunnerCheckResult {
  ok: boolean;
  status: "ok" | "unavailable";
  message: string;
  requires_auth: boolean;
}

export interface ACPRunnerInstallResult {
  installed: boolean;
  message: string;
  check_result: ACPRunnerCheckResult;
}

export interface ACPRunnerInstallTask {
  task_id: string;
  agent_id: string;
  runner_key: string;
  status: "pending" | "running" | "completed" | "failed";
  stage: string;
  message: string;
  detail?: string | null;
  installed: boolean;
  check_result?: ACPRunnerCheckResult | null;
  created_at: number;
  updated_at: number;
}

export interface ACPRunnerCreateRequest {
  key: string;
  enabled?: boolean;
  command: string;
  args?: string[];
  env?: Record<string, string>;
  trusted?: boolean;
  tool_parse_mode?: ToolParseMode;
  stdio_buffer_limit_bytes?: number | null;
}

export interface ACPRunnerUpdateRequest {
  enabled?: boolean;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  trusted?: boolean;
  tool_parse_mode?: ToolParseMode;
  stdio_buffer_limit_bytes?: number | null;
}

export interface ACPChatSessionEntry {
  session_id: string;
  title: string | null;
  cwd: string;
  updated_at: string | null;
}

export interface ACPChatStatus {
  active: boolean;
  runner: string | null;
  session_id?: string | null;
  cwd: string | null;
  chat_id: string;

  workspace_dir: string;

  target_device_id?: string | null;
}

export interface ACPChatEnterRequest {
  agent_id?: string;
  runner: string;
  cwd?: string;
  resume_session_id?: string;
  target_device_id?: string;
}

export interface ACPChatEnterResponse {
  ok: boolean;
  message: string;
  status: ACPChatStatus;
  history_preview?: string | null;
}

export interface ACPChatExitResponse {
  ok: boolean;
  message: string;
}

export type ACPPermissionOptionKind =
  | "allow_once"
  | "allow_always"
  | "reject_once"
  | "reject_always";

export interface ACPPermissionOption {
  option_id: string;
  name: string;
  kind: ACPPermissionOptionKind;
}

export interface ACPPermissionTaskContext {
  task_id: string;
  attempt_no: number;
  origin_turn_id?: string | null;
  origin_tool_call_id?: string | null;
}

export interface ACPPermissionRequest {
  id: string;
  agent_id: string;
  chat_id: string;
  session_id: string;
  runner: string;
  tool_call_id: string;
  tool_name: string;
  tool_input: unknown;
  options: ACPPermissionOption[];
  task?: ACPPermissionTaskContext | null;
}

export interface ACPPermissionSnapshot {
  pending: ACPPermissionRequest[];
}
