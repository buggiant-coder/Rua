import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SecurityPage from "./index";

const {
  apiMock,
  fileGuardSaveMock,
  fileGuardResetMock,
  toastMock,
  translateMock,
} = vi.hoisted(() => ({
  apiMock: {
    getAgentApprovalMode: vi.fn(),
    updateAgentApprovalMode: vi.fn(),
    getAgentToolGuard: vi.fn(),
    getAgentBuiltinRules: vi.fn(),
    updateAgentToolGuard: vi.fn(),
    resetAgentToolGuardToBuiltin: vi.fn(),
    listApprovalRules: vi.fn(),
    createPermissionRule: vi.fn(),
    updatePermissionRule: vi.fn(),
    deleteApprovalRule: vi.fn(),
  },
  fileGuardSaveMock: vi.fn(),
  fileGuardResetMock: vi.fn(),
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
  },
  translateMock: vi.fn(
    (key: string) =>
      ({
        "common.cancel": "Cancel",
        "common.add": "Add",
        "common.delete": "Delete",
        "common.edit": "Edit",
        "common.more": "More",
        "common.refresh": "Refresh",
        "common.reset": "Reset",
        "common.save": "Save",
        "environments.retry": "Retry",
        "security.permissionManagement.title": "Permission Management",
        "security.permissionManagement.description":
          "Manage authorization mode and permission rules",
        "security.permissionManagement.authorizationModeTitle":
          "Authorization mode",
        "security.permissionManagement.allowRulesTitle": "Allow rules",
        "security.permissionManagement.allowRulesDescription":
          "Matching calls skip repeated authorization but still pass safety checks.",
        "security.permissionManagement.allowRulesEmpty": "No allow rules",
        "security.permissionManagement.allowRulesLoadFailed":
          "Failed to load allow rules",
        "security.permissionManagement.addRule": "Add permission rule",
        "security.permissionManagement.editRule": "Edit permission rule",
        "security.permissionManagement.ruleLabel": "Rule label",
        "security.permissionManagement.ruleEffect": "Effect",
        "security.permissionManagement.ruleMatcher": "Matcher",
        "security.permissionManagement.toolPattern": "Tool pattern",
        "security.permissionManagement.ruleEnabled": "Enabled",
        "security.permissionManagement.ruleDisabled": "Disabled",
        "security.permissionManagement.toggleRule": "Toggle rule",
        "security.permissionManagement.ruleLabelRequired":
          "Rule label is required",
        "security.permissionManagement.ruleMatcherRequired":
          "Rule matcher is required",
        "security.permissionManagement.denyRulesTitle": "Deny rules",
        "security.permissionManagement.denyRulesDescription":
          "Matching calls are rejected immediately.",
        "security.permissionManagement.denyRulesEmpty": "No deny rules",
        "security.permissionManagement.precedence":
          "Deny > hard-deny safety findings > Allow > authorization mode",
        "security.permissionManagement.hitCount": "3 hits",
        "security.safetyRules.title": "Safety Rules",
        "security.safetyRules.description":
          "Every tool call is scanned by enabled safety rules.",
        "security.approvalMode.askFirstTitle": "Ask First",
        "security.approvalMode.handsOffTitle": "Hands-off",
        "security.approvalMode.justDoItTitle": "Just-Do-IT!",
        "security.approvalMode.title": "Authorization mode",
        "security.approvalMode.saveSuccess": "Approval mode saved",
        "security.approvalMode.saveFailed": "Approval mode save failed",
        "security.fileGuard.description": "Protect local files",
        "security.fileGuard.title": "File Guard",
        "security.perAgentNotice": "Per-agent security profile",
        "security.processWideNotice": "Process-wide security profile",
        "security.resetToBuiltinConfirm": "Reset",
        "security.resetToBuiltinDescription": "Reset tool guard rules",
        "security.resetToBuiltinSuccess": "Rules reset",
        "security.resetToBuiltinTitle": "Reset rules",
        "security.resetToBuiltinTooltip": "Reset to builtin",
        "security.rules.modeHint":
          "Every tool call is scanned by enabled safety rules.",
        "security.rules.title": "Rules",
        "security.saveFailed": "Save failed",
        "security.saveSuccess": "Security saved",
        "security.security": "Security",
        "security.skillScanner.description": "Scan installed skills",
        "security.skillScanner.title": "Skill Scanner",
        "security.toolGuardDescription":
          "Every tool call is scanned by enabled safety rules.",
        "security.toolGuardTitle": "Safety Rules",
      })[key] ?? key,
  ),
}));

