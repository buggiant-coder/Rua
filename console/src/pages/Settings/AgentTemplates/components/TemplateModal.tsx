import { useEffect, useMemo, useState } from "react";
import {
  DndContext,
  closestCenter,
  PointerSensor,
  useSensor,
  useSensors,
  DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  verticalListSortingStrategy,
  arrayMove,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useTranslation } from "react-i18next";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { request } from "@/api/request";
import type { AgentTemplate } from "@/api/types/agentTemplates";
import styles from "../index.module.less";
import { ChevronDown, ChevronRight, GripVertical } from "lucide-react";
import {
  FormField,
  Loading,
  Modal,
  Radio,
  RadioGroup,
  TextArea,
  TextInput,
  Toggle,
} from "@/ui";

interface MdFileInfo {
  filename: string;
  size: number;
  modified_time: string;
}

interface SubdirFile {
  filename: string;
  size: number;
  modified_time: string;
  subdir?: string;
}

interface TemplateModalProps {
  open: boolean;
  editingTemplate: AgentTemplate | null;
  onSave: (values: {
    name: string;
    description: string;
    prompt_files: string[];
  }) => Promise<void>;
  onClose: () => void;
}

type SortMode = "name" | "modified";

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

function DraggableEnabledItem({
  entry,
  fileSize,
  onToggle,
}: {
  entry: string;
  fileSize: number | null;
  onToggle: (entry: string) => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: entry });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
    zIndex: isDragging ? 1 : undefined,
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={`${styles.loadedPromptItem} ${
        isDragging ? styles.dragging : ""
      }`}
    >
      <div className={styles.dragHandle} {...attributes} {...listeners}>
        <GripVertical />
      </div>
      <div className={styles.loadedPromptName}>
        {entry.replace(/\.md$/i, "")}
        {fileSize !== null && (
          <span className={styles.fileItemMetaInline}>
            {formatFileSize(fileSize)}
          </span>
        )}
      </div>
      <span onClick={(e) => e.stopPropagation()}>
        <Toggle checked onChange={() => onToggle(entry)} />
      </span>
    </div>
  );
}

