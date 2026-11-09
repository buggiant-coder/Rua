import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import HooksPage, { type HooksInitialData } from "./index";

const labels: Record<string, string> = {
  "hooksControl.title": "Hooks",
  "hooksControl.eyebrow": "Automation control plane",
  "hooksControl.description": "Hooks description",
  "hooksControl.tabs.overview": "Overview",
  "hooksControl.tabs.activity": "Activity",
  "hooksControl.navigation.primary": "Hooks sections",
  "hooksControl.navigation.configuration": "Configuration",
  "hooksControl.navigation.runtime": "Runtime configuration",
  "hooksControl.navigation.diagnostics": "Diagnostics",
  "hooksControl.navigation.workspace": "Hook configuration workspaces",
  "hooksControl.navigation.definitionCenter": "Definition center",
  "hooksControl.navigation.agentDefaults": "Agent defaults",
  "hooksControl.navigation.chatConfiguration": "Chat configuration",
  "hooksControl.navigation.effectivePreview": "Effective configuration preview",
  "hooksControl.overview.summary": "Hooks summary",
  "hooksControl.overview.registered": "Registered definitions",
  "hooksControl.overview.active": "Default enabled",
  "hooksControl.overview.attention": "Needs attention",
  "hooksControl.overview.failed": "Failed runs",
  "hooksControl.overview.defaultsTitle": "Agent defaults",
  "hooksControl.overview.defaultsDescription": "Defaults description",
  "hooksControl.overview.lifecycleTitle": "How configuration becomes active",
  "hooksControl.overview.lifecycleDescription": "Lifecycle description",
  "hooksControl.overview.lifecycleRegister": "Register",
  "hooksControl.overview.lifecycleDefault": "Enable default",
  "hooksControl.overview.lifecycleAdopt": "Adopt",
  "hooksControl.overview.lifecycleFreeze": "Freeze",
  "hooksControl.actions.newDefinition": "New definition",
  "hooksControl.actions.newCommandHook": "New Command Hook",
  "hooksControl.actions.enable": "Enable",
  "hooksControl.actions.disable": "Disable",
  "hooksControl.actions.activate": "Enable default",
  "hooksControl.actions.deactivate": "Disable default",
  "hooksControl.actions.createInactive": "Create inactive definition",
  "hooksControl.actions.inspect": "Inspect Chat",
  "hooksControl.actions.updateConfiguration": "Update configuration",
  "hooksControl.actions.inspectContributions":
    "View contributions and diagnostics",
  "hooksControl.status.defaultOn": "Default on",
  "hooksControl.status.defaultOff": "Default off",
  "hooksControl.status.enabled": "Enabled",
  "hooksControl.status.disabled": "Disabled",
  "hooksControl.status.available": "Available",
  "hooksControl.status.valid": "Valid",
  "hooksControl.status.failed": "Failed",
  "hooksControl.status.materialized": "Materialized",
  "hooksControl.status.frozen": "Frozen for Active Turn",
  "hooksControl.source.native": "Native",
  "hooksControl.source.system": "Rua built-in",
  "hooksControl.native.turnTerminalAuditDescription":
    "Registered automatically by Rua.",
  "hooksControl.labels.revision": "Revision {{revision}}",
  "hooksControl.labels.noCapabilities": "No data capabilities",
  "hooksControl.labels.epoch": "Configuration epoch",
  "hooksControl.labels.agent": "Selected Agent",
  "hooksControl.labels.authorityDevice": "Authority Device",
  "hooksControl.labels.chat": "Chat",
  "hooksControl.labels.turn": "Turn",
  "hooksControl.labels.source": "Source",
  "hooksControl.labels.order": "Resolved order",
  "hooksControl.labels.technicalDetails": "Technical details",
  "hooksControl.labels.event": "Event",
  "hooksControl.labels.handler": "Handler",
  "hooksControl.labels.revisionLabel": "Revision",
  "hooksControl.labels.definitionId": "Definition ID",
  "hooksControl.chats.title": "Chat configuration",
  "hooksControl.chats.description": "Chat configuration description",
  "hooksControl.chats.chatId": "Chat ID",
  "hooksControl.chats.chatIdPlaceholder": "Enter a Chat ID",
  "hooksControl.chats.currentConfiguration":
    "Current materialized configuration",
  "hooksControl.chats.empty": "Choose a Chat to inspect",
  "hooksControl.chats.emptyHint": "Chat empty hint",
  "hooksControl.activity.title": "Hook activity",
  "hooksControl.activity.description": "Activity description",
  "hooksControl.activity.details": "Hook run details",
  "hooksControl.activity.chatTurn": "Chat {{chat}} · Turn {{turn}}",
  "hooksControl.contributions.diagnostic": "Diagnostic",
  "hooksControl.create.title": "Create Hook definition",
  "hooksControl.create.description": "Create description",
  "hooksControl.create.triggerSection": "Trigger",
  "hooksControl.create.identitySection": "Identity",
  "hooksControl.create.commandSection": "Command",
  "hooksControl.create.event": "Run when",
  "hooksControl.create.events.turn.terminal":
    "After Turn reaches a terminal state",
  "hooksControl.create.eventDescriptions.turn.terminal":
    "Runs after terminal commit",
  "hooksControl.create.identityOnly": "Identity only",
  "hooksControl.create.displayName": "Display name",
  "hooksControl.create.displayNamePlaceholder": "Audit terminal turns",
  "hooksControl.create.logicalId": "Logical ID",
  "hooksControl.create.logicalIdHelp": "Logical ID help",
  "hooksControl.create.program": "Program",
  "hooksControl.create.programHelp": "Program help",
  "hooksControl.create.programAbsolute": "Program must be absolute",
  "hooksControl.create.arguments": "Arguments",
  "hooksControl.create.argumentsHelp": "Arguments help",
  "hooksControl.create.workingDirectory": "Working directory",
  "hooksControl.create.workingDirectoryHelp": "Working directory help",
  "hooksControl.create.workingDirectoryAbsolute":
    "Working directory must be absolute",
  "hooksControl.create.timeout": "Timeout (seconds)",
  "hooksControl.create.timeoutInvalid": "Timeout must be positive",
  "hooksControl.create.inactiveNotice":
    "New definitions are inactive by default",
  "hooksControl.create.disabledAfterCreate": "Created as disabled",
  "hooksControl.create.disabledAfterCreateDescription": "Enable it later",
  "hooksControl.create.submit": "Create Hook",
  "hooksControl.scope.tabs": "Hook scope",
  "hooksControl.scope.agent": "Agent Hooks",
  "hooksControl.scope.project": "Project Hooks",
  "hooksControl.scope.agentTitle": "Agent Hooks",
  "hooksControl.scope.projectTitle": "Project Hooks",
  "hooksControl.scope.agentDescription": "Agent Hook description",
  "hooksControl.scope.projectDescription": "Project Hook description",
  "hooksControl.scope.selectProject": "Select Project",
  "projects.selectProject": "Select Project",
  "projects.title": "Projects",
  "hooksControl.scope.manifestHint":
    "Project Definitions are discovered from .rua/hooks.json",
  "hooksControl.scope.noProjectDefinitions": "No Project Definitions found",
  "hooksControl.scope.noProjectDefinitionsHint":
    "Add declarations to .rua/hooks.json",
  "common.cancel": "Cancel",
  "common.notAvailable": "N/A",
};

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, values?: Record<string, string>) => {
      let value = labels[key] ?? key;
      for (const [name, replacement] of Object.entries(values ?? {})) {
        value = value.replace(`{{${name}}}`, replacement);
      }
      return value;
    },
  }),
}));

