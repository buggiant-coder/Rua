import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useTranslation } from "react-i18next";
import {
  Pencil,
  Archive,
  FolderArchive,
  FolderOpen,
  EyeOff,
  List,
  Pin,
  PinOff,
  ArrowUp,
  ArrowDown,
  ChevronRight,
  Folder,
  LayoutGrid,
  Check,
  MessageSquare,
  MoreHorizontal,
  Plus,
  Search,
  SquarePen,
  X,
} from "lucide-react";

import type { DropdownItem } from "@/ui/Dropdown";
import { chatApi } from "@/api/modules/chat";
import { projectChatsApi } from "@/api/modules/projectChats";
import { projectsApi } from "@/api/modules/projects";
import { devicesApi, type DeviceDto } from "@/api/modules/sync";
import { useProjectSidebar } from "@/pages/Projects/useProjectSidebar";
import ProjectArchiveChatsDialog from "@/pages/Projects/ProjectArchiveChatsDialog";
import ProjectArchiveDialog from "@/pages/Projects/ProjectArchiveDialog";
import ProjectEditDialog from "@/pages/Projects/ProjectEditDialog";
import { ProjectShellDrawer } from "@/pages/Projects/ProjectChat/ProjectShellControls";
import CreateProjectDialog from "@/pages/Projects/CreateProjectDialog";
import type { ChatSearchResult } from "@/api/types";
import type { ProjectDescriptor } from "@/api/types/project";
import {
  conversationPath,
  type ConversationAddress,
} from "@/pages/Chat/conversationAddress";
import {
  resolveChatEntry,
  resolveOrCreateDraftChatId,
} from "@/pages/Chat/chatNavigation";
import {
  createProjectChat,
  resolveProjectChatPath,
} from "@/pages/Projects/projectChatNavigation";
import { useAgentStore } from "@/stores/agentStore";
import { Dropdown, Select, Modal, TextInput, toast } from "@/ui";
import {
  subscribeConversationNavigation,
  invalidateConversationNavigation,
  requestNewConversation,
} from "../conversationNavigationEvents";
import {
  scopeAddress,
  scopeKey,
  useConversationGroup,
  type NavigationScope,
} from "./useConversationGroups";
import {
  getProjectNavigationAgent,
  subscribeProjectNavigationAgent,
} from "../conversationNavigationAgent";
import SidebarDragHandle, { type SidebarDropTarget } from "./SidebarDragHandle";
import ConversationRow from "./ConversationRow";
import CurrentArchivedConversation from "./CurrentArchivedConversation";
import { isDraftConversation } from "@/pages/Chat/conversationLifecycle";
import styles from "./index.module.less";
import type { ProjectSort } from "./projectSort";

const EXPANDED_KEY = "rua_navigation_expanded_groups";
const SCROLL_KEY = "rua_navigation_scroll";

function readExpanded(pathname: string): string[] {
  try {
    const raw = localStorage.getItem(EXPANDED_KEY);
    if (raw === null) {
      const current = addressFromPath(pathname);
      return current
        ? ["independent", scopeKey(scopeFromAddress(current))]
        : ["independent"];
    }
    const stored: unknown = JSON.parse(raw);
    return Array.isArray(stored)
      ? stored.filter((key): key is string => typeof key === "string")
      : [];
  } catch {
    return [];
  }
}

function addressFromPath(pathname: string): ConversationAddress | null {
  const match = /^\/(agents|projects)\/([^/]+)\/chats\/([^/]+)\/?$/.exec(
    pathname,
  );
  if (!match) return null;
  try {
    return match[1] === "agents"
      ? {
          scope: "agent",
          agentId: decodeURIComponent(match[2]),
          chatId: decodeURIComponent(match[3]),
        }
      : {
          scope: "project",
          projectId: decodeURIComponent(match[2]),
          chatId: decodeURIComponent(match[3]),
        };
  } catch {
    return null;
  }
}

function scopeFromAddress(address: ConversationAddress): NavigationScope {
  return address.scope === "agent"
    ? { kind: "agent", id: address.agentId }
    : { kind: "project", id: address.projectId };
}

async function managementPath(scope: NavigationScope): Promise<string> {
  const path =
    scope.kind === "project"
      ? await resolveProjectChatPath(scope.id)
      : conversationPath(scopeAddress(scope, resolveChatEntry(scope.id)));
  return `${path}?conversations=1`;
}

type Props = {
  location: { pathname: string; search: string };
  onNavigate: (path: string) => void;
};

