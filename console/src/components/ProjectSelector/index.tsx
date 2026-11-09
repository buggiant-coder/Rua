import { useEffect, useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  Folder,
  LayoutGrid,
  MessageSquare,
  Plus,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import type { ProjectDescriptor } from "@/api/types/project";
import { Popover, Tooltip } from "@/ui";
import { rememberProjectId } from "@/pages/Projects/projectSelection";
import styles from "./index.module.less";

export interface ProjectSelectorProps {
  currentProject: ProjectDescriptor | null;
  projects: ProjectDescriptor[];
  projectDeviceLabel?: (project: ProjectDescriptor) => string;
  onSelect: (project: ProjectDescriptor) => void | Promise<void>;
  onSelectIndependent?: () => void | Promise<void>;
  onCreate?: () => void;
  onOverview?: () => void;
}

export default function ProjectSelector({
  currentProject,
  projects,
  projectDeviceLabel,
  onSelect,
  onSelectIndependent,
  onCreate,
  onOverview,
}: ProjectSelectorProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [switchingProjectId, setSwitchingProjectId] = useState<string | null>(
    null,
  );
  const activeProjects = useMemo(
    () => projects.filter((project) => project.lifecycle === "active"),
    [projects],
  );

  const currentProjectId = currentProject?.project_id;
  useEffect(() => {
    if (currentProjectId) rememberProjectId(currentProjectId);
  }, [currentProjectId]);

  const selectProject = async (project: ProjectDescriptor) => {
    if (
      project.project_id === currentProject?.project_id ||
      switchingProjectId
    ) {
      setOpen(false);
      return;
    }
    setSwitchingProjectId(project.project_id);
    setOpen(false);
    try {
      await onSelect(project);
      rememberProjectId(project.project_id);
    } finally {
      setSwitchingProjectId(null);
    }
  };

  const selectIndependent = async () => {
    setOpen(false);
    if (!currentProject || switchingProjectId || !onSelectIndependent) return;
    setSwitchingProjectId("independent");
    try {
      await onSelectIndependent();
    } finally {
      setSwitchingProjectId(null);
    }
  };

  const content = (
    <div className={styles.projectPopover} aria-label={t("projects.title")}>
      {(onCreate || onOverview) && (
        <div className={styles.projectPopoverHeader}>
          <strong>{t("projects.title")}</strong>
          {onCreate && (
            <Tooltip title={t("projects.newProject")}>
              <button
                type="button"
                className={styles.iconButton}
                aria-label={t("projects.newProject")}
                onClick={() => {
                  setOpen(false);
                  onCreate();
                }}
              >
                <Plus size={15} />
              </button>
            </Tooltip>
          )}
          {onOverview && (
            <Tooltip title={t("projects.overview")}>
              <button
                type="button"
                className={styles.iconButton}
                aria-label={t("projects.overview")}
                onClick={() => {
                  setOpen(false);
                  onOverview();
                }}
              >
                <LayoutGrid size={15} />
              </button>
            </Tooltip>
          )}
        </div>
      )}
      <div className={styles.projectPopoverList}>
        {onSelectIndependent && (
          <button
            type="button"
            className={`${styles.projectOption} ${
              currentProject === null ? styles.projectOptionActive : ""
            }`}
            aria-pressed={currentProject === null}
            disabled={Boolean(switchingProjectId)}
            onClick={() => void selectIndependent()}
          >
            <span className={styles.projectOptionMark}>
              <MessageSquare size={15} />
            </span>
            <span className={styles.projectOptionCopy}>
              <strong>{t("navigation.independentChats")}</strong>
            </span>
            {currentProject === null ? <Check size={15} /> : null}
          </button>
        )}
        {activeProjects.map((project) => {
          const active = project.project_id === currentProject?.project_id;
          return (
            <button
              key={project.project_id}
              type="button"
              className={`${styles.projectOption} ${
                active ? styles.projectOptionActive : ""
              }`}
              aria-pressed={active}
              disabled={Boolean(switchingProjectId)}
              onClick={() => void selectProject(project)}
            >
              <span className={styles.projectOptionMark}>
                <Folder size={15} />
              </span>
              <span className={styles.projectOptionCopy}>
                <strong>{project.display_name}</strong>
                <small title={project.display_path}>
                  {project.display_path}
                </small>
              </span>
              {projectDeviceLabel && (
                <span className={styles.projectOptionDevice}>
                  {projectDeviceLabel(project)}
                </span>
              )}
              {active ? <Check size={15} /> : null}
            </button>
          );
        })}
      </div>
    </div>
  );

  return (
    <Popover
      content={content}
      trigger="click"
      placement="bottom-start"
      open={open}
      onOpenChange={setOpen}
      overlayClassName={styles.projectPopoverOverlay}
    >
      <button
        type="button"
        className={styles.projectControl}
        aria-label={t("projects.selectProject")}
        title={currentProject?.display_name ?? t("navigation.independentChats")}
      >
        <span className={styles.projectMark}>
          {currentProject ? <Folder size={16} /> : <MessageSquare size={16} />}
        </span>
        <span className={styles.projectLabel}>
          <strong>
            {currentProject?.display_name ?? t("navigation.independentChats")}
          </strong>
        </span>
        <ChevronDown
          size={14}
          className={`${styles.projectChevron} ${
            open ? styles.controlChevronOpen : ""
          }`}
        />
      </button>
    </Popover>
  );
}
