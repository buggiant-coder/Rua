import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import type {
  MarkdownFile,
  MdFileInfo,
  SubdirFile,
} from "../../../../api/types";
import { agentsApi } from "../../../../api/modules/agents";
import { useAgentStore } from "../../../../stores/agentStore";
import { toast } from "@/ui";

const getParentDir = (filePath: string): string => {
  const match = filePath.match(/^(.*)[/\\]/);
  return match ? match[1] : filePath;
};

const filenameToSubdir = (filename: string): string =>
  filename.replace(/\.md$/i, "").toUpperCase();

const toMarkdownFile = (
  f: MdFileInfo & { updated_at?: number; modified_time?: string },
): MarkdownFile => ({
  ...f,
  updated_at:
    f.updated_at ?? (f.modified_time ? new Date(f.modified_time).getTime() : 0),
});

export const useAgentsData = () => {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const [files, setFiles] = useState<MarkdownFile[]>([]);
  const [selectedFile, setSelectedFile] = useState<MarkdownFile | null>(null);

  const [subdirFiles, setSubdirFiles] = useState<Record<string, SubdirFile[]>>(
    {},
  );
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());

  const [filesWithSubdirs, setFilesWithSubdirs] = useState<Set<string>>(
    new Set(),
  );
  const [fileContent, setFileContent] = useState("");
  const [originalContent, setOriginalContent] = useState("");
  const [loading, setLoading] = useState(false);
  const [workspacePath, setWorkspacePath] = useState<string | null>(null);
  const [enabledFiles, setEnabledFiles] = useState<string[]>([]);

  const [displayedLoadedFiles, setDisplayedLoadedFiles] = useState<string[]>(
    [],
  );
  const [autoRename, setAutoRename] = useState(false);
  const [isCreatingNew, setIsCreatingNew] = useState(false);
  const [sortMode, setSortMode] = useState<
    "name-asc" | "name-desc" | "time-asc" | "time-desc"
  >("name-asc");

  const fetchSubdirsInfo = useCallback(async () => {
    try {
      const info = await agentsApi.getSubdirsInfo(selectedAgent);
      const validSet = new Set(Object.keys(info));
      setFilesWithSubdirs(validSet);

      setExpandedDirs((prev) => {
        const next = new Set<string>();
        for (const fn of prev) {
          if (validSet.has(fn)) next.add(fn);
        }
        return next.size !== prev.size ? next : prev;
      });
      setSubdirFiles((prev) => {
        const next: Record<string, SubdirFile[]> = {};
        let changed = false;
        for (const [key, val] of Object.entries(prev)) {
          if (validSet.has(key)) {
            next[key] = val;
          } else {
            changed = true;
          }
        }
        return changed ? next : prev;
      });
    } catch {
      setFilesWithSubdirs(new Set());
    }
  }, [selectedAgent]);

  useEffect(() => {
    const initializeData = async () => {
      const previouslySelectedFilename = selectedFile?.filename;

      setFileContent("");
      setOriginalContent("");
      setExpandedDirs(new Set());
      setSubdirFiles({});

      await fetchEnabledFiles();
      const rawList = await agentsApi.listAgentFiles(selectedAgent);
      const fileList = rawList.map(toMarkdownFile);
      const sortedFiles = sortFiles(fileList, sortMode);
      setFiles(sortedFiles);

      if (fileList.length > 0) {
        setWorkspacePath(getParentDir(fileList[0].path));
      } else {
        setWorkspacePath("");
      }

      await fetchSubdirsInfo();

      if (previouslySelectedFilename) {
        const sameFile = sortedFiles.find(
          (f) => f.filename === previouslySelectedFilename,
        );
        if (sameFile) {
          await handleFileClick(sameFile);
        } else {
          setSelectedFile(null);
        }
      } else {
        setSelectedFile(null);
      }
    };
    initializeData();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedAgent]);

  useEffect(() => {
    if (files.length > 0) {
      setFiles((prev) => sortFiles(prev, sortMode));

      setSubdirFiles((prev) => {
        const next: Record<string, SubdirFile[]> = {};
        for (const [key, list] of Object.entries(prev)) {
          next[key] = [...list].sort((a, b) => {
            if (sortMode === "time-asc")
              return (a.updated_at || 0) - (b.updated_at || 0);
            if (sortMode === "time-desc")
              return (b.updated_at || 0) - (a.updated_at || 0);
            if (sortMode === "name-desc")
              return b.filename.localeCompare(a.filename);
            return a.filename.localeCompare(b.filename);
          });
        }
        return next;
      });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sortMode]);

  const fetchEnabledFiles = async () => {
    try {
      const result = await agentsApi.getSystemPromptFiles(selectedAgent);
      const enabled = Array.isArray(result) ? result : [];
      setEnabledFiles(enabled);
      setDisplayedLoadedFiles(enabled);
      return enabled;
    } catch (error) {
      console.error("Failed to fetch enabled files", error);
      return [];
    }
  };

  const sortFiles = (fileList: MarkdownFile[], mode: string) => {
    return [...fileList].sort((a, b) => {
      if (mode === "time-asc") return (a.updated_at || 0) - (b.updated_at || 0);
      if (mode === "time-desc")
        return (b.updated_at || 0) - (a.updated_at || 0);
      if (mode === "name-desc") return b.filename.localeCompare(a.filename);
      return a.filename.localeCompare(b.filename);
    });
  };

  const fetchFiles = async (latestEnabledFiles?: string[]) => {
    try {
      if (!Array.isArray(latestEnabledFiles)) {
        await fetchEnabledFiles();
      }
      const rawList = await agentsApi.listAgentFiles(selectedAgent);
      const fileList = rawList.map(toMarkdownFile);
      const sortedFiles = sortFiles(fileList, sortMode);
      setFiles(sortedFiles);
      if (fileList.length > 0) {
        setWorkspacePath(getParentDir(fileList[0].path));
      } else {
        setWorkspacePath("");
      }

      await fetchSubdirsInfo();

      for (const parentFilename of expandedDirs) {
        await fetchSubdirFiles(parentFilename);
      }

      if (selectedFile) {
        const sel = selectedFile as MarkdownFile & {
          _subdir?: string;
          _parentFilename?: string;
        };
        try {
          if (sel._subdir) {
            const data = await agentsApi.readSubdirFile(
              selectedAgent,
              sel._subdir,
              sel.filename,
            );
            setFileContent(data.content);
            setOriginalContent(data.content);
          } else if (sortedFiles.some((f) => f.filename === sel.filename)) {
            const data = await agentsApi.readAgentFile(
              selectedAgent,
              sel.filename,
            );
            setFileContent(data.content);
            setOriginalContent(data.content);
          } else {
            setSelectedFile(null);
            setFileContent("");
            setOriginalContent("");
          }
        } catch {
          setSelectedFile(null);
          setFileContent("");
          setOriginalContent("");
        }
      }
    } catch (error) {
      console.error("Failed to fetch files", error);
      toast.error(t("workspace.loadFileListFailed"));
    }
  };

  const fetchSubdirFiles = useCallback(
    async (parentFilename: string) => {
      const subdir = filenameToSubdir(parentFilename);
      try {
        let list = await agentsApi.listSubdirFiles(selectedAgent, subdir);

        list = [...list].sort((a, b) => {
          if (sortMode === "time-asc")
            return (a.updated_at || 0) - (b.updated_at || 0);
          if (sortMode === "time-desc")
            return (b.updated_at || 0) - (a.updated_at || 0);
          if (sortMode === "name-desc")
            return b.filename.localeCompare(a.filename);
          return a.filename.localeCompare(b.filename);
        });
        setSubdirFiles((prev) => ({ ...prev, [parentFilename]: list }));

        if (list.length > 0) {
          setFilesWithSubdirs((prev) => new Set(prev).add(parentFilename));
        } else {
          setFilesWithSubdirs((prev) => {
            const next = new Set(prev);
            next.delete(parentFilename);
            return next;
          });

          setExpandedDirs((prev) => {
            const next = new Set(prev);
            next.delete(parentFilename);
            return next;
          });
        }
      } catch {
        setSubdirFiles((prev) => ({ ...prev, [parentFilename]: [] }));
        setFilesWithSubdirs((prev) => {
          const next = new Set(prev);
          next.delete(parentFilename);
          return next;
        });
        setExpandedDirs((prev) => {
          const next = new Set(prev);
          next.delete(parentFilename);
          return next;
        });
      }
    },
    [selectedAgent, sortMode],
  );

  const handleFileClick = async (file: MarkdownFile) => {
    setExpandedDirs((prev) => {
      const next = new Set(prev);
      if (next.has(file.filename)) {
        if (selectedFile?.filename === file.filename) {
          next.delete(file.filename);
          return next;
        }
      } else {
        next.add(file.filename);
        fetchSubdirFiles(file.filename);
      }
      return next;
    });

    setSelectedFile(file);
    setAutoRename(false);
    setLoading(true);
    try {
      const data = await agentsApi.readAgentFile(selectedAgent, file.filename);
      setFileContent(data.content);
      setOriginalContent(data.content);
    } catch (error) {
      console.error("Failed to load file", error);
      toast.error(t("workspace.loadFileFailed"));
    } finally {
      setLoading(false);
    }
  };

  const handleSubdirFileClick = async (
    parentFilename: string,
    file: SubdirFile,
  ) => {
    const subdir = file.subdir || filenameToSubdir(parentFilename);
    setSelectedFile({
      filename: file.filename,
      path: file.path,
      size: file.size,
      created_time: file.created_time,
      modified_time: file.modified_time,
      updated_at: file.updated_at,
      _subdir: subdir,
      _parentFilename: parentFilename,
    } as MarkdownFile & { _subdir: string; _parentFilename: string });
    setAutoRename(false);
    setLoading(true);
    try {
      const data = await agentsApi.readSubdirFile(
        selectedAgent,
        subdir,
        file.filename,
      );
      setFileContent(data.content);
      setOriginalContent(data.content);
    } catch (error) {
      console.error("Failed to load subdir file", error);
      toast.error(t("workspace.loadFileFailed"));
    } finally {
      setLoading(false);
    }
  };

  const handleCreateSubdirFile = async (parentFilename: string) => {
    const subdir = filenameToSubdir(parentFilename);

    const existing = (subdirFiles[parentFilename] || []).map((f) =>
      f.filename.toUpperCase(),
    );
    let newName = "NEW.md";
    if (existing.includes("NEW.MD")) {
      let n = 2;
      while (existing.includes(`NEW-${n}.MD`)) n++;
      newName = `NEW-${n}.md`;
    }
    try {
      const info = await agentsApi.createSubdirFile(
        selectedAgent,
        subdir,
        newName,
      );
      toast.success(t("workspace.createSubFileSuccess") || "Sub-file created");
      setExpandedDirs((prev) => new Set(prev).add(parentFilename));
      await fetchSubdirFiles(parentFilename);
      setSelectedFile({
        filename: info.filename,
        path: info.path,
        size: info.size,
        created_time: info.created_time,
        modified_time: info.modified_time,
        updated_at: new Date(info.modified_time).getTime(),
        _subdir: subdir,
        _parentFilename: parentFilename,
      } as MarkdownFile & { _subdir: string; _parentFilename: string });
      setFileContent("");
      setOriginalContent("");
      setAutoRename(true);
    } catch (error) {
      console.error("Create subdir file failed:", error);
      toast.error(
        t("workspace.createSubFileFailed") || "Failed to create sub-file",
      );
    }
  };

  const handleSave = async () => {
    if (!selectedFile) return;

    if (isCreatingNew) {
      const filename = selectedFile.filename;
      const exists = files.some(
        (f) => f.filename.toUpperCase() === filename.toUpperCase(),
      );
      if (exists) {
        toast.error(t("workspace.duplicateFileName"));
        return;
      }
      setLoading(true);
      try {
        await agentsApi.writeAgentFile(selectedAgent, filename, fileContent);
        setOriginalContent(fileContent);
        setIsCreatingNew(false);
        toast.success(t("workspace.createSuccess"));
        await fetchFiles();
      } catch (error) {
        console.error("Create file failed:", error);
        toast.error(t("workspace.createFailed"));
      } finally {
        setLoading(false);
      }
      return;
    }

    setLoading(true);
    try {
      const sel = selectedFile as MarkdownFile & {
        _subdir?: string;
        _parentFilename?: string;
      };
      if (sel._subdir) {
        await agentsApi.writeSubdirFile(
          selectedAgent,
          sel._subdir,
          sel.filename,
          fileContent,
        );
        await fetchSubdirFiles(sel._parentFilename!);
      } else {
        await agentsApi.writeAgentFile(
          selectedAgent,
          selectedFile.filename,
          fileContent,
        );
        fetchFiles();
      }
      setOriginalContent(fileContent);
      toast.success(t("workspace.saveSuccess"));
    } catch (error) {
      console.error("Failed to save file", error);
      toast.error(t("workspace.saveFailed"));
    } finally {
      setLoading(false);
    }
  };

  const handleReset = () => {
    setFileContent(originalContent);
  };

  const handleToggleFileEnabled = async (filename: string) => {
    const isEnabling = !enabledFiles.includes(filename);

    const newEnabledFiles = isEnabling
      ? [...enabledFiles, filename]
      : enabledFiles.filter((f) => f !== filename);

    try {
      await agentsApi.setSystemPromptFiles(selectedAgent, newEnabledFiles);
      setEnabledFiles(newEnabledFiles);

      if (isEnabling && !displayedLoadedFiles.includes(filename)) {
        setDisplayedLoadedFiles((prev) => [...prev, filename]);
      }
      toast.success(
        t("workspace.configUpdated") || "System prompt configuration updated",
      );
    } catch (error) {
      console.error("Failed to update system prompt files", error);
      toast.error(
        t("workspace.configUpdateFailed") ||
          "Failed to update system prompt configuration",
      );
    }
  };

  const handleReorderFiles = async (newOrder: string[]) => {
    try {
      const stillEnabled = newOrder.filter((f) => enabledFiles.includes(f));
      await agentsApi.setSystemPromptFiles(selectedAgent, stillEnabled);
      setEnabledFiles(stillEnabled);
      setDisplayedLoadedFiles(newOrder);
    } catch (error) {
      console.error("Failed to reorder files", error);
      toast.error(t("workspace.reorderFailed"));
    }
  };

  const handleToggleExpandAll = () => {
    if (expandedDirs.size > 0) {
      setExpandedDirs(new Set());
    } else {
      const toExpand = new Set<string>();
      for (const fn of filesWithSubdirs) {
        toExpand.add(fn);
        if (!(fn in subdirFiles)) {
          fetchSubdirFiles(fn);
        }
      }
      setExpandedDirs(toExpand);
    }
  };

  const startCreateFile = () => {
    const existingNames = files.map((f) => f.filename.toUpperCase());
    let newName = "NEW.md";
    if (existingNames.includes("NEW.MD")) {
      let n = 2;
      while (existingNames.includes(`NEW-${n}.MD`)) n++;
      newName = `NEW-${n}.md`;
    }
    const phantom: MarkdownFile = {
      filename: newName,
      path: "",
      size: 0,
      created_time: "",
      modified_time: "",
      builtin: false,
      updated_at: 0,
    };
    setSelectedFile(phantom);
    setFileContent("");
    setOriginalContent("");
    setIsCreatingNew(true);
    setAutoRename(true);
  };

  const commitNewFileName = (finalName: string) => {
    const stem = finalName.replace(/\.md$/i, "");
    const filename = `${stem}.md`;
    setSelectedFile((prev) => (prev ? { ...prev, filename } : prev));
  };

  const cancelCreateFile = () => {
    setSelectedFile(null);
    setIsCreatingNew(false);
  };

  const hasChanges = fileContent !== originalContent;

  return {
    files,
    selectedFile,
    subdirFiles,
    expandedDirs,
    filesWithSubdirs,
    fileContent,
    loading,
    workspacePath,
    hasChanges,
    enabledFiles,
    displayedLoadedFiles,
    autoRename,
    isCreatingNew,
    sortMode,
    setFileContent,
    setSortMode,
    setAutoRename,
    clearSelection: () => setSelectedFile(null),
    startCreateFile,
    commitNewFileName,
    cancelCreateFile,
    fetchFiles,
    fetchSubdirFiles,
    handleFileClick,
    handleSubdirFileClick,
    handleCreateSubdirFile,
    handleSave,
    handleReset,
    handleToggleFileEnabled,
    handleReorderFiles,
    handleToggleExpandAll,
  };
};
