import { beforeEach, describe, expect, it } from "vitest";
import { __resetMobileDetectionForTests, isMobile } from "./isMobile";

function setUserAgent(value: string) {
  Object.defineProperty(navigator, "userAgent", {
    value,
    configurable: true,
  });
}

function setSearch(value: string) {
  window.history.replaceState({}, "", `/${value}`);
}

describe("isMobile", () => {
  beforeEach(() => {
    __resetMobileDetectionForTests();
    document.documentElement.classList.remove("mobile-mode");
    setUserAgent("Mozilla/5.0");
    setSearch("");
  });

  it("detects the mobile Android shell by user agent and tags html immediately", () => {
    setUserAgent("Mozilla/5.0 Mobile/1.2.3");

    expect(isMobile()).toBe(true);
    expect(document.documentElement.classList.contains("mobile-mode")).toBe(
      true,
    );
  });

  it("supports the development query override", () => {
    setSearch("?_mobile=1");

    expect(isMobile()).toBe(true);
    expect(document.documentElement.classList.contains("mobile-mode")).toBe(
      true,
    );
  });

  it("freezes the first decision even if later signals change", () => {
    setSearch("?_mobile=1");
    expect(isMobile()).toBe(true);

    setSearch("");
    setUserAgent("Mozilla/5.0");
    document.documentElement.classList.remove("mobile-mode");

    expect(isMobile()).toBe(true);
  });

  it("can reset the cached decision in tests", () => {
    expect(isMobile()).toBe(false);

    setUserAgent("Mozilla/5.0 Mobile/2.0.0");
    expect(isMobile()).toBe(false);

    __resetMobileDetectionForTests();
    expect(isMobile()).toBe(true);
  });

  it("does not treat ordinary Android browsers as mobile", () => {
    setUserAgent("Mozilla/5.0 (Linux; Android 15) Chrome/125");

    expect(isMobile()).toBe(false);
    expect(document.documentElement.classList.contains("mobile-mode")).toBe(
      false,
    );
  });
});
