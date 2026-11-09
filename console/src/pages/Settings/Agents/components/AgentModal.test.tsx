import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AgentModal } from "./AgentModal";

const apiMocks = vi.hoisted(() => ({
  listSkillPoolSkills: vi.fn(),
  listSkills: vi.fn(),
  listTemplates: vi.fn(),
  getTemplate: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: unknown) => {
      if (key === "agent.editTitle") {
        const name =
          params && typeof params === "object" && "name" in params
            ? String((params as { name: unknown }).name)
            : "";
        return `Edit ${name}`;
      }
      const labels: Record<string, string> = {
        "agent.id": "Agent ID",
        "agent.idLabel": "Agent ID",
        "agent.idPattern": "Use letters, numbers, dashes, or underscores.",
        "agent.initialSkills": "Initial skills",
        "agent.addSkillsToAgent": "Agent skills",
        "agent.name": "Name",
        "agent.description": "Description",
        "agent.workspace": "Workspace",
        "agent.selectAll": "Select all",
        "agent.selectBuiltin": "Select builtin",
        "agent.selectNone": "Select none",
        "agent.noPoolSkills": "No pool skills",
        "template.selectTemplate": "Template",
        "template.selectTemplatePlaceholder": "Select template",
        "template.noTemplate": "No template",
        "template.templateWillCopy": "Template files",
        "common.save": "Save",
        "common.cancel": "Cancel",
      };
      return labels[key] ?? key;
    },
  }),
}));

vi.mock("@/api/modules/skill", () => ({
  skillApi: {
    listSkillPoolSkills: apiMocks.listSkillPoolSkills,
    listSkills: apiMocks.listSkills,
  },
}));

vi.mock("@/api/modules/agentTemplates", () => ({
  agentTemplatesApi: {
    list: apiMocks.listTemplates,
    get: apiMocks.getTemplate,
  },
}));

