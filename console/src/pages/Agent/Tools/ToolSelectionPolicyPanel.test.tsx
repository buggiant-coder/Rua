import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ToolSelectionPolicyPanel } from "./ToolSelectionPolicyPanel";
import type {
  ToolSelectionPolicy,
  ToolSelectionPolicyPreview,
} from "@/api/modules/toolSelection";

const api = vi.hoisted(() => ({
  get: vi.fn(),
  preview: vi.fn(),
  apply: vi.fn(),
}));
vi.mock("@/api/modules/toolSelection", () => ({ toolSelectionApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const tools = [
  { name: "read", description: "Read", enabled: true, async_execution: false },
  {
    name: "phone_observe",
    description: "Observe",
    enabled: false,
    async_execution: false,
  },
  {
    name: "phone_take_photo",
    description: "Camera",
    enabled: false,
    async_execution: false,
  },
];
const restricted: ToolSelectionPolicy = {
  mode: "restricted",
  allowed_tools: [{ kind: "builtin", name: "read" }],
};
const preview: ToolSelectionPolicyPreview = {
  expected: "opaque-revision",
  current: restricted,
  candidate: {
    mode: "default_enabled",
    disabled_tools: [],
    disabled_capabilities: [],
    future_option: "server_value",
  },
};

async function selectMode(mode: string) {
  fireEvent.click(
    screen.getByRole("combobox", { name: "tools.selectionPolicy.mode" }),
  );
  fireEvent.click(await screen.findByRole("option", { name: mode }));
}

describe("ToolSelectionPolicyPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.get.mockResolvedValue({ selection_policy: restricted });
    api.preview.mockResolvedValue(preview);
    api.apply.mockResolvedValue({ selection_policy: preview.candidate });
  });

  it("previews normal mode changes and confirms the exact server candidate", async () => {
    const onSaved = vi.fn();
    render(
      <ToolSelectionPolicyPanel
        agentId="custom"
        tools={tools}
        onSaved={onSaved}
      />,
    );
    await screen.findByRole("combobox");
    expect(api.preview).not.toHaveBeenCalled();
    expect(api.apply).not.toHaveBeenCalled();
    await selectMode("tools.selectionPolicy.defaultEnabled");
    fireEvent.click(
      screen.getByRole("button", { name: "tools.selectionPolicy.preview" }),
    );
    const dialog = within(await screen.findByRole("dialog"));
    expect(api.preview).toHaveBeenCalledWith("custom", {
      mode: "default_enabled",
      disabled_tools: [],
      disabled_capabilities: [],
    });
    expect(
      dialog.getByText("tools.selectionPolicy.currentPolicy"),
    ).toBeInTheDocument();
    expect(
      dialog.getByText("tools.selectionPolicy.proposedPolicy"),
    ).toBeInTheDocument();
    expect(dialog.getByText(/server_value/)).toBeInTheDocument();
    expect(api.apply).not.toHaveBeenCalled();
    fireEvent.click(
      dialog.getByRole("button", { name: "tools.selectionPolicy.confirm" }),
    );
    await waitFor(() =>
      expect(api.apply).toHaveBeenCalledWith("custom", preview),
    );
    expect(onSaved).toHaveBeenCalledOnce();
  });

  it.each(["restricted", "default_enabled"] as const)(
    "preserves extension fields when switching from %s and removes the old inventory",
    async (mode) => {
      const future = { option: true };
      const current =
        mode === "restricted"
          ? { ...restricted, future }
          : {
              mode: "default_enabled",
              disabled_tools: [{ kind: "builtin", name: "phone_take_photo" }],
              disabled_capabilities: ["phone_use"],
              future,
            };
      api.get.mockResolvedValue({ selection_policy: current });
      render(
        <ToolSelectionPolicyPanel
          agentId="custom"
          tools={tools}
          onSaved={vi.fn()}
        />,
      );
      await screen.findByRole("combobox");
      await selectMode(
        mode === "restricted"
          ? "tools.selectionPolicy.defaultEnabled"
          : "tools.selectionPolicy.restricted",
      );
      fireEvent.click(
        screen.getByRole("button", { name: "tools.selectionPolicy.preview" }),
      );
      await waitFor(() =>
        expect(api.preview).toHaveBeenCalledWith(
          "custom",
          mode === "restricted"
            ? {
                mode: "default_enabled",
                disabled_tools: [],
                disabled_capabilities: [],
                future,
              }
            : { ...restricted, future },
        ),
      );
    },
  );

  it("preserves off-device identities and extension fields while disabling a capability", async () => {
    const current = {
      mode: "default_enabled" as const,
      disabled_tools: [
        {
          kind: "mcp" as const,
          source: "private-server",
          name: "off_device_tool",
        },
      ],
      disabled_capabilities: ["other_capability"],
      future: { option: true },
    };
    api.get.mockResolvedValue({ selection_policy: current });
    render(
      <ToolSelectionPolicyPanel
        agentId="custom"
        tools={tools}
        onSaved={vi.fn()}
      />,
    );
    const phone = within(
      await screen.findByRole("group", {
        name: "tools.selectionPolicy.phone_use",
      }),
    );
    fireEvent.click(phone.getByRole("button", { name: "common.disable" }));
    fireEvent.click(
      screen.getByRole("button", { name: "tools.selectionPolicy.preview" }),
    );
    await waitFor(() =>
      expect(api.preview).toHaveBeenCalledWith("custom", {
        ...current,
        disabled_capabilities: ["other_capability", "phone_use"],
      }),
    );
  });

  it("shows invalid configuration errors without inventing a policy", async () => {
    api.get.mockRejectedValue(new Error("Invalid Tool Selection Policy"));
    render(
      <ToolSelectionPolicyPanel
        agentId="custom"
        tools={tools}
        onSaved={vi.fn()}
      />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Invalid Tool Selection Policy",
    );
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(api.preview).not.toHaveBeenCalled();
  });

  it("cancels without applying and reports stale previews without claiming success", async () => {
    api.apply.mockRejectedValue(
      new Error("Tool configuration changed; preview again"),
    );
    const onSaved = vi.fn();
    render(
      <ToolSelectionPolicyPanel
        agentId="custom"
        tools={tools}
        onSaved={onSaved}
      />,
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "tools.selectionPolicy.preview",
      }),
    );
    fireEvent.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "common.cancel",
      }),
    );
    expect(api.apply).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "tools.selectionPolicy.preview" }),
    );
    await waitFor(() => expect(api.preview).toHaveBeenCalledTimes(2));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "tools.selectionPolicy.confirm",
      }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Tool configuration changed",
    );
    expect(onSaved).not.toHaveBeenCalled();
  });

  it("resets a pending preview when switching Agents", async () => {
    let finishFirst!: (value: ToolSelectionPolicyPreview) => void;
    api.preview.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishFirst = resolve;
        }),
    );
    const { rerender } = render(
      <ToolSelectionPolicyPanel
        agentId="first"
        tools={tools}
        onSaved={vi.fn()}
      />,
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "tools.selectionPolicy.preview",
      }),
    );
    rerender(
      <ToolSelectionPolicyPanel
        agentId="second"
        tools={tools}
        onSaved={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "tools.selectionPolicy.preview" }),
      ).toBeEnabled(),
    );
    await act(async () => finishFirst(preview));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("ignores the previous Agent's late configuration response", async () => {
    let finishFirst!: (value: {
      selection_policy: ToolSelectionPolicy;
    }) => void;
    api.get.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishFirst = resolve;
        }),
    );
    const { rerender } = render(
      <ToolSelectionPolicyPanel
        agentId="first"
        tools={tools}
        onSaved={vi.fn()}
      />,
    );
    rerender(
      <ToolSelectionPolicyPanel
        agentId="second"
        tools={tools}
        onSaved={vi.fn()}
      />,
    );
    await screen.findByRole("combobox");
    await act(async () => finishFirst({ selection_policy: preview.candidate }));
    expect(screen.getByRole("combobox")).toHaveTextContent(
      "tools.selectionPolicy.restricted",
    );
  });
});
