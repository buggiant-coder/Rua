import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ThemeProvider, useTheme } from "./ThemeContext";

function wrapper({ children }: { children: ReactNode }) {
  return <ThemeProvider>{children}</ThemeProvider>;
}

function mockSystemDark(matches: boolean) {
  const listeners: Array<(event: MediaQueryListEvent) => void> = [];
  const media = {
    matches,
    media: "(prefers-color-scheme: dark)",
    onchange: null,
    addEventListener: vi.fn(
      (_event: string, cb: (event: MediaQueryListEvent) => void) => {
        listeners.push(cb);
      },
    ),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  } as unknown as MediaQueryList;

  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: vi.fn(() => media),
  });

  return {
    media,
    emit(nextMatches: boolean) {
      for (const listener of listeners) {
        listener({ matches: nextMatches } as MediaQueryListEvent);
      }
    },
  };
}

describe("ThemeProvider", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.className = "";
    document.documentElement.removeAttribute("data-skin");
    document.documentElement.removeAttribute("data-color-scheme");
    mockSystemDark(false);
  });

  it("defaults to system theme, standard skin, and blue color scheme", () => {
    const { result } = renderHook(() => useTheme(), { wrapper });

    expect(result.current.themeMode).toBe("system");
    expect(result.current.isDark).toBe(false);
    expect(result.current.skin).toBe("standard");
    expect(result.current.colorScheme).toBe("blue");
    expect(document.documentElement).toHaveAttribute("data-skin", "standard");
    expect(document.documentElement).toHaveAttribute(
      "data-color-scheme",
      "blue",
    );
  });

  it("reads Rust rua storage keys and ignores invalid stored values", () => {
    localStorage.setItem("rua-theme", "dark");
    localStorage.setItem("rua-skin", "pill");
    localStorage.setItem("rua-color-scheme", "purple");

    const { result } = renderHook(() => useTheme(), { wrapper });

    expect(result.current.themeMode).toBe("dark");
    expect(result.current.isDark).toBe(true);
    expect(result.current.skin).toBe("pill");
    expect(result.current.colorScheme).toBe("purple");
  });

  it("persists explicit theme choices and removes storage for system mode", () => {
    const { result } = renderHook(() => useTheme(), { wrapper });

    act(() => result.current.setThemeMode("dark"));
    expect(result.current.themeMode).toBe("dark");
    expect(result.current.isDark).toBe(true);
    expect(localStorage.getItem("rua-theme")).toBe("dark");
    expect(document.documentElement.classList.contains("dark-mode")).toBe(true);

    act(() => result.current.setThemeMode("system"));
    expect(result.current.themeMode).toBe("system");
    expect(localStorage.getItem("rua-theme")).toBeNull();
  });

  it("toggles light and dark without selecting system", () => {
    const { result } = renderHook(() => useTheme(), { wrapper });

    act(() => result.current.setThemeMode("light"));
    act(() => result.current.toggleTheme());
    expect(result.current.themeMode).toBe("dark");
    expect(result.current.isDark).toBe(true);

    act(() => result.current.toggleTheme());
    expect(result.current.themeMode).toBe("light");
    expect(result.current.isDark).toBe(false);
  });

  it("persists skin and color scheme selections", () => {
    const { result } = renderHook(() => useTheme(), { wrapper });

    act(() => result.current.setSkin("industrial"));
    act(() => result.current.setColorScheme("mono"));

    expect(result.current.skin).toBe("industrial");
    expect(result.current.colorScheme).toBe("mono");
    expect(localStorage.getItem("rua-skin")).toBe("industrial");
    expect(localStorage.getItem("rua-color-scheme")).toBe("mono");
    expect(document.documentElement).toHaveAttribute("data-skin", "industrial");
    expect(document.documentElement).toHaveAttribute(
      "data-color-scheme",
      "mono",
    );
  });

  it("tracks system dark-mode changes while in system mode", () => {
    const system = mockSystemDark(false);
    const { result } = renderHook(() => useTheme(), { wrapper });

    expect(result.current.themeMode).toBe("system");
    expect(result.current.isDark).toBe(false);

    act(() => system.emit(true));
    expect(result.current.isDark).toBe(true);
  });
});
