import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AppearancePage from "./index";

const { setColorSchemeMock, setSkinMock, useThemeMock } = vi.hoisted(() => ({
  setColorSchemeMock: vi.fn(),
  setSkinMock: vi.fn(),
  useThemeMock: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock("lucide-react", () => ({
  Check: () => <span data-testid="active-check" />,
}));

vi.mock("@/components/Page", () => ({
  Page: ({ children }: { children?: ReactNode }) => <main>{children}</main>,
  PageBody: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
}));

vi.mock("@/components/PageHeader", () => ({
  PageHeader: ({ current }: { current?: ReactNode }) => <h1>{current}</h1>,
}));

vi.mock("../../../contexts/ThemeContext", () => ({
  useTheme: useThemeMock,
}));

describe("AppearancePage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useThemeMock.mockReturnValue({
      colorScheme: "blue",
      setColorScheme: setColorSchemeMock,
      skin: "standard",
      setSkin: setSkinMock,
    });
  });

  it("renders Rust color-scheme cards and updates the theme context", () => {
    render(<AppearancePage />);

    expect(
      screen.getByRole("heading", { name: "appearance.title" }),
    ).toBeVisible();

    const blue = document.querySelector(
      '[data-color-scheme="blue"]',
    ) as HTMLButtonElement;
    const mono = document.querySelector(
      '[data-color-scheme="mono"]',
    ) as HTMLButtonElement;

    expect(blue).toHaveAttribute("aria-pressed", "true");
    expect(mono).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(mono);
    expect(setColorSchemeMock).toHaveBeenCalledWith("mono");
  });

  it("renders Rust skin cards and updates the skin context", () => {
    render(<AppearancePage />);

    const standard = document.querySelector(
      '[data-skin="standard"]',
    ) as HTMLButtonElement;
    const pill = document.querySelector(
      '[data-skin="pill"]',
    ) as HTMLButtonElement;

    expect(standard).toHaveAttribute("aria-pressed", "true");
    expect(pill).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(pill);
    expect(setSkinMock).toHaveBeenCalledWith("pill");
  });

  it("marks the selected color and selected skin with check indicators", () => {
    render(<AppearancePage />);

    expect(screen.getAllByTestId("active-check")).toHaveLength(2);
  });
});
