import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ImportHubModal } from "./ImportHubModal";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: Record<string, unknown>) => {
      if (key === "skills.urlValid") return `Valid ${params?.source}`;
      if (key === "skills.examplesFrom")
        return `Examples from ${params?.source}`;
      return (
        {
          "common.cancel": "Cancel",
          "common.clear": "Clear",
          "common.loading": "Loading",
          "common.paste": "Paste",
          "skills.cancelImport": "Cancel import",
          "skills.clickToFill": "Click to fill",
          "skills.enterSkillUrl": "Enter skill URL",
          "skills.examples": "examples",
          "skills.importHub": "Import from Hub",
          "skills.invalidSkillUrlSource": "Unsupported source",
          "skills.invalidUrl": "Invalid URL",
          "skills.orChooseFromSources": "Or choose from sources",
        }[key] ?? key
      );
    },
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    loading,
    onClick,
    title,
  }: {
    children?: React.ReactNode;
    disabled?: boolean;
    loading?: boolean;
    onClick?: () => void;
    title?: string;
  }) => (
    <button
      disabled={disabled || loading}
      title={title}
      type="button"
      onClick={onClick}
    >
      {children ?? title ?? "button"}
    </button>
  ),
  Loading: () => <span>Loading</span>,
  Modal: ({
    children,
    footer,
    open,
    title,
  }: {
    children?: React.ReactNode;
    footer?: React.ReactNode;
    open?: boolean;
    title?: React.ReactNode;
  }) =>
    open ? (
      <div>
        <h2>{title}</h2>
        {children}
        {footer}
      </div>
    ) : null,
  TextInput: ({
    "aria-label": ariaLabel,
    disabled,
    onChange,
    placeholder,
    value,
  }: {
    "aria-label"?: string;
    disabled?: boolean;
    onChange?: (event: React.ChangeEvent<HTMLInputElement>) => void;
    placeholder?: string;
    value?: string;
  }) => (
    <input
      aria-label={ariaLabel}
      disabled={disabled}
      placeholder={placeholder}
      value={value ?? ""}
      onChange={onChange}
    />
  ),
}));

describe("ImportHubModal", () => {
  it("rejects unsupported URLs before starting a Rust hub install", () => {
    const onConfirm = vi.fn();

    render(
      <ImportHubModal
        open
        importing={false}
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    fireEvent.change(screen.getByLabelText("Enter skill URL"), {
      target: { value: "https://example.com/not-a-skill" },
    });

    expect(screen.getByText("Unsupported source")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Import from Hub" }),
    ).toBeDisabled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("fills supported example URLs and confirms the trimmed bundle URL", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <ImportHubModal
        open
        importing={false}
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /find-skills/ }));
    expect(screen.getByText("Valid Skills.sh")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Import from Hub" }));

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(
        "https://skills.sh/vercel-labs/skills/find-skills",
      );
    });
  });

  it("uses cancelImport while a hub import task is running", () => {
    const cancelImport = vi.fn();
    const onClose = vi.fn();

    render(
      <ImportHubModal
        open
        importing
        cancelImport={cancelImport}
        onClose={onClose}
        onConfirm={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Cancel import" }));

    expect(cancelImport).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Import from Hub" }),
    ).toBeDisabled();
  });
});
