import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { promptLibraryApi } from "./promptLibrary";

describe("promptLibraryApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("lists prompt-library files and derives updated_at from modified_time", async () => {
    vi.mocked(request).mockResolvedValueOnce([
      { filename: "prompt.md", modified_time: "2026-06-18T00:00:00Z" },
    ]);

    const files = await promptLibraryApi.listFiles();

    expect(request).toHaveBeenCalledWith("/prompt-library/files");
    expect(files[0]).toMatchObject({
      filename: "prompt.md",
      updated_at: new Date("2026-06-18T00:00:00Z").getTime(),
    });
  });

  it("uses encoded Rust prompt-library file and subdir routes", async () => {
    await promptLibraryApi.readFile("my prompt.md");
    await promptLibraryApi.saveFile("my prompt.md", "# prompt");
    await promptLibraryApi.renameFile("my prompt.md", "next.md");
    await promptLibraryApi.deleteFile("folder/my prompt.md", true);
    await promptLibraryApi.getSubdirsInfo();
    await promptLibraryApi.createSubdirFile("folder/name", "NEW.md");
    await promptLibraryApi.readSubdirFile("folder/name", "file.md");
    await promptLibraryApi.writeSubdirFile("folder/name", "file.md", "body");
    await promptLibraryApi.renameSubdirFile("folder/name", "file.md", "n.md");
    await promptLibraryApi.deleteSubdirFile("folder/name", "file.md");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/prompt-library/files/my%20prompt.md",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/prompt-library/files/my%20prompt.md",
      {
        method: "PUT",
        body: JSON.stringify({ content: "# prompt" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/prompt-library/files/my%20prompt.md/rename",
      {
        method: "POST",
        body: JSON.stringify({ new_name: "next.md" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/prompt-library/files/folder%2Fmy%20prompt.md?include_subdir=true",
      { method: "DELETE" },
    );
    expect(request).toHaveBeenNthCalledWith(5, "/prompt-library/subdirs-info");
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/prompt-library/subdirs/folder%2Fname",
      {
        method: "POST",
        body: JSON.stringify({ filename: "NEW.md" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      7,
      "/prompt-library/subdirs/folder%2Fname/file.md",
    );
    expect(request).toHaveBeenNthCalledWith(
      8,
      "/prompt-library/subdirs/folder%2Fname/file.md",
      {
        method: "PUT",
        body: JSON.stringify({ content: "body" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      9,
      "/prompt-library/subdirs/folder%2Fname/file.md/rename",
      {
        method: "POST",
        body: JSON.stringify({ new_name: "n.md" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      10,
      "/prompt-library/subdirs/folder%2Fname/file.md",
      { method: "DELETE" },
    );
  });

  it("uses Rust prompt-library sync, broadcast, and restore routes", async () => {
    await promptLibraryApi.broadcast(["a.md"], ["agent-1"], "overwrite");
    await promptLibraryApi.sync("agent-1", ["a.md"], "skip");
    await promptLibraryApi.restoreBuiltin();

    expect(request).toHaveBeenNthCalledWith(1, "/prompt-library/broadcast", {
      method: "POST",
      body: JSON.stringify({
        files: ["a.md"],
        agent_ids: ["agent-1"],
        conflict_mode: "overwrite",
      }),
    });
    expect(request).toHaveBeenNthCalledWith(2, "/prompt-library/sync", {
      method: "POST",
      body: JSON.stringify({
        agent_id: "agent-1",
        files: ["a.md"],
        conflict_mode: "skip",
      }),
    });
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/prompt-library/restore-builtin",
      { method: "POST" },
    );
  });
});