export function TemplateModal({
  open,
  editingTemplate,
  onSave,
  onClose,
}: TemplateModalProps) {
  const { t } = useTranslation();

  const schema = z.object({
    name: z.string().min(1, t("template.nameRequired")),
    description: z.string(),
  });

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<{ name: string; description: string }>({
    resolver: zodResolver(schema),
    defaultValues: { name: "", description: "" },
  });

  const [files, setFiles] = useState<MdFileInfo[]>([]);
  const [subdirFiles, setSubdirFiles] = useState<Record<string, SubdirFile[]>>(
    {},
  );
  const [filesWithSubdirs, setFilesWithSubdirs] = useState<Set<string>>(
    new Set(),
  );
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [enabledFiles, setEnabledFiles] = useState<string[]>([]);
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [sortMode, setSortMode] = useState<SortMode>("name");

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  useEffect(() => {
    if (!open) return;

    setLoadingFiles(true);

    const fetchFiles = request<MdFileInfo[]>("/prompt-library/files");
    const fetchSubdirs = request<Record<string, number>>(
      "/prompt-library/subdirs-info",
    );

    Promise.all([fetchFiles, fetchSubdirs])
      .then(([fileList, subdirInfo]) => {
        setFiles(fileList);
        setFilesWithSubdirs(new Set(Object.keys(subdirInfo)));
      })
      .finally(() => setLoadingFiles(false));

    if (editingTemplate) {
      reset({
        name: editingTemplate.name,
        description: editingTemplate.description ?? "",
      });
      setEnabledFiles([...editingTemplate.prompt_files]);
    } else {
      reset({ name: "", description: "" });
      setEnabledFiles([]);
    }
    setSortMode("name");
    setExpandedDirs(new Set());
    setSubdirFiles({});
  }, [open, editingTemplate?.id, reset]);

  const sortedUnenabledFiles = useMemo(() => {
    const enabledSet = new Set(enabledFiles);
    const unenabled = files.filter((f) => !enabledSet.has(f.filename));
    if (sortMode === "name") {
      unenabled.sort((a, b) => a.filename.localeCompare(b.filename));
    } else {
      unenabled.sort(
        (a, b) =>
          new Date(b.modified_time).getTime() -
          new Date(a.modified_time).getTime(),
      );
    }
    return unenabled;
  }, [files, enabledFiles, sortMode]);

  const getFileSize = (entry: string): number | null => {
    if (entry.includes("/")) {
      const [dir, name] = entry.split("/", 2);
      for (const [parentFilename, subs] of Object.entries(subdirFiles)) {
        const parentSubdir = parentFilename.replace(/\.md$/i, "").toUpperCase();
        if (parentSubdir === dir) {
          const sub = subs.find((s) => s.filename === name);
          return sub ? sub.size : null;
        }
      }
      return null;
    }
    const file = files.find((f) => f.filename === entry);
    return file ? file.size : null;
  };

  const toggleEnabled = (filename: string) => {
    if (enabledFiles.includes(filename)) {
      setEnabledFiles(enabledFiles.filter((f) => f !== filename));
    } else {
      setEnabledFiles([...enabledFiles, filename]);
    }
  };

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIndex = enabledFiles.indexOf(active.id as string);
    const newIndex = enabledFiles.indexOf(over.id as string);
    if (oldIndex === -1 || newIndex === -1) return;
    setEnabledFiles(arrayMove(enabledFiles, oldIndex, newIndex));
  };

  const handleFileClick = (file: MdFileInfo) => {
    const filename = file.filename;
    if (!filesWithSubdirs.has(filename)) return;

    const next = new Set(expandedDirs);
    if (next.has(filename)) {
      next.delete(filename);
    } else {
      next.add(filename);
      if (!subdirFiles[filename]) {
        const subdir = filename.replace(/\.md$/i, "");
        request<MdFileInfo[]>(`/prompt-library/subdirs/${subdir}`).then(
          (subs) => {
            setSubdirFiles((prev) => ({
              ...prev,
              [filename]: subs.map((s) => ({
                ...s,
                subdir: subdir.toUpperCase(),
              })),
            }));
          },
        );
      }
    }
    setExpandedDirs(next);
  };

  const subdirEnabledKey = (parentFilename: string, sub: SubdirFile) => {
    const subdir =
      sub.subdir || parentFilename.replace(/\.md$/i, "").toUpperCase();
    return `${subdir}/${sub.filename}`;
  };

  const handleOk = handleSubmit(async (values) => {
    await onSave({
      name: values.name,
      description: values.description || "",
      prompt_files: enabledFiles,
    });
  });

  return (
    <Modal
      className="modal-fullscreen-mobile"
      title={
        editingTemplate ? t("template.editTitle") : t("template.createTitle")
      }
      open={open}
      onOk={handleOk}
      onClose={onClose}
      width={640}
      okText={t("common.save")}
      cancelText={t("common.cancel")}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        <FormField
          label={t("template.name")}
          error={errors.name?.message}
          required
        >
          <TextInput
            {...register("name")}
            placeholder={t("template.namePlaceholder")}
            autoComplete="off"
          />
        </FormField>
        <FormField
          label={t("template.description")}
          error={errors.description?.message}
        >
          <TextArea
            {...register("description")}
            placeholder={t("template.descriptionPlaceholder")}
            rows={2}
          />
        </FormField>
      </div>

      {loadingFiles ? (
        <div style={{ textAlign: "center", padding: "16px 0" }}>
          <Loading size="sm" />
        </div>
      ) : (
        <div style={{ marginTop: 4 }}>
          {}
          <div className={styles.headerRow}>
            <span className={styles.sectionTitle}>
              {t("template.enabledFiles")} ({enabledFiles.length})
            </span>
          </div>
          <div className={styles.enabledFileList}>
            {enabledFiles.length > 0 ? (
              <DndContext
                sensors={sensors}
                collisionDetection={closestCenter}
                onDragEnd={handleDragEnd}
              >
                <SortableContext
                  items={enabledFiles}
                  strategy={verticalListSortingStrategy}
                >
                  {enabledFiles.map((entry) => (
                    <DraggableEnabledItem
                      key={entry}
                      entry={entry}
                      fileSize={getFileSize(entry)}
                      onToggle={toggleEnabled}
                    />
                  ))}
                </SortableContext>
              </DndContext>
            ) : (
              <div className={styles.emptyStateSmall}>
                {t("template.noFilesSelected")}
              </div>
            )}
          </div>

          {}
          <div className={styles.headerRow} style={{ marginTop: 12 }}>
            <span className={styles.headerLine}>
              {t("template.availableFiles")}
            </span>
            <RadioGroup
              value={sortMode}
              onChange={(value) => setSortMode(value as SortMode)}
            >
              <Radio value="name">{t("template.sortByName")}</Radio>
              <Radio value="modified">{t("template.sortByModified")}</Radio>
            </RadioGroup>
          </div>
          <div className={styles.fileListScroll}>
            {sortedUnenabledFiles.map((file) => {
              const hasSubdir = filesWithSubdirs.has(file.filename);
              const isExpanded = expandedDirs.has(file.filename);
              const subs = subdirFiles[file.filename] || [];

              return (
                <div key={file.filename}>
                  <div
                    className={styles.fileItem}
                    onClick={() => handleFileClick(file)}
                  >
                    <div className={styles.fileItemHeader}>
                      <div className={styles.fileInfo}>
                        <div className={styles.fileItemName}>
                          {file.filename.replace(/\.md$/i, "")}
                          <span className={styles.fileItemMetaInline}>
                            {formatFileSize(file.size)}
                          </span>
                        </div>
                      </div>
                      <div className={styles.fileItemActions}>
                        <span onClick={(e) => e.stopPropagation()}>
                          <Toggle
                            checked={false}
                            onChange={() => toggleEnabled(file.filename)}
                          />
                        </span>
                        {hasSubdir && (
                          <span className={styles.expandIcon}>
                            {isExpanded ? <ChevronDown /> : <ChevronRight />}
                          </span>
                        )}
                      </div>
                    </div>
                  </div>

                  {isExpanded && subs.length > 0 && (
                    <div className={styles.subdirFileList}>
                      {subs.map((sub) => {
                        const subKey = subdirEnabledKey(file.filename, sub);
                        const subEnabled = enabledFiles.includes(subKey);
                        return (
                          <div
                            key={sub.filename}
                            className={styles.subdirFileItem}
                          >
                            <div className={styles.subdirFileHeader}>
                              <div className={styles.subdirFileName}>
                                {sub.filename.replace(/\.md$/i, "")}
                                <span className={styles.fileItemMetaInline}>
                                  {formatFileSize(sub.size)}
                                </span>
                              </div>
                              <span onClick={(e) => e.stopPropagation()}>
                                <Toggle
                                  checked={subEnabled}
                                  onChange={() => toggleEnabled(subKey)}
                                />
                              </span>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </Modal>
  );
}