vi.mock("../../../api", () => ({ default: apiMock }));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: (selector: (state: { selectedAgent: string }) => unknown) =>
    selector({ selectedAgent: "security-agent" }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: translateMock,
  }),
}));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    toast: toastMock,
  };
});

vi.mock("./components", async () => {
  const React = await import("react");

  return {
    ApprovalModeSection: ({
      value,
      onChange,
    }: {
      value: string;
      onChange: (value: string) => void;
    }) => (
      <div>
        <span>approval:{value}</span>
        <button type="button" onClick={() => onChange("ask_first")}>
          Ask First
        </button>
        <button type="button" onClick={() => onChange("hands_off")}>
          Hands-off
        </button>
        <button type="button" onClick={() => onChange("just_do_it")}>
          Just-Do-IT!
        </button>
      </div>
    ),
    FileGuardSection: ({
      onSave,
    }: {
      onSave?: (handlers: {
        save: () => Promise<void>;
        reset: () => void;
        saving: boolean;
      }) => void;
    }) => {
      React.useEffect(() => {
        onSave?.({
          save: fileGuardSaveMock,
          reset: fileGuardResetMock,
          saving: false,
        });
      }, [onSave]);
      return <div>File Guard Mock</div>;
    },
    PreviewModal: () => null,
    RuleModal: () => null,
    RuleTable: ({
      rules,
      enabled,
      addRuleDisabled,
      onAddRule,
      onToggleRule,
    }: {
      rules: Array<{ id: string; disabled: boolean }>;
      enabled: boolean;
      addRuleDisabled?: boolean;
      onAddRule?: () => void;
      onToggleRule: (id: string, disabled: boolean) => void;
    }) => (
      <div>
        <span>rules-enabled:{String(enabled)}</span>
        <button type="button" disabled={addRuleDisabled} onClick={onAddRule}>
          Add
        </button>
        {rules.map((rule) => (
          <button
            key={rule.id}
            type="button"
            onClick={() => onToggleRule(rule.id, rule.disabled)}
          >
            {rule.id}
          </button>
        ))}
      </div>
    ),
    SkillScannerSection: () => <div>Skill Scanner Mock</div>,
  };
});

