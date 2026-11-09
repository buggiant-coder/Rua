import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactElement, ReactNode } from "react";
import { forwardRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "../../../../api/types/agents";
import type { SubAgentTemplate } from "../../../../api/types/subagentTemplates";
import { SubAgentCardGrid } from "./SubAgentCardGrid";
import { SubAgentModal } from "./SubAgentModal";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agent.delete": "Delete",
        "agent.edit": "Edit",
        "common.cancel": "Cancel",
        "common.confirm": "Confirm",
        "common.save": "Save",
        "subAgent.builtin": "Builtin",
        "subAgent.builtinNotDeletable": "Builtin templates cannot be deleted",
        "subAgent.createTitle": "Create sub-agent",
        "subAgent.deleteConfirm": "Delete sub-agent?",
        "subAgent.description": "Description",
        "subAgent.descriptionPlaceholder": "Describe this sub-agent",
        "subAgent.editTitle": "Edit sub-agent",
        "subAgent.emptyText": "No sub-agents",
        "subAgent.name": "Name",
        "subAgent.namePattern": "Invalid name",
        "subAgent.namePlaceholder": "sub-agent-name",
        "subAgent.nameRequired": "Name is required",
        "subAgent.systemPrompt": "System prompt",
        "subAgent.systemPromptPlaceholder": "Prompt",
        "subAgent.systemPromptRequired": "System prompt is required",
        "subAgent.tools": "Tools",
        "subAgent.toolsPlaceholder": "Select tools",
        "subAgent.visibleTo": "Visible to",
        "subAgent.visibleToAll": "All agents",
        "subAgent.visibleToHelp": "Leave empty for all agents.",
      })[key] ?? key,
  }),
}));

vi.mock("lucide-react", () => ({
  Puzzle: () => <svg aria-hidden="true" />,
}));

vi.mock("@/components/RuaCard", () => ({
  RuaCard: ({
    actions,
    children,
    footer,
    onClick,
    selected,
    tags,
    title,
  }: {
    actions?: ReactNode[];
    children?: ReactNode;
    footer?: ReactNode;
    onClick?: () => void;
    selected?: boolean;
    tags?: ReactNode[];
    title?: ReactNode;
  }) => (
    <article data-selected={selected ? "true" : "false"} onClick={onClick}>
      <h3>{title}</h3>
      {tags}
      <div onClick={(event) => event.stopPropagation()}>{actions}</div>
      {children}
      {footer}
    </article>
  ),
}));

vi.mock("@/components/EmptyState", () => ({
  EmptyState: ({ description }: { description?: ReactNode }) => (
    <p>{description}</p>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    onClick,
  }: {
    children?: ReactNode;
    disabled?: boolean;
    onClick?: (event: { stopPropagation: () => void }) => void;
  }) => (
    <button
      disabled={disabled}
      type="button"
      onClick={(event) =>
        onClick?.({
          stopPropagation: () => event.stopPropagation(),
        })
      }
    >
      {children}
    </button>
  ),
  Check: ({
    checked,
    disabled,
    onChange,
  }: {
    checked?: boolean;
    disabled?: boolean;
    onChange?: () => void;
  }) => (
    <input
      aria-label="selected"
      checked={checked}
      disabled={disabled}
      onChange={onChange}
      readOnly
      type="checkbox"
    />
  ),
  FormField: ({
    children,
    error,
    label,
  }: {
    children?: ReactNode;
    error?: ReactNode;
    label?: ReactNode;
  }) => (
    <label>
      <span>{label}</span>
      {children}
      {error ? <strong>{error}</strong> : null}
    </label>
  ),
  Modal: ({
    children,
    okText,
    onClose,
    onOk,
    open,
    title,
  }: {
    children?: ReactNode;
    okText?: ReactNode;
    onClose?: () => void;
    onOk?: () => void;
    open?: boolean;
    title?: ReactNode;
  }) =>
    open ? (
      <div role="dialog" aria-label={String(title)}>
        <h2>{title}</h2>
        {children}
        <button type="button" onClick={onClose}>
          Cancel
        </button>
        <button type="button" onClick={onOk}>
          {okText ?? "Save"}
        </button>
      </div>
    ) : null,
  Popconfirm: ({
    children,
    onConfirm,
    title,
  }: {
    children?: ReactNode;
    onConfirm?: () => void;
    title?: ReactNode;
  }) => (
    <span>
      {children}
      <button type="button" onClick={onConfirm}>
        {title}
      </button>
    </span>
  ),
  Select: ({
    mode,
    onChange,
    options,
    value,
  }: {
    mode?: string;
    onChange?: (value: string[] | string) => void;
    options?: Array<{ label: ReactNode; value: string }>;
    value?: string[] | string;
  }) => {
    const selected = Array.isArray(value) ? value : value ? [value] : [];
    return (
      <div>
        {(options ?? []).map((option) => (
          <button
            aria-pressed={selected.includes(option.value)}
            key={option.value}
            type="button"
            onClick={() => {
              if (mode === "multiple") {
                onChange?.(
                  selected.includes(option.value)
                    ? selected.filter((item) => item !== option.value)
                    : [...selected, option.value],
                );
              } else {
                onChange?.(option.value);
              }
            }}
          >
            {option.label}
          </button>
        ))}
      </div>
    );
  },
  StackedList: ({ children }: { children?: ReactNode }) => (
    <div>{children}</div>
  ),
  Tag: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  TextArea: forwardRef<
    HTMLTextAreaElement,
    React.TextareaHTMLAttributes<HTMLTextAreaElement>
  >((props, ref) => <textarea ref={ref} {...props} />),
  TextInput: forwardRef<
    HTMLInputElement,
    React.InputHTMLAttributes<HTMLInputElement>
  >((props, ref) => <input ref={ref} {...props} />),
  Tooltip: ({ children }: { children?: ReactElement }) => children,
}));

