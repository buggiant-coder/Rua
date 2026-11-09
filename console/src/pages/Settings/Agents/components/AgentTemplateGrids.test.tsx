import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "../../../../api/types/agents";
import type { AgentTemplate } from "@/api/types/agentTemplates";
import { AgentCardGrid } from "./AgentCardGrid";
import { TemplateCardGrid } from "./TemplateCardGrid";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agent.defaultNotDeletable": "Default agent cannot be deleted",
        "agent.delete": "Delete",
        "agent.deleteConfirm": "Delete agent?",
        "agent.disabled": "Disabled",
        "agent.disable": "Disable",
        "agent.disableConfirm": "Disable agent?",
        "agent.dragHandleTooltip": "Drag agent",
        "agent.edit": "Edit",
        "agent.emptyText": "No agents",
        "agent.enable": "Enable",
        "agent.enableConfirm": "Enable agent?",
        "common.cancel": "Cancel",
        "common.confirm": "Confirm",
        "template.builtin": "Builtin",
        "template.builtinNotDeletable": "Builtin templates cannot be deleted",
        "template.deleteConfirm": "Delete template?",
        "template.emptyText": "No templates",
        "template.promptFiles": "prompt files",
      })[key] ?? key,
  }),
}));

vi.mock("lucide-react", () => ({
  Bot: () => <svg aria-hidden="true" />,
  GripVertical: () => <svg aria-hidden="true" />,
  LayoutTemplate: () => <svg aria-hidden="true" />,
}));

vi.mock("@dnd-kit/core", () => ({
  DndContext: ({ children }: { children?: ReactNode }) => <>{children}</>,
  PointerSensor: vi.fn(),
  closestCenter: vi.fn(),
  useSensor: vi.fn(() => ({})),
  useSensors: vi.fn(() => []),
}));

vi.mock("@dnd-kit/sortable", () => ({
  SortableContext: ({ children }: { children?: ReactNode }) => <>{children}</>,
  verticalListSortingStrategy: {},
  useSortable: () => ({
    attributes: {},
    listeners: {},
    setNodeRef: vi.fn(),
    transform: null,
    transition: undefined,
    isDragging: false,
  }),
}));

vi.mock("@dnd-kit/utilities", () => ({
  CSS: { Transform: { toString: () => "" } },
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
  Popconfirm: ({
    children,
    disabled,
    onConfirm,
    title,
  }: {
    children?: ReactNode;
    disabled?: boolean;
    onConfirm?: () => void;
    title?: ReactNode;
  }) => (
    <span>
      {children}
      <button disabled={disabled} type="button" onClick={onConfirm}>
        {title}
      </button>
    </span>
  ),
  StackedList: ({ children }: { children?: ReactNode }) => (
    <div>{children}</div>
  ),
  Tag: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  Tooltip: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));

function agent(overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    id: "default",
    name: "Default",
    description: "Default agent",
    workspace_dir: "/tmp/default",
    enabled: true,
    ...overrides,
  };
}

function template(overrides: Partial<AgentTemplate> = {}): AgentTemplate {
  return {
    id: "default",
    name: "Default template",
    description: "Template description",
    prompt_files: ["AGENT_MODE.md"],
    prompt_file_count: 1,
    builtin: true,
    files_status: [],
    ...overrides,
  };
}

describe("AgentCardGrid", () => {
  beforeEach(() => vi.clearAllMocks());

  it("delegates edit, toggle, and delete actions for Rust agents", () => {
    const onEdit = vi.fn();
    const onToggle = vi.fn();
    const onDelete = vi.fn();

    render(
      <AgentCardGrid
        agents={[
          agent(),
          agent({
            id: "research",
            name: "Research",
            enabled: false,
            workspace_dir: "/tmp/research",
          }),
        ]}
        loading={false}
        onDelete={onDelete}
        onEdit={onEdit}
        onReorder={vi.fn()}
        onToggle={onToggle}
        reordering={false}
      />,
    );

    expect(screen.getByText("Disabled")).toBeVisible();
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[1]);
    expect(onEdit).toHaveBeenCalledWith(
      expect.objectContaining({ id: "research" }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Enable agent?" }));
    expect(onToggle).toHaveBeenCalledWith("research", false);

    fireEvent.click(
      screen.getAllByRole("button", { name: "Delete agent?" })[1],
    );
    expect(onDelete).toHaveBeenCalledWith("research");
  });

  it("keeps the default agent unselectable in batch-delete mode", () => {
    const onSelectToggle = vi.fn();

    render(
      <AgentCardGrid
        agents={[agent(), agent({ id: "research", name: "Research" })]}
        loading={false}
        onDelete={vi.fn()}
        onEdit={vi.fn()}
        onReorder={vi.fn()}
        onSelectToggle={onSelectToggle}
        onToggle={vi.fn()}
        reordering={false}
        selectMode
        selectedIds={new Set(["research"])}
      />,
    );

    const checks = screen.getAllByLabelText("selected");
    expect(checks[0]).toBeDisabled();
    expect(checks[1]).toBeChecked();

    fireEvent.click(screen.getByText("Default"));
    expect(onSelectToggle).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText("Research"));
    expect(onSelectToggle).toHaveBeenCalledWith("research");
  });

  it("renders an empty agent state", () => {
    render(
      <AgentCardGrid
        agents={[]}
        loading={false}
        onDelete={vi.fn()}
        onEdit={vi.fn()}
        onReorder={vi.fn()}
        onToggle={vi.fn()}
        reordering={false}
      />,
    );

    expect(screen.getByText("No agents")).toBeVisible();
  });
});

describe("TemplateCardGrid", () => {
  it("delegates template edit and custom-template delete", () => {
    const onEdit = vi.fn();
    const onDelete = vi.fn();

    render(
      <TemplateCardGrid
        loading={false}
        onDelete={onDelete}
        onEdit={onEdit}
        templates={[
          template(),
          template({
            id: "qa",
            name: "QA",
            builtin: false,
            prompt_file_count: 2,
          }),
        ]}
      />,
    );

    expect(screen.getByText("Builtin")).toBeVisible();
    expect(screen.getByText("2 prompt files")).toBeVisible();

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[1]);
    expect(onEdit).toHaveBeenCalledWith(expect.objectContaining({ id: "qa" }));

    fireEvent.click(
      screen.getAllByRole("button", { name: "Delete template?" })[1],
    );
    expect(onDelete).toHaveBeenCalledWith("qa");
  });

  it("keeps builtin templates unselectable in batch-delete mode", () => {
    const onSelectToggle = vi.fn();

    render(
      <TemplateCardGrid
        loading={false}
        onDelete={vi.fn()}
        onEdit={vi.fn()}
        onSelectToggle={onSelectToggle}
        selectMode
        selectedIds={new Set(["qa"])}
        templates={[
          template(),
          template({ id: "qa", name: "QA", builtin: false }),
        ]}
      />,
    );

    const checks = screen.getAllByLabelText("selected");
    expect(checks[0]).toBeDisabled();
    expect(checks[1]).toBeChecked();

    fireEvent.click(screen.getByText("Default template"));
    expect(onSelectToggle).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText("QA"));
    expect(onSelectToggle).toHaveBeenCalledWith("qa");
  });
});
