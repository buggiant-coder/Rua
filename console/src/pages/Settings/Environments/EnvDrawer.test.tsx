import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EnvDrawer } from "./EnvDrawer";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: unknown) => {
      if (key === "environments.editTitle") {
        const name =
          params && typeof params === "object" && "name" in params
            ? String((params as { name: unknown }).name)
            : "";
        return `Edit ${name}`;
      }
      const labels: Record<string, string> = {
        "common.cancel": "Cancel",
        "common.save": "Save",
        "environments.createTitle": "Create Variable",
        "environments.key": "Key",
        "environments.keyRequired": "Key is required",
        "environments.invalidKeyFormat": "Invalid key format",
        "environments.duplicateKey": "Duplicate key",
        "environments.value": "Value",
        "environments.valuePlaceholder": "Value",
      };
      return labels[key] ?? key;
    },
  }),
}));

describe("EnvDrawer", () => {
  it("trims the key and submits the Rust env replacement value", async () => {
    const onSubmit = vi.fn();

    render(
      <EnvDrawer
        open
        editingEnv={null}
        existingKeys={[]}
        saving={false}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("MY_VAR"), {
      target: { value: "  RUA_ENV  " },
    });
    fireEvent.change(screen.getByPlaceholderText("Value"), {
      target: { value: "enabled" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledWith({
        key: "RUA_ENV",
        value: "enabled",
      });
    });
  });

  it("rejects missing, invalid, and duplicate keys before saving", async () => {
    const onSubmit = vi.fn();

    render(
      <EnvDrawer
        open
        editingEnv={null}
        existingKeys={["EXISTING_KEY"]}
        saving={false}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    const keyInput = screen.getByPlaceholderText("MY_VAR");
    const saveButton = screen.getByRole("button", { name: "Save" });

    fireEvent.click(saveButton);
    expect(await screen.findByText("Key is required")).toBeVisible();

    fireEvent.change(keyInput, { target: { value: "1INVALID" } });
    fireEvent.click(saveButton);
    expect(await screen.findByText("Invalid key format")).toBeVisible();

    fireEvent.change(keyInput, { target: { value: "EXISTING_KEY" } });
    fireEvent.click(saveButton);
    expect(await screen.findByText("Duplicate key")).toBeVisible();

    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("allows editing an existing variable without tripping duplicate-key validation", async () => {
    const onSubmit = vi.fn();

    render(
      <EnvDrawer
        open
        editingEnv={{ key: "EXISTING_KEY", value: "old" }}
        existingKeys={["EXISTING_KEY", "OTHER_KEY"]}
        saving={false}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    await waitFor(() => {
      expect(screen.getByPlaceholderText("MY_VAR")).toHaveValue("EXISTING_KEY");
    });

    fireEvent.change(screen.getByPlaceholderText("Value"), {
      target: { value: "new" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledWith({
        key: "EXISTING_KEY",
        value: "new",
      });
    });
  });
});
