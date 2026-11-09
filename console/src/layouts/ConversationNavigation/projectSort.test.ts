import { beforeEach, describe, expect, it } from "vitest";
import type { ProjectDescriptor } from "@/api/types/project";
import { PROJECT_SORT_KEY, readProjectSort, sortProjects } from "./projectSort";
const projects: ProjectDescriptor[] = [
  {
    project_id: "a",
    display_name: "Alpha",
    created_at: "2026-01-01",
    updated_at: "2099-01-01",
    last_activity_at: "2026-01-03",
    resource_device_id: "local",
    display_path: "/a",
    lifecycle: "active",
  },
  {
    project_id: "b",
    display_name: "Beta",
    created_at: "2026-01-02",
    updated_at: "2026-01-02",
    last_activity_at: "2026-01-01",
    resource_device_id: "local",
    display_path: "/b",
    lifecycle: "active",
  },
  {
    project_id: "c",
    display_name: "Gamma",
    created_at: "2026-01-04",
    updated_at: "2026-01-04",
    last_activity_at: null,
    resource_device_id: "local",
    display_path: "/c",
    lifecycle: "active",
  },
];
beforeEach(() => localStorage.clear());
describe("project ordering", () => {
  it("sorts by name, creation or real chat activity without changing the catalog", () => {
    const ids = (sort: "name" | "created_at" | "last_activity_at") =>
      sortProjects(projects, sort, "en").map((p) => p.project_id);
    expect(ids("name")).toEqual(["a", "b", "c"]);
    expect(ids("created_at")).toEqual(["c", "b", "a"]);
    expect(ids("last_activity_at")).toEqual(["c", "a", "b"]);
    expect(projects.map((p) => p.project_id)).toEqual(["a", "b", "c"]);
  });
  it("keeps manual order through activity and rename changes and appends new projects", () => {
    const preferences = {
      a: { project_id: "a", pinned: false, order: 20, pin_order: 0 },
      b: { project_id: "b", pinned: false, order: 10, pin_order: 0 },
    };
    const changed = projects.map((project) => ({
      ...project,
      display_name: "Renamed",
      last_activity_at: "2099-01-01",
    }));
    expect(
      sortProjects(changed, "manual", "en", preferences).map(
        (project) => project.project_id,
      ),
    ).toEqual(["b", "a", "c"]);
    preferences.a.pinned = true;
    expect(
      sortProjects(changed, "manual", "en", preferences).map(
        (project) => project.project_id,
      ),
    ).toEqual(["a", "b", "c"]);
  });
  it("uses a deterministic id tiebreak and preserves the saved preference", () => {
    const same = projects
      .map((p) => ({
        ...p,
        display_name: "Same",
        created_at: "2026-01-01",
        last_activity_at: "2026-01-01",
      }))
      .reverse();
    expect(
      sortProjects(same, "last_activity_at", "en").map((p) => p.project_id),
    ).toEqual(["a", "b", "c"]);
    localStorage.setItem(PROJECT_SORT_KEY, "name");
    expect(readProjectSort()).toBe("name");
    localStorage.setItem(PROJECT_SORT_KEY, "invalid");
    expect(readProjectSort()).toBe("last_activity_at");
  });
});
