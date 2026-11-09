/* eslint-disable react-refresh/only-export-components */
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  redirect,
  useLoaderData,
  useNavigate,
  type LoaderFunctionArgs,
} from "react-router-dom";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  Archive,
  Folder,
  Pencil,
  Plus,
  RotateCcw,
  Trash2,
  WifiOff,
} from "lucide-react";

import { projectsApi } from "@/api/modules/projects";
import { invalidateConversationNavigation } from "@/layouts/conversationNavigationEvents";
import { devicesApi, type DeviceDto } from "@/api/modules/sync";
import type {
  ProjectCreationOutcome,
  ProjectDescriptor,
} from "@/api/types/project";
import { deviceConnectivityLabel } from "@/components/DeviceConnectivity";
import { EmptyState } from "@/components/EmptyState";
import { Page, PageBody } from "@/components/Page";
import { PageHeader } from "@/components/PageHeader";
import { extractFriendlyMessage } from "@/utils/error";
import { Btn, Modal, Tag, Tooltip, toast } from "@/ui";

import CreateProjectDialog from "./CreateProjectDialog";
import { useProjectSidebar } from "./useProjectSidebar";
import ProjectArchiveDialog from "./ProjectArchiveDialog";
import ProjectEditDialog from "./ProjectEditDialog";
import styles from "./index.module.less";
import {
  discardProjectLocalState,
  lastProjectId,
  resolveProjectChatPath,
} from "./projectChatNavigation";

interface ProjectsInitialData {
  projects: ProjectDescriptor[];
  devices: DeviceDto[];
}

export async function loader({
  request,
}: LoaderFunctionArgs): Promise<ProjectsInitialData | Response> {
  const projects = await projectsApi.list("all");
  const forceOverview =
    new URL(request.url).searchParams.get("overview") === "1";
  if (!forceOverview) {
    const activeProjects = projects.filter(
      (project) => project.lifecycle === "active",
    );
    const hintedProjectId = lastProjectId();
    const target =
      activeProjects.find(
        (project) => project.project_id === hintedProjectId,
      ) ?? activeProjects[0];
    if (target) {
      return redirect(await resolveProjectChatPath(target.project_id));
    }
  }
  const devices = await devicesApi.listDevices();
  return { projects, devices };
}

export function RouteComponent() {
  return <ProjectsPage initialData={useLoaderData() as ProjectsInitialData} />;
}

