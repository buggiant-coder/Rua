import { request } from "../request";
import type { ACPPermissionOption, ACPPermissionTaskContext } from "../types";

export interface PushMessage {
  id: string;
  text: string;
}

export interface PendingApproval {
  request_id: string;
  tool_name: string;
  created_at: number;
  expiry_seconds: number;
}

export type ApprovalStatus =
  | "pending"
  | "approved"
  | "rejected"
  | "denied"
  | "expired"
  | "consumed"
  | "superseded";

export type ApprovalReuseScope = "chat" | "runtime_session" | "agent";

export type ApprovalOwner =
  | {
      kind: "chat";
      address: {
        scope:
          | { kind: "agent"; agent_id: string }
          | { kind: "project"; project_id: string };
        chat_id: string;
      };
    }
  | {
      kind: "runtime_session";
      agent_id: string;
      session_id: string;
    };

export type ApprovalReuseMatcher =
  | {
      type: "bash_command_pattern";
      pattern: string;
    }
  | {
      type: "bash_command_family";
      program: string;
      subcommand: string;
    }
  | {
      type: "tool_name";
      pattern: string;
    };

export type PermissionRuleEffect = "allow" | "deny";
export type PermissionRuleSource = "manual" | "approval";

export interface ApprovalReuseOption {
  scope: ApprovalReuseScope;
  label: string;
  matcher: ApprovalReuseMatcher;
}

export interface ApprovalRequest {
  id: string;
  agent_id: string;
  owner?: ApprovalOwner;
  chat_id?: string;
  turn_id: string;
  tool_call_id: string;
  tool_name: string;
  status: ApprovalStatus;
  trigger?: unknown;
  tool_call: {
    display: string;
    args: unknown;
  };
  created_at: string;
  decided_at?: string | null;
  decided_by?: string | null;
  reuse_options: ApprovalReuseOption[];
  origin?: "rua" | "acp";
  runner?: string;
  acp_session_id?: string;
  acp_options?: ACPPermissionOption[];
  acp_task?: ACPPermissionTaskContext | null;
}

export interface ApprovalSnapshot {
  pending: ApprovalRequest[];
  resolved: ApprovalRequest[];
}

export interface ApprovalRule {
  id: string;
  agent_id: string;
  chat_id?: string | null;
  scope: ApprovalReuseScope;
  effect: PermissionRuleEffect;
  enabled: boolean;
  source: PermissionRuleSource;
  label: string;
  matcher: ApprovalReuseMatcher;
  source_approval_id: string;
  created_at: string;
  updated_at?: string | null;
  hit_count: number;
  status: string;
}

export interface PermissionRuleInput {
  effect: PermissionRuleEffect;
  enabled: boolean;
  label: string;
  matcher: ApprovalReuseMatcher;
}

export type ApprovalDecisionRequest =
  | {
      decision: "run_once" | "reject";
      decided_by?: string;
    }
  | {
      decision: "allow_chat" | "allow_agent";
      decided_by?: string;
      matcher: ApprovalReuseMatcher;
    };

export const consoleApi = {
  getPushMessages: (agentId: string) =>
    request<{ messages: PushMessage[]; pending_approvals: PendingApproval[] }>(
      `/agents/${encodeURIComponent(agentId)}/console/push-messages`,
    ),
  getApprovalSnapshot: (agentId: string, chatId: string) =>
    request<ApprovalSnapshot>(
      `/agents/${encodeURIComponent(
        agentId,
      )}/approvals/snapshot?chat_id=${encodeURIComponent(chatId)}`,
    ),
  decideApproval: (
    agentId: string,
    approvalId: string,
    body: ApprovalDecisionRequest,
  ) =>
    request<ApprovalRequest>(
      `/agents/${encodeURIComponent(agentId)}/approvals/${encodeURIComponent(
        approvalId,
      )}/decide`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),
  listApprovalRules: (agentId: string) =>
    request<ApprovalRule[]>(
      `/agents/${encodeURIComponent(agentId)}/permission-rules`,
    ),
  createPermissionRule: (agentId: string, body: PermissionRuleInput) =>
    request<ApprovalRule>(
      `/agents/${encodeURIComponent(agentId)}/permission-rules`,
      { method: "POST", body: JSON.stringify(body) },
    ),
  updatePermissionRule: (
    agentId: string,
    ruleId: string,
    body: PermissionRuleInput,
  ) =>
    request<ApprovalRule>(
      `/agents/${encodeURIComponent(
        agentId,
      )}/permission-rules/${encodeURIComponent(ruleId)}`,
      { method: "PUT", body: JSON.stringify(body) },
    ),
  deleteApprovalRule: (agentId: string, ruleId: string) =>
    request<void>(
      `/agents/${encodeURIComponent(
        agentId,
      )}/permission-rules/${encodeURIComponent(ruleId)}`,
      { method: "DELETE" },
    ),
};
