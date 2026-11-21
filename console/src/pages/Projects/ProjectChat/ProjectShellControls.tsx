import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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

import type { DeviceDto } from "@/api/modules/sync";
import type { ProjectResourceDirectory } from "@/api/types/project";
import { projectResourceDeviceLabel } from "../ProjectResourcePicker";
import { deviceConnectivityLabel } from "@/components/DeviceConnectivity";

import styles from "./index.module.less";
import { GitHistoryGraph } from "./GitHistoryGraph";

export type ProjectDrawerKind = "chats" | "workspace";

type ProjectShellDrawerProps = {
  devices: DeviceDto[];
  project: ProjectDescriptor;
  lazy: boolean;
  resourcePolicy: ProjectResourcePolicyView | null;
  policyLoading: boolean;
  policyError: string | null;
  onClose: () => void;
};

type DirectoryView = {
  listing?: ProjectDirectoryListing;
  error?: { message: string; offset: number };
};

function resourceProfileTranslationKey(
  profile: NonNullable<ProjectResourcePolicyView["policy"]>["profile"],
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
  const [selectedDirectoryId, setDirectoryId] = useState<string>();

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
          const directories = current.resources?.directories ?? [];
          return directories.some((d) => d.directory_id === selected)
            ? selected
            : directories[0]?.directory_id;
        });
      } catch (cause) {
        if (!disposed && request === generation)
          setError(extractFriendlyMessage(cause));
      }
    };
    void refresh();
    const unsubscribe = subscribeConversationNavigation(() => {
      void refresh();
    });
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, [projectId]);

  const directories = project?.resources?.directories ?? [];
  const directory =
    directories.find((d) => d.directory_id === selectedDirectoryId) ??
    directories[0];
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
          {directory && (
            <div style={{ padding: "12px 16px" }}>
              <Select
                aria-label={t("projects.workspace.targetDirectory")}
                value={directory.directory_id}
                options={directories.map((d) => ({
                  value: d.directory_id,
                  label: [
                    d.display_name,
                    projectResourceDeviceLabel(d.device_id, props.devices),
                    d.resource_root,
                  ]
                    .filter(Boolean)
                    .join(" · "),
                }))}
                onChange={setDirectoryId}
              />
            </div>
          )}
          <ProjectShellDrawerContent
            key={JSON.stringify([
              project.project_id,
              directory?.device_id,
              directory?.resource_root,
            ])}
            {...props}
            project={project}
            directory={directory}
            activeTab={activeTab}
            onTabChange={setActiveTab}
          />
        </>
      )}
    </Drawer>
  );
}

