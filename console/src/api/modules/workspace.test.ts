import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));
vi.mock("../config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));
vi.mock("../authHeaders", () => ({
  buildAuthHeaders: vi.fn(() => ({ "X-Test": "1" })),
}));

import { request } from "../request";
import { workspaceApi } from "./workspace";

describe("workspaceApi files", () => {
  afterEach(() => vi.clearAllMocks());

  it("lists Rust agent files and derives updated_at from modified_time", async () => {
    vi.mocked(request).mockResolvedValueOnce([
      { filename: "note.md", modified_time: "2026-06-18T10:00:00Z" },
    ]);

    const result = await workspaceApi.listFiles();

    expect(request).toHaveBeenCalledWith("/agent/files");
    expect(result[0]).toMatchObject({
      filename: "note.md",
      updated_at: new Date("2026-06-18T10:00:00Z").getTime(),
    });
  });

  it("loads, saves, renames, and deletes through encoded /agent/files paths", async () => {
    vi.mocked(request).mockResolvedValue({});

    await workspaceApi.loadFile("my file.md");
    await workspaceApi.saveFile("my file.md", "# title");
    await workspaceApi.renameFile("my file.md", "next.md");
    await workspaceApi.deleteFile("subdir/my file.md", true);

    expect(request).toHaveBeenNthCalledWith(1, "/agent/files/my%20file.md");
    expect(request).toHaveBeenNthCalledWith(2, "/agent/files/my%20file.md", {
      method: "PUT",
      body: JSON.stringify({ content: "# title" }),
    });
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agent/files/my%20file.md/rename",
      {
        method: "POST",
        body: JSON.stringify({ new_name: "next.md" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agent/files/subdir%2Fmy%20file.md?include_subdir=true",
      { method: "DELETE" },
    );
  });
});

describe("workspaceApi prompt files", () => {
  afterEach(() => vi.clearAllMocks());

  it("round-trips system prompt file selection through Rust agent routes", async () => {
    vi.mocked(request).mockResolvedValueOnce(["AGENT_MODE.md"]);
    await expect(workspaceApi.getSystemPromptFiles()).resolves.toEqual([
      "AGENT_MODE.md",
    ]);
    expect(request).toHaveBeenCalledWith("/agent/system-prompt-files");

    vi.mocked(request).mockResolvedValueOnce([
      "AGENT_MODE.md",
      "AGENT_SAFETY.md",
    ]);
    await workspaceApi.setSystemPromptFiles([
      "AGENT_MODE.md",
      "AGENT_SAFETY.md",
    ]);
    expect(request).toHaveBeenLastCalledWith("/agent/system-prompt-files", {
      method: "PUT",
      body: JSON.stringify(["AGENT_MODE.md", "AGENT_SAFETY.md"]),
    });
  });
});

describe("workspaceApi zip import/export", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("downloads the Rust workspace zip route and honors response filename", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValueOnce(
        new Response("zip", {
          status: 200,
          headers: {
            "Content-Disposition":
              'attachment; filename="workspace_default.zip"',
          },
        }),
      ),
    );

    const result = await workspaceApi.downloadWorkspace();

    expect(fetch).toHaveBeenCalledWith("/api/workspace/download", {
      method: "GET",
      headers: { "X-Test": "1" },
    });
    expect(result.filename).toBe("workspace_default.zip");
    await expect(result.blob.text()).resolves.toBe("zip");
  });

  it("uploads a zip through /workspace/upload and surfaces HTTP failures", async () => {
    const file = new File(["bad"], "bad.zip", { type: "application/zip" });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValueOnce(
        new Response("invalid zip", {
          status: 422,
          statusText: "Unprocessable Entity",
        }),
      ),
    );

    await expect(workspaceApi.uploadFile(file)).rejects.toThrow(
      "Upload failed: 422 Unprocessable Entity - invalid zip",
    );

    expect(fetch).toHaveBeenCalledWith(
      "/api/workspace/upload",
      expect.objectContaining({
        method: "POST",
        headers: { "X-Test": "1" },
      }),
    );
  });
});
