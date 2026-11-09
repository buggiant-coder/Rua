import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter, useLocation, useNavigationType } from "react-router-dom";

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});
afterEach(() => vi.unstubAllGlobals());
import { historyProvesConversationMaterialized } from "../../Chat/conversationRouteController";

import * as drafts from "../../Chat/drafts";
import {
  loadDraftChatModel,
  saveDraftChatModel,
  saveLastChatId,
} from "../../Chat/chatNavigation";
import {
  createProjectChat,
  lastProjectChatId,
  projectChatDraftKey,
  projectChatExecutionAgent,
  rememberProjectChat,
  setProjectLastSelectedAgent,
} from "../projectChatNavigation";
import ProjectChatPage, { loader } from "./index";
import { ModalProvider, ToastProvider } from "@/ui";
import { useAgentStore } from "@/stores/agentStore";
import {
  subscribeConversationNavigation,
  publishConversationDeleted,
  requestNewConversation,
} from "@/layouts/conversationNavigationEvents";
import { getProjectNavigationAgent } from "@/layouts/conversationNavigationAgent";

const {
  agentsApiMock,
  conversationApiMock,
  projectChatsApiMock,
  projectsApiMock,
  modelSelectorProps,
  chatOptionsModalProps,
  acpEnterDialogProps,
  surfaceProps,
  taskDrawerProps,
  exportConversationMock,
} = vi.hoisted(() => ({
  agentsApiMock: {
    listAgents: vi.fn(),
    getAgentRunningConfig: vi.fn(),
  },
  conversationApiMock: {
    exitAcp: vi.fn(),
  },
  projectChatsApiMock: {
    listChats: vi.fn(),
    searchChats: vi.fn(),
    getChat: vi.fn(),
    updateChat: vi.fn(),
    deleteChat: vi.fn(),
    switchAgent: vi.fn(),
    getResourcePolicy: vi.fn(),
  },
  projectsApiMock: {
    get: vi.fn(),
    list: vi.fn(),
    browseResource: vi.fn(),
    browseDirectory: vi.fn(),
    getGitStatus: vi.fn(),
    create: vi.fn(),
    getLifecyclePreview: vi.fn(),
    archive: vi.fn(),
    reopen: vi.fn(),
    permanentlyDelete: vi.fn(),
  },
  modelSelectorProps: { current: null as Record<string, unknown> | null },
  chatOptionsModalProps: { current: null as Record<string, unknown> | null },
  acpEnterDialogProps: { current: null as Record<string, unknown> | null },
  surfaceProps: { current: null as Record<string, unknown> | null },
  taskDrawerProps: { current: null as Record<string, unknown> | null },
  exportConversationMock: vi.fn(),
}));

vi.mock("react-i18next", async () => {
  const messages = (await import("@/locales/en.json")).default as Record<
    string,
    unknown
  >;
  const t = (key: string, options: Record<string, unknown> = {}) => {
    const value = key
      .split(".")
      .reduce<unknown>(
        (current, segment) =>
          current && typeof current === "object" && segment in current
            ? (current as Record<string, unknown>)[segment]
            : undefined,
        messages,
      );
    if (typeof value !== "string") return key;
    return value.replace(/\{\{(\w+)\}\}/g, (_, name: string) =>
      String(options[name] ?? ""),
    );
  };
  return {
    useTranslation: () => ({ t }),
  };
});

vi.mock("@/api/modules/agents", () => ({
  agentsApi: agentsApiMock,
}));

vi.mock("@/api/modules/conversation", () => ({
  conversationApi: conversationApiMock,
  agentConversationTarget: (agentId: string) => ({
    scope: "agent",
    agentId,
  }),
}));

vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: projectChatsApiMock,
}));

vi.mock("@/api/modules/projects", () => ({
  projectsApi: projectsApiMock,
}));

vi.mock("@/pages/Chat/exportConversation", () => ({
  exportConversationAsMarkdown: exportConversationMock,
}));

vi.mock("@/hooks/useMultiDeviceFeatureAvailability", () => ({
  useMultiDeviceFeatureAvailability: () => ({
    devices: [
      {
        device_id: "controller",
        public_key: "controller-key",
        friendly_name: "Controller",
        label: "Controller",
        added_at_ms: 1,
        last_seen_ms: 1,
        is_self: true,
        connectivity_state: "connected",
        session_path: "direct",
        session_rtt_ms: 1,
      },
      {
        device_id: "resource-device",
        public_key: "resource-key",
        friendly_name: "Resource",
        label: "Resource",
        added_at_ms: 1,
        last_seen_ms: 1,
        is_self: false,
        connectivity_state: "connected",
        session_path: "direct",
        session_rtt_ms: 1,
      },
    ],
    loaded: true,
    available: true,
  }),
}));

vi.mock("@/pages/Chat", () => ({
  ChatOptionsModal: (props: Record<string, unknown>) => {
    chatOptionsModalProps.current = props;
    return props.open ? <div role="dialog">Conversation options</div> : null;
  },
}));

vi.mock("@/pages/Chat/ACPEnterDialog", () => ({
  default: (props: Record<string, unknown>) => {
    acpEnterDialogProps.current = props;
    return props.open ? <div role="dialog">Enter ACP</div> : null;
  },
}));

vi.mock("@/pages/Chat/ModelSelector", () => ({
  default: (props: Record<string, unknown>) => {
    modelSelectorProps.current = props;
    return <span>Model selector</span>;
  },
}));

vi.mock("@/pages/Chat/owned/OwnedConversationSurface", () => ({
  default: (props: Record<string, unknown>) => {
    surfaceProps.current = props;
    return (
      <div data-testid="owned-project-surface">
        {props.composerLeftTools ? (
          <div data-testid="project-composer-left-tools">
            {props.composerLeftTools as import("react").ReactNode}
          </div>
        ) : null}
        {props.composerDeviceChip as import("react").ReactNode}
        <textarea aria-label="Project message" />
        <button
          type="button"
          disabled={Boolean(props.submitDisabled)}
          title={String(props.submitDisabledReason || "")}
        >
          Send
        </button>
        <button
          type="button"
          onClick={() =>
            (props.onSubmitAccepted as (() => void) | undefined)?.()
          }
        >
          Accept
        </button>
        <button
          type="button"
          onClick={() =>
            (
              props.onHistoryChange as
                | ((history: Record<string, unknown>) => void)
                | undefined
            )?.({
              messages: [
                {
                  role: "user",
                  content: [{ type: "text", text: "Hello" }],
                },
              ],
              status: "running",
              model: {
                provider_id: "dashscope",
                model: "qwen-plus",
              },
              control: {
                acp: { active: false },
                queue: { entries: [], paused: false },
                project_chat_agent_state: {
                  selection: {
                    agent_id: "default",
                    selected_at: "2026-08-01T00:00:00Z",
                    revision: 1,
                  },
                  selected_agent_available: true,
                  switchable: true,
                  blockers: [],
                },
              },
            })
          }
        >
          Materialize history
        </button>
        <button
          type="button"
          onClick={() =>
            (
              props.onHistoryChange as
                | ((history: Record<string, unknown>) => void)
                | undefined
            )?.({
              messages: [{ type: "ask_user_request" }],
              status: "idle",
              control: {
                acp: { active: false },
                queue: { entries: [], paused: false },
                project_chat_agent_state: {
                  selection: {
                    agent_id: "default",
                    selected_at: "2026-08-01T00:00:00Z",
                    revision: 1,
                  },
                  selected_agent_available: true,
                  switchable: true,
                  blockers: [],
                },
              },
            })
          }
        >
          Load history
        </button>
        <button
          type="button"
          onClick={() =>
            (
              props.onHistoryChange as
                | ((history: Record<string, unknown>) => void)
                | undefined
            )?.({
              messages: [{ role: "user", content: "history remains" }],
              status: "idle",
              control: {
                acp: { active: false },
                queue: { entries: [], paused: false },
                project_chat_agent_state: {
                  selection: {
                    agent_id: "deleted-agent",
                    selected_at: "2026-08-01T00:00:00Z",
                    revision: 3,
                  },
                  selected_agent_available: false,
                  switchable: true,
                  blockers: [],
                },
              },
            })
          }
        >
          Load unavailable Agent
        </button>
      </div>
    );
  },
}));

