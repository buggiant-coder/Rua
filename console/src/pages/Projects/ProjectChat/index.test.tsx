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
import { openChatDraft, saveLastChatId } from "../../Chat/chatNavigation";
import {
  lastProjectChatId,
  projectChatDraftKey,
  projectChatExecutionAgent,
  rememberProjectChat,
} from "../projectChatNavigation";
import ProjectChatPage from "./index";
import { loader } from "../../Chat/route";
import { ModalProvider, ToastProvider } from "@/ui";
import {
  loadDraftConversationModel,
  saveDraftConversationModel,
} from "@/pages/Chat/conversationLifecycle";
import { useAgentStore } from "@/stores/agentStore";
import {
  subscribeConversationNavigation,
  publishConversationDeleted,
  requestNewConversation,
} from "@/layouts/conversationNavigationEvents";
import { getProjectNavigationAgent } from "@/layouts/conversationNavigationAgent";

const {
  historyApiMock,
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
  historyApiMock: {
    getChat: vi.fn(),
    getAgentState: vi.fn(),
    switchAgent: vi.fn(),
  },
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

vi.mock("@/api/modules/chat", () => ({ chatApi: historyApiMock }));
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
                chat_agent_state: {
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
                chat_agent_state: {
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
                chat_agent_state: {
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
  resources: {
    revision: 1,
    directories: [
      {
        directory_id: "main",
        device_id: "resource-device",
        resource_root: "/repos/rua-rust",
      },
    ],
  },
  lifecycle: "active" as const,
  created_at: "2026-08-01T00:00:00Z",
  updated_at: "2026-08-01T00:00:00Z",
};

const secondProject = {
  ...project,
  project_id: "project-2",
  display_name: "codex",
  resources: {
    revision: 1,
    directories: [
      {
        directory_id: "codex",
        device_id: "other-device",
        resource_root: "/repos/codex",
      },
    ],
  },
};

describe("ProjectChat loader", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    historyApiMock.getAgentState.mockResolvedValue(
      materializedHistory().control.chat_agent_state,
    );
    sessionStorage.clear();
    projectsApiMock.get.mockResolvedValue(project);
    projectsApiMock.list.mockResolvedValue([project]);
  });

  it("uses a saved empty Chat’s header to resolve its Agent and Project", async () => {
    const emptyHistory = {
      agent_id: "code",
      project_id: "project-1",
      messages: [],
      meta: {},
    };
    historyApiMock.getChat.mockResolvedValue(emptyHistory);

    await expect(
      loader({
        params: { chatId: "chat-after-restart" },
        request: new Request("http://localhost/chats/chat-after-restart"),
        context: {},
      }),
    ).resolves.toMatchObject({
      project,
      chatId: "chat-after-restart",
      history: emptyHistory,
      agentId: "code",
      unmaterialized: false,
    });
    expect(historyApiMock.getChat).toHaveBeenCalledWith("chat-after-restart", {
      turnLimit: 20,
    });
  });

  it("keeps a local draft lazy without loading remote history", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-local-draft");
    openChatDraft("project-1");

    await expect(
      loader({
        params: { chatId: "chat-local-draft" },
        request: new Request("http://localhost/chats/chat-local-draft"),
        context: {},
      }),
    ).resolves.toMatchObject({
      project,
      chatId: "chat-local-draft",
      history: null,
      unmaterialized: true,
    });
    expect(historyApiMock.getChat).not.toHaveBeenCalled();
  });
});

