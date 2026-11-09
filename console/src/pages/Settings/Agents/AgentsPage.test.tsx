import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AgentsPage from "./index";

const mockAgentsApi = vi.hoisted(() => ({
  listAgents: vi.fn(),
  getAgent: vi.fn(),
  updateAgent: vi.fn(),
  renameAgent: vi.fn(),
  createAgent: vi.fn(),
  deleteAgent: vi.fn(),
  reorderAgents: vi.fn(),
  toggleAgentEnabled: vi.fn(),
}));

const mockSkillApi = vi.hoisted(() => ({
  listSkillPoolSkills: vi.fn(),
  listSkills: vi.fn(),
  downloadSkillPoolSkill: vi.fn(),
  deleteSkill: vi.fn(),
}));

vi.mock("../../../api/modules/agents", () => ({
  agentsApi: mockAgentsApi,
}));

vi.mock("../../../api/modules/skill", () => ({
  skillApi: mockSkillApi,
}));

vi.mock("../../../api/modules/subagentTemplates", () => ({
  subagentTemplatesApi: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
}));

vi.mock("@/api/modules/agentTemplates", () => ({
  agentTemplatesApi: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: unknown) => {
      if (key === "agent.editTitle") {
        const name =
          params && typeof params === "object" && "name" in params
            ? String((params as { name: unknown }).name)
            : "";
        return `Edit Agent - ${name}`;
      }
      const labels: Record<string, string> = {
        "agent.agents": "Agents",
        "agent.id": "Agent ID",
        "agent.idLabel": "Agent ID",
        "agent.idPattern": "Use letters, numbers, dashes, or underscores.",
        "agent.initialSkills": "Initial skills",
        "agent.addSkillsToAgent": "Agent skills",
        "agent.selectAll": "Select all",
        "agent.selectBuiltin": "Select builtin",
        "agent.selectNone": "Select none",
        "agent.noPoolSkills": "No pool skills",
        "agent.createSuccess": "Created",
        "agent.edit": "Edit",
        "agent.name": "Name",
        "agent.description": "Description",
        "agent.workspace": "Workspace",
        "agent.updateSuccess": "Updated",
        "agent.loadFailed": "Load failed",
        "agentMgmt.createAgent": "Create agent",
        "agentMgmt.tabAgent": "Agents",
        "agentMgmt.tabSubAgent": "Sub-agents",
        "agentMgmt.tabTemplates": "Templates",
        "common.save": "Save",
        "common.cancel": "Cancel",
        "common.confirm": "Confirm",
        "template.selectTemplate": "Template",
        "template.selectTemplatePlaceholder": "Select template",
        "template.noTemplate": "No template",
        "template.templateWillCopy": "Template files",
      };
      return labels[key] ?? key;
    },
  }),
}));

describe("AgentsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockAgentsApi.listAgents.mockResolvedValue({
      agents: [
        {
          id: "default",
          name: "Old name",
          description: "",
          workspace_dir: "/tmp/default",
          enabled: true,
        },
      ],
    });
    mockAgentsApi.getAgent.mockResolvedValue({
      id: "default",
      name: "Old name",
      description: "",
      workspace_dir: "/tmp/default",
    });
    mockAgentsApi.updateAgent.mockResolvedValue({
      id: "default",
      name: "New name",
      description: "",
      workspace_dir: "/tmp/default",
    });
    mockAgentsApi.renameAgent.mockResolvedValue({
      id: "default",
      workspace_dir: "/tmp/default",
    });
    mockAgentsApi.createAgent.mockResolvedValue({
      id: "researcher",
      name: "Researcher",
      description: "",
      workspace_dir: "",
      enabled: true,
    });
    mockSkillApi.listSkillPoolSkills.mockResolvedValue([]);
    mockSkillApi.listSkills.mockResolvedValue([]);
    mockSkillApi.downloadSkillPoolSkill.mockResolvedValue({});
    mockSkillApi.deleteSkill.mockResolvedValue({});
  });

  it("sends edited agent name to update API", async () => {
    render(
      <MemoryRouter initialEntries={["/settings/agents"]}>
        <AgentsPage />
      </MemoryRouter>,
    );

    await screen.findByText("Old name");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));

    const nameInput = await screen.findByPlaceholderText(
      "agent.namePlaceholder",
    );
    await waitFor(() => {
      expect(nameInput).toHaveValue("Old name");
    });
    fireEvent.change(nameInput, { target: { value: "New name" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(mockAgentsApi.updateAgent).toHaveBeenCalledWith(
        "default",
        expect.objectContaining({ name: "New name" }),
      );
    });
    expect(mockAgentsApi.renameAgent).not.toHaveBeenCalled();
  });

  it("creates an agent with selected Rust skill-pool skills", async () => {
    mockSkillApi.listSkillPoolSkills.mockResolvedValueOnce([
      { name: "builtin-writer", source: "builtin" },
      { name: "market-reviewer", source: "clawhub" },
    ]);

    render(
      <MemoryRouter initialEntries={["/settings/agents"]}>
        <AgentsPage />
      </MemoryRouter>,
    );

    await screen.findByText("Old name");
    fireEvent.click(screen.getByRole("button", { name: "Create agent" }));

    await screen.findByText("builtin-writer");
    fireEvent.click(screen.getByText("market-reviewer"));
    fireEvent.change(screen.getByPlaceholderText("agent.idPlaceholder"), {
      target: { value: "researcher" },
    });
    fireEvent.change(screen.getByPlaceholderText("agent.namePlaceholder"), {
      target: { value: "Researcher" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(mockAgentsApi.createAgent).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "researcher",
          name: "Researcher",
          skill_names: ["market-reviewer"],
        }),
      );
    });
  });
});