export default function ProjectsPage({
  initialData,
}: {
  initialData: ProjectsInitialData;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const sidebar = useProjectSidebar();
  const [projects, setProjects] = useState(initialData.projects);
  const [devices, setDevices] = useState(initialData.devices);
  const [createOpen, setCreateOpen] = useState(false);
  const [renameProject, setRenameProject] = useState<ProjectDescriptor | null>(
    null,
  );
  const [archiveProject, setArchiveProject] =
    useState<ProjectDescriptor | null>(null);
  const [reopeningProjectId, setReopeningProjectId] = useState<string | null>(
    null,
  );
  const [deleteProject, setDeleteProject] = useState<ProjectDescriptor | null>(
    null,
  );
  const [deleting, setDeleting] = useState(false);
  const [focusedProjectId, setFocusedProjectId] = useState<string | null>(null);

  const hiddenProjects = projects.filter(
    (project) =>
      project.lifecycle === "active" &&
      sidebar.snapshot?.items[project.project_id]?.hidden,
  );
  const activeProjects = projects.filter(
    (project) =>
      project.lifecycle === "active" &&
      !sidebar.snapshot?.items[project.project_id]?.hidden,
  );
  const archivedProjects = projects.filter(
    (project) => project.lifecycle === "archived",
  );
  const devicesById = useMemo(
    () => new Map(devices.map((device) => [device.device_id, device])),
    [devices],
  );

  const refresh = useCallback(async () => {
    const [nextProjects, nextDevices] = await Promise.all([
      projectsApi.list("all"),
      devicesApi.listDevices(),
    ]);
    setProjects(nextProjects);
    setDevices(nextDevices);
  }, []);

  const openProject = useCallback(
    async (project: ProjectDescriptor) => {
      navigate(await resolveProjectChatPath(project.project_id));
    },
    [navigate],
  );

  useEffect(() => {
    if (!focusedProjectId) return;
    const timer = window.setTimeout(() => setFocusedProjectId(null), 1_600);
    const element = document.getElementById(`project-${focusedProjectId}`);
    if (typeof element?.scrollIntoView === "function") {
      element.scrollIntoView({ block: "nearest" });
    }
    return () => window.clearTimeout(timer);
  }, [focusedProjectId]);

  const handleCreated = (outcome: ProjectCreationOutcome) => {
    invalidateConversationNavigation();
    setProjects((current) => [
      outcome.project,
      ...current.filter(
        (project) => project.project_id !== outcome.project.project_id,
      ),
    ]);
    setCreateOpen(false);
    setFocusedProjectId(outcome.project.project_id);
    if (outcome.overlap_warnings.length > 0) {
      toast.warning(
        t(
          "projects.overlapCreated",
          "Project created. Its writable tree overlaps another Project.",
        ),
      );
    } else {
      toast.success(t("projects.created", "Project created"));
    }
    void openProject(outcome.project);
  };

  const focusProject = (project: ProjectDescriptor) => {
    setProjects((current) =>
      current.some((item) => item.project_id === project.project_id)
        ? current
        : [project, ...current],
    );
    setCreateOpen(false);
    setFocusedProjectId(project.project_id);
    if (project.lifecycle === "active") void openProject(project);
  };

  const reopenProject = async (project: ProjectDescriptor) => {
    setReopeningProjectId(project.project_id);
    try {
      const result = await projectsApi.reopen(project.project_id);
      invalidateConversationNavigation();
      setProjects((current) =>
        current.map((item) =>
          item.project_id === result.project.project_id ? result.project : item,
        ),
      );
      toast.success(t("projects.reopened", "Project reopened"));
      await openProject(result.project);
    } catch (error) {
      toast.error(extractFriendlyMessage(error));
    } finally {
      setReopeningProjectId(null);
    }
  };

  const permanentlyDeleteProject = async () => {
    if (!deleteProject) return;
    setDeleting(true);
    try {
      await projectsApi.permanentlyDelete(deleteProject.project_id);
      discardProjectLocalState(deleteProject.project_id);
      invalidateConversationNavigation();
      setProjects((current) =>
        current.filter(
          (project) => project.project_id !== deleteProject.project_id,
        ),
      );
      setDeleteProject(null);
      toast.success(
        t("projects.permanentlyDeleted", "Project permanently deleted"),
      );
    } catch (error) {
      toast.error(extractFriendlyMessage(error));
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Page className={styles.page}>
      <PageHeader
        current={t("projects.title", "Projects")}
        onRefresh={refresh}
        extra={
          <Btn icon={<Plus size={16} />} onClick={() => setCreateOpen(true)}>
            {t("projects.newProject", "New Project")}
          </Btn>
        }
      />
      <PageBody className={styles.body}>
        <ProjectSection
          title={t("projects.active", "Active")}
          projects={activeProjects}
          devicesById={devicesById}
          focusedProjectId={focusedProjectId}
          onOpen={openProject}
          onArchive={setArchiveProject}
          onRename={(project) => {
            setRenameProject(project);
          }}
        />
        <ProjectSection
          title={t("navigation.hiddenProjects")}
          projects={hiddenProjects}
          devicesById={devicesById}
          focusedProjectId={focusedProjectId}
          onOpen={openProject}
          onRename={setRenameProject}
          onRestore={(project) =>
            void sidebar.update({
              action: "set_hidden",
              project_id: project.project_id,
              hidden: false,
            })
          }
        />
        {archivedProjects.length > 0 ? (
          <ProjectSection
            title={t("projects.archived", "Archived")}
            projects={archivedProjects}
            devicesById={devicesById}
            focusedProjectId={focusedProjectId}
            archived
            onOpen={openProject}
            reopeningProjectId={reopeningProjectId}
            onReopen={reopenProject}
            onDelete={setDeleteProject}
            onRename={(project) => {
              setRenameProject(project);
            }}
          />
        ) : null}
        {projects.length === 0 ? (
          <EmptyState
            icon={<Folder />}
            description={t("projects.empty", "No Projects")}
            action={
              <Btn
                icon={<Plus size={16} />}
                onClick={() => setCreateOpen(true)}
              >
                {t("projects.newProject", "New Project")}
              </Btn>
            }
          />
        ) : null}
      </PageBody>

      <CreateProjectDialog
        open={createOpen}
        devices={devices}
        onClose={() => setCreateOpen(false)}
        onCreated={handleCreated}
        onViewDuplicate={focusProject}
      />

      {renameProject && (
        <ProjectEditDialog
          key={renameProject.project_id}
          project={renameProject}
          onClose={() => setRenameProject(null)}
          onSaved={(updated) => {
            setProjects((current) =>
              current.map((project) =>
                project.project_id === updated.project_id ? updated : project,
              ),
            );
            invalidateConversationNavigation();
            setRenameProject(null);
          }}
        />
      )}
      {archiveProject && (
        <ProjectArchiveDialog
          project={archiveProject}
          onClose={() => setArchiveProject(null)}
          onArchived={(updated) => {
            setProjects((current) =>
              current.map((project) =>
                project.project_id === updated.project_id ? updated : project,
              ),
            );
            setArchiveProject(null);
            toast.success(t("projects.archivedSuccess", "Project archived"));
          }}
        />
      )}
      <Modal
        open={deleteProject !== null}
        onClose={() => setDeleteProject(null)}
        title={t("projects.permanentDelete", "Permanently Delete Project")}
        onOk={permanentlyDeleteProject}
        okText={t("projects.permanentlyDelete", "Permanently Delete")}
        confirmLoading={deleting}
        okButtonProps={{ variant: "danger" }}
        width={480}
      >
        <p>
          {t(
            "projects.permanentDeleteDescription",
            "Project Chats and Rua-owned data will be permanently deleted. The directory and its files will remain untouched.",
          )}
        </p>
      </Modal>
    </Page>
  );
}

function ProjectSection({
  title,
  projects,
  devicesById,
  focusedProjectId,
  archived = false,
  reopeningProjectId = null,
  onOpen,
  onArchive,
  onReopen,
  onDelete,
  onRename,
  onRestore,
}: {
  title: string;
  projects: ProjectDescriptor[];
  devicesById: Map<string, DeviceDto>;
  focusedProjectId: string | null;
  archived?: boolean;
  reopeningProjectId?: string | null;
  onOpen?: (project: ProjectDescriptor) => void | Promise<void>;
  onArchive?: (project: ProjectDescriptor) => void;
  onReopen?: (project: ProjectDescriptor) => void | Promise<void>;
  onDelete?: (project: ProjectDescriptor) => void;
  onRename: (project: ProjectDescriptor) => void;
  onRestore?: (project: ProjectDescriptor) => void;
}) {
  const { t } = useTranslation();
  if (projects.length === 0) return null;
  return (
    <section className={styles.section}>
      <div className={styles.sectionHeader}>
        {archived ? <Archive size={14} /> : <Folder size={14} />}
        <h2>{title}</h2>
        <span>{projects.length}</span>
      </div>
      <div className={styles.projectList}>
        {projects.map((project) => {
          const device = devicesById.get(project.resource_device_id);
          const availability = connectivityMeta(device?.connectivity_state, t);
          return (
            <div
              id={`project-${project.project_id}`}
              key={project.project_id}
              className={`${styles.projectRow} ${
                focusedProjectId === project.project_id
                  ? styles.projectRowFocused
                  : ""
              }`}
            >
              <button
                type="button"
                className={styles.projectOpen}
                disabled={!onOpen}
                onClick={() => void onOpen?.(project)}
              >
                <span className={styles.projectIcon}>
                  <Folder size={18} />
                </span>
                <div className={styles.projectCopy}>
                  <div className={styles.projectTitleLine}>
                    <strong>{project.display_name}</strong>
                    {archived ? (
                      <Tag>{t("projects.archived", "Archived")}</Tag>
                    ) : null}
                  </div>
                  <code title={project.display_path}>
                    {project.display_path}
                  </code>
                  <div className={styles.mobileAvailability}>
                    <span
                      className={`${styles.statusDot} ${
                        styles[availability.dotClass]
                      }`}
                    />
                    <span>{availability.label}</span>
                  </div>
                </div>
                <div className={styles.projectDevice}>
                  <span
                    className={`${styles.statusDot} ${
                      styles[availability.dotClass]
                    }`}
                  />
                  <span>
                    {device?.friendly_name ||
                      device?.label ||
                      project.resource_device_id}
                  </span>
                  <span className={styles.availabilityLabel}>
                    {availability.label}
                  </span>
                  {availability.showOfflineIcon ? <WifiOff size={13} /> : null}
                </div>
              </button>
              <div className={styles.projectActions}>
                <Tooltip title={t("projects.editProject")}>
                  <Btn
                    variant="ghost"
                    icon={<Pencil size={15} />}
                    onClick={() => onRename(project)}
                  />
                </Tooltip>
                {onRestore && (
                  <Btn variant="ghost" onClick={() => onRestore(project)}>
                    {t("navigation.restoreProject")}
                  </Btn>
                )}
                {!archived && onArchive ? (
                  <Tooltip title={t("projects.archive", "Archive Project")}>
                    <Btn
                      variant="ghost"
                      icon={<Archive size={15} />}
                      aria-label={t("projects.archive", "Archive Project")}
                      onClick={() => onArchive(project)}
                    />
                  </Tooltip>
                ) : null}
                {archived && onReopen ? (
                  <Btn
                    variant="ghost"
                    icon={<RotateCcw size={15} />}
                    loading={reopeningProjectId === project.project_id}
                    onClick={() => void onReopen(project)}
                  >
                    {t("projects.reopen", "Reopen")}
                  </Btn>
                ) : null}
                {archived && onDelete ? (
                  <Tooltip
                    title={t(
                      "projects.permanentDelete",
                      "Permanently Delete Project",
                    )}
                  >
                    <Btn
                      variant="ghost"
                      icon={<Trash2 size={15} />}
                      aria-label={t(
                        "projects.permanentDelete",
                        "Permanently Delete Project",
                      )}
                      onClick={() => onDelete(project)}
                    />
                  </Tooltip>
                ) : null}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

function connectivityMeta(
  state: DeviceDto["connectivity_state"] | undefined,
  t: TFunction,
): {
  label: string;
  dotClass: "statusOnline" | "statusOffline" | "statusWarning" | "statusDanger";
  showOfflineIcon: boolean;
} {
  switch (state) {
    case "connected":
      return {
        label: deviceConnectivityLabel(t, state),
        dotClass: "statusOnline",
        showOfflineIcon: false,
      };
    case "connecting":
      return {
        label: deviceConnectivityLabel(t, state),
        dotClass: "statusWarning",
        showOfflineIcon: false,
      };
    case "not_connected":
    default:
      return {
        label: deviceConnectivityLabel(t, state),
        dotClass: "statusOffline",
        showOfflineIcon: true,
      };
  }
}