const {
  updateDefault,
  listProjectDefinitions,
  updateProjectDefault,
  createDefinition,
} = vi.hoisted(() => ({
  updateDefault: vi.fn(),
  listProjectDefinitions: vi.fn(),
  updateProjectDefault: vi.fn(),
  createDefinition: vi.fn(),
}));
vi.mock("@/api/modules/hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/modules/hooks")>()),
  hooksApi: {
    updateDefault,
    listProjectDefinitions,
    updateProjectDefault,
    createDefinition,
  },
}));

const project = {
  project_id: "project-1",
  display_name: "Rua",
  resource_device_id: "device-1",
  display_path: "/repo/rua",
  lifecycle: "active" as const,
  created_at: "2026-08-24T00:00:00Z",
  updated_at: "2026-08-24T00:00:00Z",
};
const secondProject = {
  ...project,
  project_id: "project-2",
  display_name: "Second Project",
  display_path: "/repo/second",
};

describe("HooksPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it("shows registration and activation as separate states", async () => {
    const initialData: HooksInitialData = {
      agentId: "agent-1",
      definitions: [
        {
          id: "rua:native/turn-terminal-audit",
          owner_namespace: "rua:native",
          logical_id: "turn-terminal-audit",
          display_name: "Turn terminal audit",
          source: "native",
          latest_revision: "v1",
          revision: "v1",
          event: "turn.terminal",
          capabilities: ["identity"],
          handler_kind: "native",
          registered: true,
          availability: { status: "available" },
          validation: { status: "valid" },
          active: false,
          disabled: true,
          revision_changed: false,
        },
      ],
      runs: [],
      preview: {
        hooks: [
          {
            definition_revision: "rua:native/turn-terminal-audit@v1",
            event: "turn.terminal",
            resolved_order: 0,
            parameters: {},
            selected_by: ["agent"],
            overridden_parameters: [],
          },
        ],
        warnings: [],
      },
      projects: [project],
    };
    updateDefault.mockResolvedValue([
      { ...initialData.definitions[0], active: true },
    ]);

    render(<HooksPage initialData={initialData} />);
    expect(screen.getByRole("tab", { name: "Agent Hooks" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Project Hooks" })).toHaveAttribute(
      "aria-selected",
      "false",
    );
    expect(
      screen.getByRole("button", { name: "agent.switchAgent" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "New Command Hook" }),
    ).toHaveTextContent("");
    expect(screen.getByText("Disabled")).toBeInTheDocument();
    expect(screen.getByText("Rua built-in")).toBeInTheDocument();
    expect(
      screen.getByText("Registered automatically by Rua."),
    ).toBeInTheDocument();
    expect(screen.getByText("Available")).toBeInTheDocument();
    const technicalDetails = screen
      .getByText("Technical details")
      .closest("details");
    expect(technicalDetails).not.toHaveAttribute("open");

    fireEvent.click(screen.getByRole("button", { name: "Enable" }));
    await waitFor(() =>
      expect(screen.getByText("Enabled")).toBeInTheDocument(),
    );
    expect(updateDefault).toHaveBeenCalledWith(
      "agent-1",
      "rua:native/turn-terminal-audit",
      true,
    );
    fireEvent.click(screen.getByText("Technical details"));
    expect(technicalDetails).toHaveAttribute("open");
    expect(screen.getByText("turn.terminal")).toBeInTheDocument();
  });

  it("switches to Project Hooks and updates the selected Project default", async () => {
    const initialData: HooksInitialData = {
      agentId: "agent-1",
      definitions: [],
      runs: [],
      preview: { hooks: [], warnings: [] },
      projects: [project],
    };
    const projectDefinition = {
      id: "project:project-1/audit-build",
      owner_namespace: "project:project-1",
      logical_id: "audit-build",
      display_name: "Audit build",
      source: "project" as const,
      latest_revision: "v1",
      revision: "v1",
      event: "turn.terminal",
      capabilities: ["identity"],
      handler_kind: "command",
      registered: true,
      availability: { status: "available" as const },
      validation: { status: "valid" as const },
      active: false,
      disabled: true,
      revision_changed: false,
    };
    listProjectDefinitions.mockResolvedValue([projectDefinition]);
    updateProjectDefault.mockResolvedValue([
      { ...projectDefinition, active: true },
    ]);

    render(<HooksPage initialData={initialData} />);
    fireEvent.click(screen.getByRole("tab", { name: "Project Hooks" }));
    expect(
      screen.queryByRole("button", { name: "agent.switchAgent" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "New Command Hook" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Select Project" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Audit build")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Enable" }));
    await waitFor(() =>
      expect(updateProjectDefault).toHaveBeenCalledWith(
        "project-1",
        projectDefinition.id,
        true,
      ),
    );
    expect(await screen.findByText("Enabled")).toBeInTheDocument();
  });

  it("opens Project Hooks on the Project most recently used by Project Chat", async () => {
    localStorage.setItem(
      "rua_project_last_project",
      secondProject.project_id,
    );
    listProjectDefinitions.mockResolvedValue([]);
    const initialData: HooksInitialData = {
      agentId: "agent-1",
      definitions: [],
      runs: [],
      preview: { hooks: [], warnings: [] },
      projects: [project, secondProject],
    };

    render(<HooksPage initialData={initialData} />);
    fireEvent.click(screen.getByRole("tab", { name: "Project Hooks" }));

    expect(
      screen.getByRole("button", { name: "Select Project" }),
    ).toHaveTextContent("Second Project");
    await waitFor(() =>
      expect(listProjectDefinitions).toHaveBeenCalledWith("project-2"),
    );

    fireEvent.click(screen.getByRole("button", { name: "Select Project" }));
    fireEvent.click(screen.getByRole("button", { name: /Rua/ }));
    await waitFor(() =>
      expect(localStorage.getItem("rua_project_last_project")).toBe(
        "project-1",
      ),
    );
  });

  it("creates an inactive definition from the labeled drawer form", async () => {
    const initialData: HooksInitialData = {
      agentId: "agent-1",
      definitions: [],
      runs: [],
      preview: { hooks: [], warnings: [] },
      projects: [project],
    };
    createDefinition.mockResolvedValue({
      id: "agent:agent-1/audit-turn",
      owner_namespace: "agent:agent-1",
      logical_id: "audit-turn",
      display_name: "Audit turn",
      source: "user",
      latest_revision: "v1",
      revision: "v1",
      event: "turn.terminal",
      capabilities: ["identity"],
      handler_kind: "command",
      registered: true,
      availability: { status: "available" },
      validation: { status: "valid" },
      active: false,
      disabled: true,
      revision_changed: false,
    });

    render(<HooksPage initialData={initialData} />);
    fireEvent.click(screen.getByRole("button", { name: "New Command Hook" }));
    fireEvent.change(screen.getByLabelText("Display name"), {
      target: { value: "Audit turn" },
    });
    fireEvent.change(screen.getByLabelText("Logical ID"), {
      target: { value: "audit-turn" },
    });
    fireEvent.change(screen.getByLabelText("Program"), {
      target: { value: "/usr/bin/python3" },
    });
    fireEvent.change(screen.getByLabelText("Arguments"), {
      target: { value: "./audit.py\n--strict" },
    });
    fireEvent.change(screen.getByLabelText("Working directory"), {
      target: { value: "/workspace/project" },
    });
    fireEvent.change(screen.getByLabelText("Timeout (seconds)"), {
      target: { value: "45" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create Hook" }));

    await waitFor(() =>
      expect(createDefinition).toHaveBeenCalledWith(
        "agent-1",
        expect.objectContaining({
          logical_id: "audit-turn",
          display_name: "Audit turn",
          event: "turn.terminal",
          handler: {
            kind: "command",
            program: "/usr/bin/python3",
            args: ["./audit.py", "--strict"],
            working_directory: "/workspace/project",
            timeout_seconds: 45,
            available: true,
          },
        }),
      ),
    );
    expect(await screen.findByText("Audit turn")).toBeInTheDocument();
  });

  it("rejects relative command paths before creating a Hook", () => {
    const initialData: HooksInitialData = {
      agentId: "agent-1",
      definitions: [],
      runs: [],
      preview: { hooks: [], warnings: [] },
      projects: [project],
    };

    render(<HooksPage initialData={initialData} />);
    fireEvent.click(screen.getByRole("button", { name: "New Command Hook" }));
    fireEvent.change(screen.getByLabelText("Display name"), {
      target: { value: "Audit" },
    });
    fireEvent.change(screen.getByLabelText("Logical ID"), {
      target: { value: "audit" },
    });
    fireEvent.change(screen.getByLabelText("Program"), {
      target: { value: "python3" },
    });
    fireEvent.change(screen.getByLabelText("Working directory"), {
      target: { value: "./project" },
    });

    expect(screen.getByText("Program must be absolute")).toBeInTheDocument();
    expect(
      screen.getByText("Working directory must be absolute"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create Hook" })).toBeDisabled();
    expect(createDefinition).not.toHaveBeenCalled();
  });
});