describe("ProjectChatPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
    historyApiMock.getAgentState.mockResolvedValue(
      materializedHistory().control.chat_agent_state,
    );
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

  it("keeps the draft execution location independent of Project resources", async () => {
    renderPage({
      project: {
        ...project,
        resources: {
          revision: 1,
          directories: [
            {
              directory_id: "main",
              device_id: "resource-device",
              resource_root: "/repo/main",
            },
            {
              directory_id: "docs",
              device_id: "phone",
              resource_root: "/repo/docs",
            },
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
    openChatDraft(project.project_id);

    const { container } = renderPage({
      project,
      chatId: "chat-1",
      initialHistory: null,
    });

    expect(await screen.findByText("Default Agent")).toBeVisible();
    expect(screen.getByText("rua-rust")).toBeVisible();
    expect(screen.queryByTitle("Resource · Connected")).not.toBeInTheDocument();
    expect(surfaceProps.current?.executionLocation).toMatchObject({
      devices: expect.arrayContaining([
        expect.objectContaining({ is_self: true }),
      ]),
      archived: false,
    });
    expect(surfaceProps.current?.draftExecutionPosition).toEqual({
      device_id: "controller",
    });
    expect(surfaceProps.current?.address).toEqual({ chatId: "chat-1" });
    expect(surfaceProps.current?.executionAgentId).toBe("default");
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
      target: { projectId: "project-1" },
      chatId: "chat-1",
      targetDeviceId: "controller",
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
      target: { projectId: "project-1" },
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
      target: { projectId: "project-1" },
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
          expect(requestNewConversation({ chatId: "chat-existing" })).toBe(
            true,
          );
        });
      }
      await waitFor(() =>
        expect(screen.getByTestId("location")).toHaveTextContent(
          "/chats/chat-new-from-header",
        ),
      );
      expect(
        loadDraftConversationModel("project:project-1", "chat-new-from-header"),
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
          "/chats/chat-new-from-header",
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
    openChatDraft("project-1");
    saveDraftConversationModel("project:project-1", "chat-model-handoff", {
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
    openChatDraft(project.project_id);
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
    expect(surfaceProps.current?.address).toEqual({ chatId: "chat-3" });
    expect(surfaceProps.current?.executionAgentId).toBe("default");
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
    historyApiMock.switchAgent.mockReturnValue(
      new Promise((resolve) => {
        resolveSwitch = resolve;
      }),
    );

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

    fireEvent.click(screen.getByRole("button", { name: "Chat Agent" }));
    expect(historyApiMock.switchAgent).not.toHaveBeenCalled();
    const codeAgent = screen.getByRole("button", { name: /Code Agent/ });
    await waitFor(() => expect(codeAgent).toBeEnabled());
    fireEvent.click(codeAgent);
    fireEvent.click(
      await screen.findByRole("button", { name: "Switch Agent" }),
    );

    await waitFor(() =>
      expect(historyApiMock.switchAgent).toHaveBeenCalledWith(
        "chat-materialized",
        "default",
        "code",
      ),
    );
    expect(
      screen.getByRole("button", { name: "Chat Agent" }),
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
        screen.getByRole("button", { name: "Chat Agent" }),
      ).toHaveTextContent("Code Agent"),
    );
    expect(getProjectNavigationAgent()).toEqual({
      projectId: "project-1",
      chatId: "chat-materialized",
      agentId: "code",
    });
    expect(useAgentStore.getState().selectedAgent).toBe("code");
    page.unmount();
    expect(getProjectNavigationAgent()).toBeNull();
  });

  it("adopts the accepted header Agent after a new selection on the same draft", async () => {
    useAgentStore.getState().setSelectedAgent("code");
    const chatId = openChatDraft(project.project_id);
    renderPage({ project, chatId, initialHistory: null });
    await waitFor(() =>
      expect(surfaceProps.current?.executionAgentId).toBe("code"),
    );
    const observeHistory = surfaceProps.current?.onHistoryChange as (
      history: unknown,
    ) => void;
    act(() =>
      observeHistory({ agent_id: "default", messages: [], status: "idle" }),
    );
    expect(surfaceProps.current?.executionAgentId).toBe("default");
    expect(surfaceProps.current?.unmaterialized).toBe(false);
    expect(useAgentStore.getState().selectedAgent).toBe("code");
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

    const refreshLists = vi.fn();
    const unsubscribe = subscribeConversationNavigation(refreshLists);
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
          chat_agent_state: {
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
        screen.getByRole("button", { name: "Chat Agent" }),
      ).toHaveTextContent("Code Agent"),
    );
    expect(surfaceProps.current?.executionAgentId).toBe("code");
    expect(refreshLists).toHaveBeenCalledWith(
      { chatId: "chat-live-selection" },
      false,
      "project-1",
    );
    refreshLists.mockClear();

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
          chat_agent_state: {
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
      screen.getByRole("button", { name: "Chat Agent" }),
    ).toHaveTextContent("Code Agent");
    expect(surfaceProps.current?.executionAgentId).toBe("code");
    expect(refreshLists).not.toHaveBeenCalled();
    unsubscribe();
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
        screen.getByRole("button", { name: "Chat Agent" }),
      ).toHaveTextContent("Unavailable Agent"),
    );
    expect(screen.queryByText("deleted-agent")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toHaveAttribute(
      "title",
      "Select an available Agent before submitting",
    );
    expect(surfaceProps.current?.address).toEqual({
      chatId: "chat-unavailable-agent",
    });
    expect(surfaceProps.current?.executionAgentId).toBe("deleted-agent");
  });

  it("applies an explicit Project draft selection to the global Agent preference", async () => {
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
    useAgentStore.getState().setSelectedAgent("default");
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-independent");
    openChatDraft(project.project_id);

    renderPage({
      project,
      chatId: "chat-independent",
      initialHistory: null,
    });
    await screen.findByText("Default Agent");
    fireEvent.click(screen.getByRole("button", { name: "Chat Agent" }));
    const codeAgent = screen.getByRole("button", { name: /Code Agent/ });
    await waitFor(() => expect(codeAgent).toBeEnabled());
    fireEvent.click(codeAgent);

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Chat Agent" }),
      ).toHaveTextContent("Code Agent"),
    );
    expect(useAgentStore.getState().selectedAgent).toBe("code");
    expect(historyApiMock.switchAgent).not.toHaveBeenCalled();
  });

  it("preserves an unavailable global Agent choice and blocks submission until a valid selection", async () => {
    useAgentStore.getState().setSelectedAgent("missing-agent");
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-invalid-hint");
    openChatDraft(project.project_id);

    renderPage({
      project,
      chatId: "chat-invalid-hint",
      initialHistory: null,
    });

    await waitFor(() =>
      expect(surfaceProps.current?.submitDisabled).toBe(true),
    );
    expect(surfaceProps.current?.address).toEqual({
      chatId: "chat-invalid-hint",
    });
    expect(surfaceProps.current?.executionAgentId).toBe("missing-agent");
  });

  it("keeps right drawers mutually exclusive without replacing the conversation", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-drawers");
    openChatDraft(project.project_id);
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

  it.each([
    ["resource-write", "Resource write"],
    [null, "Uses Agent permissions"],
  ])(
    "shows the effective permission summary and approval counts (%s)",
    async (profile, expected) => {
      if (profile === null)
        projectChatsApiMock.getResourcePolicy.mockResolvedValue({
          policy: null,
          pending_approval_count: 1,
          chat_rule_count: 2,
          agent_rule_count: 3,
        });
      renderPage({
        project,
        chatId: "chat-resource-policy",
        initialHistory: materializedHistory(),
      });
      await screen.findByText("Default Agent");

      fireEvent.click(screen.getByRole("button", { name: "Workspace" }));

      expect(await screen.findByText("AGENTS.md")).toBeVisible();
      fireEvent.click(screen.getByRole("tab", { name: "Context" }));
      expect(await screen.findByText(expected!)).toBeVisible();
      expect(projectChatsApiMock.getResourcePolicy).toHaveBeenCalledWith(
        "project-1",
        "chat-resource-policy",
      );
      expect(
        screen.getByText("/repos/rua-rust", { selector: "code" }),
      ).toBeVisible();
      expect(screen.getByText("1 pending · 2 Chat · 3 Agent")).toBeVisible();
    },
  );

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

  it("exports Project Chat history by Chat ID", async () => {
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
      expect(exportConversationMock).toHaveBeenCalledWith("chat-export"),
    );
  });

  it("opens the shared Task activity drawer for the Project Chat", async () => {
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
        offset: 2,
        limit: 20,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
        view: "user",
      },
    );
  });

  it("pages matching messages in the Project drawer without losing earlier hits", async () => {
    projectChatsApiMock.listChats.mockResolvedValueOnce({
      data: [chatSpec("chat-1", "Initial"), chatSpec("chat-2", "Fix race")],
      total: 2,
    });
    projectChatsApiMock.searchChats
      .mockResolvedValueOnce({
        total: 2,
        offset: 0,
        limit: 1,
        next_offset: 1,
        results: [
          {
            chat_id: "chat-2",
            chat_name: "Fix race",
            card_index: 0,
            role: "user",
            snippet: "fix race",
          },
        ],
      })
      .mockResolvedValueOnce({
        total: 2,
        offset: 1,
        limit: 1,
        next_offset: null,
        results: [
          {
            chat_id: "chat-2",
            chat_name: "Fix race",
            message_id: "later",
            card_index: 4,
            role: "assistant",
            snippet: "A later reply",
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
      0,
    );
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(await screen.findByText("A later reply")).toBeVisible();
    expect(screen.getAllByText("Fix race")).toHaveLength(2);
    expect(projectChatsApiMock.searchChats).toHaveBeenLastCalledWith(
      "project-1",
      "fix race",
      1,
    );
    expect(
      screen.queryByRole("button", { name: "Load more" }),
    ).not.toBeInTheDocument();
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
        "/chats/chat-recent",
      ),
    );
    expect(lastProjectChatId("project-1")).toBe("chat-recent");
  });

  it("uses the global Agent and inherits the source model after final sidebar deletion", async () => {
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
      "/chats/chat-only",
    );
    await waitFor(() =>
      expect(surfaceProps.current?.executionAgentId).toBe("code"),
    );
    act(() => {
      expect(publishConversationDeleted({ chatId: "different-chat" }, [])).toBe(
        false,
      );
      expect(publishConversationDeleted({ chatId: "another-chat" }, [])).toBe(
        false,
      );
      expect(publishConversationDeleted({ chatId: "other-chat" }, [])).toBe(
        false,
      );
    });
    expect(screen.getByTestId("location")).toHaveTextContent(
      "/chats/chat-only",
    );
    drafts.save(projectChatDraftKey("project-1", "chat-only"), "deleted draft");
    act(() => {
      expect(publishConversationDeleted({ chatId: "chat-only" }, [])).toBe(
        true,
      );
    });
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/chats/chat-inherited-after-sidebar-delete",
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
    ).toBe("default");
    expect(
      loadDraftConversationModel(
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
        "/chats/chat-lazy-after-delete",
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
      expect(listener).toHaveBeenLastCalledWith(
        { chatId: "new-chat" },
        false,
        "project-1",
      );
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

  it("opens the most recent standalone Chat across Agents while preserving the Project Chat", async () => {
    useAgentStore.setState({ selectedAgent: "agent-independent" });
    saveLastChatId("other-agent-chat");
    saveLastChatId("current-agent-chat");
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
        "/chats/current-agent-chat",
      ),
    );
    expect(useAgentStore.getState().selectedAgent).toBe("agent-independent");
    expect(historyApiMock.switchAgent).not.toHaveBeenCalled();
  });

  it("opens management from navigation and preserves unrelated query and hash", async () => {
    renderPage(
      {
        project,
        chatId: "project-chat",
        initialHistory: materializedHistory(),
      },
      "/chats/project-chat?conversations=1&keep=yes#latest",
    );
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "Project Chats",
    );
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/chats/project-chat?keep=yes#latest",
      ),
    );
  });

  it("switches Project and Chat through one canonical navigation", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-1");
    openChatDraft(project.project_id);
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
      expect(screen.getByTestId("location")).toHaveTextContent("/chats/chat-2"),
    );
    expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
  });

  it("resolves a saved deep link from its Chat ID and persisted header", async () => {
    historyApiMock.getChat.mockResolvedValue({
      agent_id: "code",
      project_id: "project-1",
      messages: [{ id: "m1", role: "user" }],
    });
    const data = await loader({ params: { chatId: "chat-saved" } } as never);
    expect(historyApiMock.getChat).toHaveBeenCalledWith("chat-saved", {
      turnLimit: 20,
    });
    expect(data.agentId).toBe("code");
    expect(data.history?.messages).toHaveLength(1);
    expect(data.project?.project_id).toBe("project-1");
  });

  it("resolves a saved independent Chat without looking up a Project", async () => {
    historyApiMock.getChat.mockResolvedValue({
      agent_id: "author",
      project_id: null,
      messages: [],
    });
    const data = await loader({ params: { chatId: "independent" } } as never);
    expect(data.unmaterialized).toBe(false);
    expect(data.agentId).toBe("author");
    expect(projectsApiMock.get).not.toHaveBeenCalled();
  });

  it("surfaces an unknown deep link without inventing a draft", async () => {
    historyApiMock.getChat.mockRejectedValue(new Error("Chat not found"));
    await expect(
      loader({ params: { chatId: "missing" } } as never),
    ).rejects.toThrow("Chat not found");
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
      chat_agent_state: {
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
