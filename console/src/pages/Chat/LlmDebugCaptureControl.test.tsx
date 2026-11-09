import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "@/ui";
import LlmDebugCaptureControl from "./LlmDebugCaptureControl";

const getCapture = vi.fn();
const startCapture = vi.fn();
const stopCapture = vi.fn();
const openDirectory = vi.fn();
const getProjectCapture = vi.fn();
const startProjectCapture = vi.fn();
const stopProjectCapture = vi.fn();
const getAgentRunningConfig = vi.fn();

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "chat.debugCapture.title": "LLM Debug Capture",
        "chat.debugCapture.toggle": "Toggle local LLM Debug Capture",
        "chat.debugCapture.openDirectory": "Open capture directory",
        "chat.debugCapture.openDirectoryFailed":
          "Failed to open capture directory",
        "chat.debugCapture.copyPath": "Copy capture path",
      })[key] ?? key,
  }),
}));

vi.mock("../../api/modules/chat", () => ({
  chatApi: {
    getLlmDebugCapture: (...args: unknown[]) => getCapture(...args),
    startLlmDebugCapture: (...args: unknown[]) => startCapture(...args),
    stopLlmDebugCapture: (...args: unknown[]) => stopCapture(...args),
  },
}));

vi.mock("../../api/modules/agents", () => ({
  agentsApi: {
    getAgentRunningConfig: (...args: unknown[]) =>
      getAgentRunningConfig(...args),
  },
}));

vi.mock("../../desktop/runtime", () => ({
  isDesktopShell: () => true,
  openLlmDebugCaptureDirectory: (...args: unknown[]) => openDirectory(...args),
}));

vi.mock("../../api/modules/projectChats", () => ({
  projectChatsApi: {
    getLlmDebugCapture: (...args: unknown[]) => getProjectCapture(...args),
    startLlmDebugCapture: (...args: unknown[]) => startProjectCapture(...args),
    stopLlmDebugCapture: (...args: unknown[]) => stopProjectCapture(...args),
  },
}));

describe("LlmDebugCaptureControl", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getAgentRunningConfig.mockResolvedValue({ show_debug_menu: true });
    getCapture.mockResolvedValue({ active: false });
    startCapture.mockResolvedValue({
      active: true,
      capture_id: "70b21983-1ca2-4f56-b933-a45d09f96431",
      path: "/tmp/debug/llm-captures/70b21983-1ca2-4f56-b933-a45d09f96431",
    });
    stopCapture.mockResolvedValue({ active: false });
    openDirectory.mockResolvedValue(true);
    getProjectCapture.mockResolvedValue({ active: false });
    startProjectCapture.mockResolvedValue({
      active: true,
      capture_id: "8044d13d-c997-4bf6-a8cb-6bde4b238f71",
      path: "/tmp/debug/llm-captures/8044d13d-c997-4bf6-a8cb-6bde4b238f71",
    });
    stopProjectCapture.mockResolvedValue({ active: false });
  });

  it("stays hidden and skips capture status requests when the global menu option is disabled", async () => {
    getAgentRunningConfig.mockResolvedValue({ show_debug_menu: false });

    render(<LlmDebugCaptureControl agentId="agent/one" chatId="chat/two" />);

    await waitFor(() =>
      expect(getAgentRunningConfig).toHaveBeenCalledWith("agent/one"),
    );
    expect(
      screen.queryByRole("checkbox", {
        name: "Toggle local LLM Debug Capture",
      }),
    ).not.toBeInTheDocument();
    expect(getCapture).not.toHaveBeenCalled();
    expect(getProjectCapture).not.toHaveBeenCalled();
  });

  it("loads and toggles capture for the complete Agent Chat address", async () => {
    render(<LlmDebugCaptureControl agentId="agent/one" chatId="chat/two" />);

    await waitFor(() =>
      expect(getAgentRunningConfig).toHaveBeenCalledWith("agent/one"),
    );
    await waitFor(() =>
      expect(getCapture).toHaveBeenCalledWith("chat/two", "agent/one"),
    );
    const toggle = screen.getByRole("checkbox", {
      name: "Toggle local LLM Debug Capture",
    });
    expect(toggle).not.toBeChecked();

    fireEvent.click(toggle);
    await waitFor(() =>
      expect(startCapture).toHaveBeenCalledWith("chat/two", "agent/one"),
    );
    expect(toggle).toBeChecked();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Open capture directory",
      }),
    );
    await waitFor(() =>
      expect(openDirectory).toHaveBeenCalledWith(
        "/tmp/debug/llm-captures/70b21983-1ca2-4f56-b933-a45d09f96431",
      ),
    );

    fireEvent.click(toggle);
    await waitFor(() =>
      expect(stopCapture).toHaveBeenCalledWith("chat/two", "agent/one"),
    );
    expect(toggle).not.toBeChecked();
  });

  it("reports a desktop bridge failure instead of silently copying the path", async () => {
    getCapture.mockResolvedValue({
      active: true,
      capture_id: "70b21983-1ca2-4f56-b933-a45d09f96431",
      path: "/tmp/debug/llm-captures/70b21983-1ca2-4f56-b933-a45d09f96431",
    });
    openDirectory.mockRejectedValue(new Error("command not allowed"));
    const toastSpy = vi.spyOn(toast, "error").mockImplementation(() => "");
    const consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    render(<LlmDebugCaptureControl agentId="agent/one" chatId="chat/two" />);

    fireEvent.click(
      await screen.findByRole("button", { name: "Open capture directory" }),
    );

    await waitFor(() =>
      expect(toastSpy).toHaveBeenCalledWith("Failed to open capture directory"),
    );
    expect(consoleSpy).toHaveBeenCalledWith(
      "Failed to open LLM Debug Capture directory:",
      expect.any(Error),
    );
  });

  it("uses the Project Chat address when projectId is present", async () => {
    render(
      <LlmDebugCaptureControl
        agentId="execution-agent"
        chatId="shared-chat"
        projectId="project-one"
      />,
    );

    await waitFor(() =>
      expect(getAgentRunningConfig).toHaveBeenCalledWith("execution-agent"),
    );
    await waitFor(() =>
      expect(getProjectCapture).toHaveBeenCalledWith(
        "project-one",
        "shared-chat",
      ),
    );
    expect(getCapture).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Toggle local LLM Debug Capture",
      }),
    );
    await waitFor(() =>
      expect(startProjectCapture).toHaveBeenCalledWith(
        "project-one",
        "shared-chat",
      ),
    );
    expect(startCapture).not.toHaveBeenCalled();
  });
});