export default function ConversationNavigation({
  location,
  onNavigate,
}: Props) {
  const { t, i18n } = useTranslation();
  const agents = useAgentStore((state) => state.agents);
  const selectedAgent = useAgentStore((state) => state.selectedAgent);
  const catalogLoading = useAgentStore((state) => state.catalogLoading);
  const catalogError = useAgentStore((state) => state.catalogError);
  const ensureCatalogLoaded = useAgentStore(
    (state) => state.ensureCatalogLoaded,
  );
  const refreshCatalog = useAgentStore((state) => state.refreshCatalog);
  const [projects, setProjects] = useState<ProjectDescriptor[]>([]);
  const sidebar = useProjectSidebar(
    projects,
    i18n?.resolvedLanguage || i18n?.language || "zh",
  );
  const { sort: projectSort, sortedProjects, sections, sectionOf } = sidebar;
  const [orderAnnouncement, setOrderAnnouncement] = useState("");
  const [sectionEditor, setSectionEditor] = useState<{
    id: string | null;
    name: string;
  } | null>(null);
  const [moveToSection, setMoveToSection] = useState<{
    id: string;
    section: string;
  } | null>(null);
  const [projectAction, setProjectAction] = useState<{
    kind: "edit" | "archive" | "resources" | "archive-chats";
    project: ProjectDescriptor;
  } | null>(null);
  const [createProjectOpen, setCreateProjectOpen] = useState(false);
  const [devices, setDevices] = useState<DeviceDto[]>([]);
  const [projectsLoading, setProjectsLoading] = useState(true);
  const projectsLoaded = useRef(false);
  const [projectsError, setProjectsError] = useState<string | null>(null);
  const projectRequest = useRef(0);
  const projectsRefreshing = useRef(false);
  const projectRefreshQueued = useRef(false);
  const projectsMounted = useRef(true);
  const [expanded, setExpanded] = useState(() =>
    readExpanded(location.pathname),
  );
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchScopeKey, setSearchScopeKey] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const restoringScroll = useRef<number | null>(null);
  const clickedRow = useRef<{ element: HTMLElement; top: number } | null>(null);
  const independentListRef = useRef<HTMLDivElement>(null);
  const restoreScroll = useCallback(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    if (restoringScroll.current !== null) {
      scroll.scrollTop = restoringScroll.current;
    } else if (clickedRow.current?.element.isConnected) {
      const { element, top } = clickedRow.current;
      scroll.scrollTop +=
        element.getBoundingClientRect().top -
        scroll.getBoundingClientRect().top -
        top;
    }
  }, []);
  const releaseScroll = () => {
    restoringScroll.current = null;
    clickedRow.current = null;
  };
  const previousPath = useRef(location.pathname);
  const current = addressFromPath(location.pathname);
  const currentAddressRef = useRef(current);
  currentAddressRef.current = current;
  useEffect(() => {
    currentAddressRef.current = addressFromPath(location.pathname);
    return () => {
      currentAddressRef.current = null;
    };
  }, [location.pathname]);
  const getCurrentAddress = () => currentAddressRef.current;
  const currentScope: NavigationScope = current
    ? scopeFromAddress(current)
    : { kind: "agent", id: selectedAgent };
  const currentKey = scopeKey(currentScope);
  const currentPath = current ? conversationPath(current) : null;
  const lastRevealedPath = useRef(currentPath);
  const projectAgent = useSyncExternalStore(
    subscribeProjectNavigationAgent,
    getProjectNavigationAgent,
  );
  const independentAgentId =
    current?.scope === "agent"
      ? current.agentId
      : current?.scope === "project"
      ? projectAgent?.projectId === current.projectId &&
        projectAgent.chatId === current.chatId
        ? projectAgent.agentId
        : null
      : selectedAgent;
  const [displayedIndependentAgentId, setDisplayedIndependentAgentId] =
    useState(independentAgentId);
  useLayoutEffect(() => {
    if (
      !independentAgentId ||
      independentAgentId === displayedIndependentAgentId
    )
      return;
    // Keep the old space while the new Agent's first page is loading.
    const list = independentListRef.current;
    if (list) list.style.minHeight = `${list.getBoundingClientRect().height}px`;
    setDisplayedIndependentAgentId(independentAgentId);
  }, [independentAgentId, displayedIndependentAgentId]);
  const independentReady = useCallback(() => {
    if (independentListRef.current)
      independentListRef.current.style.minHeight = "";
    restoreScroll();
  }, [restoreScroll]);
  const independentAgent = agents.find(
    (agent) => agent.id === independentAgentId,
  );
  const independentScope: NavigationScope | null = independentAgentId
    ? { kind: "agent", id: independentAgentId }
    : null;
  const currentScopeName =
    currentScope.kind === "project"
      ? projects.find((project) => project.project_id === currentScope.id)
          ?.display_name || t("projects.title")
      : t("navigation.independentChats");

  const loadProjects = useCallback(async function refreshProjects() {
    if (!projectsMounted.current) return;
    if (projectsRefreshing.current) {
      projectRefreshQueued.current = true;
      return;
    }
    projectsRefreshing.current = true;
    projectRefreshQueued.current = false;
    const request = ++projectRequest.current;
    setProjectsLoading(true);
    setProjectsError(null);
    try {
      const result = await projectsApi.list("active");
      if (request === projectRequest.current) {
        projectsLoaded.current = true;
        setProjects(result);
      }
    } catch (error) {
      if (request === projectRequest.current)
        setProjectsError(
          error instanceof Error ? error.message : String(error),
        );
    } finally {
      if (request === projectRequest.current) setProjectsLoading(false);
      projectsRefreshing.current = false;
      if (projectRefreshQueued.current && projectsMounted.current)
        void refreshProjects();
    }
  }, []);

  useEffect(() => {
    projectsMounted.current = true;
    void ensureCatalogLoaded().catch(() => {});
    void loadProjects();
    let refreshTimer: ReturnType<typeof setTimeout> | undefined;
    const unsubscribe = subscribeConversationNavigation((address) => {
      if (address && address.scope !== "project") return;
      if (refreshTimer !== undefined) clearTimeout(refreshTimer);
      refreshTimer = setTimeout(() => {
        void loadProjects();
      }, 150);
    });
    // Sync writes canonical Project files outside this page's mutation events.
    // Reconcile the catalog at the same visible cadence as Sidebar metadata.
    const foreground = () => {
      if (document.visibilityState !== "hidden")
        invalidateConversationNavigation(undefined, true);
    };
    window.addEventListener("focus", foreground);
    window.addEventListener("rua:foreground", foreground);
    document.addEventListener("visibilitychange", foreground);
    const poll = setInterval(foreground, 10000);
    return () => {
      projectsMounted.current = false;
      projectRefreshQueued.current = false;
      projectRequest.current += 1;
      if (refreshTimer !== undefined) clearTimeout(refreshTimer);
      clearInterval(poll);
      window.removeEventListener("focus", foreground);
      window.removeEventListener("rua:foreground", foreground);
      document.removeEventListener("visibilitychange", foreground);
      unsubscribe();
    };
  }, [ensureCatalogLoaded, loadProjects]);

  useEffect(() => {
    if (!createProjectOpen) return;
    let cancelled = false;
    void devicesApi
      .listDevices()
      .then((result) => {
        if (!cancelled) setDevices(result);
      })
      .catch((error: unknown) => {
        if (!cancelled)
          toast.error(error instanceof Error ? error.message : String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [createProjectOpen]);

  useEffect(() => {
    setSearchOpen(false);
    setSearchQuery("");
  }, [currentKey]);

  useEffect(() => {
    if (previousPath.current === location.pathname) return;
    previousPath.current = location.pathname;
    restoringScroll.current = null;
  }, [location.pathname]);

  useEffect(() => {
    if (!currentPath || currentPath === lastRevealedPath.current) return;
    lastRevealedPath.current = currentPath;
    setExpanded((keys) =>
      Array.from(
        new Set([
          ...keys,
          ...(currentKey.startsWith("agent:") ? ["independent"] : []),
          currentKey,
        ]),
      ),
    );
  }, [currentKey, currentPath]);

  useEffect(() => {
    try {
      localStorage.setItem(EXPANDED_KEY, JSON.stringify(expanded));
    } catch {
      /* Browser storage is optional. */
    }
  }, [expanded]);

  useLayoutEffect(() => {
    try {
      restoringScroll.current = Number(localStorage.getItem(SCROLL_KEY)) || 0;
    } catch {
      /* Browser storage is optional. */
    }
    restoreScroll();
    if (typeof ResizeObserver === "undefined" || !contentRef.current) return;
    const observer = new ResizeObserver(restoreScroll);
    observer.observe(contentRef.current);
    return () => observer.disconnect();
  }, [restoreScroll]);
  useLayoutEffect(restoreScroll);

  const toggle = (key: string) =>
    setExpanded((keys) =>
      keys.includes(key)
        ? keys.filter((value) => value !== key)
        : [...keys, key],
    );
  const newChat = (scope: NavigationScope) => {
    if (current && scopeKey(scope) === currentKey) {
      requestNewConversation(current);
      return;
    }
    const chatId =
      scope.kind === "project"
        ? createProjectChat(scope.id)
        : resolveOrCreateDraftChatId(scope.id);
    onNavigate(conversationPath(scopeAddress(scope, chatId)));
  };
  const manage = (scope: NavigationScope) => {
    void managementPath(scope).then(onNavigate);
  };
  const scopes = [
    ...(independentScope
      ? [
          {
            scope: independentScope,
            name: `${t("navigation.independentChats")} · ${
              independentAgent?.name || independentAgentId
            }`,
          },
        ]
      : []),
    ...sortedProjects.map((project) => ({
      scope: { kind: "project" as const, id: project.project_id },
      name: project.display_name,
    })),
  ];
  const searchScope = scopes.find(
    (item) => scopeKey(item.scope) === searchScopeKey,
  );
  const independentOpen = expanded.includes("independent");

  const announceMove = async (operation: Promise<boolean>) => {
    if (await operation)
      setOrderAnnouncement(t("navigation.projectManualOrderSaved"));
  };
  const dropProject = (id: string, target: SidebarDropTarget) => {
    void announceMove(sidebar.moveProject(id, target));
  };
  const dropSection = (id: string, area: string) => {
    void sidebar.moveSection(id, area);
  };
  const projectMenu = (id: string): DropdownItem[] => {
    const pinned = Boolean(sidebar.snapshot?.items[id]?.pinned);
    const moves = sidebar.projectMoves(id);
    const disabled = sidebar.pending || !sidebar.snapshot;
    const project = projects.find((project) => project.project_id === id)!;
    return [
      {
        key: "pin",
        label: t(pinned ? "navigation.unpinProject" : "navigation.pinProject"),
        icon: pinned ? <PinOff size={16} /> : <Pin size={16} />,
        disabled,
        onClick: () =>
          void sidebar.update({
            action: "set_pinned",
            project_id: id,
            pinned: !pinned,
          }),
      },
      {
        key: "edit",
        label: t("projects.editProject"),
        icon: <Pencil size={16} />,
        onClick: () => setProjectAction({ kind: "edit", project }),
      },
      { type: "divider" },
      {
        key: "section",
        label: t("navigation.moveToSection"),
        icon: <List size={16} />,
        disabled,
        onClick: () => setMoveToSection({ id, section: sectionOf(id) ?? "" }),
      },
      {
        key: "move-up",
        label: t("navigation.moveProjectUp"),
        icon: <ArrowUp size={16} />,
        disabled: disabled || !moves.up,
        onClick: () => void announceMove(sidebar.stepProject(id, -1)),
      },
      {
        key: "move-down",
        label: t("navigation.moveProjectDown"),
        icon: <ArrowDown size={16} />,
        disabled: disabled || !moves.down,
        onClick: () => void announceMove(sidebar.stepProject(id, 1)),
      },
      { type: "divider" },
      {
        key: "resources",
        label: t("projects.browseResources"),
        icon: <FolderOpen size={16} />,
        onClick: () => setProjectAction({ kind: "resources", project }),
      },
      {
        key: "manage",
        label: t("projects.manageChats"),
        icon: <MessageSquare size={16} />,
        onClick: () => manage({ kind: "project", id }),
      },
      {
        key: "archive-chats",
        label: t("projects.archiveChats"),
        icon: <Archive size={16} />,
        onClick: () => setProjectAction({ kind: "archive-chats", project }),
      },
      { type: "divider" },
      {
        key: "hide",
        label: t("navigation.hideProject"),
        icon: <EyeOff size={16} />,
        disabled,
        onClick: () =>
          void sidebar.update({
            action: "set_hidden",
            project_id: id,
            hidden: true,
          }),
      },
      {
        key: "archive-project",
        label: t("projects.archiveProject", "Archive Project"),
        icon: <FolderArchive size={16} />,
        onClick: () => setProjectAction({ kind: "archive", project }),
      },
    ];
  };
  const group = (scope: NavigationScope, name: string, open: boolean) => (
    <ConversationGroup
      key={scopeKey(scope)}
      scope={scope}
      name={name}
      open={open}
      current={current}
      getCurrentAddress={getCurrentAddress}
      onToggle={() => toggle(scopeKey(scope))}
      onNew={() => newChat(scope)}
      onManage={() => manage(scope)}
      projectMenu={scope.kind === "project" ? projectMenu(scope.id) : undefined}
      projectTitle={
        scope.kind === "project" ? (
          <SidebarDragHandle
            onDropTarget={(target) => dropProject(scope.id, target)}
            dragDisabled={sidebar.pending || !sidebar.snapshot}
            className={styles.groupToggle}
            aria-expanded={open}
            onClick={() => toggle(scopeKey(scope))}
          >
            {open ? (
              <FolderOpen size={16} aria-hidden="true" />
            ) : (
              <Folder size={16} aria-hidden="true" />
            )}
            <span>{name}</span>
          </SidebarDragHandle>
        ) : undefined
      }
      onNavigate={onNavigate}
    />
  );

  return (
    <nav
      className={styles.navigation}
      aria-label={t("navigation.conversations")}
    >
      <div className={styles.tools}>
        <button
          type="button"
          className={styles.toolButton}
          aria-label={t("navigation.newConversation")}
          title={t("navigation.newInGroup", { name: currentScopeName })}
          onClick={() => newChat(currentScope)}
        >
          <SquarePen size={17} />
          <span>{t("navigation.newConversation")}</span>
          <small className={styles.newScope}>{currentScopeName}</small>
        </button>
        <button
          type="button"
          className={styles.toolButton}
          aria-expanded={searchOpen}
          onClick={() => {
            setSearchScopeKey(currentKey);
            setSearchOpen((value) => !value);
          }}
        >
          <Search size={17} />
          <span>{t("navigation.search")}</span>
        </button>
      </div>
      {searchOpen && (
        <div className={styles.searchControls}>
          <div className={styles.searchScopeRow}>
            <Select
              className={styles.scopeSelect}
              aria-label={t("navigation.searchScope")}
              value={searchScope ? searchScopeKey : undefined}
              placeholder={t("navigation.searchScope")}
              onChange={(value) => {
                if (value) setSearchScopeKey(value);
              }}
              options={scopes.map((item) => ({
                value: scopeKey(item.scope),
                label: item.name,
              }))}
            />
            <button
              type="button"
              className={styles.iconButton}
              aria-label={t("navigation.closeSearch")}
              onClick={() => setSearchOpen(false)}
            >
              <X size={16} />
            </button>
          </div>
          <input
            autoFocus
            type="search"
            aria-label={t("navigation.search")}
            placeholder={t("navigation.searchPlaceholder")}
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
          />
          <p>{t("navigation.searchHint")}</p>
        </div>
      )}
      <div
        className={styles.scroll}
        data-sidebar-scroll
        ref={scrollRef}
        onWheel={releaseScroll}
        onTouchStart={releaseScroll}
        onPointerDown={releaseScroll}
        onKeyDown={releaseScroll}
        onClickCapture={(event) => {
          const element = (event.target as Element).closest<HTMLElement>(
            `.${styles.chatLink}`,
          );
          if (!element || !scrollRef.current) return;
          clickedRow.current = {
            element,
            top:
              element.getBoundingClientRect().top -
              scrollRef.current.getBoundingClientRect().top,
          };
        }}
        onScroll={(event) => {
          if (restoringScroll.current !== null) return;
          try {
            localStorage.setItem(
              SCROLL_KEY,
              String(event.currentTarget.scrollTop),
            );
          } catch {
            /* Browser storage is optional. */
          }
        }}
      >
        <div ref={contentRef}>
          {searchOpen && searchQuery.trim() && searchScope && (
            <ScopedSearch
              scope={searchScope.scope}
              query={searchQuery}
              onNavigate={(path) => {
                restoringScroll.current = null;
                setSearchOpen(false);
                setSearchQuery("");
                onNavigate(path);
              }}
            />
          )}
          <div hidden={Boolean(searchOpen && searchQuery.trim())}>
            <section className={styles.group}>
              <div className={styles.groupHeader}>
                <button
                  type="button"
                  className={styles.groupToggle}
                  aria-expanded={independentOpen}
                  onClick={() => toggle("independent")}
                >
                  <ChevronRight
                    size={14}
                    className={independentOpen ? styles.expanded : undefined}
                  />
                  <MessageSquare size={16} />
                  <span>{t("navigation.independentChats")}</span>
                </button>
                <button
                  type="button"
                  className={styles.iconButton}
                  aria-label={t("navigation.newInGroup", {
                    name: t("navigation.independentChats"),
                  })}
                  disabled={!independentScope}
                  onClick={() => independentScope && newChat(independentScope)}
                >
                  <Plus size={16} />
                </button>
              </div>
              <div
                ref={independentListRef}
                hidden={!independentOpen}
                aria-hidden={!independentScope || undefined}
                style={{ visibility: independentScope ? undefined : "hidden" }}
              >
                {catalogLoading && (
                  <p className={styles.status}>{t("common.loading")}</p>
                )}
                {catalogError && (
                  <LoadError
                    error={catalogError}
                    onRetry={() => void refreshCatalog().catch(() => {})}
                  />
                )}
                {displayedIndependentAgentId && (
                  <ConversationGroup
                    key={`agent:${displayedIndependentAgentId}`}
                    scope={{ kind: "agent", id: displayedIndependentAgentId }}
                    name={independentAgent?.name || independentAgentId || ""}
                    open={independentOpen}
                    hideHeader
                    current={current}
                    getCurrentAddress={getCurrentAddress}
                    onToggle={() => {}}
                    onNew={() => independentScope && newChat(independentScope)}
                    onManage={() =>
                      independentScope && manage(independentScope)
                    }
                    onNavigate={onNavigate}
                    onReady={independentReady}
                  />
                )}
              </div>
            </section>
            <div className={styles.sectionLabel}>
              <span>{t("projects.title")}</span>
              <Dropdown
                items={[
                  {
                    key: "create",
                    label: t("projects.newProject"),
                    icon: <Plus size={16} />,
                    onClick: () => setCreateProjectOpen(true),
                  },
                  {
                    key: "create-section",
                    label: t("navigation.createSection"),
                    disabled: sidebar.pending || !sidebar.snapshot,
                    onClick: () => setSectionEditor({ id: null, name: "" }),
                  },
                  {
                    key: "manage",
                    label: t("navigation.manageProjects"),
                    icon: <LayoutGrid size={16} />,
                    onClick: () => onNavigate("/projects?overview=1"),
                  },
                  { type: "divider" },
                  ...(
                    [
                      "manual",
                      "name",
                      "created_at",
                      "last_activity_at",
                    ] as ProjectSort[]
                  ).map((value) => ({
                    key: value,
                    label: t(
                      value === "manual"
                        ? "navigation.projectSortManual"
                        : value === "name"
                        ? "navigation.projectSortName"
                        : value === "created_at"
                        ? "navigation.projectSortCreated"
                        : "navigation.projectSortActivity",
                    ),
                    icon: (
                      <span className={styles.sortCheck}>
                        {projectSort === value && <Check size={15} />}
                      </span>
                    ),
                    disabled: sidebar.pending || !sidebar.snapshot,
                    onClick: () =>
                      void sidebar.update({ action: "set_sort", sort: value }),
                  })),
                ]}
                dropdownRender={(menu) => (
                  <>
                    <div className={styles.projectMenuTitle}>
                      {t("navigation.projectActions")}
                    </div>
                    {menu}
                  </>
                )}
              >
                <button
                  type="button"
                  className={styles.iconButton}
                  aria-label={t("navigation.projectActions")}
                  title={t("navigation.projectActions")}
                >
                  <MoreHorizontal size={16} />
                </button>
              </Dropdown>
            </div>
            {projectsLoading && !projectsLoaded.current && (
              <p className={styles.status}>{t("common.loading")}</p>
            )}
            {projectsError && (
              <LoadError
                error={projectsError}
                onRetry={() => void loadProjects()}
              />
            )}
            {!projectsLoading && !projectsError && projects.length === 0 && (
              <p className={styles.status}>{t("navigation.noProjects")}</p>
            )}
            {sidebar.error && (
              <LoadError
                error={sidebar.error}
                onRetry={() => void sidebar.refresh()}
              />
            )}
            <span className={styles.srOnly} aria-live="polite">
              {orderAnnouncement}
            </span>
            {[
              ...(sortedProjects.some(
                (p) => sidebar.snapshot?.items[p.project_id]?.pinned,
              )
                ? [{ id: "pinned", name: t("navigation.pinnedProjects") }]
                : []),
              { id: "", name: t("navigation.defaultSection") },
              ...sections.map((section) => ({
                id: section.section_id,
                name: section.name,
              })),
            ].map((area) => (
              <section
                key={area.id}
                data-project-drop-area={area.id}
                aria-label={area.name}
              >
                {(sections.length > 0 || area.id === "pinned") && (
                  <div className={styles.sectionLabel}>
                    <span>{area.name}</span>
                    {area.id && area.id !== "pinned" && (
                      <>
                        <SidebarDragHandle
                          dragDisabled={sidebar.pending || !sidebar.snapshot}
                          kind="section"
                          onDropTarget={(target) =>
                            dropSection(area.id, target.area)
                          }
                          aria-label={t("navigation.reorderSection", {
                            name: area.name,
                          })}
                        />
                        <Dropdown
                          items={[
                            {
                              key: "rename",
                              label: t("common.rename"),
                              onClick: () =>
                                setSectionEditor({
                                  id: area.id,
                                  name: area.name,
                                }),
                            },
                            ...([-1, 1] as const).map((direction) => ({
                              key: String(direction),
                              label: t(
                                direction < 0
                                  ? "navigation.moveProjectUp"
                                  : "navigation.moveProjectDown",
                              ),
                              disabled:
                                sidebar.pending ||
                                !(direction < 0
                                  ? sidebar.sectionMoves(area.id).up
                                  : sidebar.sectionMoves(area.id).down),
                              onClick: () =>
                                void sidebar.stepSection(area.id, direction),
                            })),
                            {
                              key: "delete",
                              label: t("navigation.deleteSection"),
                              disabled: sidebar.pending,
                              onClick: () =>
                                void sidebar.update({
                                  action: "delete_section",
                                  section_id: area.id,
                                }),
                            },
                          ]}
                        >
                          <button
                            type="button"
                            className={styles.iconButton}
                            aria-label={t("navigation.sectionActions", {
                              name: area.name,
                            })}
                          >
                            <MoreHorizontal size={14} />
                          </button>
                        </Dropdown>
                      </>
                    )}
                  </div>
                )}
                {sortedProjects
                  .filter((project) =>
                    area.id === "pinned"
                      ? sidebar.snapshot?.items[project.project_id]?.pinned
                      : !sidebar.snapshot?.items[project.project_id]?.pinned &&
                        (sectionOf(project.project_id) ?? "") === area.id,
                  )
                  .map((project) => (
                    <div
                      key={project.project_id}
                      data-project-drop-id={project.project_id}
                    >
                      {group(
                        { kind: "project", id: project.project_id },
                        project.display_name,
                        expanded.includes(`project:${project.project_id}`),
                      )}
                    </div>
                  ))}
              </section>
            ))}
          </div>
        </div>
      </div>
      <Modal
        open={sectionEditor !== null}
        title={t(
          sectionEditor?.id
            ? "navigation.renameSection"
            : "navigation.createSection",
        )}
        onClose={() => setSectionEditor(null)}
        confirmLoading={sidebar.pending}
        onOk={async () => {
          if (!sectionEditor) return;
          const ok = await sidebar.update(
            sectionEditor.id
              ? {
                  action: "rename_section",
                  section_id: sectionEditor.id,
                  name: sectionEditor.name,
                }
              : { action: "create_section", name: sectionEditor.name },
          );
          if (ok) setSectionEditor(null);
        }}
      >
        <TextInput
          autoFocus
          aria-label={t("navigation.sectionName")}
          value={sectionEditor?.name ?? ""}
          onChange={(event) =>
            setSectionEditor(
              (current) => current && { ...current, name: event.target.value },
            )
          }
        />
      </Modal>
      <Modal
        open={moveToSection !== null}
        title={t("navigation.moveToSection")}
        onClose={() => setMoveToSection(null)}
        confirmLoading={sidebar.pending}
        onOk={async () => {
          if (
            moveToSection &&
            (await sidebar.update({
              action: "move_to_section",
              project_id: moveToSection.id,
              section_id: moveToSection.section || null,
            }))
          )
            setMoveToSection(null);
        }}
      >
        <Select
          aria-label={t("navigation.sectionName")}
          value={moveToSection?.section ?? ""}
          options={[
            { value: "", label: t("navigation.defaultSection") },
            ...sections.map((section) => ({
              value: section.section_id,
              label: section.name,
            })),
          ]}
          onChange={(section) =>
            setMoveToSection((current) => current && { ...current, section })
          }
        />
      </Modal>
      {projectAction?.kind === "edit" && (
        <ProjectEditDialog
          key={projectAction.project.project_id}
          project={projectAction.project}
          onClose={() => setProjectAction(null)}
          onSaved={(updated) => {
            setProjects((current) =>
              current.map((project) =>
                project.project_id === updated.project_id ? updated : project,
              ),
            );
            invalidateConversationNavigation();
            setProjectAction(null);
          }}
        />
      )}
      {projectAction?.kind === "archive-chats" && (
        <ProjectArchiveChatsDialog
          project={projectAction.project}
          onClose={() => setProjectAction(null)}
        />
      )}
      {projectAction?.kind === "archive" && (
        <ProjectArchiveDialog
          project={projectAction.project}
          onClose={() => setProjectAction(null)}
          onArchived={(updated) => {
            setProjects((current) =>
              current.filter(
                (project) => project.project_id !== updated.project_id,
              ),
            );
            setProjectAction(null);
            if (
              current?.scope === "project" &&
              current.projectId === updated.project_id
            )
              onNavigate("/projects?overview=1");
          }}
        />
      )}
      {projectAction?.kind === "resources" && (
        <ProjectShellDrawer
          localResourceDevice={
            devices.find(
              (device) =>
                device.device_id === projectAction.project.resource_device_id,
            )?.is_self === true
          }
          project={projectAction.project}
          deviceLabel={
            devices.find(
              (device) =>
                device.device_id === projectAction.project.resource_device_id,
            )?.friendly_name || projectAction.project.resource_device_id
          }
          availabilityLabel={
            devices.find(
              (device) =>
                device.device_id === projectAction.project.resource_device_id,
            )?.connectivity_state ?? "unknown"
          }
          lazy
          resourcePolicy={null}
          policyLoading={false}
          policyError={null}
          onClose={() => setProjectAction(null)}
        />
      )}
      <CreateProjectDialog
        open={createProjectOpen}
        devices={devices}
        onClose={() => setCreateProjectOpen(false)}
        onCreated={(outcome) => {
          setCreateProjectOpen(false);
          setProjects((current) => [
            ...current.filter(
              (project) => project.project_id !== outcome.project.project_id,
            ),
            outcome.project,
          ]);
          invalidateConversationNavigation();
          void resolveProjectChatPath(outcome.project.project_id).then(
            onNavigate,
          );
        }}
        onViewDuplicate={(project) => {
          setCreateProjectOpen(false);
          void resolveProjectChatPath(project.project_id).then(onNavigate);
        }}
      />
    </nav>
  );
}

type GroupProps = {
  projectMenu?: import("@/ui").DropdownItem[];
  projectTitle?: React.ReactNode;
  scope: NavigationScope;
  name: string;
  open: boolean;
  hideHeader?: boolean;
  onReady?: () => void;
  current: ConversationAddress | null;
  getCurrentAddress: () => ConversationAddress | null;
  onToggle: () => void;
  onNew: () => void;
  onManage: () => void;
  onNavigate: (path: string) => void;
};

function ConversationGroup({
  projectMenu,
  projectTitle,
  scope,
  name,
  open,
  hideHeader,
  onReady,
  current,
  getCurrentAddress,
  onToggle,
  onNew,
  onManage,
  onNavigate,
}: GroupProps) {
  const { t } = useTranslation();
  const {
    chats,
    removeChat,
    loaded,
    loading,
    error,
    hasMore,
    reload,
    loadMore,
  } = useConversationGroup(scope, open);
  useLayoutEffect(() => {
    if (loaded || error) onReady?.();
  }, [loaded, error, onReady]);
  const currentOutsideList =
    current &&
    scopeKey(scopeFromAddress(current)) === scopeKey(scope) &&
    !isDraftConversation(
      scope.kind === "agent" ? scope.id : `project:${scope.id}`,
      current.chatId,
    ) &&
    !chats.some((chat) => chat.id === current.chatId)
      ? current
      : null;
  return (
    <section className={styles.group}>
      {!hideHeader && (
        <div className={styles.groupHeader}>
          {projectTitle ?? (
            <button
              type="button"
              className={styles.groupToggle}
              aria-expanded={open}
              onClick={onToggle}
            >
              {open ? (
                <FolderOpen size={16} aria-hidden="true" />
              ) : (
                <Folder size={16} aria-hidden="true" />
              )}
              <span>{name}</span>
            </button>
          )}
          <button
            type="button"
            className={styles.iconButton}
            aria-label={t("navigation.newInGroup", { name })}
            onClick={onNew}
          >
            <Plus size={16} />
          </button>
          <Dropdown
            items={
              projectMenu ?? [
                {
                  key: "manage",
                  label: t("navigation.manageInGroup", { name }),
                  onClick: onManage,
                },
              ]
            }
          >
            <button
              type="button"
              className={styles.iconButton}
              aria-label={t("navigation.groupActions", { name })}
            >
              <MoreHorizontal size={16} />
            </button>
          </Dropdown>
        </div>
      )}
      <div hidden={!open} className={styles.chats}>
        {chats.map((chat) => (
          <ConversationRow
            key={chat.id}
            scope={scope}
            chat={chat}
            current={current}
            getCurrentAddress={getCurrentAddress}
            onRemoved={removeChat}
            onManage={onManage}
            onNavigate={onNavigate}
          />
        ))}
        {currentOutsideList && (loaded || error) && (
          <CurrentArchivedConversation
            key={conversationPath(currentOutsideList)}
            scope={scope}
            current={currentOutsideList}
            empty={!loading && !error && chats.length === 0}
            getCurrentAddress={getCurrentAddress}
            onManage={onManage}
            onNavigate={onNavigate}
          />
        )}
        {loading && !loaded && (
          <p className={styles.status} role="status">
            {t("common.loading")}
          </p>
        )}
        {error && <LoadError error={error} onRetry={reload} />}
        {!loading && !error && chats.length === 0 && !currentOutsideList && (
          <p className={styles.status}>{t("navigation.noConversations")}</p>
        )}
        {hasMore && !error && (
          <button
            type="button"
            className={styles.loadMore}
            disabled={loading}
            onClick={loadMore}
          >
            {t("projects.chat.loadMore")}
          </button>
        )}
      </div>
    </section>
  );
}

function ScopedSearch({
  scope,
  query,
  onNavigate,
}: {
  scope: NavigationScope;
  query: string;
  onNavigate: (path: string) => void;
}) {
  const { t } = useTranslation();
  const [results, setResults] = useState<ChatSearchResult[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const requestRef = useRef(0);
  const { kind, id } = scope;
  useEffect(() => {
    const request = ++requestRef.current;
    setResults([]);
    setError(null);
    setLoading(true);
    const timer = window.setTimeout(() => {
      const search =
        kind === "project"
          ? projectChatsApi.searchChats(id, query.trim())
          : chatApi.searchChats(query.trim(), id);
      void search
        .then((response) => {
          if (request === requestRef.current) setResults(response.results);
        })
        .catch((cause) => {
          if (request === requestRef.current)
            setError(cause instanceof Error ? cause.message : String(cause));
        })
        .finally(() => {
          if (request === requestRef.current) setLoading(false);
        });
    }, 300);
    return () => {
      window.clearTimeout(timer);
      requestRef.current += 1;
    };
  }, [id, kind, query, revision]);
  return (
    <div className={styles.searchResults}>
      {loading && (
        <p className={styles.status} role="status">
          {t("common.loading")}
        </p>
      )}
      {error && (
        <LoadError
          error={error}
          onRetry={() => setRevision((value) => value + 1)}
        />
      )}
      {!loading && !error && results.length === 0 && (
        <p className={styles.status}>{t("chat.search.noResults")}</p>
      )}
      {!loading && results.length >= 50 && (
        <p className={styles.status}>{t("navigation.searchLimit")}</p>
      )}
      {results.map((result, index) => (
        <button
          type="button"
          className={styles.searchResult}
          key={`${result.chat_id}:${index}`}
          onClick={() => {
            try {
              sessionStorage.setItem(
                "rua_search_scroll_target",
                JSON.stringify({
                  cardIndex: result.card_index,
                  query: query.trim(),
                  messageId: result.message_id,
                  turnId: result.turn_id,
                  conversationId: result.chat_id,
                  ownerKey: kind === "project" ? `project:${id}` : id,
                }),
              );
            } catch {
              /* Browser storage is optional. */
            }
            onNavigate(conversationPath(scopeAddress(scope, result.chat_id)));
            window.dispatchEvent(new Event("rua-search-scroll"));
          }}
        >
          <span>{result.chat_name || t("navigation.untitled")}</span>
          <small>{result.snippet}</small>
        </button>
      ))}
    </div>
  );
}

function LoadError({ error, onRetry }: { error: string; onRetry: () => void }) {
  const { t } = useTranslation();
  return (
    <div className={styles.error} role="alert">
      <span title={error}>{t("navigation.loadFailed")}</span>
      <button type="button" onClick={onRetry}>
        {t("common.retry")}
      </button>
    </div>
  );
}
