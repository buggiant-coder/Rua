import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { projectsApi } from "@/api/modules/projects";
import type {
  ProjectDescriptor,
  ProjectSidebarSnapshot,
} from "@/api/types/project";
import { useProjectSidebar } from "./useProjectSidebar";

vi.mock("@/api/modules/projects", () => ({
  projectsApi: { getSidebar: vi.fn(), updateSidebar: vi.fn() },
}));
vi.mock("@/ui", () => ({ toast: { error: vi.fn() } }));
const projects = ["a", "b", "c"].map((id) => ({
  project_id: id,
  display_name: id,
  created_at: "2026-01-01",
})) as ProjectDescriptor[];
let server: ProjectSidebarSnapshot;
beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  server = {
    revision: "r1",
    sort: "name",
    items: {},
    sections: {
      work: { section_id: "work", name: "Work", order: 0 },
      later: { section_id: "later", name: "Later", order: 1 },
    },
  };
  vi.mocked(projectsApi.getSidebar).mockImplementation(async () => server);
  vi.mocked(projectsApi.updateSidebar).mockImplementation(
    async (revision, action) => {
      if (revision !== server.revision) throw new Error("stale revision");
      server = { ...server, revision: server.revision + "x" };
      if (action.action === "move_to_section")
        server = {
          ...server,
          items: {
            ...server.items,
            [action.project_id]: {
              project_id: action.project_id,
              pinned: false,
              section_id: action.section_id,
              order: 0,
              pin_order: 0,
            },
          },
        };
      if (action.action === "reorder") server = { ...server, sort: "manual" };
      return server;
    },
  );
});
async function setup() {
  const hook = renderHook(() => useProjectSidebar(projects, "en"));
  await waitFor(() =>
    expect(hook.result.current.snapshot?.revision).toBe("r1"),
  );
  return hook;
}
it("uses the committed revision for cross-section reorder and preserves visible automatic order", async () => {
  const { result } = await setup();
  await act(async () => {
    expect(
      await result.current.moveProject("b", { area: "work", projectId: null }),
    ).toBe(true);
  });
  expect(projectsApi.updateSidebar).toHaveBeenNthCalledWith(1, "r1", {
    action: "move_to_section",
    project_id: "b",
    section_id: "work",
  });
  expect(projectsApi.updateSidebar).toHaveBeenNthCalledWith(2, "r1x", {
    action: "reorder",
    project_id: "b",
    ordered_project_ids: ["a", "c", "b"],
  });
});
it("keeps a partial move and reconciles when reorder fails", async () => {
  const { result } = await setup();
  const normal = vi.mocked(projectsApi.updateSidebar).getMockImplementation()!;
  vi.mocked(projectsApi.updateSidebar).mockImplementation(
    async (revision, action) => {
      if (action.action === "reorder") throw new Error("conflict");
      return normal(revision, action);
    },
  );
  const changed = vi.fn();
  window.addEventListener("rua:project-sidebar-changed", changed);
  try {
    await act(async () => {
      expect(
        await result.current.moveProject("b", {
          area: "work",
          projectId: null,
        }),
      ).toBe(false);
    });
    expect(result.current.sectionOf("b")).toBe("work");
    expect(result.current.sort).toBe("name");
    expect(result.current.pending).toBe(false);
    expect(changed).toHaveBeenCalledTimes(1);
    expect(projectsApi.getSidebar).toHaveBeenCalledTimes(2);
  } finally {
    window.removeEventListener("rua:project-sidebar-changed", changed);
  }
});
it("rejects overlapping user intents throughout a multi-request move", async () => {
  const { result } = await setup();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const normal = vi.mocked(projectsApi.updateSidebar).getMockImplementation()!;
  vi.mocked(projectsApi.updateSidebar).mockImplementation(
    async (revision, action) => {
      if (action.action === "reorder") await gate;
      return normal(revision, action);
    },
  );
  let move!: Promise<boolean>;
  await act(async () => {
    move = result.current.moveProject("b", { area: "work", projectId: null });
  });
  expect(result.current.pending).toBe(true);
  await act(async () => {
    expect(await result.current.stepProject("c", -1)).toBe(false);
  });
  expect(projectsApi.updateSidebar).toHaveBeenCalledTimes(2);
  await act(async () => {
    release();
    expect(await move).toBe(true);
  });
});
it("refreshes stale revisions without retrying a move into a deleted section", async () => {
  const { result } = await setup();
  server = { ...server, revision: "remote", sections: {} };
  await act(async () => {
    expect(
      await result.current.moveProject("b", { area: "work", projectId: null }),
    ).toBe(false);
  });
  expect(projectsApi.updateSidebar).toHaveBeenCalledTimes(1);
  expect(result.current.snapshot?.revision).toBe("remote");
  await act(async () => {
    expect(
      await result.current.moveProject("b", { area: "work", projectId: null }),
    ).toBe(false);
  });
  expect(projectsApi.updateSidebar).toHaveBeenCalledTimes(1);
});
it("makes keyboard and drag moves produce the same order", async () => {
  const { result } = await setup();
  await act(async () => {
    await result.current.stepProject("b", -1);
  });
  const keyboard = vi.mocked(projectsApi.updateSidebar).mock.calls[0][1];
  await act(async () => {
    await result.current.moveProject("b", { area: "", projectId: "a" });
  });
  expect(vi.mocked(projectsApi.updateSidebar).mock.calls[1][1]).toEqual(
    keyboard,
  );
});
it("falls back to the default section and keeps pinning separate from remembered membership", async () => {
  server.items = {
    a: {
      project_id: "a",
      pinned: true,
      section_id: "deleted",
      order: 0,
      pin_order: 0,
    },
    b: {
      project_id: "b",
      pinned: false,
      section_id: "deleted",
      order: 0,
      pin_order: 0,
    },
  };
  const { result } = await setup();
  expect(result.current.sectionOf("b")).toBe(null);
  expect(result.current.projectMoves("b").down).toBe(true);
  await act(async () => {
    await result.current.moveProject("a", { area: "", projectId: "b" });
  });
  expect(projectsApi.updateSidebar).toHaveBeenNthCalledWith(1, "r1", {
    action: "move_to_section",
    project_id: "a",
    section_id: null,
  });
});

