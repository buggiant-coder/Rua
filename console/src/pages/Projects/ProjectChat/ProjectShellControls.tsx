import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronRight,
  Copy,
  File,
  FileCode2,
  Files,
  Folder,
  FolderGit2,
  GitBranch,
  Link,
  LoaderCircle,
  LockKeyhole,
  RefreshCw,
  Server,
  ShieldCheck,
  TriangleAlert,
} from "lucide-react";

import {
  canOpenProjectDirectory,
  openProjectDirectory,
} from "@/desktop/runtime";
import type { ProjectResourcePolicyView } from "@/api/modules/projectChats";
import { projectsApi } from "@/api/modules/projects";
import type {
  ProjectDescriptor,
  ProjectDirectoryListing,
  ProjectGitCommit,
  ProjectGitStatus,
} from "@/api/types/project";
import { Btn, Drawer, Tabs, Tooltip, Select } from "@/ui";
import { subscribeConversationNavigation } from "@/layouts/conversationNavigationEvents";
import { extractFriendlyMessage } from "@/utils/error";
import { usePullPastBottom } from "@/hooks/usePullPastBottom";

import styles from "./index.module.less";
import { GitHistoryGraph } from "./GitHistoryGraph";

export type ProjectDrawerKind = "chats" | "workspace";

type ProjectShellDrawerProps = {
  workingDirectoryId?: string;
  localResourceDevice?: boolean;
  project: ProjectDescriptor;
  deviceLabel: string;
  availabilityLabel: string;
  lazy: boolean;
  resourcePolicy: ProjectResourcePolicyView | null;
  policyLoading: boolean;
  policyError: string | null;
  onClose: () => void;
};

function resourceProfileTranslationKey(
  profile: ProjectResourcePolicyView["policy"]["profile"],
): string {
  switch (profile) {
    case "read-only":
      return "projects.workspace.profileReadOnly";
    case "danger-full-access":
      return "projects.workspace.profileFullAccess";
    default:
      return "projects.workspace.profileResourceWrite";
  }
}

export function ProjectShellDrawer(props: ProjectShellDrawerProps) {
  const { t } = useTranslation();
  const { onClose } = props;
  const projectId = props.project.project_id;
  const [project, setProject] = useState<ProjectDescriptor | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedDirectoryId, setDirectoryId] = useState(
    props.workingDirectoryId,
  );

  useEffect(() => {
    let generation = 0;
    let disposed = false;
    const refresh = async () => {
      const request = ++generation;
      try {
        const current = await projectsApi.get(projectId);
        if (disposed || request !== generation) return;
        setProject(current);
        setError(null);
        setDirectoryId((selected) => {
          const directories = current.resources?.directories ?? [
            { directory_id: "initial" },
          ];
          return directories.some(
            (directory) => directory.directory_id === selected,
          )
            ? selected
            : current.resources?.primary_directory_id ?? "initial";
        });
      } catch (cause) {
        if (!disposed && request === generation)
          setError(extractFriendlyMessage(cause));
      }
    };
    void refresh();
    const unsubscribe = subscribeConversationNavigation((address) => {
      if (
        !address ||
        (address.scope === "project" && address.projectId === projectId)
      )
        void refresh();
    });
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, [projectId]);

  const directoryId =
    selectedDirectoryId ??
    project?.resources?.primary_directory_id ??
    "initial";
  const [activeTab, setActiveTab] = useState("files");
  return (
    <Drawer
      open
      onClose={onClose}
      title={t("projects.workspace.title")}
      width={420}
      className={styles.workspaceShellDrawer}
    >
      {error ? (
        <WorkspaceState icon={<TriangleAlert size={15} />} text={error} />
      ) : !project || project.project_id !== projectId ? (
        <WorkspaceState
          icon={<LoaderCircle className={styles.spin} size={15} />}
          text={t("projects.workspace.loadingFiles")}
        />
      ) : (
        <>
          <div style={{ padding: "12px 16px" }}>
            <Select
              aria-label={t("projects.workspace.targetDirectory")}
              value={directoryId}
              options={(
                project.resources?.directories ?? [
                  {
                    directory_id: "initial",
                    display_path: project.display_path,
                  },
                ]
              ).map((directory) => ({
                value: directory.directory_id,
                label: directory.display_path,
              }))}
              onChange={setDirectoryId}
            />
          </div>
          <ProjectShellDrawerContent
            key={`${project.project_id}:${directoryId}`}
            {...props}
            project={project}
            directoryId={directoryId}
            activeTab={activeTab}
            onTabChange={setActiveTab}
          />
        </>
      )}
    </Drawer>
  );
}

