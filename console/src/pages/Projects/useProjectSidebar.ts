import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { projectsApi } from "@/api/modules/projects";
import type {
  ProjectDescriptor,
  ProjectSidebarSnapshot,
  ProjectSidebarAction,
} from "@/api/types/project";
import { extractFriendlyMessage } from "@/utils/error";
import { toast } from "@/ui";

import {
  readProjectSort,
  sortProjects,
  PROJECT_SORT_KEY,
} from "@/layouts/ConversationNavigation/projectSort";

export function useProjectSidebar(
  projects: ProjectDescriptor[] = [],
  locale = "zh",
) {
  const [snapshot, setSnapshot] = useState<ProjectSidebarSnapshot | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const current = useRef(snapshot);
  const busy = useRef(false);
  const version = useRef(0);
  const refreshing = useRef(false);
  const refreshQueued = useRef(false);
  const mounted = useRef(true);
  const refresh = useCallback(async function refreshSidebar() {
    if (!mounted.current) return;
    if (busy.current || refreshing.current) {
      refreshQueued.current = true;
      return;
    }
    refreshQueued.current = false;
    refreshing.current = true;
    const request = ++version.current;
    try {
      let next = await projectsApi.getSidebar();
      if (next.revision === "empty") {
        let legacy: string | null = null;
        try {
          legacy = localStorage.getItem("rua_navigation_project_sort");
        } catch {
          /* Browser cache is optional. */
        }
        if (
          legacy === "name" ||
          legacy === "created_at" ||
          legacy === "last_activity_at"
        ) {
          try {
            next = await projectsApi.updateSidebar(next.revision, {
              action: "set_sort",
              sort: legacy,
            });
          } catch {
            next = await projectsApi.getSidebar();
          }
        }
      }
      if (request !== version.current || busy.current) return;
      current.current = next;
      setSnapshot((old) => (old?.revision === next.revision ? old : next));
      setError(null);
    } catch (error) {
      if (request === version.current) setError(extractFriendlyMessage(error));
    } finally {
      refreshing.current = false;
      if (refreshQueued.current && mounted.current && !busy.current)
        void refreshSidebar();
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    const foreground = () => {
      if (document.visibilityState !== "hidden") void refresh();
    };
    window.addEventListener("rua:project-sidebar-changed", foreground);
    window.addEventListener("focus", foreground);
    window.addEventListener("rua:foreground", foreground);
    const timer = setInterval(foreground, 10000);
    return () => {
      mounted.current = false;
      refreshQueued.current = false;
      version.current++;
      clearInterval(timer);
      window.removeEventListener("rua:project-sidebar-changed", foreground);
      window.removeEventListener("focus", foreground);
      window.removeEventListener("rua:foreground", foreground);
    };
  }, [refresh]);
  const commit = useCallback(
    async (actions: ProjectSidebarAction[]) => {
      if (busy.current || !current.current) return false;
      busy.current = true;
      setPending(true);
      version.current++;
      let committed = false;
      try {
        for (const action of actions) {
          const next = await projectsApi.updateSidebar(
            current.current.revision,
            action,
          );
          current.current = next;
          setSnapshot(next);
          committed = true;
        }
        setError(null);
        return true;
      } catch (error) {
        toast.error(extractFriendlyMessage(error));
        return false;
      } finally {
        // Publish even a partial commit; a failed reorder must not hide a completed move.
        if (committed)
          window.dispatchEvent(new Event("rua:project-sidebar-changed"));
        busy.current = false;
        setPending(false);
        void refresh();
      }
    },
    [refresh],
  );
  const update = (action: ProjectSidebarAction) => commit([action]);
  const sort = snapshot?.sort ?? readProjectSort();
  const sortedProjects = useMemo(
    () =>
      sortProjects(
        projects.filter(
          (project) => !snapshot?.items[project.project_id]?.hidden,
        ),
        sort,
        locale,
        snapshot?.items,
      ),
    [projects, sort, locale, snapshot],
  );
  const sections = Object.values(snapshot?.sections ?? {}).sort(
    (a, b) => a.order - b.order || a.section_id.localeCompare(b.section_id),
  );
  const sectionOf = (id: string) => {
    const section = snapshot?.items[id]?.section_id;
    return section && snapshot?.sections?.[section] ? section : null;
  };
  const areaOf = (id: string) =>
    snapshot?.items[id]?.pinned ? "pinned" : sectionOf(id) ?? "";
  useEffect(() => {
    if (!snapshot) return;
    try {
      localStorage.setItem(PROJECT_SORT_KEY, sort);
    } catch {
      /* Optional cache. */
    }
  }, [sort, snapshot]);

  const moveProject = (
    id: string,
    target: { area: string; projectId: string | null },
  ) => {
    if (
      id === target.projectId ||
      !sortedProjects.some((p) => p.project_id === id)
    )
      return Promise.resolve(false);
    if (
      target.area &&
      target.area !== "pinned" &&
      !snapshot?.sections?.[target.area]
    )
      return Promise.resolve(false);
    const ordered = sortedProjects
      .map((p) => p.project_id)
      .filter((value) => value !== id);
    const index = target.projectId
      ? ordered.indexOf(target.projectId)
      : ordered.length;
    if (index < 0) return Promise.resolve(false);
    ordered.splice(index, 0, id);
    const actions: ProjectSidebarAction[] = [];
    if (areaOf(id) !== target.area)
      actions.push(
        target.area === "pinned"
          ? { action: "set_pinned", project_id: id, pinned: true }
          : {
              action: "move_to_section",
              project_id: id,
              section_id: target.area || null,
            },
      );
    actions.push({
      action: "reorder",
      project_id: id,
      ordered_project_ids: ordered,
    });
    return commit(actions);
  };
  const projectMoves = (id: string) => {
    const peers = sortedProjects.filter(
      (p) => areaOf(p.project_id) === areaOf(id),
    );
    const at = peers.findIndex((p) => p.project_id === id);
    return { peers, at, up: at > 0, down: at >= 0 && at < peers.length - 1 };
  };
  const stepProject = (id: string, direction: -1 | 1) => {
    const moves = projectMoves(id);
    if (!(direction < 0 ? moves.up : moves.down)) return Promise.resolve(false);
    return moveProject(id, {
      area: areaOf(id),
      projectId:
        moves.peers[moves.at + (direction < 0 ? -1 : 2)]?.project_id ?? null,
    });
  };
  const moveSection = (id: string, before: string | null) => {
    const ids = sections
      .map((s) => s.section_id)
      .filter((value) => value !== id);
    const at = before === null ? ids.length : ids.indexOf(before);
    if (at < 0 || !snapshot?.sections?.[id]) return Promise.resolve(false);
    ids.splice(at, 0, id);
    return update({
      action: "reorder_section",
      section_id: id,
      ordered_section_ids: ids,
    });
  };
  const sectionMoves = (id: string) => {
    const at = sections.findIndex((s) => s.section_id === id);
    return { up: at > 0, down: at >= 0 && at < sections.length - 1 };
  };
  const stepSection = (id: string, direction: -1 | 1) => {
    const moves = sectionMoves(id);
    if (!(direction < 0 ? moves.up : moves.down)) return Promise.resolve(false);
    const at = sections.findIndex((s) => s.section_id === id);
    return moveSection(
      id,
      sections[at + (direction < 0 ? -1 : 2)]?.section_id ?? null,
    );
  };
  return {
    snapshot,
    pending,
    error,
    refresh,
    update,
    sort,
    sortedProjects,
    sections,
    sectionOf,
    moveProject,
    stepProject,
    projectMoves,
    moveSection,
    stepSection,
    sectionMoves,
  };
}
