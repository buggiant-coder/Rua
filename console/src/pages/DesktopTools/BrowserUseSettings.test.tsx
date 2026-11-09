import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({
  browserStatus: vi.fn(),
  selectBrowser: vi.fn(),
  recheckBrowser: vi.fn(),
}));
vi.mock("@/api/modules/desktopAutomation", () => ({
  desktopAutomationApi: api,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
import { BrowserUseSettings } from "./BrowserUseSettings";
const missing = {
  device_id: "device-a",
  available: false,
  state: "unavailable",
  selected_path: null,
  resolved_path: null,
  driver_version: "0.38.1",
  diagnostics: ["Install a Chromium browser"],
};
const ready = {
  ...missing,
  available: true,
  state: "ready",
  selected_path: "/自定义 Chrome",
  resolved_path: "/自定义 Chrome",
  diagnostics: [],
};

describe("Browser Use device settings", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    api.browserStatus.mockResolvedValue(missing);
  });
  it("validates a local path and shows the resulting runtime status", async () => {
    api.selectBrowser.mockResolvedValue(ready);
    render(<BrowserUseSettings />);
    expect(await screen.findByText("desktop.browserMissingHint")).toBeVisible();
    expect(screen.getByText("Install a Chromium browser")).not.toBeVisible();
    fireEvent.click(screen.getByText("desktop.browserSelect"));
    expect(screen.getByRole("textbox")).toBeVisible();
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/自定义 Chrome" },
    });
    fireEvent.click(screen.getByText("desktop.browserSave"));
    expect(await screen.findByText("desktop.browserReady")).toBeVisible();
    expect(api.selectBrowser).toHaveBeenCalledWith("/自定义 Chrome");
    expect(screen.queryByText("Install a Chromium browser")).toBeNull();
  });
  it("keeps the previous working status when a new selection fails", async () => {
    api.browserStatus.mockResolvedValue(ready);
    api.selectBrowser.mockRejectedValue(
      new Error("Selected path is not executable"),
    );
    render(<BrowserUseSettings />);
    await screen.findByText("desktop.browserReady");
    fireEvent.click(screen.getByText("desktop.browserChange"));
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/bad" },
    });
    fireEvent.click(screen.getByText("desktop.browserSave"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "desktop.browserRetryHint",
    );
    expect(
      screen.getByText("Selected path is not executable"),
    ).not.toBeVisible();
    fireEvent.click(screen.getByText("desktop.technicalDetails"));
    expect(screen.getByText("Selected path is not executable")).toBeVisible();
    expect(screen.getByText("desktop.browserReady")).toBeVisible();
    expect(screen.getByText("/自定义 Chrome")).toBeVisible();
  });
  it("rechecks and restores automatic discovery without installing software", async () => {
    api.recheckBrowser.mockResolvedValue(ready);
    api.selectBrowser.mockResolvedValue({ ...ready, selected_path: null });
    render(<BrowserUseSettings />);
    await screen.findByText("desktop.browserMissingHint");
    fireEvent.click(screen.getByText("desktop.browserRecheck"));
    await screen.findByText("desktop.browserReady");
    expect(api.recheckBrowser).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByText("desktop.browserChange"));
    fireEvent.click(screen.getByText("desktop.browserAutomatic"));
    await screen.findByText("desktop.browserReady");
    expect(api.selectBrowser).toHaveBeenCalledWith(null);
  });
  it("identifies missing execution-device components rather than blocking WebUI", async () => {
    api.browserStatus.mockResolvedValue({
      ...missing,
      resolved_path: "/Applications/Chrome",
      diagnostics: [
        "Browser Use is available in Rua Desktop on macOS and Windows.",
      ],
    });
    render(<BrowserUseSettings />);
    expect(await screen.findByText("desktop.hostRequiredTitle")).toBeVisible();
    expect(screen.getByText("desktop.hostRequiredHint")).toBeVisible();
    expect(
      screen.queryByText("desktop.browserMissingHint"),
    ).not.toBeInTheDocument();
  });
});
