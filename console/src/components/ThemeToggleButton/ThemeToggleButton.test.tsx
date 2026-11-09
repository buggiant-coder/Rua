import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const mockSetThemeMode = vi.fn();

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock("../../contexts/ThemeContext", () => ({
  useTheme: vi.fn(() => ({
    themeMode: "system",
    isDark: false,
    setThemeMode: mockSetThemeMode,
  })),
}));

import { useTheme } from "../../contexts/ThemeContext";
import ThemeToggleButton from "./index";

describe("ThemeToggleButton", () => {
  afterEach(() => vi.clearAllMocks());

  it("renders an icon-only button for the active theme", () => {
    vi.mocked(useTheme).mockReturnValueOnce({
      themeMode: "light",
      isDark: false,
      setThemeMode: mockSetThemeMode,
    } as ReturnType<typeof useTheme>);

    render(<ThemeToggleButton />);

    expect(screen.getByRole("button")).toBeInTheDocument();
  });

  it("offers light, dark, and system theme modes", async () => {
    render(<ThemeToggleButton />);

    fireEvent.click(screen.getByRole("button"));

    expect(await screen.findByText("theme.light")).toBeInTheDocument();
    expect(screen.getByText("theme.dark")).toBeInTheDocument();
    expect(screen.getByText("theme.system")).toBeInTheDocument();

    fireEvent.click(screen.getByText("theme.dark"));

    expect(mockSetThemeMode).toHaveBeenCalledWith("dark");
  });
});