vi.mock("@/pages/Chat/tasks/TaskActivityDrawer", () => ({
  default: (props: Record<string, unknown>) => {
    taskDrawerProps.current = props;
    return props.open ? <div role="dialog">Task activity</div> : null;
  },
}));

const project = {
  project_id: "project-1",
  display_name: "rua-rust",
  resource_device_id: "resource-device",
  display_path: "/repos/rua-rust",
  lifecycle: "active" as const,
  created_at: "2026-08-01T00:00:00Z",
  updated_at: "2026-08-01T00:00:00Z",
};

const secondProject = {
  ...project,
  project_id: "project-2",
  display_name: "codex",
  display_path: "/repos/codex",
};

describe("ProjectChat loader", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    projectsApiMock.get.mockResolvedValue(project);
    projectsApiMock.list.mockResolvedValue([project]);
  });

  it("treats empty history for a complete remote address as Lazy Chat", async () => {
    const emptyHistory = { messages: [], meta: {} };
    projectChatsApiMock.getChat.mockResolvedValue(emptyHistory);

    await expect(
      loader({
        params: { projectId: "project-1", chatId: "chat-after-restart" },
        request: new Request(
          "http://localhost/projects/project-1/chats/chat-after-restart",
        ),
        context: {},
      }),
    ).resolves.toMatchObject({
      project,
      chatId: "chat-after-restart",
      initialHistory: emptyHistory,
      unmaterialized: true,
    });
    expect(projectChatsApiMock.getChat).toHaveBeenCalledWith(
      "project-1",
      "chat-after-restart",
      { turnLimit: 1 },
    );
  });

  it("keeps a local draft lazy without loading remote history", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-local-draft");
    createProjectChat("project-1");

    await expect(
      loader({
        params: { projectId: "project-1", chatId: "chat-local-draft" },
        request: new Request(
          "http://localhost/projects/project-1/chats/chat-local-draft",
        ),
        context: {},
      }),
    ).resolves.toMatchObject({
      project,
      chatId: "chat-local-draft",
      initialHistory: null,
      unmaterialized: true,
    });
    expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
  });
});

