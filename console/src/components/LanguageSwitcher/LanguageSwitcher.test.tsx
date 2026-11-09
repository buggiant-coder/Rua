import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { mockChangeLanguage, mockUpdateLanguage } = vi.hoisted(() => ({
  mockChangeLanguage: vi.fn(),
  mockUpdateLanguage: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    i18n: {
      language: "en",
      resolvedLanguage: "en",
      changeLanguage: mockChangeLanguage,
    },
  }),
}));

vi.mock("../../api/modules/language", () => ({
  languageApi: { updateLanguage: mockUpdateLanguage },
}));

import LanguageSwitcher from "./index";

describe("LanguageSwitcher", () => {
  beforeEach(() => localStorage.clear());

  afterEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it("renders the icon-only language button", () => {
    render(<LanguageSwitcher />);

    expect(screen.getByRole("button")).toBeInTheDocument();
  });

  it("shows all Rust console language options and persists selection", async () => {
    render(<LanguageSwitcher />);

    fireEvent.click(screen.getByRole("button"));

    expect(await screen.findByText("English")).toBeInTheDocument();
    expect(screen.getByText("简体中文")).toBeInTheDocument();
    expect(screen.getByText("日本語")).toBeInTheDocument();
    expect(screen.getByText("Русский")).toBeInTheDocument();

    fireEvent.click(screen.getByText("日本語"));

    expect(mockChangeLanguage).toHaveBeenCalledWith("ja");
    expect(localStorage.getItem("language")).toBe("ja");
    await waitFor(() => expect(mockUpdateLanguage).toHaveBeenCalledWith("ja"));
  });

  it("logs backend persistence failures without preventing local language switch", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    mockUpdateLanguage.mockRejectedValueOnce(new Error("offline"));
    render(<LanguageSwitcher />);

    fireEvent.click(screen.getByRole("button"));
    fireEvent.click(await screen.findByText("简体中文"));

    expect(mockChangeLanguage).toHaveBeenCalledWith("zh");
    expect(localStorage.getItem("language")).toBe("zh");
    await waitFor(() => expect(errorSpy).toHaveBeenCalled());
    errorSpy.mockRestore();
  });
});
