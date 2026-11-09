import { request } from "../request";

export interface ArgMatcher {
  command?: string;
  flag?: string;
  value_contains?: string;
  value_in?: string[];
}

export interface PathMatcher {
  contains?: string[];
  prefix?: string[];
  glob?: string;
  on_redirect?: boolean;
}

export interface ToolGuardRule {
  id: string;
  tools: string[];
  params: string[];
  category: string;
  severity: string;
  patterns: string[];
  exclude_patterns: string[];
  description: string;
  remediation: string;
  rule_type?: "regex" | "ast";
  command_set?: string[];
  arg_matchers?: ArgMatcher[];
  path_matchers?: PathMatcher[];
  lexical?: string[];
  structural?: string[];
  action?: string;
}

export type ApprovalMode =
  | "ask_first"
  | "balanced"
  | "hands_off"
  | "just_do_it";

export interface ApprovalModeConfig {
  approval_mode: ApprovalMode;
}

export interface ToolGuardConfig {
  enabled: boolean;
  custom_rules: ToolGuardRule[];
  disabled_rules: string[];
}

export interface FileGuardResponse {
  enabled: boolean;
  paths: string[];
  builtin_paths?: string[];
  disabled_patterns?: string[];
}

export interface FileGuardUpdateBody {
  enabled?: boolean;
  paths?: string[];
  disabled_patterns?: string[];
}

export interface SkillScannerWhitelistEntry {
  skill_name: string;
  content_hash: string;
  added_at: string;
}

export type SkillScannerMode = "block" | "warn" | "off";

export interface SkillScannerConfig {
  mode: SkillScannerMode;
  timeout: number;
  whitelist: SkillScannerWhitelistEntry[];
}

export interface BlockedSkillFinding {
  severity: string;
  title: string;
  description: string;
  file_path: string;
  line_number: number | null;
  rule_id: string;
}

export interface BlockedSkillRecord {
  skill_name: string;
  blocked_at: string;
  max_severity: string;
  findings: BlockedSkillFinding[];
  content_hash: string;
  action: "blocked" | "warned";
  source?: string;
}

export interface SecurityScanErrorResponse {
  type: "security_scan_failed";
  detail: string;
  skill_name: string;
  max_severity: string;
  findings: BlockedSkillFinding[];
}

export interface ManualScanResultItem {
  skill_name: string;
  severity: string;
  findings_count: number;
  action: string;
}

export interface ManualScanResponse {
  scanned: number;
  issues: number;
  results: ManualScanResultItem[];
}

export interface NetworkSecretBackupResponse {
  armored_text: string;
}

export interface NetworkSecretBackupQrResponse {
  svg: string;
}

export const securityApi = {
  exportNetworkSecretBackup: (passphrase: string) =>
    request<NetworkSecretBackupResponse>("/security/master-key-export", {
      method: "POST",
      body: JSON.stringify({ passphrase }),
    }),

  renderNetworkSecretBackupQr: (text: string) =>
    request<NetworkSecretBackupQrResponse>("/security/master-key-export/qr", {
      method: "POST",
      body: JSON.stringify({ text }),
    }),

  getAgentToolGuard: (agentId: string) =>
    request<ToolGuardConfig>(`/agents/${agentId}/security/tool-guard`),

  updateAgentToolGuard: (agentId: string, body: ToolGuardConfig) =>
    request<ToolGuardConfig>(`/agents/${agentId}/security/tool-guard`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  getAgentBuiltinRules: (agentId: string) =>
    request<ToolGuardRule[]>(
      `/agents/${agentId}/security/tool-guard/builtin-rules`,
    ),

  resetAgentToolGuardToBuiltin: (agentId: string) =>
    request<ToolGuardConfig>(
      `/agents/${agentId}/security/tool-guard/reset-to-builtin`,
      { method: "POST" },
    ),

  getAgentApprovalMode: (agentId: string) =>
    request<ApprovalModeConfig>(`/agents/${agentId}/security/approval-mode`),

  updateAgentApprovalMode: (agentId: string, body: ApprovalModeConfig) =>
    request<ApprovalModeConfig>(`/agents/${agentId}/security/approval-mode`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  updateToolGuard: (body: ToolGuardConfig) =>
    request<ToolGuardConfig>("/config/security/tool-guard", {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  getFileGuard: () => request<FileGuardResponse>("/config/security/file-guard"),

  updateFileGuard: (body: FileGuardUpdateBody) =>
    request<FileGuardResponse>("/config/security/file-guard", {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  getSkillScanner: () =>
    request<SkillScannerConfig>("/config/security/skill-scanner"),

  updateSkillScanner: (body: SkillScannerConfig) =>
    request<SkillScannerConfig>("/config/security/skill-scanner", {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  getBlockedHistory: () =>
    request<BlockedSkillRecord[]>(
      "/config/security/skill-scanner/blocked-history",
    ),

  clearBlockedHistory: () =>
    request<{ cleared: boolean }>(
      "/config/security/skill-scanner/blocked-history",
      { method: "DELETE" },
    ),

  removeBlockedEntry: (index: number) =>
    request<{ removed: boolean }>(
      `/config/security/skill-scanner/blocked-history/${index}`,
      { method: "DELETE" },
    ),

  addToWhitelist: (skillName: string, contentHash: string = "") =>
    request<{ whitelisted: boolean; skill_name: string }>(
      "/config/security/skill-scanner/whitelist",
      {
        method: "POST",
        body: JSON.stringify({
          skill_name: skillName,
          content_hash: contentHash,
        }),
      },
    ),

  removeFromWhitelist: (skillName: string) =>
    request<{ removed: boolean; skill_name: string }>(
      `/config/security/skill-scanner/whitelist/${encodeURIComponent(
        skillName,
      )}`,
      { method: "DELETE" },
    ),

  triggerScan: () =>
    request<ManualScanResponse>("/skills/scanner/scan", {
      method: "POST",
    }),
};
