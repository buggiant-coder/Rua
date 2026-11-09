import type {
  ProjectDescriptor,
  ProjectSidebarItem,
  ProjectSidebarSort,
} from "@/api/types/project";

export type ProjectSort = ProjectSidebarSort;
export const PROJECT_SORT_KEY = "rua_navigation_project_sort";
export function readProjectSort(): ProjectSort {
  try {
    const value = localStorage.getItem(PROJECT_SORT_KEY);
    if (
      value === "manual" ||
      value === "name" ||
      value === "created_at" ||
      value === "last_activity_at"
    )
      return value;
  } catch {
    /* Browser preferences are optional. */
  }
  return "last_activity_at";
}
function time(value: string | null | undefined) {
  const result = Date.parse(value || "");
  return Number.isFinite(result) ? result : 0;
}
export function sortProjects(
  projects: ProjectDescriptor[],
  sort: ProjectSort,
  locale: string,
  preferences: Record<string, ProjectSidebarItem> = {},
) {
  const collator = new Intl.Collator(locale, {
    numeric: true,
    sensitivity: "base",
  });
  return [...projects].sort((a, b) => {
    const left = preferences[a.project_id];
    const right = preferences[b.project_id];
    const pinned =
      Number(Boolean(right?.pinned)) - Number(Boolean(left?.pinned));
    if (pinned) return pinned;
    if (left?.pinned && right?.pinned)
      return (
        left.pin_order - right.pin_order ||
        a.project_id.localeCompare(b.project_id)
      );
    if (sort === "manual") {
      if (left && !right) return -1;
      if (!left && right) return 1;
      return (
        (left?.order ?? 0) - (right?.order ?? 0) ||
        time(a.created_at) - time(b.created_at) ||
        a.project_id.localeCompare(b.project_id)
      );
    }
    const order =
      sort === "name"
        ? collator.compare(a.display_name, b.display_name)
        : sort === "created_at"
        ? time(b.created_at) - time(a.created_at)
        : time(b.last_activity_at || b.created_at) -
          time(a.last_activity_at || a.created_at);
    return (
      order ||
      collator.compare(a.display_name, b.display_name) ||
      a.project_id.localeCompare(b.project_id)
    );
  });
}