function agent(overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    id: "default",
    name: "Default",
    description: "",
    workspace_dir: "/tmp/default",
    enabled: true,
    ...overrides,
  };
}

function template(overrides: Partial<SubAgentTemplate> = {}): SubAgentTemplate {
  return {
    id: "researcher",
    name: "researcher",
    description: "Research helper",
    system_prompt: "Research carefully",
    tools: ["Bash", "Read"],
    visible_to: ["default"],
    builtin: false,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

describe("SubAgentModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("creates Rust sub-agent templates with canonical tool names and visibility", async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);

    render(
      <SubAgentModal
        agents={[agent({ id: "coder", name: "Coder" })]}
        editingTemplate={null}
        onClose={vi.fn()}
        onSave={onSave}
        open
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("sub-agent-name"), {
      target: { value: "writer" },
    });
    fireEvent.change(screen.getByPlaceholderText("Describe this sub-agent"), {
      target: { value: "Writing helper" },
    });
    fireEvent.change(screen.getByPlaceholderText("Prompt"), {
      target: { value: "Write concise responses" },
    });
    fireEvent.click(screen.getByText("Bash"));
    fireEvent.click(screen.getByText("Read"));
    fireEvent.click(screen.getByText("Coder"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSave).toHaveBeenCalledWith({
        name: "writer",
        description: "Writing helper",
        system_prompt: "Write concise responses",
        tools: ["Bash", "Read"],
        visible_to: ["coder"],
      });
    });
  });

  it("hydrates edit mode and keeps sub-agent ids immutable in the form", async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);

    render(
      <SubAgentModal
        agents={[agent({ id: "default", name: "Default" })]}
        editingTemplate={template()}
        onClose={vi.fn()}
        onSave={onSave}
        open
      />,
    );

    expect(screen.getByPlaceholderText("sub-agent-name")).toBeDisabled();
    fireEvent.change(screen.getByPlaceholderText("Describe this sub-agent"), {
      target: { value: "Updated helper" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSave).toHaveBeenCalledWith({
        name: "researcher",
        description: "Updated helper",
        system_prompt: "Research carefully",
        tools: ["Bash", "Read"],
        visible_to: ["default"],
      });
    });
  });
});

describe("SubAgentCardGrid", () => {
  it("renders visibility labels and delegates edit/delete actions", () => {
    const onEdit = vi.fn();
    const onDelete = vi.fn();

    render(
      <SubAgentCardGrid
        agents={[
          agent({ id: "default", name: "Default" }),
          agent({ id: "coder", name: "Coder" }),
        ]}
        loading={false}
        onDelete={onDelete}
        onEdit={onEdit}
        templates={[
          template({ id: "researcher", visible_to: ["coder"] }),
          template({
            id: "builtin",
            name: "builtin",
            builtin: true,
            visible_to: [],
          }),
        ]}
      />,
    );

    expect(screen.getByText("Coder")).toBeVisible();
    expect(screen.getByText("All agents")).toBeVisible();

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    expect(onEdit).toHaveBeenCalledWith(
      expect.objectContaining({ id: "researcher" }),
    );

    fireEvent.click(
      screen.getAllByRole("button", { name: "Delete sub-agent?" })[0],
    );
    expect(onDelete).toHaveBeenCalledWith("researcher");
  });

  it("uses batch-select mode while keeping builtin templates unselectable", () => {
    const onSelectToggle = vi.fn();

    render(
      <SubAgentCardGrid
        agents={[agent()]}
        loading={false}
        onDelete={vi.fn()}
        onEdit={vi.fn()}
        onSelectToggle={onSelectToggle}
        selectMode
        selectedIds={new Set(["researcher"])}
        templates={[
          template({ id: "researcher" }),
          template({ id: "builtin", name: "builtin", builtin: true }),
        ]}
      />,
    );

    const checks = screen.getAllByLabelText("selected");
    expect(checks[0]).toBeChecked();
    expect(checks[1]).toBeDisabled();

    fireEvent.click(screen.getByText("researcher"));
    expect(onSelectToggle).toHaveBeenCalledWith("researcher");

    fireEvent.click(checks[1]);
    expect(onSelectToggle).toHaveBeenCalledTimes(1);
  });

  it("renders an empty state when there are no sub-agent templates", () => {
    render(
      <SubAgentCardGrid
        agents={[]}
        loading={false}
        onDelete={vi.fn()}
        onEdit={vi.fn()}
        templates={[]}
      />,
    );

    expect(screen.getByText("No sub-agents")).toBeVisible();
  });
});