it("keeps the moved section identity for keyboard and drag ordering", async () => {
  const { result } = await setup();
  await act(async () => {
    await result.current.stepSection("work", 1);
  });
  expect(projectsApi.updateSidebar).toHaveBeenLastCalledWith("r1", {
    action: "reorder_section",
    section_id: "work",
    ordered_section_ids: ["later", "work"],
  });
  await act(async () => {
    await result.current.moveSection("work", null);
  });
  expect(vi.mocked(projectsApi.updateSidebar).mock.calls[1][1]).toEqual(
    vi.mocked(projectsApi.updateSidebar).mock.calls[0][1],
  );
});

it("coalesces refresh storms and discards queued work on unmount", async () => {
  let release!: (value: ProjectSidebarSnapshot) => void;
  vi.mocked(projectsApi.getSidebar).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const hook = renderHook(() => useProjectSidebar(projects, "en"));
  act(() => {
    for (let i = 0; i < 25; i++)
      window.dispatchEvent(new Event("rua:project-sidebar-changed"));
  });
  expect(projectsApi.getSidebar).toHaveBeenCalledTimes(1);
  await act(async () => {
    release(server);
  });
  await waitFor(() => expect(projectsApi.getSidebar).toHaveBeenCalledTimes(2));
  expect(hook.result.current.snapshot?.revision).toBe("r1");
  hook.unmount();
  window.dispatchEvent(new Event("rua:project-sidebar-changed"));
  expect(projectsApi.getSidebar).toHaveBeenCalledTimes(2);
});
