import { afterEach, describe, expect, it, vi } from "vitest";

import { openExternal } from "./openExternal";

const desktopMock = vi.hoisted(() => ({
  enabled: false,
  open: vi.fn(),
}));

vi.mock("../desktop/runtime", () => ({
  isDesktopShell: () => desktopMock.enabled,
  openDesktopExternal: desktopMock.open,
}));

describe("openExternal", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    desktopMock.enabled = false;
    desktopMock.open.mockReset();
    document.body.innerHTML = "";
  });

  it("does nothing for empty URLs", async () => {
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);

    await expect(openExternal("")).resolves.toBe(false);

    expect(openSpy).not.toHaveBeenCalled();
  });

  it("does not open unsafe or fragment-only URLs", async () => {
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click");

    for (const url of [
      "javascript:alert(1)",
      "data:text/html,<h1>boom</h1>",
      "#",
      "#settings",
      "http:example.com",
    ]) {
      await expect(openExternal(url)).resolves.toBe(false);
    }

    expect(clickSpy).not.toHaveBeenCalled();
    expect(openSpy).not.toHaveBeenCalled();
  });

  it("allows relative and supported external protocols", async () => {
    const clicked: string[] = [];
    vi.spyOn(window, "open").mockImplementation(() => null);
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicked.push(this.href);
    });

    await openExternal("/docs/faq");
    await openExternal("mailto:support@example.com");

    expect(clicked).toEqual([
      "http://localhost:3000/docs/faq",
      "mailto:support@example.com",
    ]);
  });

  it("clicks a defensive external anchor and falls back to window.open", async () => {
    const clicked: HTMLAnchorElement[] = [];
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);
    const clickSpy = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(function (this: HTMLAnchorElement) {
        clicked.push(this);
      });

    await openExternal("https://rua.example/docs");

    expect(clickSpy).toHaveBeenCalledOnce();
    expect(clicked[0]).toMatchObject({
      href: "https://rua.example/docs",
      target: "_blank",
      rel: "noopener noreferrer external",
    });
    expect(document.body.querySelector("a")).toBeNull();
    expect(openSpy).toHaveBeenCalledWith(
      "https://rua.example/docs",
      "_blank",
      "noopener,noreferrer",
    );
  });

  it("swallows anchor and window.open failures", async () => {
    vi.spyOn(document, "createElement").mockImplementation(() => {
      throw new Error("no dom");
    });
    vi.spyOn(window, "open").mockImplementation(() => {
      throw new Error("blocked");
    });

    await expect(openExternal("https://rua.example/docs")).resolves.toBe(
      true,
    );
  });

  it("uses only the named Desktop Shell external-link command", async () => {
    desktopMock.enabled = true;
    desktopMock.open.mockResolvedValue(true);
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click");

    await expect(openExternal("https://rua.example/docs")).resolves.toBe(
      true,
    );

    expect(desktopMock.open).toHaveBeenCalledWith("https://rua.example/docs");
    expect(openSpy).not.toHaveBeenCalled();
    expect(clickSpy).not.toHaveBeenCalled();
  });
});
