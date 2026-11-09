import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import EnvironmentsPage from "./index";

const mockApi = vi.hoisted(() => ({
  listEnvs: vi.fn(),
  saveEnvs: vi.fn(),
  deleteEnv: vi.fn(),
}));

const mockConfirm = vi.hoisted(() => vi.fn());
const mockToast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
}));

vi.mock("../../../api", () => ({ default: mockApi }));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    Modal: {
      ...actual.Modal,
      confirm: mockConfirm,
    },
    toast: mockToast,
  };
});

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: unknown) => {
      if (key === "environments.deleteConfirm") {
        const name =
          params && typeof params === "object" && "name" in params
            ? String((params as { name: unknown }).name)
            : "";
        return `Delete ${name}?`;
      }
      if (key === "environments.editTitle") {
        const name =
          params && typeof params === "object" && "name" in params
            ? String((params as { name: unknown }).name)
            : "";
        return `Edit ${name}`;
      }
      const labels: Record<string, string> = {
        "common.actions": "Actions",
        "common.cancel": "Cancel",
        "common.delete": "Delete",
        "common.edit": "Edit",
        "common.more": "More",
        "common.refresh": "Refresh",
        "common.refreshPage": "Refresh page",
        "common.save": "Save",
        "environments.createSuccess": "Created",
        "environments.createTitle": "Create Variable",
        "environments.createVariable": "Create variable",
        "environments.deleteFailed": "Delete failed",
        "environments.deleteSuccess": "Deleted",
        "environments.deleteVariable": "Delete variable",
        "environments.environments": "Environments",
        "environments.key": "Key",
        "environments.keyRequired": "Key is required",
        "environments.invalidKeyFormat": "Invalid key format",
        "environments.duplicateKey": "Duplicate key",
        "environments.noVariables": "No variables",
        "environments.saveFailed": "Save failed",
        "environments.updateSuccess": "Updated",
        "environments.value": "Value",
        "environments.valuePlaceholder": "Value",
      };
      return labels[key] ?? key;
    },
  }),
}));

describe("EnvironmentsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockApi.listEnvs.mockResolvedValue([
      { key: "EXISTING_KEY", value: "old" },
      { key: "KEEP_ME", value: "keep" },
    ]);
    mockApi.saveEnvs.mockResolvedValue([]);
    mockApi.deleteEnv.mockResolvedValue([]);
    mockConfirm.mockImplementation(({ onOk }) => onOk?.());
  });

  it("loads Rust env rows and refreshes from the device-local env API", async () => {
    render(<EnvironmentsPage />);

    expect(await screen.findByText("EXISTING_KEY")).toBeVisible();
    expect(screen.getByText("old")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "More" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Refresh page" }));

    await waitFor(() => {
      expect(mockApi.listEnvs).toHaveBeenCalledTimes(2);
    });
  });

  it("creates a variable by saving the full Rust env map", async () => {
    render(<EnvironmentsPage />);
    await screen.findByText("EXISTING_KEY");

    fireEvent.click(screen.getByRole("button", { name: "Create variable" }));
    fireEvent.change(screen.getByPlaceholderText("MY_VAR"), {
      target: { value: "NEW_ENV" },
    });
    fireEvent.change(screen.getByPlaceholderText("Value"), {
      target: { value: "new-value" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(mockApi.saveEnvs).toHaveBeenCalledWith({
        EXISTING_KEY: "old",
        KEEP_ME: "keep",
        NEW_ENV: "new-value",
      });
    });
    expect(mockToast.success).toHaveBeenCalledWith("Created");
  });

  it("edits a key by replacing the old key in the full env map", async () => {
    render(<EnvironmentsPage />);
    await screen.findByText("EXISTING_KEY");

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    await waitFor(() => {
      expect(screen.getByPlaceholderText("MY_VAR")).toHaveValue("EXISTING_KEY");
    });
    fireEvent.change(screen.getByPlaceholderText("MY_VAR"), {
      target: { value: "RENAMED_KEY" },
    });
    fireEvent.change(screen.getByPlaceholderText("Value"), {
      target: { value: "renamed-value" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(mockApi.saveEnvs).toHaveBeenCalledWith({
        KEEP_ME: "keep",
        RENAMED_KEY: "renamed-value",
      });
    });
    expect(mockToast.success).toHaveBeenCalledWith("Updated");
  });

  it("deletes through the encoded Rust env route after confirmation", async () => {
    render(<EnvironmentsPage />);
    await screen.findByText("EXISTING_KEY");

    fireEvent.click(screen.getAllByRole("button", { name: "Delete" })[0]);

    expect(mockConfirm).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Delete variable",
        content: "Delete EXISTING_KEY?",
      }),
    );
    await waitFor(() => {
      expect(mockApi.deleteEnv).toHaveBeenCalledWith("EXISTING_KEY");
    });
    expect(mockToast.success).toHaveBeenCalledWith("Deleted");
  });
});