function ProjectShellDrawerContent({
  directoryId,
  localResourceDevice,
  activeTab,
  onTabChange,
  project,
  deviceLabel,
  availabilityLabel,
  lazy,
  resourcePolicy,
  policyLoading,
  policyError,
}: ProjectShellDrawerProps & {
  directoryId: string;
  activeTab: string;
  onTabChange: (tab: string) => void;
}) {
  const { t } = useTranslation();
  const rootPath =
    project.resources?.directories.find(
      (directory) => directory.directory_id === directoryId,
    )?.display_path ?? project.display_path;
  const [listings, setListings] = useState(
    () => new Map<string, ProjectDirectoryListing>(),
  );
  const [expanded, setExpanded] = useState(() => new Set<string>());
  const [filesLoading, setFilesLoading] = useState(false);
  const [filesError, setFilesError] = useState<string | null>(null);
  const [gitStatus, setGitStatus] = useState<ProjectGitStatus | null>(null);
  const [gitHistory, setGitHistory] = useState<ProjectGitCommit[]>([]);
  const [gitHistoryTotal, setGitHistoryTotal] = useState(0);
  const [gitLoading, setGitLoading] = useState(false);
  const [gitLoadingMore, setGitLoadingMore] = useState(false);
  const [gitError, setGitError] = useState<string | null>(null);

  const loadDirectory = useCallback(
    async (path: string, initial = false) => {
      if (initial) setFilesLoading(true);
      setFilesError(null);
      try {
        const listing = await projectsApi.browseResource(
          project.project_id,
          directoryId,
          initial ? "" : path,
        );
        setListings((current) => {
          const next = new Map(current);
          next.set(path, listing);
          next.set(listing.path, listing);
          return next;
        });
        setExpanded((current) => new Set(current).add(listing.path));
      } catch (error) {
        setFilesError(
          error instanceof Error
            ? error.message
            : t("projects.workspace.filesUnavailable"),
        );
      } finally {
        if (initial) setFilesLoading(false);
      }
    },
    [project.project_id, directoryId, t],
  );

  const loadGit = useCallback(
    async (offset = 0) => {
      const append = offset > 0;
      if (append) {
        setGitLoadingMore(true);
      } else {
        setGitLoading(true);
      }
      setGitError(null);
      try {
        const [status, history] = await Promise.all([
          projectsApi.getGitStatus(project.project_id, directoryId),
          projectsApi.getGitHistory(
            project.project_id,
            offset,
            30,
            directoryId,
          ),
        ]);
        if (status) setGitStatus(status);
        setGitHistory((current) => {
          if (!append) return history.items;
          const known = new Set(current.map((item) => item.oid));
          return [
            ...current,
            ...history.items.filter((item) => !known.has(item.oid)),
          ];
        });
        setGitHistoryTotal(history.total);
      } catch (error) {
        if (!append) {
          setGitStatus(null);
          setGitHistory([]);
          setGitHistoryTotal(0);
        }
        setGitError(
          error instanceof Error
            ? error.message
            : t("projects.workspace.gitUnavailable"),
        );
      } finally {
        setGitLoading(false);
        setGitLoadingMore(false);
      }
    },
    [project.project_id, directoryId, t],
  );

  const hasMoreGitHistory = gitHistory.length < gitHistoryTotal;
  const pullPastGitHistory = usePullPastBottom({
    enabled: hasMoreGitHistory && !gitLoading && !gitLoadingMore,
    onLoadMore: () => void loadGit(gitHistory.length),
  });

  useEffect(() => {
    setListings(new Map());
    setExpanded(new Set());
    void loadDirectory(rootPath, true);
    void loadGit();
  }, [loadDirectory, loadGit, rootPath]);

  const toggleDirectory = useCallback(
    (path: string) => {
      if (expanded.has(path)) {
        setExpanded((current) => {
          const next = new Set(current);
          next.delete(path);
          return next;
        });
        return;
      }
      setExpanded((current) => new Set(current).add(path));
      if (!listings.has(path)) void loadDirectory(path);
    },
    [expanded, listings, loadDirectory],
  );

  const filesPanel = (
    <div className={styles.workspacePanel}>
      <div className={styles.workspacePanelToolbar}>
        <span title={rootPath}>{project.display_name}</span>
        <Tooltip title={t("common.refresh")}>
          <Btn
            variant="ghost"
            size="sm"
            icon={<RefreshCw size={14} />}
            aria-label={t("projects.workspace.refreshFiles")}
            loading={filesLoading}
            onClick={() => void loadDirectory(rootPath, true)}
          />
        </Tooltip>
      </div>
      {filesError ? (
        <WorkspaceState icon={<TriangleAlert size={15} />} text={filesError} />
      ) : filesLoading && listings.size === 0 ? (
        <WorkspaceState
          icon={<LoaderCircle className={styles.spin} size={15} />}
          text={t("projects.workspace.loadingFiles")}
        />
      ) : (
        <div className={styles.fileTree}>
          <DirectoryRows
            path={rootPath}
            depth={0}
            listings={listings}
            expanded={expanded}
            onToggle={toggleDirectory}
            onCopy={(path) => void navigator.clipboard.writeText(path)}
            copyLabel={t("projects.workspace.copyPath")}
          />
        </div>
      )}
    </div>
  );

  const gitPanel = (
    <div
      className={styles.workspacePanel}
      onScroll={pullPastGitHistory.onScroll}
      onWheel={pullPastGitHistory.onWheel}
      onTouchStart={pullPastGitHistory.onTouchStart}
      onTouchMove={pullPastGitHistory.onTouchMove}
      onTouchEnd={pullPastGitHistory.onTouchEnd}
      onTouchCancel={pullPastGitHistory.onTouchEnd}
    >
      <div className={styles.workspacePanelToolbar}>
        <span>{t("projects.workspace.sourceControl")}</span>
        <Tooltip title={t("common.refresh")}>
          <Btn
            variant="ghost"
            size="sm"
            icon={<RefreshCw size={14} />}
            aria-label={t("projects.workspace.refreshGit")}
            loading={gitLoading}
            onClick={() => void loadGit()}
          />
        </Tooltip>
      </div>
      {gitError ? (
        <WorkspaceState icon={<TriangleAlert size={15} />} text={gitError} />
      ) : gitLoading && !gitStatus ? (
        <WorkspaceState
          icon={<LoaderCircle className={styles.spin} size={15} />}
          text={t("projects.workspace.loadingGit")}
        />
      ) : !gitStatus?.repository ? (
        <WorkspaceState
          icon={<FolderGit2 size={15} />}
          text={t("projects.workspace.notGitRepository")}
        />
      ) : (
        <div className={styles.gitView}>
          <div className={styles.gitBranchRow}>
            <GitBranch size={14} />
            <strong>
              {gitStatus.detached
                ? t("projects.workspace.detachedHead")
                : gitStatus.branch || t("projects.workspace.unknownBranch")}
            </strong>
            {gitStatus.ahead > 0 ? <span>+{gitStatus.ahead}</span> : null}
            {gitStatus.behind > 0 ? <span>-{gitStatus.behind}</span> : null}
          </div>
          <div className={styles.workspaceGroupHeader}>
            <span>{t("projects.workspace.changes")}</span>
            <span>{gitStatus.changes.length}</span>
          </div>
          {gitStatus.changes.length === 0 ? (
            <div className={styles.workspaceEmpty}>
              {t("projects.workspace.clean")}
            </div>
          ) : (
            gitStatus.changes.map((change) => (
              <div
                key={`${change.path}:${change.status}`}
                className={styles.gitChangeRow}
              >
                <FileCode2 size={14} />
                <span className={styles.workspaceListText} title={change.path}>
                  {change.path}
                </span>
                <small>{change.status}</small>
              </div>
            ))
          )}
          {gitHistory.length > 0 ? (
            <>
              <div className={styles.workspaceGroupHeader}>
                <span>{t("projects.workspace.history")}</span>
              </div>
              <div className={styles.gitHistoryList}>
                <GitHistoryGraph commits={gitHistory} />
                {hasMoreGitHistory ? (
                  <button
                    type="button"
                    className={styles.gitLoadMore}
                    disabled={gitLoadingMore}
                    onClick={() => void loadGit(gitHistory.length)}
                  >
                    {gitLoadingMore ? (
                      <LoaderCircle size={13} className={styles.spin} />
                    ) : null}
                    {t("projects.workspace.loadMore")}
                  </button>
                ) : null}
              </div>
            </>
          ) : null}
        </div>
      )}
    </div>
  );

  const contextPanel = (
    <div className={styles.workspaceContext}>
      <div className={styles.workspaceRoot}>
        <span className={styles.projectOptionMark}>
          <Folder size={16} />
        </span>
        <span>
          <strong>{project.display_name}</strong>
          <code title={rootPath}>{rootPath}</code>
        </span>
      </div>
      <dl className={styles.workspaceFacts}>
        <div>
          <dt>
            <Server size={13} />
            {t("projects.workspace.resource")}
          </dt>
          <dd>{deviceLabel}</dd>
        </div>
        <div>
          <dt>{t("projects.workspace.availability")}</dt>
          <dd>{availabilityLabel}</dd>
        </div>
      </dl>
      <div className={styles.workspaceSectionLabel}>
        {t("projects.workspace.execution")}
      </div>
      {lazy ? (
        <WorkspaceState
          icon={<LockKeyhole size={15} />}
          text={t("projects.workspace.afterFirstTurn")}
        />
      ) : policyLoading ? (
        <WorkspaceState
          icon={<LoaderCircle className={styles.spin} size={15} />}
          text={t("projects.workspace.loadingPolicy")}
        />
      ) : policyError ? (
        <WorkspaceState icon={<TriangleAlert size={15} />} text={policyError} />
      ) : resourcePolicy ? (
        <dl className={styles.workspaceFacts}>
          <div>
            <dt>
              <ShieldCheck size={13} />
              {t("projects.workspace.profile")}
            </dt>
            <dd>
              {t(resourceProfileTranslationKey(resourcePolicy.policy.profile))}
            </dd>
          </div>
          <div>
            <dt>{t("projects.workspace.approvals")}</dt>
            <dd>
              {t("projects.workspace.approvalsSummary", {
                pending: resourcePolicy.pending_approval_count,
                chats: resourcePolicy.chat_rule_count,
                agents: resourcePolicy.agent_rule_count,
              })}
            </dd>
          </div>
        </dl>
      ) : null}
    </div>
  );

  return (
    <>
      {localResourceDevice && canOpenProjectDirectory() && (
        <Btn
          variant="ghost"
          onClick={async () => {
            try {
              const listing = await projectsApi.browseResource(
                project.project_id,
                directoryId,
              );
              await openProjectDirectory(listing.path);
            } catch (error) {
              setFilesError(
                error instanceof Error ? error.message : String(error),
              );
            }
          }}
        >
          {t("projects.openResourceDirectory")}
        </Btn>
      )}
      <Tabs
        activeKey={activeTab}
        onChange={onTabChange}
        className={styles.workspaceTabs}
        items={[
          {
            key: "files",
            label: (
              <span className={styles.workspaceTabLabel}>
                <Files size={13} />
                {t("projects.workspace.files")}
              </span>
            ),
            children: filesPanel,
          },
          {
            key: "git",
            label: (
              <span className={styles.workspaceTabLabel}>
                <GitBranch size={13} />
                {t("projects.workspace.git")}
                {gitStatus?.repository && gitStatus.changes.length > 0 ? (
                  <small>{gitStatus.changes.length}</small>
                ) : null}
              </span>
            ),
            children: gitPanel,
          },
          {
            key: "context",
            label: t("projects.workspace.context"),
            children: contextPanel,
          },
        ]}
      />
    </>
  );
}

