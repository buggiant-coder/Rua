import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MdFileInfo, SubdirFile } from "../../../../api/types";
import { useAgentsData } from "./useAgentsData";

const {
  selectedAgentRef,
  getSystemPromptFilesMock,
  listAgentFilesMock,
  getSubdirsInfoMock,
  listSubdirFilesMock,
  readAgentFileMock,
  setSystemPromptFilesMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  selectedAgentRef: { current: "agent-a" },
  getSystemPromptFilesMock: vi.fn(),
  listAgentFilesMock: vi.fn(),
  getSubdirsInfoMock: vi.fn(),
  listSubdirFilesMock: vi.fn(),
  readAgentFileMock: vi.fn(),
  setSystemPromptFilesMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
  },
  tMock: (key: string) => key,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: selectedAgentRef.current }),
}));

vi.mock("../../../../api/modules/agents", () => ({
  agentsApi: {
    getSystemPromptFiles: getSystemPromptFilesMock,
    listAgentFiles: listAgentFilesMock,
    getSubdirsInfo: getSubdirsInfoMock,
    listSubdirFiles: listSubdirFilesMock,
    readAgentFile: readAgentFileMock,
    setSystemPromptFiles: setSystemPromptFilesMock,
    readSubdirFile: vi.fn(),
    createSubdirFile: vi.fn(),
    writeSubdirFile: vi.fn(),
    writeAgentFile: vi.fn(),
  },
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
}));

function mdFile(filename: string, modified: string): MdFileInfo {
  return {
    filename,
    path: `/tmp/agent/${filename}`,
    size: 10,
    created_time: "2026-06-01T00:00:00.000Z",
    modified_time: modified,
    builtin: false,
  };
}

function subdirFile(filename: string): SubdirFile {
  return {
    filename,
    path: `/tmp/agent/AGENT_MODE/${filename}`,
    size: 5,
    created_time: "2026-06-01T00:00:00.000Z",
    modified_time: "2026-06-03T00:00:00.000Z",
    builtin: false,
    subdir: "AGENT_MODE",
    updated_at: Date.parse("2026-06-03T00:00:00.000Z"),
  };
}

describe("useAgentsData", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    selectedAgentRef.current = "agent-a";
    getSystemPromptFilesMock.mockResolvedValue(["AGENT_MODE.md"]);
    listAgentFilesMock.mockResolvedValue([
      mdFile("10_AGENT_SAFETY.md", "2026-06-02T00:00:00.000Z"),
      mdFile("AGENT_MODE.md", "2026-06-03T00:00:00.000Z"),
    ]);
    getSubdirsInfoMock.mockResolvedValue({ "AGENT_MODE.md": 1 });
    listSubdirFilesMock.mockResolvedValue([subdirFile("note.md")]);
    readAgentFileMock.mockResolvedValue({ content: "# Mode" });
    setSystemPromptFilesMock.mockResolvedValue(["AGENT_MODE.md"]);
  });

  it("initializes files, workspace path, subdir markers, and enabled prompt files", async () => {
    const { result } = renderHook(() => useAgentsData());

    await waitFor(() => expect(result.current.files.length).toBe(2));

    expect(getSystemPromptFilesMock).toHaveBeenCalledWith("agent-a");
    expect(listAgentFilesMock).toHaveBeenCalledWith("agent-a");
    expect(getSubdirsInfoMock).toHaveBeenCalledWith("agent-a");
    expect(result.current.files.map((file) => file.filename)).toEqual([
      "10_AGENT_SAFETY.md",
      "AGENT_MODE.md",
    ]);
    expect(result.current.workspacePath).toBe("/tmp/agent");
    expect(result.current.enabledFiles).toEqual(["AGENT_MODE.md"]);
    expect(Array.from(result.current.filesWithSubdirs)).toEqual([
      "AGENT_MODE.md",
    ]);
  });

  it("opens a file, expands its Rust-derived subdir, and reads content", async () => {
    const { result } = renderHook(() => useAgentsData());
    await waitFor(() => expect(result.current.files.length).toBe(2));
    const modeFile = result.current.files.find(
      (file) => file.filename === "AGENT_MODE.md",
    )!;

    await act(async () => {
      await result.current.handleFileClick(modeFile);
    });

    expect(listSubdirFilesMock).toHaveBeenCalledWith("agent-a", "AGENT_MODE");
    expect(readAgentFileMock).toHaveBeenCalledWith("agent-a", "AGENT_MODE.md");
    expect(result.current.selectedFile?.filename).toBe("AGENT_MODE.md");
    expect(result.current.fileContent).toBe("# Mode");
    expect(Array.from(result.current.expandedDirs)).toEqual(["AGENT_MODE.md"]);
  });

  it("enables an additional system prompt file through the selected agent route", async () => {
    const { result } = renderHook(() => useAgentsData());
    await waitFor(() => expect(result.current.files.length).toBe(2));

    await act(async () => {
      await result.current.handleToggleFileEnabled("10_AGENT_SAFETY.md");
    });

    expect(setSystemPromptFilesMock).toHaveBeenCalledWith("agent-a", [
      "AGENT_MODE.md",
      "10_AGENT_SAFETY.md",
    ]);
    expect(result.current.enabledFiles).toEqual([
      "AGENT_MODE.md",
      "10_AGENT_SAFETY.md",
    ]);
    expect(result.current.displayedLoadedFiles).toEqual([
      "AGENT_MODE.md",
      "10_AGENT_SAFETY.md",
    ]);
    expect(toastMock.success).toHaveBeenCalledWith("workspace.configUpdated");
  });

  it("reorders displayed prompt files while persisting only enabled files", async () => {
    const { result } = renderHook(() => useAgentsData());
    await waitFor(() => expect(result.current.files.length).toBe(2));

    await act(async () => {
      await result.current.handleReorderFiles([
        "10_AGENT_SAFETY.md",
        "AGENT_MODE.md",
      ]);
    });

    expect(setSystemPromptFilesMock).toHaveBeenCalledWith("agent-a", [
      "AGENT_MODE.md",
    ]);
    expect(result.current.enabledFiles).toEqual(["AGENT_MODE.md"]);
    expect(result.current.displayedLoadedFiles).toEqual([
      "10_AGENT_SAFETY.md",
      "AGENT_MODE.md",
    ]);
  });
});