function seedToolGuard() {
  apiMock.getAgentToolGuard.mockResolvedValue({
    enabled: true,
    custom_rules: [
      {
        id: "custom-danger",
        tools: ["Bash"],
        params: ["command"],
        category: "exec",
        severity: "high",
        patterns: [],
        exclude_patterns: [],
        description: "custom",
        remediation: "review",
      },
    ],
    disabled_rules: ["custom-danger"],
  });
  apiMock.getAgentApprovalMode.mockResolvedValue({
    approval_mode: "hands_off",
  });
  apiMock.listApprovalRules.mockResolvedValue([
    {
      id: "rule-git-status",
      agent_id: "security-agent",
      scope: "agent",
      effect: "allow",
      enabled: true,
      source: "approval",
      label: "Bash(git status)",
      matcher: {
        type: "bash_command_family",
        program: "git",
        subcommand: "status",
      },
      source_approval_id: "approval-1",
      created_at: "2026-06-22T00:00:00Z",
      updated_at: null,
      hit_count: 3,
      status: "active",
    },
  ]);
  apiMock.deleteApprovalRule.mockResolvedValue(undefined);
  apiMock.createPermissionRule.mockImplementation(
    (_agentId: string, body: Record<string, unknown>) =>
      Promise.resolve({
        id: "rule-new",
        agent_id: "security-agent",
        scope: "agent",
        source: "manual",
        source_approval_id: "manual",
        created_at: "2026-06-22T00:00:00Z",
        updated_at: "2026-06-22T00:00:00Z",
        hit_count: 0,
        status: body.enabled ? "active" : "disabled",
        ...body,
      }),
  );
  apiMock.updatePermissionRule.mockImplementation(
    (_agentId: string, ruleId: string, body: Record<string, unknown>) =>
      Promise.resolve({
        id: ruleId,
        agent_id: "security-agent",
        scope: "agent",
        source: "approval",
        source_approval_id: "approval-1",
        created_at: "2026-06-22T00:00:00Z",
        updated_at: "2026-06-22T00:00:00Z",
        hit_count: 3,
        status: body.enabled ? "active" : "disabled",
        ...body,
      }),
  );
  apiMock.getAgentBuiltinRules.mockResolvedValue([
    {
      id: "builtin-bash",
      tools: ["Bash"],
      params: ["command"],
      category: "exec",
      severity: "high",
      patterns: [],
      exclude_patterns: [],
      description: "builtin",
      remediation: "review",
    },
  ]);
  apiMock.updateAgentToolGuard.mockResolvedValue({});
  apiMock.resetAgentToolGuardToBuiltin.mockResolvedValue({});
}

