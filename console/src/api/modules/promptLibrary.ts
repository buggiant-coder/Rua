import { request } from "../request";
import type { MdFileInfo, MdFileContent, SubdirFile } from "../types";

export const promptLibraryApi = {
  listFiles: () =>
    request<MdFileInfo[]>("/prompt-library/files").then((files) =>
      files.map((file) => ({
        ...file,
        updated_at: new Date(file.modified_time).getTime(),
      })),
    ),

  readFile: (fileName: string) =>
    request<MdFileContent>(
      `/prompt-library/files/${encodeURIComponent(fileName)}`,
    ),

  saveFile: (fileName: string, content: string) =>
    request<Record<string, unknown>>(
      `/prompt-library/files/${encodeURIComponent(fileName)}`,
      { method: "PUT", body: JSON.stringify({ content }) },
    ),

  deleteFile: (fileName: string, includeSubdir = false) =>
    request<Record<string, unknown>>(
      `/prompt-library/files/${encodeURIComponent(fileName)}${
        includeSubdir ? "?include_subdir=true" : ""
      }`,
      { method: "DELETE" },
    ),

  renameFile: (oldName: string, newName: string) =>
    request<Record<string, unknown>>(
      `/prompt-library/files/${encodeURIComponent(oldName)}/rename`,
      { method: "POST", body: JSON.stringify({ new_name: newName }) },
    ),

  getSubdirsInfo: () =>
    request<Record<string, number>>("/prompt-library/subdirs-info"),

  listSubdirFiles: (subdir: string) =>
    request<MdFileInfo[]>(
      `/prompt-library/subdirs/${encodeURIComponent(subdir)}`,
    ).then(
      (files) =>
        files.map((file) => ({
          ...file,
          subdir,
          updated_at: new Date(file.modified_time).getTime(),
        })) as SubdirFile[],
    ),

  createSubdirFile: (subdir: string, filename = "NEW.md") =>
    request<MdFileInfo>(
      `/prompt-library/subdirs/${encodeURIComponent(subdir)}`,
      { method: "POST", body: JSON.stringify({ filename }) },
    ),

  readSubdirFile: (subdir: string, filename: string) =>
    request<MdFileContent>(
      `/prompt-library/subdirs/${encodeURIComponent(
        subdir,
      )}/${encodeURIComponent(filename)}`,
    ),

  writeSubdirFile: (subdir: string, filename: string, content: string) =>
    request<Record<string, unknown>>(
      `/prompt-library/subdirs/${encodeURIComponent(
        subdir,
      )}/${encodeURIComponent(filename)}`,
      { method: "PUT", body: JSON.stringify({ content }) },
    ),

  renameSubdirFile: (subdir: string, oldName: string, newName: string) =>
    request<Record<string, unknown>>(
      `/prompt-library/subdirs/${encodeURIComponent(
        subdir,
      )}/${encodeURIComponent(oldName)}/rename`,
      { method: "POST", body: JSON.stringify({ new_name: newName }) },
    ),

  deleteSubdirFile: (subdir: string, filename: string) =>
    request<Record<string, unknown>>(
      `/prompt-library/subdirs/${encodeURIComponent(
        subdir,
      )}/${encodeURIComponent(filename)}`,
      { method: "DELETE" },
    ),

  broadcast: (
    files: string[],
    agentIds: string[],
    conflictMode: string = "ask",
  ) =>
    request<{
      results: Array<{
        agent_id: string;
        file: string;
        final_name: string | null;
        error?: string;
        conflict?: boolean;
        suggested_name?: string;
      }>;
    }>("/prompt-library/broadcast", {
      method: "POST",
      body: JSON.stringify({
        files,
        agent_ids: agentIds,
        conflict_mode: conflictMode,
      }),
    }),

  sync: (agentId: string, files: string[], conflictMode: string = "ask") =>
    request<{
      results: Array<{
        file: string;
        final_name: string | null;
        error?: string;
        conflict?: boolean;
        suggested_name?: string;
      }>;
    }>("/prompt-library/sync", {
      method: "POST",
      body: JSON.stringify({
        agent_id: agentId,
        files,
        conflict_mode: conflictMode,
      }),
    }),

  restoreBuiltin: () =>
    request<{ restored: string[]; count: number }>(
      "/prompt-library/restore-builtin",
      { method: "POST" },
    ),
};