describe("AgentModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMocks.listSkillPoolSkills.mockResolvedValue([]);
    apiMocks.listSkills.mockResolvedValue([]);
    apiMocks.listTemplates.mockResolvedValue([]);
    apiMocks.getTemplate.mockResolvedValue({ prompt_files: [] });
  });

  it("submits edited agent name", async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);

    render(
      <AgentModal
        open
        editingAgent={{
          id: "default",
          name: "Old name",
          description: "",
          workspace_dir: "/tmp/default",
          enabled: true,
        }}
        initialValues={{
          id: "default",
          name: "Old name",
          description: "",
          workspace_dir: "/tmp/default",
          template_id: null,
        }}
        selectedSkills={[]}
        onSelectedSkillsChange={vi.fn()}
        onInstalledSkillsLoaded={vi.fn()}
        onSave={onSave}
        onClose={vi.fn()}
      />,
    );

    const nameInput = screen.getByPlaceholderText("agent.namePlaceholder");
    await waitFor(() => {
      expect(nameInput).toHaveValue("Old name");
    });
    expect(screen.getByDisplayValue("default")).toHaveAttribute("readonly");
    expect(screen.getByDisplayValue("default")).toHaveAttribute(
      "aria-readonly",
      "true",
    );
    expect(screen.getByDisplayValue("default")).not.toBeDisabled();
    expect(screen.getByDisplayValue("/tmp/default")).toHaveAttribute(
      "readonly",
    );
    expect(screen.getByDisplayValue("/tmp/default")).not.toBeDisabled();

    fireEvent.change(nameInput, { target: { value: "New name" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSave).toHaveBeenCalledWith(
        expect.objectContaining({ id: "default", name: "New name" }),
      );
    });
  });

  it("loads create-mode templates and applies Rust skill-pool selection helpers", async () => {
    apiMocks.listSkillPoolSkills.mockResolvedValueOnce([
      { name: "builtin-writer", source: "builtin" },
      { name: "market-reviewer", source: "clawhub" },
    ]);
    apiMocks.listTemplates.mockResolvedValueOnce([
      { id: "qa", name: "QA", builtin: true },
    ]);
    const onSelectedSkillsChange = vi.fn();
    const onInstalledSkillsLoaded = vi.fn();

    render(
      <AgentModal
        open
        editingAgent={null}
        selectedSkills={[]}
        onSelectedSkillsChange={onSelectedSkillsChange}
        onInstalledSkillsLoaded={onInstalledSkillsLoaded}
        onSave={vi.fn().mockResolvedValue(undefined)}
        onClose={vi.fn()}
      />,
    );

    expect(await screen.findByText("builtin-writer")).toBeVisible();
    expect(screen.getByText("market-reviewer")).toBeVisible();
    expect(screen.getByText("Template")).toBeVisible();
    expect(screen.getByText("Select template")).toBeVisible();
    expect(apiMocks.listTemplates).toHaveBeenCalledTimes(1);
    expect(onInstalledSkillsLoaded).toHaveBeenCalledWith([]);
    expect(onSelectedSkillsChange).toHaveBeenCalledWith([]);

    fireEvent.click(screen.getByRole("button", { name: "Select builtin" }));
    expect(onSelectedSkillsChange).toHaveBeenLastCalledWith(["builtin-writer"]);

    fireEvent.click(screen.getByRole("button", { name: "Select all" }));
    expect(onSelectedSkillsChange).toHaveBeenLastCalledWith([
      "builtin-writer",
      "market-reviewer",
    ]);

    fireEvent.click(screen.getByRole("button", { name: "Select none" }));
    expect(onSelectedSkillsChange).toHaveBeenLastCalledWith([]);
  });

  it("hydrates editing-mode selected skills from installed workspace skills that still exist in the pool", async () => {
    apiMocks.listSkillPoolSkills.mockResolvedValueOnce([
      { name: "builtin-writer", source: "builtin" },
      { name: "market-reviewer", source: "clawhub" },
    ]);
    apiMocks.listSkills.mockResolvedValueOnce([
      { name: "builtin-writer" },
      { name: "workspace-only" },
    ]);
    const onSelectedSkillsChange = vi.fn();
    const onInstalledSkillsLoaded = vi.fn();

    render(
      <AgentModal
        open
        editingAgent={{
          id: "default",
          name: "Default",
          description: "",
          workspace_dir: "/tmp/default",
          enabled: true,
        }}
        initialValues={{
          id: "default",
          name: "Default",
          description: "",
          workspace_dir: "/tmp/default",
          template_id: null,
        }}
        selectedSkills={[]}
        onSelectedSkillsChange={onSelectedSkillsChange}
        onInstalledSkillsLoaded={onInstalledSkillsLoaded}
        onSave={vi.fn().mockResolvedValue(undefined)}
        onClose={vi.fn()}
      />,
    );

    await screen.findByText("builtin-writer");

    expect(apiMocks.listSkills).toHaveBeenCalledWith("default");
    expect(apiMocks.listTemplates).not.toHaveBeenCalled();
    expect(onInstalledSkillsLoaded).toHaveBeenCalledWith(["builtin-writer"]);
    expect(onSelectedSkillsChange).toHaveBeenCalledWith(["builtin-writer"]);
  });

  it("keeps invalid create-mode agent ids client-side before calling Rust create routes", async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);

    render(
      <AgentModal
        open
        editingAgent={null}
        selectedSkills={[]}
        onSelectedSkillsChange={vi.fn()}
        onInstalledSkillsLoaded={vi.fn()}
        onSave={onSave}
        onClose={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("agent.idPlaceholder"), {
      target: { value: "bad id" },
    });
    fireEvent.change(screen.getByPlaceholderText("agent.namePlaceholder"), {
      target: { value: "Bad ID" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(
      await screen.findByText("Use letters, numbers, dashes, or underscores."),
    ).toBeVisible();
    expect(onSave).not.toHaveBeenCalled();
  });
});