describe("SecurityPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    seedToolGuard();
  });

  it("shows Permission Management with authorization mode, allow rules, and deny rules", async () => {
    const view = render(<SecurityPage />);

    await screen.findByRole("tab", { name: "Permission Management" });
    expect(screen.getByRole("tab", { name: "Safety Rules" })).toBeVisible();
    expect(screen.getByRole("tab", { name: "File Guard" })).toBeVisible();
    expect(screen.getByRole("tab", { name: "Skill Scanner" })).toBeVisible();
    expect(screen.getAllByRole("tab")).toHaveLength(4);

    expect(await screen.findByText("approval:hands_off")).toBeVisible();
    expect(screen.getByText("Allow rules")).toBeVisible();
    await waitFor(() =>
      expect(view.container).toHaveTextContent("Bash(git status)"),
    );
    expect(view.container).toHaveTextContent("3 hits");
    expect(screen.getByText("Deny rules")).toBeVisible();
    expect(
      screen.getByText(
        "Deny > hard-deny safety findings > Allow > authorization mode",
      ),
    ).toBeVisible();
    expect(
      screen.getByText(
        "Matching calls skip repeated authorization but still pass safety checks.",
      ),
    ).toBeVisible();
    expect(
      screen.getByText("Matching calls are rejected immediately."),
    ).toBeVisible();
    expect(apiMock.getAgentApprovalMode).toHaveBeenCalledWith("security-agent");
    expect(apiMock.listApprovalRules).toHaveBeenCalledWith("security-agent");
    expect(apiMock.getAgentToolGuard).toHaveBeenCalledWith("security-agent");
    expect(apiMock.getAgentBuiltinRules).toHaveBeenCalledWith("security-agent");
    expect(screen.getByRole("button", { name: "Ask First" })).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Balanced" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Hands-off" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Just-Do-IT!" })).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Ask First" }));
    await screen.findByText("approval:ask_first");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(apiMock.updateAgentApprovalMode).toHaveBeenCalledWith(
        "security-agent",
        { approval_mode: "ask_first" },
      );
    });
    expect(apiMock.updateAgentToolGuard).not.toHaveBeenCalled();
    expect(toastMock.success).toHaveBeenCalledWith("Approval mode saved");
  });

  it("creates, disables, edits, and deletes permission rules from the lists", async () => {
    render(<SecurityPage />);

    await screen.findByText("Bash(git status)");

    fireEvent.click(
      screen.getAllByRole("button", { name: "Add permission rule" })[0],
    );
    const addDialog = await screen.findByRole("dialog");
    fireEvent.change(within(addDialog).getByLabelText("Rule label"), {
      target: { value: "Block WebSearch" },
    });
    fireEvent.change(within(addDialog).getByLabelText("Effect"), {
      target: { value: "deny" },
    });
    fireEvent.change(within(addDialog).getByLabelText("Tool pattern"), {
      target: { value: "WebSearch" },
    });
    fireEvent.click(within(addDialog).getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(apiMock.createPermissionRule).toHaveBeenCalledWith(
        "security-agent",
        {
          effect: "deny",
          enabled: true,
          label: "Block WebSearch",
          matcher: { type: "tool_name", pattern: "WebSearch" },
        },
      ),
    );
    expect(await screen.findByText("Block WebSearch")).toBeVisible();

    fireEvent.click(screen.getAllByLabelText("Toggle rule")[0]);
    await waitFor(() =>
      expect(apiMock.updatePermissionRule).toHaveBeenCalledWith(
        "security-agent",
        "rule-git-status",
        expect.objectContaining({ enabled: false }),
      ),
    );
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    const editDialog = await screen.findByRole("dialog");
    fireEvent.change(within(editDialog).getByLabelText("Rule label"), {
      target: { value: "Allow git status quietly" },
    });
    fireEvent.click(within(editDialog).getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(apiMock.updatePermissionRule).toHaveBeenCalledWith(
        "security-agent",
        "rule-git-status",
        expect.objectContaining({ label: "Allow git status quietly" }),
      ),
    );

    fireEvent.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    await waitFor(() =>
      expect(apiMock.deleteApprovalRule).toHaveBeenCalledWith(
        "security-agent",
        "rule-git-status",
      ),
    );
  });

  it("loads the selected agent's Safety Rules profile and saves only rule state", async () => {
    render(<SecurityPage />);

    await screen.findByRole("tab", { name: "Permission Management" });
    fireEvent.click(screen.getByRole("tab", { name: "Safety Rules" }));
    await screen.findAllByText("Per-agent security profile");
    expect(
      screen.getAllByText("Every tool call is scanned by enabled safety rules.")
        .length,
    ).toBeGreaterThan(0);
    expect(apiMock.getAgentToolGuard).toHaveBeenCalledWith("security-agent");
    expect(apiMock.getAgentBuiltinRules).toHaveBeenCalledWith("security-agent");
    expect(screen.getByText("builtin-bash")).toBeInTheDocument();
    expect(screen.getByText("custom-danger")).toBeInTheDocument();
    expect(screen.getByText("rules-enabled:true")).toBeInTheDocument();
    expect(screen.queryByText("Guarded tools")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(apiMock.updateAgentToolGuard).toHaveBeenCalledWith(
        "security-agent",
        {
          enabled: true,
          custom_rules: [expect.objectContaining({ id: "custom-danger" })],
          disabled_rules: ["custom-danger"],
        },
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith("Security saved");
  });

  it("switches process-wide tabs and delegates file-guard save/reset handlers", async () => {
    render(<SecurityPage />);
    await screen.findAllByText("Per-agent security profile");

    fireEvent.click(screen.getByRole("tab", { name: "File Guard" }));
    expect(screen.getAllByText("Process-wide security profile").length).toBe(2);
    expect(screen.getByText("File Guard Mock")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(fileGuardSaveMock).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "More" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: /Reset/ }));
    expect(fileGuardResetMock).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("tab", { name: "Skill Scanner" }));
    expect(screen.getByText("Skill Scanner Mock")).toBeVisible();
  });

  it("shows the Rust load error and retries through the page action", async () => {
    apiMock.getAgentToolGuard
      .mockRejectedValueOnce(new Error("security unavailable"))
      .mockResolvedValueOnce({
        enabled: true,
        custom_rules: [],
        disabled_rules: [],
      });

    render(<SecurityPage />);

    expect(await screen.findByText("security unavailable")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    await screen.findAllByText("Per-agent security profile");
    expect(apiMock.getAgentToolGuard).toHaveBeenCalledTimes(2);
  });
});
