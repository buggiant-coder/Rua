import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TemplateModal } from "./TemplateModal";
import type { AgentTemplate } from "@/api/types/agentTemplates";

const { requestMock, tMock } = vi.hoisted(() => ({
  requestMock: vi.fn(),
  tMock: vi.fn((key: string) => key),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("@/api/request", () => ({
  request: requestMock,
}));

vi.mock("@dnd-kit/core", () => {
  return {
    DndContext: ({ children }: { children: ReactNode }) => <>{children}</>,
    closestCenter: vi.fn(),
    PointerSensor: vi.fn(),
    useSensor: vi.fn(() => ({})),
    useSensors: vi.fn(() => []),
  };
});

vi.mock("@dnd-kit/sortable", () => {
  return {
    SortableContext: ({ children }: { children: ReactNode }) => <>{children}</>,
    verticalListSortingStrategy: {},
    arrayMove: <T,>(items: T[], oldIndex: number, newIndex: number): T[] => {
      const next = [...items];
      const [item] = next.splice(oldIndex, 1);
      next.splice(newIndex, 0, item);
      return next;
    },
    useSortable: () => ({
      attributes: {},
      listeners: {},
      setNodeRef: vi.fn(),
      transform: null,
      transition: undefined,
      isDragging: false,
    }),
  };
});

vi.mock("@dnd-kit/utilities", () => ({
  CSS: { Transform: { toString: () => "" } },
}));

function template(overrides: Partial<AgentTemplate> = {}): AgentTemplate {
  return {
    id: "qa",
    name: "QA",
    description: "QA template",
    prompt_files: ["AGENT_MODE.md", "AGENT_MODE/zh.md"],
    builtin: false,
    prompt_file_count: 2,
    ...overrides,
  };
}

describe("TemplateModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    requestMock.mockImplementation((path: string) => {
      if (path === "/prompt-library/files") {
        return Promise.resolve([
          {
            filename: "AGENT_zOther.md",
            size: 512,
            modified_time: "2026-01-03T00:00:00Z",
          },
          {
            filename: "AGENT_MODE.md",
            size: 2048,
            modified_time: "2026-01-02T00:00:00Z",
          },
          {
            filename: "AGENT_SAFETY.md",
            size: 1024,
            modified_time: "2026-01-01T00:00:00Z",
          },
        ]);
      }
      if (path === "/prompt-library/subdirs-info") {
        return Promise.resolve({ "AGENT_MODE.md": 1 });
      }
      if (path === "/prompt-library/subdirs/AGENT_MODE") {
        return Promise.resolve([
          {
            filename: "zh.md",
            size: 256,
            modified_time: "2026-01-04T00:00:00Z",
          },
        ]);
      }
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });
  });

  it("creates templates from Rust prompt-library files and subdir prompt keys", async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);

    render(
      <TemplateModal
        open
        editingTemplate={null}
        onSave={onSave}
        onClose={vi.fn()}
      />,
    );

    await screen.findByText("AGENT_SAFETY");
    expect(requestMock).toHaveBeenCalledWith("/prompt-library/files");
    expect(requestMock).toHaveBeenCalledWith("/prompt-library/subdirs-info");

    expect(screen.getByText("AGENT_MODE")).toBeVisible();
    expect(screen.getByText("AGENT_SAFETY")).toBeVisible();

    fireEvent.change(screen.getByPlaceholderText("template.namePlaceholder"), {
      target: { value: "Research" },
    });
    fireEvent.change(
      screen.getByPlaceholderText("template.descriptionPlaceholder"),
      {
        target: { value: "Research template" },
      },
    );

    fireEvent.click(screen.getAllByRole("checkbox")[1]);
    fireEvent.click(screen.getByText("AGENT_MODE"));

    await screen.findByText("zh");
    expect(requestMock).toHaveBeenCalledWith(
      "/prompt-library/subdirs/AGENT_MODE",
    );

    fireEvent.click(screen.getAllByRole("checkbox")[2]);
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));

    await waitFor(() => {
      expect(onSave).toHaveBeenCalledWith({
        name: "Research",
        description: "Research template",
        prompt_files: ["AGENT_SAFETY.md", "AGENT_MODE/zh.md"],
      });
    });
  });

  it("hydrates edit mode from the selected Rust template without fetching language directories", async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);

    render(
      <TemplateModal
        open
        editingTemplate={template()}
        onSave={onSave}
        onClose={vi.fn()}
      />,
    );

    await screen.findByDisplayValue("QA");
    expect(screen.getByDisplayValue("QA template")).toBeInTheDocument();
    expect(
      screen
        .getAllByRole("checkbox")
        .filter((input) => (input as HTMLInputElement).checked),
    ).toHaveLength(2);
    expect(screen.getByText("AGENT_MODE")).toBeInTheDocument();
    expect(screen.getByText("AGENT_MODE/zh")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "common.save" }));

    await waitFor(() => {
      expect(onSave).toHaveBeenCalledWith({
        name: "QA",
        description: "QA template",
        prompt_files: ["AGENT_MODE.md", "AGENT_MODE/zh.md"],
      });
    });
    expect(requestMock).not.toHaveBeenCalledWith(
      expect.stringContaining("/md_files/"),
    );
  });
});
