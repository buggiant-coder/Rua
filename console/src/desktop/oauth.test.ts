import { beforeEach, describe, expect, it, vi } from "vitest";

const desktopMock = vi.hoisted(() => ({
  enabled: false,
  open: vi.fn(),
}));

vi.mock("./runtime", () => ({
  isDesktopShell: () => desktopMock.enabled,
  openDesktopExternal: desktopMock.open,
}));

import { openOAuthAuthorization } from "./oauth";

describe("openOAuthAuthorization", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    desktopMock.enabled = false;
    desktopMock.open.mockReset();
  });

  it("opens OAuth in the system browser inside Desktop Shell", async () => {
    desktopMock.enabled = true;
    desktopMock.open.mockResolvedValue(true);
    const popup = vi.spyOn(window, "open");

    await expect(
      openOAuthAuthorization(
        "https://auth.example/authorize",
        "provider-oauth",
        "width=720,height=820",
      ),
    ).resolves.toEqual({ mode: "external", popup: null });

    expect(desktopMock.open).toHaveBeenCalledWith(
      "https://auth.example/authorize",
    );
    expect(popup).not.toHaveBeenCalled();
  });

  it("keeps the existing popup behavior in ordinary browsers", async () => {
    const opened = {} as Window;
    vi.spyOn(window, "open").mockReturnValue(opened);

    await expect(
      openOAuthAuthorization(
        "https://auth.example/authorize",
        "provider-oauth",
        "width=720,height=820",
      ),
    ).resolves.toEqual({ mode: "popup", popup: opened });
  });

  it("rejects unsafe OAuth URLs before either bridge is called", async () => {
    desktopMock.enabled = true;

    await expect(
      openOAuthAuthorization(
        "javascript:alert(1)",
        "provider-oauth",
        "width=720",
      ),
    ).rejects.toThrow("invalid");

    expect(desktopMock.open).not.toHaveBeenCalled();
  });
});
