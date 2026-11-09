import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ProjectSelector from "./index";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "projects.title": "Projects",
        "projects.selectProject": "Select Project",
        "navigation.independentChats": "Independent chats",
      })[key] ?? key,
  }),
}));

const project = {
  project_id: "project-a",
  display_name: "Project A",
  resource_device_id: "device-a",
  display_path: "/projects/a",
  lifecycle: "active" as const,
  created_at: "2026-08-24T00:00:00Z",
  updated_at: "2026-08-24T00:00:00Z",
};
const secondProject = {
  ...project,
  project_id: "project-b",
  display_name: "Project B",
  display_path: "/projects/b",
};

describe("ProjectSelector", () => {
  beforeEach(() => localStorage.clear());

  it("renders the shared compact control and remembers selection", async () => {
    const onSelect = vi.fn();
    render(
      <ProjectSelector
        currentProject={project}
        projects={[project, secondProject]}
        onSelect={onSelect}
      />,
    );

    const trigger = screen.getByRole("button", { name: "Select Project" });
    expect(trigger).toHaveTextContent("Project A");
    await waitFor(() =>
      expect(localStorage.getItem("rua_project_last_project")).toBe(
        "project-a",
      ),
    );

    fireEvent.click(trigger);
    expect(
      screen.queryByRole("button", { name: "Independent chats" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Project B/ }));
    await waitFor(() => expect(onSelect).toHaveBeenCalledWith(secondProject));
    expect(localStorage.getItem("rua_project_last_project")).toBe(
      "project-b",
    );
  });

  it("shows independent chats as the current scope and selects an active project", async () => {
    const onSelect = vi.fn();
    render(
      <ProjectSelector
        currentProject={null}
        projects={[project, { ...secondProject, lifecycle: "archived" }]}
        onSelect={onSelect}
        onSelectIndependent={vi.fn()}
      />,
    );
    expect(
      screen.getByRole("button", { name: "Select Project" }),
    ).toHaveTextContent("Independent chats");
    expect(localStorage.getItem("rua_project_last_project")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Select Project" }));
    expect(
      screen.getByRole("button", { name: "Independent chats" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(
      screen.queryByRole("button", { name: /Project B/ }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Project A/ }));
    await waitFor(() => expect(onSelect).toHaveBeenCalledWith(project));
  });

  it("navigates to independent chats without changing the remembered project", async () => {
    const onSelectIndependent = vi.fn();
    const onSelect = vi.fn();
    render(
      <ProjectSelector
        currentProject={project}
        projects={[project]}
        onSelect={onSelect}
        onSelectIndependent={onSelectIndependent}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Select Project" }));
    fireEvent.click(screen.getByRole("button", { name: "Independent chats" }));
    await waitFor(() => expect(onSelectIndependent).toHaveBeenCalledOnce());
    expect(onSelect).not.toHaveBeenCalled();
    expect(localStorage.getItem("rua_project_last_project")).toBe(
      "project-a",
    );
  });
});