function WorkspaceState({
  icon,
  text,
}: {
  icon: React.ReactNode;
  text: string;
}) {
  return (
    <div className={styles.resourcePolicyState}>
      {icon}
      <span>{text}</span>
    </div>
  );
}

function DirectoryRows({
  path,
  depth,
  listings,
  expanded,
  onToggle,
  onCopy,
  copyLabel,
}: {
  path: string;
  depth: number;
  listings: Map<string, ProjectDirectoryListing>;
  expanded: Set<string>;
  onToggle: (path: string) => void;
  onCopy: (path: string) => void;
  copyLabel: string;
}) {
  const listing = listings.get(path);
  if (!listing) return null;
  return (
    <>
      {listing.entries
        .filter((entry) => entry.name !== ".git")
        .map((entry) => {
          const directory = entry.kind === "directory";
          const open = directory && expanded.has(entry.path);
          return (
            <div key={entry.path}>
              <div
                className={styles.fileTreeRow}
                style={{ paddingLeft: `${8 + depth * 14}px` }}
              >
                <button
                  type="button"
                  className={styles.fileTreeMain}
                  onClick={() =>
                    directory ? onToggle(entry.path) : onCopy(entry.path)
                  }
                >
                  <span className={styles.fileTreeChevron}>
                    {directory ? (
                      <ChevronRight
                        size={13}
                        className={open ? styles.fileTreeChevronOpen : ""}
                      />
                    ) : null}
                  </span>
                  {directory ? <Folder size={14} /> : <File size={14} />}
                  <span className={styles.workspaceListText} title={entry.path}>
                    {entry.name}
                  </span>
                  {entry.is_symlink ? <Link size={11} /> : null}
                </button>
                <Tooltip title={copyLabel}>
                  <button
                    type="button"
                    className={styles.fileTreeAction}
                    aria-label={copyLabel}
                    onClick={() => onCopy(entry.path)}
                  >
                    <Copy size={12} />
                  </button>
                </Tooltip>
              </div>
              {open ? (
                <DirectoryRows
                  path={entry.path}
                  depth={depth + 1}
                  listings={listings}
                  expanded={expanded}
                  onToggle={onToggle}
                  onCopy={onCopy}
                  copyLabel={copyLabel}
                />
              ) : null}
            </div>
          );
        })}
    </>
  );
}
