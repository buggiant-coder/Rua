import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { it, expect, vi } from "vitest";
import ProjectArchiveChatsDialog from "./ProjectArchiveChatsDialog";
import { projectsApi } from "@/api/modules/projects";
vi.mock("react-i18next", () => {
  const t = (key: string) => key;
  return { useTranslation: () => ({ t }) };
});
vi.mock("@/api/modules/projects", () => ({
  projectsApi: { archiveIdleChats: vi.fn() },
}));
it("reports skipped work and retries exactly failed candidates", async () => {
  vi.mocked(projectsApi.archiveIdleChats)
    .mockResolvedValueOnce({
      items: [
        { chat_id: "idle", status: "archived", reason: null },
        { chat_id: "busy", status: "skipped", reason: "queued_input" },
        { chat_id: "failed", status: "failed", reason: "offline" },
      ],
    })
    .mockResolvedValueOnce({
      items: [{ chat_id: "failed", status: "archived", reason: null }],
    });
  const close = vi.fn();
  render(
    <ProjectArchiveChatsDialog
      project={{
        project_id: "p",
        display_name: "Project",
        resource_device_id: "d",
        display_path: "/app",
        lifecycle: "active",
        created_at: "",
        updated_at: "",
      }}
      onClose={close}
    />,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "projects.archiveChats" }),
  );
  await screen.findByText("busy");
  expect(projectsApi.archiveIdleChats).toHaveBeenCalledWith("p", undefined);
  fireEvent.click(
    screen.getByRole("button", { name: "projects.retryFailedArchives" }),
  );
  await waitFor(() =>
    expect(projectsApi.archiveIdleChats).toHaveBeenLastCalledWith("p", [
      "failed",
    ]),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "projects.retryFailedArchives" }),
    ).not.toBeInTheDocument(),
  );
  expect(close).not.toHaveBeenCalled();
});
