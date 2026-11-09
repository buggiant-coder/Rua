import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { describe, it, expect, vi } from "vitest";
import type { ChannelSchemaField } from "../../../../api/types/channel";
import { ChannelSchemaFields } from "./ChannelSchemaFields";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    i18n: { language: "en" },
    t: (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  }),
}));

const fields: ChannelSchemaField[] = [
  {
    name: "app_secret",
    label: { en: "Secret", zh: "密钥" },
    kind: "password",
    required: true,
    secret: true,
    default: "",
    options: [],
  },
  {
    name: "ws_port",
    label: { en: "Port", zh: "端口" },
    kind: "number",
    required: false,
    secret: false,
    default: 6199,
    ge: 1,
    le: 65535,
    options: [],
  },
  {
    name: "groups",
    label: { en: "Groups", zh: "群" },
    kind: "textarea",
    required: false,
    secret: false,
    default: {},
    options: [],
  },
];

function Form({ onSave }: { onSave: (values: unknown) => void }) {
  const { control, handleSubmit } = useForm<Record<string, any>>({
    defaultValues: {
      app_secret: "***",
      ws_port: 6199,
      groups: { room: true },
      unknown: { retained: true },
    },
  });
  return (
    <form onSubmit={handleSubmit(onSave)}>
      <ChannelSchemaFields fields={fields} control={control} enabled />
      <button type="submit">Save</button>
    </form>
  );
}

describe("schema-driven channel fields", () => {
  it("uses secret controls and submits numbers while retaining unrendered settings", async () => {
    const onSave = vi.fn();
    render(<Form onSave={onSave} />);
    expect(screen.getByLabelText("Secret")).toHaveAttribute("type", "password");
    fireEvent.change(screen.getByLabelText("Port"), {
      target: { value: "6200" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave.mock.calls[0][0]).toMatchObject({
      app_secret: "***",
      ws_port: 6200,
      groups: { room: true },
      unknown: { retained: true },
    });
  });

  it("rejects invalid JSON and missing required credentials", async () => {
    const onSave = vi.fn();
    render(<Form onSave={onSave} />);
    fireEvent.change(screen.getByLabelText("Secret"), {
      target: { value: "" },
    });
    fireEvent.change(screen.getByLabelText("Groups"), {
      target: { value: "not-json" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Enter a JSON object")).toBeVisible();
    expect(screen.getByText("Required")).toBeVisible();
    expect(onSave).not.toHaveBeenCalled();
  });
});
