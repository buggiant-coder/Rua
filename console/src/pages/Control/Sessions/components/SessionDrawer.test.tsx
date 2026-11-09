import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { forwardRef } from "react";
import { describe, expect, it, vi } from "vitest";
import { SessionDrawer } from "./SessionDrawer";
import type { Session } from "./constants";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "common.cancel": "Cancel",
        "common.save": "Save",
        "sessions.editSession": "Edit session",
        "sessions.fieldChannel": "Channel",
        "sessions.fieldId": "ID",
        "sessions.fieldName": "Name",
        "sessions.fieldUserId": "User ID",
        "sessions.sessionNamePlaceholder": "Session name",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    loading,
    onClick,
  }: {
    children?: React.ReactNode;
    loading?: boolean;
    onClick?: () => void;
  }) => (
    <button disabled={loading} type="button" onClick={onClick}>
      {children}
    </button>
  ),
  Drawer: ({
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
  FormField: ({
    children,
    label,
  }: {
    children?: React.ReactNode;
    label?: React.ReactNode;
  }) => (
    <label>
      {label}
      {children}
    </label>
  ),
  TextInput: forwardRef<
    HTMLInputElement,
    {
      disabled?: boolean;
      onChange?: (event: React.ChangeEvent<HTMLInputElement>) => void;
      placeholder?: string;
      value?: string;
    }
  >(({ disabled, onChange, placeholder, value, ...rest }, ref) => (
    <input
      ref={ref}
      disabled={disabled}
      placeholder={placeholder}
      value={value}
      onChange={onChange}
      {...rest}
    />
  )),
}));

const session: Session = {
  id: "chat-uuid",
  user_id: "user-a",
  channel: "console",
  name: "Old title",
  created_at: "2026-06-18T00:00:00Z",
  updated_at: "2026-06-19T00:00:00Z",
} as Session;

describe("SessionDrawer", () => {
  it("submits a renamed Rust chat session while preserving immutable fields", async () => {
    const onSubmit = vi.fn();

    render(
      <SessionDrawer
        open
        saving={false}
        editingSession={session}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    expect(screen.getByDisplayValue("chat-uuid")).toBeDisabled();
    expect(screen.getByDisplayValue("user-a")).toBeDisabled();
    expect(screen.getByDisplayValue("console")).toBeDisabled();

    fireEvent.change(screen.getByPlaceholderText("Session name"), {
      target: { value: "Renamed title" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledWith({
        ...session,
        name: "Renamed title",
      });
    });
  });

  it("does not submit when no editing session is present", async () => {
    const onSubmit = vi.fn();

    render(
      <SessionDrawer
        open
        saving={false}
        editingSession={null}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("Session name"), {
      target: { value: "No target" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSubmit).not.toHaveBeenCalled();
    });
  });
});
