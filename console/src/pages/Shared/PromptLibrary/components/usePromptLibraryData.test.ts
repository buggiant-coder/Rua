import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MdFileInfo, SubdirFile } from "../../../../api/types";
import { usePromptLibraryData } from "./usePromptLibraryData";

const {
  listFilesMock,
  getSubdirsInfoMock,
  listSubdirFilesMock,
  readFileMock,
  saveFileMock,
  restoreBuiltinMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  listFilesMock: vi.fn(),
  getSubdirsInfoMock: vi.fn(),
  listSubdirFilesMock: vi.fn(),
  readFileMock: vi.fn(),
  saveFileMock: vi.fn(),
  restoreBuiltinMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
  },
  tMock: (key: string, params?: Record<string, unknown>) =>
    params ? `${key}:${JSON.stringify(params)}` : key,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../../api/modules/promptLibrary", () => ({
  promptLibraryApi: {
    listFiles: listFilesMock,
    getSubdirsInfo: getSubdirsInfoMock,
    listSubdirFiles: listSubdirFilesMock,
    readFile: readFileMock,
    saveFile: saveFileMock,
    restoreBuiltin: restoreBuiltinMock,
    readSubdirFile: vi.fn(),
    createSubdirFile: vi.fn(),
    writeSubdirFile: vi.fn(),
  },
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
}));

function promptFile(filename: string, modified: string): MdFileInfo {
  return {
    filename,
    path: `/tmp/prompts/${filename}`,
    size: 10,
    created_time: "2026-06-01T00:00:00.000Z",
    modified_time: modified,
    builtin: false,
  };
}

function subdirFile(filename: string): SubdirFile {
  return {
    filename,
    path: `/tmp/prompts/WRITING/${filename}`,
    size: 5,
    created_time: "2026-06-01T00:00:00.000Z",
    modified_time: "2026-06-03T00:00:00.000Z",
    builtin: false,
    subdir: "WRITING",
    updated_at: Date.parse("2026-06-03T00:00:00.000Z"),
  };
}

describe("usePromptLibraryData", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listFilesMock.mockResolvedValue([
      promptFile("summary.md", "2026-06-02T00:00:00.000Z"),
      promptFile("writing.md", "2026-06-03T00:00:00.000Z"),
    ]);
    getSubdirsInfoMock.mockResolvedValue({ "writing.md": 1 });
    listSubdirFilesMock.mockResolvedValue([subdirFile("tone.md")]);
    readFileMock.mockResolvedValue({ content: "# Writing" });
    saveFileMock.mockResolvedValue({ written: true });
    restoreBuiltinMock.mockResolvedValue({
      restored: ["summary.md"],
      count: 1,
    });
  });

  it("initializes Rust prompt-library files and subdir markers", async () => {
    const { result } = renderHook(() => usePromptLibraryData());

    await waitFor(() => expect(result.current.files.length).toBe(2));

    expect(listFilesMock).toHaveBeenCalledTimes(1);
    expect(getSubdirsInfoMock).toHaveBeenCalledTimes(1);
    expect(result.current.files.map((file) => file.filename)).toEqual([
      "summary.md",
      "writing.md",
    ]);
    expect(result.current.libraryPath).toBe("/tmp/prompts");
    expect(Array.from(result.current.filesWithSubdirs)).toEqual(["writing.md"]);
  });

  it("opens a prompt file and auto-expands its Rust-derived subdir", async () => {
    const { result } = renderHook(() => usePromptLibraryData());
    await waitFor(() => expect(result.current.files.length).toBe(2));
    const writing = result.current.files.find(
      (file) => file.filename === "writing.md",
    )!;

    await act(async () => {
      await result.current.handleFileClick(writing);
    });

    expect(listSubdirFilesMock).toHaveBeenCalledWith("WRITING");
    expect(readFileMock).toHaveBeenCalledWith("writing.md");
    expect(result.current.selectedFile?.filename).toBe("writing.md");
    expect(result.current.fileContent).toBe("# Writing");
    expect(Array.from(result.current.expandedDirs)).toEqual(["writing.md"]);
  });

  it("creates a new prompt-library file only when saved", async () => {
    const { result } = renderHook(() => usePromptLibraryData());
    await waitFor(() => expect(result.current.files.length).toBe(2));

    act(() => {
      result.current.startCreateFile();
      result.current.commitNewFileName("notes");
      result.current.setFileContent("# Notes");
    });

    expect(result.current.selectedFile?.filename).toBe("notes.md");
    expect(saveFileMock).not.toHaveBeenCalled();

    await act(async () => {
      await result.current.handleSave();
    });

    expect(saveFileMock).toHaveBeenCalledWith("notes.md", "# Notes");
    expect(result.current.isCreatingNew).toBe(false);
    expect(toastMock.success).toHaveBeenCalledWith("workspace.createSuccess");
  });

  it("restores builtin prompt-library files and refreshes the list", async () => {
    const { result } = renderHook(() => usePromptLibraryData());
    await waitFor(() => expect(result.current.files.length).toBe(2));

    let restored: string[] | null = null;
    await act(async () => {
      restored = await result.current.restoreBuiltin();
    });

    expect(restored).toEqual(["summary.md"]);
    expect(restoreBuiltinMock).toHaveBeenCalledTimes(1);
    expect(listFilesMock).toHaveBeenCalledTimes(2);
    expect(toastMock.success).toHaveBeenCalledWith(
      'promptLibrary.restoreBuiltinSuccess:{"count":1}',
    );
  });
});