function ProjectShellDrawerContent({
  directory,
  devices,
  activeTab,
  onTabChange,
  project,
  lazy,
  resourcePolicy,
  policyLoading,
  policyError,
}: ProjectShellDrawerProps & {
  directory?: ProjectResourceDirectory;
  activeTab: string;
  onTabChange: (tab: string) => void;
}) {
  const { t } = useTranslation();
  const deviceId = directory?.device_id;
  const rootPath = directory?.resource_root ?? "";
  const target = useMemo(
    () =>
      deviceId && rootPath
        ? { device_id: deviceId, resource_root: rootPath }
        : undefined,
    [deviceId, rootPath],
  );
  const fileReads = useRef(new Map<string, AbortController>());
  const gitRead = useRef<AbortController | null>(null);

  const device = devices.find((d) => d.device_id === directory?.device_id);
  const localResourceDevice = device?.is_self;
  const deviceLabel =
    directory && projectResourceDeviceLabel(directory.device_id, devices);
  const availabilityLabel = deviceConnectivityLabel(
    t,
    device?.connectivity_state,
  );
  const [listings, setListings] = useState(
    () => new Map<string, DirectoryView>(),
  );
  const [expanded, setExpanded] = useState(() => new Set<string>());
  const [filesLoading, setFilesLoading] = useState(false);
  const [openError, setOpenError] = useState<string | null>(null);
  const [gitStatus, setGitStatus] = useState<ProjectGitStatus | null>(null);
  const [gitHistory, setGitHistory] = useState<ProjectGitCommit[]>([]);
  const [nextGitOffset, setNextGitOffset] = useState<number | null>(null);
  const [gitLoading, setGitLoading] = useState(false);
  const [gitLoadingMore, setGitLoadingMore] = useState(false);
  const [gitError, setGitError] = useState<string | null>(null);

  const loadDirectory = useCallback(
    async (path: string, refresh = false, offset = 0) => {
      if (!target) return;
      const requests = fileReads.current;
      if (refresh) {
        for (const request of requests.values()) request.abort();
        requests.clear();
        setFilesLoading(true);
        setOpenError(null);
      } else if (requests.has(path) || requests.has(target.resource_root))
        return;
      const request = new AbortController();
      requests.set(path, request);
      try {
        const listing = await projectsApi.browseResource(
          project.project_id,
          target,
          path,
          offset,
          request.signal,
        );
        if (request.signal.aborted) return;
        setListings((current) => {
          const next = refresh
            ? new Map<string, DirectoryView>()
            : new Map(current);
          const previous = offset ? current.get(path)?.listing : undefined;
          const merged = previous
            ? {
                ...listing,
                entries: Array.from(
                  new Map(
                    [...previous.entries, ...listing.entries].map((entry) => [
                      entry.path,
                      entry,
                    ]),
                  ).values(),
                ),
              }
            : listing;
          const view = { listing: merged };
          next.set(path, view);
          next.set(listing.path, view);
          return next;
        });
        if (refresh) setExpanded(new Set([listing.path]));
      } catch (error) {
        if (!request.signal.aborted) {
          const message = extractFriendlyMessage(error);
          setListings((current) =>
            new Map(current).set(path, {
              ...current.get(path),
              error: { message, offset },
            }),
          );
        }
      } finally {
        if (requests.get(path) === request) requests.delete(path);
        if (refresh && !request.signal.aborted) setFilesLoading(false);
      }
    },
    [project.project_id, target],
  );

  const loadGit = useCallback(
    async (offset = 0) => {
      if (!target) return;
      if (offset && gitRead.current) return;
      gitRead.current?.abort();
      const request = new AbortController();
      gitRead.current = request;
      setGitLoading(!offset);
      setGitLoadingMore(Boolean(offset));
      setGitError(null);
      try {
        const [status, history] = await Promise.all([
          offset
            ? null
            : projectsApi.getGitStatus(
                project.project_id,
                target,
                request.signal,
              ),
          projectsApi.getGitHistory(
            project.project_id,
            target,
            offset,
            request.signal,
          ),
        ]);
        if (request.signal.aborted) return;
        if (status) setGitStatus(status);
        setGitHistory((current) => {
          if (!offset) return history.items;
          const known = new Set(current.map((item) => item.oid));
          return [
            ...current,
            ...history.items.filter((item) => !known.has(item.oid)),
          ];
        });
        const next = offset + history.items.length;
        setNextGitOffset(
          history.items.length && next < history.total ? next : null,
        );
      } catch (error) {
        if (request.signal.aborted) return;
        if (!offset) {
          setGitStatus(null);
          setGitHistory([]);
          setNextGitOffset(null);
        }
        setGitError(extractFriendlyMessage(error));
      } finally {
        if (gitRead.current === request) {
          gitRead.current = null;
          setGitLoading(false);
          setGitLoadingMore(false);
        }
      }
    },
    [project.project_id, target],
  );

  const hasMoreGitHistory = nextGitOffset !== null;
  const pullPastGitHistory = usePullPastBottom({
    enabled: hasMoreGitHistory && !gitLoading && !gitLoadingMore,
    onLoadMore: () => {
      if (nextGitOffset !== null) void loadGit(nextGitOffset);
    },
  });

  useEffect(() => {
    const requests = fileReads.current;
    if (activeTab === "files") void loadDirectory(rootPath, true);
    if (activeTab === "git") void loadGit();
    return () => {
      for (const request of requests.values()) request.abort();
      requests.clear();
      gitRead.current?.abort();
      gitRead.current = null;
    };
  }, [activeTab, loadDirectory, loadGit, rootPath]);

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
      if (!listings.get(path)?.listing) void loadDirectory(path);
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
      {openError && (
        <WorkspaceState icon={<TriangleAlert size={15} />} text={openError} />
      )}
      {filesLoading && listings.size === 0 ? (
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
            onMore={(path, offset) => void loadDirectory(path, false, offset)}
            moreLabel={t("projects.workspace.loadMore")}
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
                    onClick={() =>
                      nextGitOffset !== null && void loadGit(nextGitOffset)
                    }
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
      {directory && (
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
      )}
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
              {t(
                resourcePolicy.policy
                  ? resourceProfileTranslationKey(resourcePolicy.policy.profile)
                  : "projects.workspace.agentPermissions",
              )}
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
      {target && localResourceDevice && canOpenProjectDirectory() && (
        <Btn
          variant="ghost"
          onClick={async () => {
            setOpenError(null);
            try {
              const listing = await projectsApi.browseResource(
                project.project_id,
                target,
                rootPath,
              );
              await openProjectDirectory(listing.path);
            } catch (error) {
              setOpenError(
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
            children: directory ? (
              filesPanel
            ) : (
              <p>{t("projects.noResourceFolders")}</p>
            ),
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
            children: directory ? (
              gitPanel
            ) : (
              <p>{t("projects.noResourceFolders")}</p>
            ),
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
  onMore,
  moreLabel,
}: {
  path: string;
  depth: number;
  listings: Map<string, DirectoryView>;
  expanded: Set<string>;
  onToggle: (path: string) => void;
  onCopy: (path: string) => void;
  copyLabel: string;
  onMore: (path: string, offset: number) => void;
  moreLabel: string;
}) {
  const { t } = useTranslation();
  const view = listings.get(path);
  if (!view) return null;
  const { listing, error } = view;
  const nextOffset = error?.offset === 0 ? null : listing?.next_offset;
  return (
    <>
      {error && (
        <div
          className={styles.workspaceEmpty}
          style={{ paddingLeft: `${8 + depth * 14}px` }}
          role="alert"
        >
          {t("projects.workspace.incompleteListing", {
            path,
            detail: error.message,
          })}
        </div>
      )}
      {listing?.entries
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
                  onMore={onMore}
                  moreLabel={moreLabel}
                  copyLabel={copyLabel}
                />
              ) : null}
            </div>
          );
        })}
      {!!listing?.omitted_entries && (
        <div
          className={styles.workspaceEmpty}
          style={{ paddingLeft: `${8 + depth * 14}px` }}
        >
          {t("projects.workspace.omittedEntries", {
            path,
            count: listing.omitted_entries,
          })}
        </div>
      )}
      {nextOffset != null && (
        <button
          type="button"
          className={styles.gitLoadMore}
          onClick={() => onMore(path, nextOffset)}
        >
          {moreLabel}
        </button>
      )}
    </>
  );
}