describe("ProjectChatPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
    sessionStorage.clear();
    localStorage.clear();
    useAgentStore.setState({
      selectedAgent: "default",
      agents: [],
      catalogLoaded: false,
      catalogLoading: false,
      catalogError: null,
    });
    modelSelectorProps.current = null;
    chatOptionsModalProps.current = null;
    surfaceProps.current = null;
    taskDrawerProps.current = null;
    exportConversationMock.mockResolvedValue({
      filename: "chat-controls.md",
      folderHint: "",
      saved: true,
    });
    agentsApiMock.listAgents.mockResolvedValue({
      agents: [
        {
          id: "default",
          name: "Default Agent",
          description: "",
          workspace_dir: "/agents/default",
          enabled: true,
        },
      ],
    });
    agentsApiMock.getAgentRunningConfig.mockResolvedValue({
      show_debug_menu: false,
    });
    projectsApiMock.get.mockResolvedValue(project);
    projectsApiMock.list.mockResolvedValue([project, secondProject]);
    projectsApiMock.getLifecyclePreview.mockResolvedValue({
      project,
      impact: {
        affected_chat_count: 2,
        active_turn_count: 1,
        queued_input_count: 1,
        pending_approval_count: 0,
        pending_prompt_count: 0,
        active_acp_count: 0,
        background_task_count: 3,
      },
      resource_device_revoked: false,
    });
    projectsApiMock.archive.mockResolvedValue({
      project: { ...project, lifecycle: "archived" },
      impact: null,
      resource_device_revoked: false,
      changed: true,
    });
    projectsApiMock.reopen.mockResolvedValue({
      project: { ...project, lifecycle: "active" },
      changed: true,
    });
    projectsApiMock.permanentlyDelete.mockResolvedValue({
      project_id: "project-1",
      deleted_chat_count: 1,
      changed: true,
    });
    projectsApiMock.browseResource.mockResolvedValue({
      device_id: "resource-dev",
      path: "/repos/rua-rust",
      parent_path: "/repos",
      proposed_name: "rua-rust",
      entries: [
        {
          name: "console",
          path: "/repos/rua-rust/console",
          kind: "directory",
          is_symlink: false,
        },
        {
          name: "AGENTS.md",
          path: "/repos/rua-rust/AGENTS.md",
          kind: "file",
          is_symlink: false,
        },
      ],
    });
    projectsApiMock.getGitStatus.mockResolvedValue({
      repository: true,
      branch: "main",
      detached: false,
      ahead: 0,
      behind: 0,
      changes: [
        {
          path: "console/src/App.tsx",
          status: "M",
          staged: false,
        },
      ],
      head: {
        oid: "1234567890abcdef",
        summary: "Latest commit",
        authored_at: "2026-08-03T00:00:00Z",
      },
    });
    projectChatsApiMock.getChat.mockResolvedValue({
      messages: [],
      meta: {},
    });
    projectChatsApiMock.listChats.mockResolvedValue({
      data: [],
      total: 0,
    });
    projectChatsApiMock.searchChats.mockResolvedValue({ results: [] });
    projectChatsApiMock.updateChat.mockImplementation(
      async (_projectId, _chatId, update) => ({
        ...chatSpec(_chatId, _chatId),
        ...update,
      }),
    );
    projectChatsApiMock.deleteChat.mockResolvedValue({
      deleted: true,
      chat_id: "deleted",
      was_busy: false,
    });
    projectChatsApiMock.getResourcePolicy.mockResolvedValue({
      policy: {
        profile: "resource-write",
        source: "project_default",
        cwd: "/repos/rua-rust",
        writable_roots: ["/repos/rua-rust"],
        dedicated_operations_root_confined: true,
        tool_guard_enabled: true,
        approval_mode: "ask_first",
      },
      pending_approval_count: 1,
      chat_rule_count: 2,
      agent_rule_count: 3,
    });
    conversationApiMock.exitAcp.mockResolvedValue({
      ok: true,
      message: "Exited ACP",
    });
  });

  it("does not render a working-folder selector for multi-folder projects", async () => {
    renderPage({
      project: {
        ...project,
        resources: {
          revision: 1,
          primary_directory_id: "main",
          directories: [
            { directory_id: "main", display_path: "/repo/main" },
            { directory_id: "docs", display_path: "/repo/docs" },
          ],
        },
      } as typeof project,
      chatId: "draft",
      initialHistory: null,
    });
    await screen.findByText("Default Agent");
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.queryByText("/repo/main")).not.toBeInTheDocument();
  });

  it("mounts the production surface with the Project target and focus header", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-1");
    createProjectChat(project.project_id);

    const { container } = renderPage({
      project,
      chatId: "chat-1",
      initialHistory: null,
    });

    expect(await screen.findByText("Default Agent")).toBeVisible();
    expect(screen.getByText("rua-rust")).toBeVisible();
    expect(screen.queryByTitle("Resource · Connected")).not.toBeInTheDocument();
    const deviceChip = screen.getByRole("button", { name: /Resource/ });
    expect(deviceChip).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(deviceChip);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(surfaceProps.current?.address).toEqual({
      scope: "project",
      projectId: "project-1",
      chatId: "chat-1",
    });
    expect(surfaceProps.current?.projectExecutionAgentId).toBe("default");
    expect(surfaceProps.current?.unmaterialized).toBe(true);
    expect(surfaceProps.current?.draftKey).toBe("project:project-1:chat-1");
    expect(surfaceProps.current?.activityWake).toBeNull();
    expect(surfaceProps.current?.attachmentsDisabled).toBeUndefined();
    expect(surfaceProps.current?.composerLeftTools).toBeTruthy();
    expect(
      screen.getByTestId("project-composer-left-tools"),
    ).toBeInTheDocument();
    expect(modelSelectorProps.current).toMatchObject({
      chatId: "chat-1",
      draftSelection: true,
      disabled: false,
    });
    expect(container.querySelector(".rua-owned-chat-page")).not.toBeNull();
    expect(container.querySelector(".rua-owned-chat-header")).not.toBeNull();
    expect(screen.getByRole("button", { name: "Manage chats" })).toHaveClass(
      "rua-btn",
    );

    fireEvent.click(
      screen.getByRole("button", { name: "More Project actions" }),
    );
    const lazyAcp = screen.getByRole("menuitem", {
      name: "Enter ACP pass-through",
    });
    expect(lazyAcp).not.toHaveAttribute("aria-disabled", "true");
    fireEvent.click(lazyAcp);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(acpEnterDialogProps.current).toMatchObject({
      agentId: "default",
      target: { scope: "project", projectId: "project-1" },
      discoveryTarget: { scope: "agent", agentId: "default" },
      chatId: "chat-1",
      targetDeviceId: "resource-device",
    });
    expect(screen.queryByRole("menuitem", { name: "Archive" })).toBeNull();
  });

  it("opens Conversation options for an unmaterialized Project Chat", async () => {
    renderPage({
      project,
      chatId: "chat-lazy-options",
      initialHistory: null,
    });

    await screen.findByText("Default Agent");
    fireEvent.click(
      screen.getByRole("button", { name: "More Project actions" }),
    );
    const options = await screen.findByRole("menuitem", {
      name: "Chat options",
    });
    expect(options).not.toHaveAttribute("aria-disabled", "true");
    fireEvent.click(options);
    expect(screen.getByRole("dialog")).toHaveTextContent(
      "Conversation options",
    );
    expect(chatOptionsModalProps.current).toMatchObject({
      chatId: "chat-lazy-options",
      selectedAgent: "default",
      target: { scope: "project", projectId: "project-1" },
    });
  });

  it("scopes Conversation options to a materialized Project Chat", async () => {
    renderPage({
      project,
      chatId: "chat-materialized-options",
      initialHistory: materializedHistory(),
    });

    await screen.findByText("Default Agent");
    expect(chatOptionsModalProps.current).toMatchObject({
      chatId: "chat-materialized-options",
      selectedAgent: "default",
      target: { scope: "project", projectId: "project-1" },
    });
  });

  it.each(["header", "sidebar"])(
    "uses the shared Project creation action from %s",
    async (entry) => {
      vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-new-from-header");
      renderPage({
        project,
        chatId: "chat-existing",
        initialHistory: {
          ...materializedHistory(),
          model: {
            provider_id: "dashscope",
            model: "qwen-plus",
            thinking_strength: "high",
          },
        },
      });
      await screen.findByText("Default Agent");

      if (entry === "header") {
        fireEvent.click(
          screen.getAllByRole("button", { name: "New Project Chat" })[0],
        );
      } else {
        act(() => {
          expect(
            requestNewConversation({
              scope: "project",
              projectId: "project-1",
              chatId: "chat-existing",
            }),
          ).toBe(true);
        });
      }
      await waitFor(() =>
        expect(screen.getByTestId("location")).toHaveTextContent(
          "/projects/project-1/chats/chat-new-from-header",
        ),
      );
      expect(
        loadDraftChatModel("project:project-1", "chat-new-from-header"),
      ).toEqual({
        provider_id: "dashscope",
        model: "qwen-plus",
        thinking_strength: "high",
      });
      expect(
        projectChatExecutionAgent("project-1", "chat-new-from-header"),
      ).toBe("default");
      drafts.save(
        projectChatDraftKey("project-1", "chat-new-from-header"),
        "unfinished Project prompt",
      );
      fireEvent.click(
        screen.getAllByRole("button", { name: "New Project Chat" })[0],
      );
      await waitFor(() =>
        expect(screen.getByTestId("location")).toHaveTextContent(
          "/projects/project-1/chats/chat-new-from-header",
        ),
      );
      expect(
        drafts.load(projectChatDraftKey("project-1", "chat-new-from-header")),
      ).toBe("unfinished Project prompt");
      expect(crypto.randomUUID).toHaveBeenCalledTimes(1);

      fireEvent.click(
        screen.getByRole("button", { name: "More Project actions" }),
      );
      fireEvent.click(screen.getByRole("menuitem", { name: "New Project" }));
      expect(screen.getByRole("dialog")).toHaveTextContent("New Project");
      fireEvent.click(
        within(screen.getByRole("dialog")).getByRole("button", {
          name: "Close",
        }),
      );

      fireEvent.click(screen.getByRole("button", { name: "Select Project" }));
      fireEvent.click(screen.getByRole("button", { name: "New Project" }));
      expect(screen.getByRole("dialog")).toHaveTextContent("New Project");
    },
  );

  it("materializes the Project draft model when the first Submit is accepted", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-model-handoff");
    createProjectChat("project-1");
    saveDraftChatModel("project:project-1", "chat-model-handoff", {
      provider_id: "dashscope",
      model: "qwen-plus",
    });
    renderPage({
      project,
      chatId: "chat-model-handoff",
      initialHistory: null,
    });
    await screen.findByText("Default Agent");

    expect(modelSelectorProps.current).toMatchObject({
      chatModel: {
        provider_id: "dashscope",
        model: "qwen-plus",
      },
      draftSelection: true,
    });

    fireEvent.click(screen.getByRole("button", { name: "Accept" }));
    expect(modelSelectorProps.current).toMatchObject({
      chatModel: null,
      draftSelection: false,
    });
    expect(surfaceProps.current?.unmaterialized).toBe(false);

    fireEvent.click(
      screen.getByRole("button", { name: "Materialize history" }),
    );
    await waitFor(() =>
      expect(surfaceProps.current?.unmaterialized).toBe(false),
    );
    expect(modelSelectorProps.current).toMatchObject({
      chatModel: {
        provider_id: "dashscope",
        model: "qwen-plus",
      },
      draftSelection: false,
    });
  });

  it("does not gate Submit on a separate Project availability state", async () => {
    renderPage({
      project,
      chatId: "chat-2",
    });

    const input = screen.getByLabelText("Project message");
    fireEvent.change(input, { target: { value: "keep this draft" } });

    expect(input).toHaveValue("keep this draft");
    await waitFor(() =>
      expect(agentsApiMock.listAgents).toHaveBeenCalledTimes(1),
    );
    expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
  });

  it("clears pre-submit state and draft after the first Submit is accepted", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-3");
    createProjectChat(project.project_id);
    drafts.save(
      projectChatDraftKey(project.project_id, "chat-3"),
      "materialize me",
    );

    renderPage({
      project,
      chatId: "chat-3",
      initialHistory: null,
    });
    await screen.findByText("Default Agent");

    fireEvent.click(screen.getByRole("button", { name: "Accept" }));

    expect(surfaceProps.current?.unmaterialized).toBe(false);
    expect(drafts.load(projectChatDraftKey(project.project_id, "chat-3"))).toBe(
      "",
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Materialize history" }),
    );

    await waitFor(() =>
      expect(surfaceProps.current?.unmaterialized).toBe(false),
    );
    expect(drafts.load(projectChatDraftKey(project.project_id, "chat-3"))).toBe(
      "",
    );
    expect(surfaceProps.current?.address).toEqual({
      scope: "project",
      projectId: "project-1",
      chatId: "chat-3",
    });
    expect(surfaceProps.current?.projectExecutionAgentId).toBe("default");
    expect(surfaceProps.current?.unmaterialized).toBe(false);
  });

  it("opens from controller-local Agents and commits a materialized switch without optimism", async () => {
    agentsApiMock.listAgents.mockResolvedValue({
      agents: [
        {
          id: "default",
          name: "Default Agent",
          description: "",
          workspace_dir: "/agents/default",
          enabled: true,
        },
        {
          id: "code",
          name: "Code Agent",
          description: "",
          workspace_dir: "/agents/code",
          enabled: true,
        },
      ],
    });
    let resolveSwitch!: (value: {
      selection: {
        agent_id: string;
        selected_at: string;
        revision: number;
      };
      selected_agent_available: boolean;
      switchable: boolean;
      blockers: never[];
    }) => void;
    projectChatsApiMock.switchAgent.mockReturnValue(
      new Promise((resolve) => {
        resolveSwitch = resolve;
      }),
    );

    setProjectLastSelectedAgent("project-1", "code");
    useAgentStore.setState({ selectedAgent: "ordinary-agent" });
    const page = renderPage({
      project,
      chatId: "chat-materialized",
    });
    fireEvent.click(screen.getByRole("button", { name: "Load history" }));
    expect(await screen.findByText("Default Agent")).toBeVisible();
    expect(getProjectNavigationAgent()).toEqual({
      projectId: "project-1",
      chatId: "chat-materialized",
      agentId: "default",
    });
    expect(useAgentStore.getState().selectedAgent).toBe("ordinary-agent");

    fireEvent.click(screen.getByRole("button", { name: "Project Chat Agent" }));
    expect(projectChatsApiMock.switchAgent).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /Code Agent/ }));
    fireEvent.click(
      await screen.findByRole("button", { name: "Switch Agent" }),
    );

    await waitFor(() =>
      expect(projectChatsApiMock.switchAgent).toHaveBeenCalledWith(
        "project-1",
        "chat-materialized",
        "default",
        "code",
      ),
    );
    expect(
      screen.getByRole("button", { name: "Project Chat Agent" }),
    ).toHaveTextContent("Default Agent");

    expect(getProjectNavigationAgent()?.agentId).toBe("default");
    await act(async () => {
      resolveSwitch({
        selection: {
          agent_id: "code",
          selected_at: "2026-08-01T00:01:00Z",
          revision: 2,
        },
        selected_agent_available: true,
        switchable: true,
        blockers: [],
      });
    });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Project Chat Agent" }),
      ).toHaveTextContent("Code Agent"),
    );
    expect(getProjectNavigationAgent()).toEqual({
      projectId: "project-1",
      chatId: "chat-materialized",
      agentId: "code",
    });
    expect(useAgentStore.getState().selectedAgent).toBe("ordinary-agent");
    page.unmount();
    expect(getProjectNavigationAgent()).toBeNull();
  });

  it("keeps a newer live Agent selection when stale Sync history arrives", async () => {
    agentsApiMock.listAgents.mockResolvedValue({
      agents: [
        {
          id: "default",
          name: "Default Agent",
          description: "",
          workspace_dir: "/agents/default",
          enabled: true,
        },
        {
          id: "code",
          name: "Code Agent",
          description: "",
          workspace_dir: "/agents/code",
          enabled: true,
        },
      ],
    });
    renderPage({
      project,
      chatId: "chat-live-selection",
      initialHistory: materializedHistory(),
    });
    expect(await screen.findByText("Default Agent")).toBeVisible();

    act(() => {
      (
        surfaceProps.current?.onHistoryChange as
          | ((history: Record<string, unknown>) => void)
          | undefined
      )?.({
        messages: [],
        status: "idle",
        control: {
          revision: 4,
          placement: {
            authority_device_id: "resource-dev",
            authority_epoch: "epoch-project-live",
            switchable: false,
          },
          active_turn: null,
          queue: { entries: [], paused: false },
          acp: { active: false },
          project_chat_agent_state: {
            selection: {
              agent_id: "code",
              selected_at: "2026-08-01T00:01:00Z",
              revision: 2,
            },
            selected_agent_available: true,
            switchable: true,
            blockers: [],
          },
          allowed_controls: {
            stop: false,
            queue: false,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: false,
            enter_acp: true,
            exit_acp: false,
          },
        },
      });
    });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Project Chat Agent" }),
      ).toHaveTextContent("Code Agent"),
    );
    expect(surfaceProps.current?.projectExecutionAgentId).toBe("code");

    act(() => {
      (
        surfaceProps.current?.onHistoryChange as
          | ((history: Record<string, unknown>) => void)
          | undefined
      )?.({
        messages: [],
        status: "idle",
        control: {
          acp: { active: false },
          queue: { entries: [], paused: false },
          project_chat_agent_state: {
            selection: {
              agent_id: "default",
              selected_at: "2026-08-01T00:00:00Z",
              revision: 1,
            },
            selected_agent_available: true,
            switchable: true,
            blockers: [],
          },
        },
      });
    });
    expect(
      screen.getByRole("button", { name: "Project Chat Agent" }),
    ).toHaveTextContent("Code Agent");
    expect(surfaceProps.current?.projectExecutionAgentId).toBe("code");
  });

  it("keeps an unavailable durable selection without falling back", async () => {
    renderPage({
      project,
      chatId: "chat-unavailable-agent",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Load unavailable Agent" }),
    );

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Project Chat Agent" }),
      ).toHaveTextContent("Unavailable Agent"),
    );
    expect(screen.queryByText("deleted-agent")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toHaveAttribute(
      "title",
      "Select an available Agent before submitting",
    );
    expect(surfaceProps.current?.address).toEqual({
      scope: "project",
      projectId: "project-1",
      chatId: "chat-unavailable-agent",
    });
    expect(surfaceProps.current?.projectExecutionAgentId).toBe("deleted-agent");
  });

  it("keeps the Project selector independent from the ordinary Agent selection", async () => {
    agentsApiMock.listAgents.mockResolvedValue({
      agents: [
        {
          id: "default",
          name: "Default Agent",
          description: "",
          workspace_dir: "/agents/default",
          enabled: true,
        },
        {
          id: "code",
          name: "Code Agent",
          description: "",
          workspace_dir: "/agents/code",
          enabled: true,
        },
      ],
    });
    useAgentStore.getState().setSelectedAgent("ordinary-agent");
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-independent");
    createProjectChat(project.project_id);

    renderPage({
      project,
      chatId: "chat-independent",
      initialHistory: null,
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Project Chat Agent" }));
    fireEvent.click(screen.getByRole("button", { name: /Code Agent/ }));

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Project Chat Agent" }),
      ).toHaveTextContent("Code Agent"),
    );
    expect(useAgentStore.getState().selectedAgent).toBe("ordinary-agent");
    expect(projectChatsApiMock.switchAgent).not.toHaveBeenCalled();
  });

  it("falls back to Default Agent when the per-Project recent hint is invalid", async () => {
    setProjectLastSelectedAgent(project.project_id, "missing-agent");
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-invalid-hint");
    createProjectChat(project.project_id);

    renderPage({
      project,
      chatId: "chat-invalid-hint",
      initialHistory: null,
    });

    expect(await screen.findByText("Default Agent")).toBeVisible();
    expect(surfaceProps.current?.address).toEqual({
      scope: "project",
      projectId: "project-1",
      chatId: "chat-invalid-hint",
    });
    expect(surfaceProps.current?.projectExecutionAgentId).toBe("default");
  });

  it("keeps right drawers mutually exclusive without replacing the conversation", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-drawers");
    createProjectChat(project.project_id);
    renderPage({
      project,
      chatId: "chat-drawers",
      initialHistory: null,
    });
    await screen.findByText("Default Agent");
    const conversation = screen.getByTestId("owned-project-surface");

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
    expect(projectChatsApiMock.listChats).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    expect(screen.getByTestId("owned-project-surface")).toBe(conversation);

    fireEvent.click(screen.getByRole("button", { name: "Workspace" }));
    await waitFor(() =>
      expect(screen.getByRole("dialog")).toHaveTextContent("Workspace"),
    );
    fireEvent.click(screen.getByRole("tab", { name: "Context" }));
    expect(screen.getByText("Available after first turn")).toBeVisible();
    expect(projectChatsApiMock.getResourcePolicy).not.toHaveBeenCalled();
    expect(screen.getByTestId("owned-project-surface")).toBe(conversation);
  });

  it("keeps the narrow-screen chat list closed until opened as an overlay", async () => {
    const width = window.innerWidth;
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: 390,
    });
    try {
      renderPage({
        project,
        chatId: "narrow-chat",
        initialHistory: materializedHistory(),
      });
      await screen.findByText("Default Agent");
      expect(
        screen.queryByRole("complementary", { name: "Manage chats" }),
      ).not.toBeInTheDocument();
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
      expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    } finally {
      Object.defineProperty(window, "innerWidth", {
        configurable: true,
        value: width,
      });
    }
  });

  it("loads effective policy from the resource Device when Workspace opens", async () => {
    renderPage({
      project,
      chatId: "chat-resource-policy",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");

    fireEvent.click(screen.getByRole("button", { name: "Workspace" }));

    expect(await screen.findByText("AGENTS.md")).toBeVisible();
    fireEvent.click(screen.getByRole("tab", { name: "Context" }));
    expect(await screen.findByText("Resource write")).toBeVisible();
    expect(projectChatsApiMock.getResourcePolicy).toHaveBeenCalledWith(
      "project-1",
      "chat-resource-policy",
    );
    expect(
      screen.getByText("/repos/rua-rust", { selector: "code" }),
    ).toBeVisible();
    expect(screen.getByText("1 pending · 2 Chat · 3 Agent")).toBeVisible();
  });

  it("reuses Project header controls without exposing authority placement", async () => {
    renderPage({
      project,
      chatId: "chat-controls",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");

    fireEvent.click(
      screen.getByRole("button", { name: "More Project actions" }),
    );
    const menu = screen.getByRole("menu");
    expect(
      Array.from(menu.children).map((item) =>
        item.classList.contains("rua-ui-dropdown-divider")
          ? "divider"
          : item.textContent,
      ),
    ).toEqual([
      "New Project",
      "Enter ACP pass-through",
      "Export conversation",
      "Edit model context",
      "divider",
      "Refresh page",
      "Chat options",
    ]);
    expect(
      screen.queryByRole("menuitem", { name: "Projects Overview" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("menuitem", { name: /Execution device/i }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("menuitem", { name: "Chat options" }));
    expect(screen.getByRole("dialog")).toHaveTextContent(
      "Conversation options",
    );
  });

  it("exports Project Chat history through the shared Conversation target", async () => {
    renderPage({
      project,
      chatId: "chat-export",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");

    fireEvent.click(
      screen.getByRole("button", { name: "More Project actions" }),
    );
    fireEvent.click(
      screen.getByRole("menuitem", { name: "Export conversation" }),
    );

    await waitFor(() =>
      expect(exportConversationMock).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "project",
          projectId: "project-1",
        }),
        "chat-export",
      ),
    );
  });

  it("opens the shared Task activity drawer with the Project target", async () => {
    renderPage({
      project,
      chatId: "chat-tasks",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");

    fireEvent.click(screen.getByRole("button", { name: "Task activity" }));

    expect(screen.getByRole("dialog")).toHaveTextContent("Task activity");
    expect(taskDrawerProps.current).toMatchObject({
      open: true,
      agentId: "default",
      chatId: "chat-tasks",
      target: {
        scope: "project",
        projectId: "project-1",
      },
    });
  });

  it("keeps Project lifecycle actions out of the Chat menu", async () => {
    const archived = { ...project, lifecycle: "archived" as const };
    renderPage({
      project: archived,
      chatId: "chat-archived",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();

    fireEvent.click(
      screen.getByRole("button", { name: "More Project actions" }),
    );
    expect(screen.queryByRole("menuitem", { name: "Reopen" })).toBeNull();
    expect(
      screen.queryByRole("menuitem", { name: "Permanently Delete" }),
    ).toBeNull();
  });

  it.each([false, true])(
    "routes ordinary ACP exit but hides fixed collaboration exit (%s)",
    async (collaboration) => {
      renderPage({
        project,
        chatId: "chat-acp",
        initialHistory: {
          ...materializedHistory(),
          ...(collaboration
            ? {
                meta: {
                  collaboration: { executor: { kind: "acp", runner: "codex" } },
                },
              }
            : {}),
          control: {
            revision: 3,
            placement: {
              authority_device_id: "resource-device",
              authority_epoch: "epoch-3",
              switchable: false,
            },
            active_turn: null,
            queue: { paused: false, entries: [] },
            acp: {
              active: true,
              binding: {
                device_id: "resource-device",
                runner: "codex",
                cwd: "/repos/rua-rust",
                session_id: "session-1",
              },
            },
            allowed_controls: {
              stop: true,
              queue: true,
              interject: true,
              edit_queued_input: true,
              delete_queued_input: true,
              switch_authority: false,
              enter_acp: false,
              exit_acp: true,
            },
          },
        },
      });
      await screen.findByText("Default Agent");
      expect(screen.getByRole("status")).toHaveTextContent("codex");

      if (collaboration) {
        expect(screen.queryByRole("button", { name: "Exit ACP" })).toBeNull();
        return;
      }

      fireEvent.click(screen.getByRole("button", { name: "Exit ACP" }));

      await waitFor(() =>
        expect(conversationApiMock.exitAcp).toHaveBeenCalledWith(
          {
            scope: "project",
            projectId: "project-1",
          },
          "chat-acp",
        ),
      );
    },
  );

  it("renders API-ordered chats, real running state, and incremental pages without Archive", async () => {
    projectChatsApiMock.listChats
      .mockResolvedValueOnce({
        data: [
          chatSpec("chat-pinned", "Pinned design", {
            pinned: true,
            status: "idle",
            updated_at: "2026-08-01T01:00:00Z",
          }),
          chatSpec("chat-running", "Running build", {
            status: "running",
            updated_at: "2026-08-01T02:00:00Z",
          }),
        ],
        total: 3,
      })
      .mockResolvedValueOnce({
        data: [
          chatSpec("chat-third", "Third chat", {
            updated_at: "2026-08-01T00:00:00Z",
          }),
        ],
        total: 3,
      });

    renderPage({
      project,
      chatId: "chat-running",
      initialHistory: materializedHistory("default", "running"),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");

    const pinned = await screen.findByText("Pinned design");
    const running = screen.getByText("Running build");
    expect(
      pinned.compareDocumentPosition(running) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      screen.getByRole("tab", { name: "Archived", exact: true }),
    ).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(await screen.findByText("Third chat")).toBeVisible();
    expect(projectChatsApiMock.listChats).toHaveBeenNthCalledWith(
      1,
      "project-1",
      {
        offset: 0,
        limit: 20,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
        view: "user",
      },
    );
    expect(projectChatsApiMock.listChats).toHaveBeenNthCalledWith(
      2,
      "project-1",
      {
        offset: 1,
        limit: 20,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
        view: "user",
      },
    );
  });

  it("debounces Project Chat search through the server API", async () => {
    projectChatsApiMock.listChats.mockResolvedValueOnce({
      data: [chatSpec("chat-1", "Initial"), chatSpec("chat-2", "Fix race")],
      total: 2,
    });
    projectChatsApiMock.searchChats.mockResolvedValueOnce({
      results: [
        {
          chat_id: "chat-2",
          chat_name: "Fix race",
          card_index: 0,
          role: "user",
          snippet: "fix race",
        },
      ],
    });

    renderPage({
      project,
      chatId: "chat-1",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    await screen.findByText("Initial");

    fireEvent.change(
      screen.getByRole("textbox", { name: "Search Project Chats" }),
      {
        target: { value: " fix race " },
      },
    );

    expect(await screen.findByText("Fix race")).toBeVisible();
    expect(projectChatsApiMock.searchChats).toHaveBeenLastCalledWith(
      "project-1",
      "fix race",
    );
  });

  it("renames and pins only after API success while preserving API ordering", async () => {
    let chats = [
      chatSpec("chat-a", "Alpha", { pinned: false }),
      chatSpec("chat-b", "Beta", { pinned: false }),
    ];
    projectChatsApiMock.listChats.mockImplementation(async () => ({
      data: chats,
      total: chats.length,
    }));
    projectChatsApiMock.updateChat.mockImplementation(
      async (
        _projectId: string,
        chatId: string,
        update: Record<string, unknown>,
      ) => {
        const updated = {
          ...chats.find((chat) => chat.id === chatId)!,
          ...update,
        };
        chats =
          update.pinned === true
            ? [updated, ...chats.filter((chat) => chat.id !== chatId)]
            : chats.map((chat) => (chat.id === chatId ? updated : chat));
        return updated;
      },
    );

    renderPage({
      project,
      chatId: "chat-a",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    await screen.findByText("Alpha");

    fireEvent.click(screen.getByRole("button", { name: "Rename Alpha" }));
    fireEvent.change(
      await screen.findByRole("textbox", { name: "Project Chat name" }),
      {
        target: { value: "Renamed Alpha" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByText("Renamed Alpha")).toBeVisible();
    expect(projectChatsApiMock.updateChat).toHaveBeenCalledWith(
      "project-1",
      "chat-a",
      { name: "Renamed Alpha" },
    );

    fireEvent.click(screen.getByRole("button", { name: "Pin Renamed Alpha" }));
    await waitFor(() =>
      expect(projectChatsApiMock.updateChat).toHaveBeenLastCalledWith(
        "project-1",
        "chat-a",
        expect.objectContaining({ pinned: true }),
      ),
    );
    const renamed = screen.getByText("Renamed Alpha");
    const beta = screen.getByText("Beta");
    expect(
      renamed.compareDocumentPosition(beta) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("confirms idle inactive deletion, uses force=false, and preserves the route", async () => {
    const active = chatSpec("chat-active", "Active", {
      updated_at: "2026-08-01T02:00:00Z",
    });
    const inactive = chatSpec("chat-inactive", "Inactive", {
      updated_at: "2026-08-01T01:00:00Z",
    });
    projectChatsApiMock.listChats.mockResolvedValue({
      data: [active, inactive],
      total: 2,
    });
    projectChatsApiMock.deleteChat.mockResolvedValue({
      deleted: true,
      chat_id: inactive.id,
      was_busy: false,
    });
    rememberProjectChat(project.project_id, active.id);

    renderPage({
      project,
      chatId: active.id,
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    await screen.findByText("Inactive");
    fireEvent.click(screen.getByRole("button", { name: "Delete Inactive" }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete" }));

    await waitFor(() =>
      expect(projectChatsApiMock.deleteChat).toHaveBeenCalledWith(
        "project-1",
        "chat-inactive",
        false,
      ),
    );
    expect(screen.getByTestId("location")).toHaveTextContent("/");
    expect(lastProjectChatId("project-1")).toBe("chat-active");
    await waitFor(() =>
      expect(screen.queryByText("Inactive")).not.toBeInTheDocument(),
    );
  });

  it("force-deletes a busy active Chat and opens the most recently active known sibling", async () => {
    const running = chatSpec("chat-running", "Running", {
      status: "running",
      updated_at: "2026-08-01T01:00:00Z",
    });
    const recent = chatSpec("chat-recent", "Recent", {
      updated_at: "2026-08-01T03:00:00Z",
    });
    const older = chatSpec("chat-older", "Older", {
      updated_at: "2026-08-01T02:00:00Z",
    });
    projectChatsApiMock.listChats.mockResolvedValue({
      data: [running, recent, older],
      total: 3,
    });
    projectChatsApiMock.deleteChat.mockResolvedValue({
      deleted: true,
      chat_id: running.id,
      was_busy: true,
    });
    rememberProjectChat(project.project_id, running.id);

    renderPage({
      project,
      chatId: running.id,
      initialHistory: materializedHistory("default", "running"),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    await screen.findByText("Running");
    fireEvent.click(screen.getByRole("button", { name: "Delete Running" }));
    expect(await screen.findByText(/hard-stop only this Chat/i)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Stop and Delete" }));

    await waitFor(() =>
      expect(projectChatsApiMock.deleteChat).toHaveBeenCalledWith(
        "project-1",
        "chat-running",
        true,
      ),
    );
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-1/chats/chat-recent",
      ),
    );
    expect(lastProjectChatId("project-1")).toBe("chat-recent");
  });

  it("handles final sidebar deletion in the matching Project and inherits its Agent and model with route replacement", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue(
      "chat-inherited-after-sidebar-delete",
    );
    const model = {
      provider_id: "dashscope",
      model: "qwen-plus",
      thinking_strength: "high",
    };
    renderPage(
      {
        project,
        chatId: "chat-only",
        initialHistory: { ...materializedHistory("code"), model },
      },
      "/projects/project-1/chats/chat-only",
    );
    await waitFor(() =>
      expect(surfaceProps.current?.projectExecutionAgentId).toBe("code"),
    );
    act(() => {
      expect(
        publishConversationDeleted(
          { scope: "agent", agentId: "project-1", chatId: "chat-only" },
          [],
        ),
      ).toBe(false);
      expect(
        publishConversationDeleted(
          { scope: "project", projectId: "project-2", chatId: "chat-only" },
          [],
        ),
      ).toBe(false);
      expect(
        publishConversationDeleted(
          { scope: "project", projectId: "project-1", chatId: "other-chat" },
          [],
        ),
      ).toBe(false);
    });
    expect(screen.getByTestId("location")).toHaveTextContent(
      "/projects/project-1/chats/chat-only",
    );
    drafts.save(projectChatDraftKey("project-1", "chat-only"), "deleted draft");
    act(() => {
      expect(
        publishConversationDeleted(
          { scope: "project", projectId: "project-1", chatId: "chat-only" },
          [],
        ),
      ).toBe(true);
    });
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-1/chats/chat-inherited-after-sidebar-delete",
      ),
    );
    expect(screen.getByTestId("location")).toHaveAttribute(
      "data-navigation-type",
      "REPLACE",
    );
    expect(
      projectChatExecutionAgent(
        "project-1",
        "chat-inherited-after-sidebar-delete",
      ),
    ).toBe("code");
    expect(
      loadDraftChatModel(
        "project:project-1",
        "chat-inherited-after-sidebar-delete",
      ),
    ).toEqual(model);
    expect(drafts.load(projectChatDraftKey("project-1", "chat-only"))).toBe("");
    expect(projectChatsApiMock.deleteChat).not.toHaveBeenCalled();
  });

  it("opens an unmaterialized Chat after deleting the final materialized Chat", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-lazy-after-delete");
    const only = chatSpec("chat-only", "Only chat");
    projectChatsApiMock.listChats.mockResolvedValue({
      data: [only],
      total: 1,
    });
    projectChatsApiMock.deleteChat.mockResolvedValue({
      deleted: true,
      chat_id: only.id,
      was_busy: false,
    });
    rememberProjectChat(project.project_id, only.id);

    renderPage({
      project,
      chatId: only.id,
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    await screen.findByText("Only chat");
    fireEvent.click(screen.getByRole("button", { name: "Delete Only chat" }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete" }));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-1/chats/chat-lazy-after-delete",
      ),
    );
    expect(lastProjectChatId("project-1")).toBeNull();
  });

  it("shows the deletion error while keeping the Chat row and route", async () => {
    const chat = chatSpec("chat-failure", "Keep me");
    projectChatsApiMock.listChats.mockResolvedValue({
      data: [chat],
      total: 1,
    });
    projectChatsApiMock.deleteChat.mockRejectedValue(
      new Error("Resource Device offline"),
    );

    renderPage({
      project,
      chatId: "chat-current",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    await screen.findByText("Keep me");
    fireEvent.click(screen.getByRole("button", { name: "Delete Keep me" }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete" }));

    expect(await screen.findByText("Resource Device offline")).toBeVisible();
    expect(screen.getByText("Keep me")).toBeVisible();
    expect(screen.getByTestId("location")).toHaveTextContent("/");
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Delete Keep me" }),
      ).toBeEnabled(),
    );
  });

  it("reconfirms and force-deletes when an idle Chat becomes busy", async () => {
    const chat = chatSpec("chat-race", "Race");
    projectChatsApiMock.listChats.mockResolvedValue({
      data: [chat],
      total: 1,
    });
    projectChatsApiMock.deleteChat
      .mockRejectedValueOnce(new Error("Project Chat is busy"))
      .mockResolvedValueOnce({
        deleted: true,
        chat_id: chat.id,
        was_busy: true,
      });

    renderPage({
      project,
      chatId: "chat-current",
      initialHistory: materializedHistory(),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Manage chats" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Project Chats");
    await screen.findByText("Race");
    fireEvent.click(screen.getByRole("button", { name: "Delete Race" }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete" }));
    expect(await screen.findByText("Project Chat became busy")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Stop and Delete" }));

    await waitFor(() =>
      expect(projectChatsApiMock.deleteChat).toHaveBeenNthCalledWith(
        2,
        "project-1",
        "chat-race",
        true,
      ),
    );
    await waitFor(() =>
      expect(screen.queryByText("Race")).not.toBeInTheDocument(),
    );
  });

  it("refreshes navigation after materialization and title changes without following transcript tokens", async () => {
    const listener = vi.fn();
    const unsubscribe = subscribeConversationNavigation(listener);
    try {
      renderPage({ project, chatId: "new-chat", initialHistory: null });
      const onHistoryChange = surfaceProps.current?.onHistoryChange as (
        history: unknown,
      ) => void;
      act(() => onHistoryChange({ messages: [], status: "idle" }));
      expect(listener).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "Accept" }));
      expect(listener).toHaveBeenLastCalledWith({
        scope: "project",
        projectId: "project-1",
        chatId: "new-chat",
      });
      const materialized = {
        ...materializedHistory(),
        title: "First question",
      };
      act(() => onHistoryChange(materialized));
      listener.mockClear();
      act(() =>
        onHistoryChange({
          ...materialized,
          messages: [
            ...materialized.messages,
            { role: "assistant", content: "partial answer" },
          ],
        }),
      );
      expect(listener).not.toHaveBeenCalled();
      act(() => onHistoryChange({ ...materialized, title: "Updated title" }));
      expect(listener).toHaveBeenCalledOnce();
    } finally {
      unsubscribe();
    }
  });

  it("opens independent chats for the current Project Agent without changing the ordinary preference", async () => {
    useAgentStore.setState({ selectedAgent: "agent-independent" });
    saveLastChatId("agent-independent", "other-agent-chat");
    saveLastChatId("default", "current-agent-chat");
    renderPage({
      project,
      chatId: "project-chat",
      initialHistory: materializedHistory("default"),
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Select Project" }));
    fireEvent.click(screen.getByRole("button", { name: "Standalone chats" }));
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/current-agent-chat",
      ),
    );
    expect(useAgentStore.getState().selectedAgent).toBe("agent-independent");
    expect(projectChatsApiMock.switchAgent).not.toHaveBeenCalled();
  });

  it("opens management from navigation and preserves unrelated query and hash", async () => {
    renderPage(
      {
        project,
        chatId: "project-chat",
        initialHistory: materializedHistory(),
      },
      "/projects/project-1/chats/project-chat?conversations=1&keep=yes#latest",
    );
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "Project Chats",
    );
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-1/chats/project-chat?keep=yes#latest",
      ),
    );
  });

  it("switches Project and Chat through one canonical navigation", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-1");
    createProjectChat(project.project_id);
    rememberProjectChat(secondProject.project_id, "chat-2");
    projectChatsApiMock.getChat.mockResolvedValue({
      messages: [],
      meta: {},
    });

    renderPage({
      project,
      projects: [project, secondProject],
      chatId: "chat-1",
    });
    await screen.findByText("Default Agent");

    fireEvent.click(screen.getByRole("button", { name: "Select Project" }));
    fireEvent.click(await screen.findByRole("button", { name: /codex/ }));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-2/chats/chat-2",
      ),
    );
    expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
  });

  it("validates a materialized deep link before returning loader data", async () => {
    projectChatsApiMock.getChat.mockResolvedValue({
      messages: [{ role: "user" }],
      meta: {},
    });

    const data = await loader({
      params: { projectId: "project-1", chatId: "chat-saved" },
      request: new Request(
        "http://localhost/projects/project-1/chats/chat-saved",
      ),
    } as never);

    expect(projectChatsApiMock.getChat).toHaveBeenCalledWith(
      "project-1",
      "chat-saved",
      { turnLimit: 1 },
    );
    expect(data.initialHistory?.messages).toHaveLength(1);
  });

  it("keeps a cross-Device empty address as Lazy Chat", async () => {
    setProjectLastSelectedAgent("project-1", "default");
    projectChatsApiMock.getChat.mockResolvedValue({ messages: [], meta: {} });

    const data = await loader({
      params: { projectId: "project-1", chatId: "chat-remote" },
      request: new Request(
        "http://localhost/projects/project-1/chats/chat-remote",
      ),
    } as never);
    expect(data.initialHistory).toEqual({ messages: [], meta: {} });
    expect(data.unmaterialized).toBe(true);
    expect(lastProjectChatId("project-1")).toBeNull();

    renderPage(data);
    expect(await screen.findByText("Default Agent")).toBeVisible();
    expect(surfaceProps.current?.unmaterialized).toBe(true);
  });

  it("rejects a wrong-owner deep link before mounting the conversation", async () => {
    projectChatsApiMock.getChat.mockRejectedValue(
      new Error("Project Chat belongs to Project project-2"),
    );

    await expect(
      loader({
        params: { projectId: "project-1", chatId: "chat-wrong-owner" },
        request: new Request(
          "http://localhost/projects/project-1/chats/chat-wrong-owner",
        ),
      } as never),
    ).rejects.toThrow("belongs to Project project-2");
  });
});

function LocationProbe() {
  const location = useLocation();
  const navigationType = useNavigationType();
  return (
    <div data-testid="location" data-navigation-type={navigationType}>
      {location.pathname}
      {location.search}
      {location.hash}
    </div>
  );
}

function renderPage(
  initialData: React.ComponentProps<typeof ProjectChatPage>["initialData"],
  initialPath = "/",
) {
  const resolvedInitialData = {
    ...initialData,
    unmaterialized:
      initialData.unmaterialized ??
      !(
        initialData.initialHistory &&
        historyProvesConversationMaterialized(initialData.initialHistory)
      ),
  };
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <LocationProbe />
      <ModalProvider>
        <ToastProvider>
          <ProjectChatPage initialData={resolvedInitialData} />
        </ToastProvider>
      </ModalProvider>
    </MemoryRouter>,
  );
}

function chatSpec(
  id: string,
  name: string,
  overrides: Record<string, unknown> = {},
) {
  return {
    id,
    user_id: "project",
    channel: "console",
    name,
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-01T00:00:00Z",
    status: "idle",
    pinned: false,
    ...overrides,
  };
}

function materializedHistory(
  agentId = "default",
  status: "idle" | "running" = "idle",
) {
  return {
    messages: [{ role: "user", content: "materialized" }],
    status,
    control: {
      acp: { active: false },
      queue: { entries: [], paused: false },
      project_chat_agent_state: {
        selection: {
          agent_id: agentId,
          selected_at: "2026-08-01T00:00:00Z",
          revision: 1,
        },
        selected_agent_available: true,
        switchable: true,
        blockers: [],
      },
    },
  };
}
