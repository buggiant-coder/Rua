import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ status: vi.fn() }));
vi.mock("@/api/modules/phoneUse", () => ({ phoneUseApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
import { PhoneUseSettings } from "./PhoneUseSettings";
import styles from "./index.module.less";

describe("Phone Use native status", () => {
  beforeEach(() => vi.clearAllMocks());
  it("reports separate permissions and refreshes without granting them", async () => {
    api.status.mockResolvedValue({
      generation: "g",
      platform: "android",
      state: "permission_required",
      accessibility: false,
      camera_permission: true,
    });
    render(<PhoneUseSettings />);
    const accessibility = await screen.findByText("phone.accessibility");
    expect(
      within(
        accessibility.closest(`.${styles.permissionRow}`) as HTMLElement,
      ).getByText("phone.required"),
    ).toBeInTheDocument();
    expect(
      within(
        screen
          .getByText("phone.camera")
          .closest(`.${styles.permissionRow}`) as HTMLElement,
      ).getByText("phone.granted"),
    ).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "phone.title" })).toHaveClass(
      styles.section,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "desktop.browserRecheck" }),
    );
    await waitFor(() => expect(api.status).toHaveBeenCalledTimes(2));
  });
  it("shows connection failure rather than pretending capability is ready", async () => {
    api.status.mockRejectedValue(new Error("native service unavailable"));
    render(<PhoneUseSettings />);
    expect(
      await screen.findByText(/native service unavailable/),
    ).toBeInTheDocument();
    expect(screen.getByText("phone.unavailable")).toBeInTheDocument();
    expect(screen.getByText("phone.unavailableTitle")).toBeInTheDocument();
    const details = screen
      .getByText(/native service unavailable/)
      .closest("details");
    expect(details).not.toHaveAttribute("open");
  });
  it("shows running then ready without a resume control", async () => {
    const status = {
      generation: "g",
      platform: "android",
      accessibility: true,
      camera_permission: true,
    };
    api.status
      .mockResolvedValueOnce({ ...status, state: "executing" })
      .mockResolvedValueOnce({ ...status, state: "ready" });
    render(<PhoneUseSettings />);
    expect(await screen.findByText("phone.executing")).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "desktop.browserRecheck" }),
    );
    expect(await screen.findByText("phone.connected")).toBeInTheDocument();
    expect(screen.queryByText("phone.paused")).not.toBeInTheDocument();
    expect(screen.getAllByRole("button")).toHaveLength(1);
  });
  it("renders unsupported desktop hosts as a friendly notice, not raw exception JSON", async () => {
    api.status.mockRejectedValue(
      new Error(
        'Phone Use requires an Android execution device - {"detail":"Phone Use requires an Android execution device"}',
      ),
    );
    render(<PhoneUseSettings />);
    expect(
      await screen.findByText("phone.hostRequiredTitle"),
    ).toBeInTheDocument();
    expect(screen.getByText("phone.hostRequiredHint")).toBeInTheDocument();
    expect(screen.queryByText(/Phone Use requires/)).not.toBeInTheDocument();
    expect(
      screen.queryByText("desktop.technicalDetails"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "phone.title" }),
    ).toBeInTheDocument();
  });
  it("shows a checking state and prevents competing checks during initial load", () => {
    api.status.mockReturnValue(new Promise(() => {}));
    render(<PhoneUseSettings />);
    expect(screen.getByText("desktop.permissionUnknown")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "desktop.browserRecheck" }),
    ).toBeDisabled();
  });
});
