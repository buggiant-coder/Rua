import { request } from "../request";
import { getApiUrl } from "../config";
import { buildAuthHeaders } from "../authHeaders";
import type { MdFileInfo, MdFileContent, SubdirFile } from "../types";

function getSelectedAgentId(): string {
  try {
    const agentStorage = sessionStorage.getItem("rua-agent-storage");
    if (agentStorage) {
      const parsed = JSON.parse(agentStorage);
      const selectedAgent = parsed?.state?.selectedAgent;
      if (selectedAgent) {
        return selectedAgent;
      }
    }
  } catch (error) {
    console.warn("Failed to get selected agent from storage:", error);
  }
  return "default";
}

function generateFallbackFilename(): string {
  const agentId = getSelectedAgentId();
  const now = new Date();
  const timestamp = now
    .toISOString()
    .replace(/[-:]/g, "")
    .replace(/\..+/, "")
    .replace("T", "_")
    .slice(0, 15);
  return `rua_workspace_${agentId}_${timestamp}.zip`;
}

export interface WorkspaceDownloadResult {
  blob: Blob;
  filename: string;
}

export const workspaceApi = {
  listFiles: () =>
    request<MdFileInfo[]>("/agent/files").then((files) =>
      files.map((file) => ({
        ...file,
        updated_at: new Date(file.modified_time).getTime(),
      })),
    ),

  loadFile: (fileName: string) =>
    request<MdFileContent>(`/agent/files/${encodeURIComponent(fileName)}`),

  saveFile: (fileName: string, content: string) =>
    request<Record<string, unknown>>(
      `/agent/files/${encodeURIComponent(fileName)}`,
      {
        method: "PUT",
        body: JSON.stringify({ content }),
      },
    ),

  deleteFile: (fileName: string, includeSubdir = false) =>
    request<Record<string, unknown>>(
      `/agent/files/${encodeURIComponent(fileName)}${
        includeSubdir ? "?include_subdir=true" : ""
      }`,
      {
        method: "DELETE",
      },
    ),

  renameFile: (oldName: string, newName: string) =>
    request<Record<string, unknown>>(
      `/agent/files/${encodeURIComponent(oldName)}/rename`,
      {
        method: "POST",
        body: JSON.stringify({ new_name: newName }),
      },
    ),

  downloadWorkspace: async (): Promise<WorkspaceDownloadResult> => {
    const response = await fetch(getApiUrl("/workspace/download"), {
      method: "GET",
      headers: buildAuthHeaders(),
    });

    if (!response.ok) {
      throw new Error(
        `Workspace download failed: ${response.status} ${response.statusText}`,
      );
    }

    const blob = await response.blob();

    const disposition = response.headers.get("Content-Disposition");
    let filename: string;

    if (disposition) {
      const filenameMatch = disposition.match(/filename="(.+?)"/);
      if (filenameMatch && filenameMatch[1]) {
        filename = filenameMatch[1];
      } else {
        filename = generateFallbackFilename();
      }
    } else {
      filename = generateFallbackFilename();
    }

    return { blob, filename };
  },

  uploadFile: async (
    file: File,
  ): Promise<{ success: boolean; message: string }> => {
    const formData = new FormData();
    formData.append("file", file);

    const response = await fetch(getApiUrl("/workspace/upload"), {
      method: "POST",
      headers: buildAuthHeaders(),
      body: formData,
    });

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(
        `Upload failed: ${response.status} ${response.statusText} - ${errorText}`,
      );
    }

    return await response.json();
  },

  getSubdirsInfo: () => request<Record<string, number>>("/agent/subdirs-info"),

  listSubdirFiles: (subdir: string) =>
    request<MdFileInfo[]>(`/agent/subdirs/${encodeURIComponent(subdir)}`).then(
      (files) =>
        files.map((file) => ({
          ...file,
          subdir,
          updated_at: new Date(file.modified_time).getTime(),
        })) as SubdirFile[],
    ),

  createSubdirFile: (subdir: string, filename = "new.md") =>
    request<MdFileInfo>(`/agent/subdirs/${encodeURIComponent(subdir)}`, {
      method: "POST",
      body: JSON.stringify({ filename }),
    }),

  readSubdirFile: (subdir: string, filename: string) =>
    request<MdFileContent>(
      `/agent/subdirs/${encodeURIComponent(subdir)}/${encodeURIComponent(
        filename,
      )}`,
    ),

  writeSubdirFile: (subdir: string, filename: string, content: string) =>
    request<Record<string, unknown>>(
      `/agent/subdirs/${encodeURIComponent(subdir)}/${encodeURIComponent(
        filename,
      )}`,
      {
        method: "PUT",
        body: JSON.stringify({ content }),
      },
    ),

  renameSubdirFile: (subdir: string, oldName: string, newName: string) =>
    request<Record<string, unknown>>(
      `/agent/subdirs/${encodeURIComponent(subdir)}/${encodeURIComponent(
        oldName,
      )}/rename`,
      {
        method: "POST",
        body: JSON.stringify({ new_name: newName }),
      },
    ),

  deleteSubdirFile: (subdir: string, filename: string) =>
    request<Record<string, unknown>>(
      `/agent/subdirs/${encodeURIComponent(subdir)}/${encodeURIComponent(
        filename,
      )}`,
      {
        method: "DELETE",
      },
    ),

  getSystemPromptFiles: () => request<string[]>("/agent/system-prompt-files"),

  setSystemPromptFiles: (files: string[]) =>
    request<string[]>("/agent/system-prompt-files", {
      method: "PUT",
      body: JSON.stringify(files),
    }),
};
