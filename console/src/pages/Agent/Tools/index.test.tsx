import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ToolsPage from "./index";

const { handleToggle } = vi.hoisted(() => ({ handleToggle: vi.fn() }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  }),
}));

vi.mock("./useTools", () => ({
  useTools: () => ({
    tools: (
      [
        ["read", "file"],
        ["chat_send", "chat"],
        ["edit", "file"],
        ["python", "shell"],
        ["bash", "shell"],
      ] as const
    ).map(([name, display_group]) => ({
      name,
      description: `Description for ${name}`,
      enabled: true,
      async_execution: false,
      display_group,
    })),
    loading: false,
    batchLoading: false,
    toggleEnabled: handleToggle,
    enableAll: vi.fn(),
    disableAll: vi.fn(),
    setTools: vi.fn(),
  }),
}));

vi.mock("./WebSearchConfigModal", () => ({ WebSearchConfigModal: () => null }));

describe("ToolsPage groups", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
  });

  it("renders named groups with counts and keeps tool toggles targeted", () => {
    render(<ToolsPage />);
    const files = screen.getByRole("region", { name: "File" });
    expect(within(files).getByText("2")).toBeInTheDocument();
    expect(within(files).getByText("read")).toBeInTheDocument();
    const execution = screen.getByRole("region", { name: "Shell" });
    expect(within(execution).getByText("python")).toBeInTheDocument();
    expect(within(execution).getByText("bash")).toBeInTheDocument();
    fireEvent.click(
      within(execution).getAllByRole("button", { name: "common.disable" })[1],
    );
    expect(handleToggle).toHaveBeenCalledWith(
      expect.objectContaining({ name: "bash" }),
    );
  });

  it("collapses groups independently and preserves state across search", () => {
    render(<ToolsPage />);
    const shellToggle = screen.getByRole("button", { name: "Shell" });
    expect(shellToggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(shellToggle);
    expect(shellToggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText("bash")).not.toBeVisible();
    expect(screen.getByText("read")).toBeVisible();
    const search = screen.getByPlaceholderText("tools.searchPlaceholder");
    fireEvent.change(search, { target: { value: "file_read" } });
    fireEvent.change(search, { target: { value: "" } });
    expect(screen.getByRole("button", { name: "Shell" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    fireEvent.click(screen.getByRole("button", { name: "Shell" }));
    expect(screen.getByText("bash")).toBeVisible();
  });

  it("searches across groups, updates counts, and omits empty groups", () => {
    render(<ToolsPage />);
    const search = screen.getByPlaceholderText("tools.searchPlaceholder");
    fireEvent.change(search, { target: { value: "read" } });
    expect(screen.getAllByRole("region")).toHaveLength(1);
    expect(
      within(screen.getByRole("region", { name: "File" })).getByText("1"),
    ).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "no-such-tool" } });
    expect(screen.queryByRole("region")).not.toBeInTheDocument();
    expect(screen.getByText("tools.noMatch")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "" } });
    expect(screen.getAllByRole("region")).toHaveLength(3);
  });

  it("keeps the selected descending name order within groups", () => {
    localStorage.setItem("rua.tools.sortField", "name");
    localStorage.setItem("rua.tools.sortOrder", "desc");
    render(<ToolsPage />);
    // Group order follows the insertion order of the sorted tool list:
    // desc(read, python, edit, chat_send, bash) → File (read, edit), Shell (python, bash), Chat (chat_send).
    expect(
      screen
        .getAllByRole("heading", { level: 2 })
        .map((heading) => heading.textContent),
    ).toEqual(["File2", "Shell2", "Chat1"]);
    const execution = screen.getByRole("region", { name: "Shell" });
    fireEvent.click(
      within(execution).getAllByRole("button", { name: "common.disable" })[0],
    );
    expect(handleToggle).toHaveBeenCalledWith(
      expect.objectContaining({ name: "python" }),
    );
  });
});
