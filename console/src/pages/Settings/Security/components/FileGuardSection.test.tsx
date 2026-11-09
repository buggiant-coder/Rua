import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FileGuardSection } from "./FileGuardSection";

const { getFileGuardMock, updateFileGuardMock, toastMock, tMock } = vi.hoisted(
  () => ({
    getFileGuardMock: vi.fn(),
    updateFileGuardMock: vi.fn(),
    toastMock: {
      success: vi.fn(),
      error: vi.fn(),
      warning: vi.fn(),
    },
    tMock: vi.fn((key: string) => key),
  }),
);

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../../utils/isMobile", () => ({
  isMobile: () => false,
}));

vi.mock("../../../../api", () => ({
  default: {
    getFileGuard: getFileGuardMock,
    updateFileGuard: updateFileGuardMock,
  },
}));

vi.mock("@/ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ui")>();
  return {
    ...actual,
    toast: toastMock,
  };
});

const fileGuardConfig = {
  enabled: true,
  paths: ["~/custom-secret"],
  builtin_paths: ["~/.ssh/"],
  disabled_patterns: ["~/.ssh/"],
};

describe("FileGuardSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getFileGuardMock.mockResolvedValue(fileGuardConfig);
    updateFileGuardMock.mockResolvedValue(fileGuardConfig);
  });

  it("loads process-wide file guard config and exposes save/reset handlers", async () => {
    const onSave = vi.fn();

    render(<FileGuardSection onSave={onSave} />);

    await waitFor(() => expect(getFileGuardMock).toHaveBeenCalledTimes(1));

    expect(await screen.findByText("~/.ssh/")).toBeVisible();
    expect(screen.getByText("~/custom-secret")).toBeVisible();
    expect(screen.getByText("security.fileGuard.builtin")).toBeVisible();
    expect(screen.getByText("security.fileGuard.custom")).toBeVisible();
    expect(screen.getAllByRole("checkbox")[0]).toBeChecked();
    expect(onSave).toHaveBeenLastCalledWith(
      expect.objectContaining({
        save: expect.any(Function),
        reset: expect.any(Function),
        saving: false,
      }),
    );
  });

  it("adds a trimmed custom path through the Rust file-guard update route", async () => {
    render(<FileGuardSection />);
    await screen.findByText("~/custom-secret");

    fireEvent.change(
      screen.getByPlaceholderText("security.fileGuard.inputPlaceholder"),
      { target: { value: "  ~/.gnupg/  " } },
    );
    fireEvent.click(
      screen.getByRole("button", { name: "security.fileGuard.add" }),
    );

    await waitFor(() => {
      expect(updateFileGuardMock).toHaveBeenCalledWith({
        enabled: true,
        paths: ["~/custom-secret", "~/.gnupg/"],
        disabled_patterns: ["~/.ssh/"],
      });
    });
    expect(toastMock.success).toHaveBeenCalledWith(
      "security.fileGuard.saveSuccess",
    );
    expect(
      screen.getByPlaceholderText("security.fileGuard.inputPlaceholder"),
    ).toHaveValue("");
  });

  it("warns on duplicate paths without writing config", async () => {
    render(<FileGuardSection />);
    await screen.findByText("~/custom-secret");

    fireEvent.change(
      screen.getByPlaceholderText("security.fileGuard.inputPlaceholder"),
      { target: { value: " ~/custom-secret " } },
    );
    fireEvent.click(
      screen.getByRole("button", { name: "security.fileGuard.add" }),
    );

    expect(updateFileGuardMock).not.toHaveBeenCalled();
    expect(toastMock.warning).toHaveBeenCalledWith(
      "security.fileGuard.duplicate",
    );
  });

  it("rolls back the global enable toggle when the Rust update fails", async () => {
    updateFileGuardMock.mockRejectedValueOnce(new Error("disk readonly"));

    render(<FileGuardSection />);
    await screen.findByText("~/custom-secret");

    const globalToggle = screen.getAllByRole("checkbox")[0];
    expect(globalToggle).toBeChecked();

    fireEvent.click(globalToggle);

    await waitFor(() => {
      expect(updateFileGuardMock).toHaveBeenCalledWith({ enabled: false });
    });
    await waitFor(() => expect(globalToggle).toBeChecked());
    expect(toastMock.error).toHaveBeenCalledWith(
      "security.fileGuard.saveFailed",
    );
  });

  it("updates disabled builtin path patterns through row toggles", async () => {
    render(<FileGuardSection />);
    await screen.findByText("~/custom-secret");

    const builtinPathToggle = screen.getAllByRole("checkbox")[1];
    expect(builtinPathToggle).not.toBeChecked();

    fireEvent.click(builtinPathToggle);

    await waitFor(() => {
      expect(updateFileGuardMock).toHaveBeenCalledWith({
        enabled: true,
        paths: ["~/custom-secret"],
        disabled_patterns: [],
      });
    });
    expect(toastMock.success).toHaveBeenCalledWith(
      "security.fileGuard.saveSuccess",
    );
  });
});
