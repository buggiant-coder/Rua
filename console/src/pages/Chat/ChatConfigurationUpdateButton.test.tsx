import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ChatConfigurationUpdateButton } from "./ChatConfigurationUpdateButton";

const api = vi.hoisted(() => ({
  previewConfiguration: vi.fn(),
  applyConfiguration: vi.fn(),
}));
vi.mock("@/api/modules/conversation", () => ({ conversationApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const target = { scope: "project" as const, projectId: "my-project" };
const preview = {
  current: {
    epoch: "old",
    capability_configuration: {
      selection_policy: {
        mode: "restricted",
        allowed_tools: [{ kind: "builtin", name: "read" }],
      },
    },
    model: { model: "old-model" },
  },
  candidate: {
    epoch: "candidate",
    capability_configuration: {
      selection_policy: {
        mode: "default_enabled",
        disabled_tools: [],
        disabled_capabilities: [],
      },
    },
    model: { model: "new-model" },
  },
  applied: false,
};

describe("ChatConfigurationUpdateButton", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.previewConfiguration.mockResolvedValue(preview);
    api.applyConfiguration.mockResolvedValue({ ...preview, applied: true });
  });

  it("only creates a complete candidate on explicit request, then commits the exact preview", async () => {
    const onApplied = vi.fn();
    render(
      <ChatConfigurationUpdateButton
        target={target}
        chatId="chat"
        onApplied={onApplied}
      />,
    );
    expect(api.previewConfiguration).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "tools.selectionPolicy.updateChat" }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(api.previewConfiguration).toHaveBeenCalledWith(target, "chat");
    expect(
      within(dialog).getByText(
        "capability_configuration.selection_policy.mode",
      ),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("model.model")).toBeInTheDocument();
    expect(api.applyConfiguration).not.toHaveBeenCalled();
    fireEvent.click(
      within(dialog).getByRole("button", {
        name: "tools.selectionPolicy.applyChat",
      }),
    );
    await waitFor(() =>
      expect(api.applyConfiguration).toHaveBeenCalledWith(
        target,
        "chat",
        preview,
      ),
    );
    expect(onApplied).toHaveBeenCalledOnce();
  });

  it("does not apply a cancelled candidate or silently regenerate after a stale epoch error", async () => {
    api.applyConfiguration.mockRejectedValue(
      new Error("Configuration epoch changed"),
    );
    const onApplied = vi.fn();
    render(
      <ChatConfigurationUpdateButton
        target={target}
        chatId="chat"
        onApplied={onApplied}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "tools.selectionPolicy.updateChat" }),
    );
    fireEvent.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "common.cancel",
      }),
    );
    expect(api.applyConfiguration).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "tools.selectionPolicy.updateChat" }),
    );
    await waitFor(() =>
      expect(api.previewConfiguration).toHaveBeenCalledTimes(2),
    );
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "tools.selectionPolicy.applyChat",
      }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Configuration epoch changed",
    );
    expect(api.previewConfiguration).toHaveBeenCalledTimes(2);
    expect(onApplied).not.toHaveBeenCalled();
  });
});
