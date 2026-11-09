import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MemoryRouter,
  Route,
  Routes,
  useLocation,
  useNavigate,
  useNavigationType,
} from "react-router-dom";
import { ModalProvider, ToastProvider } from "@/ui";
import i18n from "@/i18n";
import { useAgentStore } from "@/stores/agentStore";
import { __resetMobileDetectionForTests } from "@/utils/isMobile";
import ChatPage from "../../index";
import { conversationApi } from "@/api/modules/conversation";
import { rememberProjectChat } from "@/pages/Projects/projectChatNavigation";
import {
  subscribeConversationNavigation,
  invalidateConversationNavigation,
  publishConversationMetadata,
  beginConversationMetadataUpdate,
  publishConversationDeleted,
  requestNewConversation,
} from "@/layouts/conversationNavigationEvents";

const chatApiMock = vi.hoisted(() => ({
  listChats: vi.fn(),
  updateChat: vi.fn(),
  setAuthorityPlacement: vi.fn(),
  materialize: vi.fn(),
  deleteChat: vi.fn(),
  getChat: vi.fn(),
  searchChats: vi.fn(),
  getRunningConfig: vi.fn(),
  updateRunningConfig: vi.fn(),
}));

const consoleFetchMock = vi.hoisted(() => vi.fn());

vi.mock("@/api/consoleTransport", () => ({
  consoleFetch: consoleFetchMock,
}));

const projectsApiMock = vi.hoisted(() => ({ list: vi.fn() }));

vi.mock("@/api/modules/projects", () => ({ projectsApi: projectsApiMock }));

const devicesApiMock = vi.hoisted(() => ({
  listDevices: vi.fn(),
}));

const syncApiMock = vi.hoisted(() => ({
  getStatus: vi.fn(),
}));

const saveTextFileMock = vi.hoisted(() => vi.fn());
const ownedSurfaceRenderMock = vi.hoisted(() => vi.fn());
const ownedSurfaceHistoryChangeMock = vi.hoisted(() => vi.fn());
const ownedSurfaceHistoryRegistrationMock = vi.hoisted(() => vi.fn());

const agentsApiMock = vi.hoisted(() => ({
  listAgents: vi.fn(),
  getAgentChatRunningDefaults: vi.fn(),
  getAgentRunningConfig: vi.fn(),
  updateAgentRunningConfig: vi.fn(),
}));

const acpApiMock = vi.hoisted(() => ({
  listRunners: vi.fn(),
  listChatRunners: vi.fn(),
  chatStatus: vi.fn(),
  exitChat: vi.fn(),
  enterChat: vi.fn(),
  listChatSessions: vi.fn(),
}));

const taskApiMock = vi.hoisted(() => ({
  listTasks: vi.fn(),
  getTask: vi.fn(),
  cancelTask: vi.fn(),
  retryTask: vi.fn(),
  getFullResult: vi.fn(),
  activityPath: vi.fn(
    (target: { agentId: string }, chatId: string) =>
      `/agents/${target.agentId}/chats/${chatId}/tasks/activity`,
  ),
}));

vi.mock("@/api/modules/chat", () => ({
  chatApi: chatApiMock,
}));

vi.mock("@/api/modules/sync", () => ({
  devicesApi: devicesApiMock,
  syncApi: syncApiMock,
}));

vi.mock("@/utils/saveFile", () => ({
  getDownloadFolderHint: () => "",
  saveTextFile: saveTextFileMock,
}));

vi.mock("@/api/modules/agents", () => ({
  agentsApi: agentsApiMock,
}));

vi.mock("@/api/modules/acp", () => ({
  acpApi: acpApiMock,
}));

vi.mock("@/api/modules/task", () => ({
  taskApi: taskApiMock,
}));

vi.mock("../../ModelSelector", () => ({
  default: ({
    chatId,
    chatModel,
    draftSelection,
    onChatModelChange,
  }: {
    chatId?: string;
    chatModel?: { model?: string } | null;
    draftSelection?: boolean;
    onChatModelChange?: (model: { provider_id: string; model: string }) => void;
  }) => (
    <button
      type="button"
      data-testid="model-selector"
      data-draft-selection={draftSelection ? "true" : "false"}
      onClick={() =>
        onChatModelChange?.({
          provider_id: "provider-draft",
          model: "model-draft",
        })
      }
    >
      {chatId || (draftSelection ? "draft" : "agent")}
      {chatModel?.model ? `:${chatModel.model}` : ""}
    </button>
  ),
}));

vi.mock("../OwnedConversationSurface", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  function OwnedConversationSurfaceMock({
    address,
    onHistoryChange,
    authorityPlacementDeviceId,
    currentDeviceId,
    activeAcpRunner,
    composerDeviceChip,
    composerLeftTools,
    onSubmitAccepted,
  }: {
    address: { scope: "agent"; agentId: string; chatId: string };
    onHistoryChange?: (history: unknown) => void;
    authorityPlacementDeviceId?: string | null;
    currentDeviceId?: string | null;
    activeAcpRunner?: string | null;
    composerDeviceChip?: React.ReactNode;
    composerLeftTools?: React.ReactNode;
    onSubmitAccepted?: () => void;
  }) {
    const { chatId: conversationId, agentId } = address;
    ownedSurfaceHistoryChangeMock(onHistoryChange);
    ownedSurfaceHistoryRegistrationMock(conversationId, onHistoryChange);
    React.useEffect(() => {
      let cancelled = false;
      if (!conversationId) {
        onHistoryChange?.({ status: "idle", messages: [] });
        return;
      }
      void chatApiMock
        .getChat(conversationId, agentId ?? "default")
        .then((history) => {
          if (!cancelled) onHistoryChange?.(history);
        });
      return () => {
        cancelled = true;
      };
    }, [agentId, conversationId, onHistoryChange]);

    ownedSurfaceRenderMock(
      conversationId,
      authorityPlacementDeviceId ?? null,
      currentDeviceId ?? null,
    );
    return (
      <div data-testid="owned-surface">
        {conversationId || "none"}
        <span
          data-testid="active-acp-runner"
          data-runner={activeAcpRunner || ""}
        />
        {composerLeftTools}
        {composerDeviceChip}
        <button type="button" onClick={onSubmitAccepted}>
          accept submission
        </button>
      </div>
    );
  }
  return {
    default: OwnedConversationSurfaceMock,
  };
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

function NavigationProbe() {
  const navigate = useNavigate();
  const location = useLocation();
  const chatId = location.pathname.split("/").at(-1) || "chat-1";
  return (
    <>
      <button type="button" onClick={() => navigate("/skills")}>
        go skills
      </button>
      <button
        type="button"
        onClick={() => navigate("/agents/default/chats/return-chat")}
      >
        go default chat
      </button>
      <button
        type="button"
        onClick={() => navigate(`/agents/agent-b/chats/${chatId}`)}
      >
        go agent b chat
      </button>
    </>
  );
}

function label(key: string, options?: Record<string, unknown>) {
  return i18n.t(key, options);
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function runningConfig() {
  return {
    max_iters: 100,
    auto_continue_on_text_only: false,
    llm_retry_enabled: true,
    llm_max_retries: 3,
    llm_backoff_base: 1,
    llm_backoff_cap: 30,
    llm_max_concurrent: 5,
    llm_max_qpm: 0,
    llm_rate_limit_pause: 30,
    llm_rate_limit_jitter: 0,
    llm_acquire_timeout: 120,
    max_input_length: 131_072,
    history_max_length: 10_000,
    context_compact: {
      token_count_model: "default",
      token_count_use_mirror: false,
      token_count_estimate_divisor: 4,
      context_compact_enabled: true,
      blocking_compact_ratio: 0.95,
      history_replacement_enabled: false,
      early_background_compaction_enabled: false,
      early_compaction_target_ratio: 0.5,
      context_reserve_ratio: 0.1,
      compact_with_thinking_block: true,
      compact_enhanced_prompt: true,
      thinking_turns_to_keep: 1,
      min_keep_messages: 5,
    },
    tool_result_compact: {
      enabled: true,
      projection_truncation_enabled: false,
      recent_n: 2,
      old_max_bytes: 4000,
      execution_output_max_tokens: 10_000,
      retention_days: 5,
    },
    memory_summary: {
      memory_summary_enabled: true,
      dream_cron: "0 23 * * *",
      search_mode: "precise",
    },
    embedding_config: {
      backend: "",
      api_key: "",
      base_url: "",
      model_name: "",
      dimensions: 1024,
      enable_cache: true,
      use_dimensions: true,
      max_cache_size: 1000,
      max_input_length: 8192,
      max_batch_size: 16,
    },
    memory_manager_backend: "remelight",
  };
}

function renderChat(initialPath: string) {
  return render(
    <ToastProvider>
      <ModalProvider>
        <MemoryRouter initialEntries={[initialPath]}>
          <NavigationProbe />
          <Routes>
            <Route
              path="/agents/:agentId/chats/:chatId"
              element={
                <>
                  <LocationProbe />
                  <ChatPage />
                </>
              }
            />
            <Route
              path="/projects/:projectId/chats/:chatId"
              element={<LocationProbe />}
            />
            <Route path="/skills" element={<div>skills page</div>} />
          </Routes>
        </MemoryRouter>
      </ModalProvider>
    </ToastProvider>,
  );
}

function controlledJsonLineResponse() {
  const encoder = new TextEncoder();
  let controller: ReadableStreamDefaultController<Uint8Array> | null = null;
  const body = new ReadableStream<Uint8Array>({
    start(nextController) {
      controller = nextController;
    },
  });
  return {
    response: Promise.resolve(new Response(body, { status: 200 })),
    send(payload: unknown) {
      if (!controller) throw new Error("activity stream is not ready");
      controller.enqueue(encoder.encode(`${JSON.stringify(payload)}\n`));
    },
    close() {
      controller?.close();
    },
  };
}

describe("owned ChatPage route identity", () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    consoleFetchMock
      .mockReset()
      .mockRejectedValue(new Error("No activity fixture"));
    vi.spyOn(window, "innerWidth", "get").mockReturnValue(600);
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
      },
    );
    vi.spyOn(window, "confirm").mockReturnValue(true);
    vi.spyOn(window, "prompt").mockReturnValue(null);
    localStorage.clear();
    sessionStorage.clear();
    await i18n.changeLanguage("zh");
    document.documentElement.classList.remove("mobile-mode");
    __resetMobileDetectionForTests();
    useAgentStore.setState({
      selectedAgent: "default",
      agents: [
        {
          id: "default",
          name: "小Rua",
          description: "",
          workspace_dir: "",
          enabled: true,
        },
      ],
    });
    projectsApiMock.list.mockResolvedValue([]);
    chatApiMock.listChats.mockResolvedValue({ data: [], total: 0 });
    agentsApiMock.listAgents.mockResolvedValue({
      agents: [
        {
          id: "default",
          name: "小Rua",
          description: "",
          workspace_dir: "",
          enabled: true,
        },
      ],
    });
    agentsApiMock.getAgentChatRunningDefaults.mockResolvedValue(
      runningConfig(),
    );
    agentsApiMock.getAgentRunningConfig.mockResolvedValue({
      show_debug_menu: false,
    });
    chatApiMock.getRunningConfig.mockImplementation(async () =>
      runningConfig(),
    );
    chatApiMock.updateRunningConfig.mockImplementation(
      async (_chatId: string, config: unknown) => config,
    );
    chatApiMock.updateChat.mockImplementation((_id, patch) =>
      Promise.resolve({
        id: _id,
        user_id: "default",
        channel: "console",
        created_at: null,
        updated_at: null,
        ...patch,
      }),
    );
    chatApiMock.setAuthorityPlacement.mockImplementation(
      (chatId: string, deviceId: string, _agentId: string, model?: unknown) =>
        Promise.resolve({
          id: chatId,
          user_id: "default",
          channel: "console",
          created_at: null,
          updated_at: null,
          meta: { authority_placement: { device_id: deviceId } },
          model: model ?? null,
        }),
    );
    chatApiMock.deleteChat.mockResolvedValue({ success: true, chat_id: "x" });
    chatApiMock.getChat.mockResolvedValue({ status: "idle", messages: [] });
    chatApiMock.searchChats.mockResolvedValue({ results: [] });
    taskApiMock.listTasks.mockResolvedValue([]);
    acpApiMock.chatStatus.mockImplementation(
      async (
        _agentId: string,
        chatId: string,
        targetDeviceId?: string | null,
      ) => ({
        active: false,
        runner: null,
        cwd: null,
        chat_id: chatId,
        workspace_dir: "/workspace",
        target_device_id: targetDeviceId ?? null,
      }),
    );
    acpApiMock.exitChat.mockResolvedValue({ ok: true, message: "Exited ACP" });
    acpApiMock.listRunners.mockResolvedValue([]);
    acpApiMock.listChatRunners.mockResolvedValue([]);
    acpApiMock.enterChat.mockResolvedValue({
      ok: true,
      message: "Entered ACP",
      status: {
        active: true,
        runner: "codex",
        cwd: "/workspace",
        chat_id: "chat-1",
        workspace_dir: "/workspace",
        target_device_id: null,
      },
    });
    acpApiMock.listChatSessions.mockResolvedValue([]);
    devicesApiMock.listDevices.mockResolvedValue([
      {
        device_id: "device-current",
        public_key: "pk-current",
        friendly_name: "Current device",
        label: "Current device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: true,
      },
      {
        device_id: "device-remote",
        public_key: "pk-remote",
        friendly_name: "Remote device",
        label: "Remote device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: false,
      },
    ]);
    syncApiMock.getStatus.mockResolvedValue({
      enabled: true,
      paired: true,
      peer_count: 1,
    });
    saveTextFileMock.mockResolvedValue({ ok: true, usedPicker: false });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("keeps desktop conversation management closed until opened as an overlay", async () => {
    vi.spyOn(window, "innerWidth", "get").mockReturnValue(1280);
    renderChat("/agents/default/chats/chat-1");
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(chatApiMock.listChats).not.toHaveBeenCalled();
    const conversation = screen.getByTestId("owned-surface");
    fireEvent.click(
      screen.getByRole("button", {
        name: label("navigation.manageConversations"),
      }),
    );
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      label("navigation.manageConversations"),
    );
    expect(screen.queryByRole("separator")).not.toBeInTheDocument();
    expect(screen.getByTestId("owned-surface")).toBe(conversation);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("opens conversation management from navigation and consumes only its query parameter", async () => {
    renderChat("/agents/default/chats/chat-1?conversations=1&keep=yes#latest");
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      label("navigation.manageConversations"),
    );
    expect(screen.getByTestId("location")).toHaveTextContent(
      "/agents/default/chats/chat-1?keep=yes#latest",
    );
  });

  it("shows the independent scope in the shared project selector", async () => {
    renderChat("/agents/default/chats/chat-1");
    expect(
      screen.getByRole("button", { name: label("projects.selectProject") }),
    ).toHaveTextContent(label("navigation.independentChats"));
    await waitFor(() =>
      expect(projectsApiMock.list).toHaveBeenCalledWith("active"),
    );
  });

  it("refreshes selector projects for catalog changes without reloading them on chat changes", async () => {
    renderChat("/agents/default/chats/chat-1");
    await waitFor(() => expect(projectsApiMock.list).toHaveBeenCalledOnce());
    act(() =>
      invalidateConversationNavigation({
        scope: "agent",
        agentId: "default",
        chatId: "chat-1",
      }),
    );
    await act(async () => {});
    expect(projectsApiMock.list).toHaveBeenCalledOnce();
    projectsApiMock.list.mockResolvedValue([
      {
        project_id: "new-project",
        display_name: "New catalog entry",
        resource_device_id: "device-current",
        display_path: "/projects/new",
        lifecycle: "active",
        created_at: "2026-10-01T00:00:00Z",
        updated_at: "2026-10-01T00:00:00Z",
      },
    ]);
    act(() => invalidateConversationNavigation());
    await waitFor(() => expect(projectsApiMock.list).toHaveBeenCalledTimes(2));
    fireEvent.click(
      screen.getByRole("button", { name: label("projects.selectProject") }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /New catalog entry/ }),
      ).toBeVisible(),
    );
  });

  it("switches from the independent scope to a project's remembered conversation", async () => {
    const project = {
      project_id: "project-a",
      display_name: "Navigation Project",
      resource_device_id: "device-current",
      display_path: "/projects/a",
      lifecycle: "active",
      created_at: "2026-10-01T00:00:00Z",
      updated_at: "2026-10-01T00:00:00Z",
    };
    projectsApiMock.list.mockResolvedValue([project]);
    rememberProjectChat("project-a", "project-chat");
    renderChat("/agents/default/chats/chat-1");
    fireEvent.click(
      screen.getByRole("button", { name: label("projects.selectProject") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /Navigation Project/ }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-a/chats/project-chat",
      ),
    );
    expect(chatApiMock.updateChat).not.toHaveBeenCalled();
  });

  it("applies sidebar titles only to the full current scope and keeps them during stale live updates", async () => {
    const history = {
      messages: [{ role: "user", content: "Hello" }],
      title: "Original title",
      status: "running",
    };
    chatApiMock.getChat.mockResolvedValue(history);
    renderChat("/agents/default/chats/chat-1");
    await screen.findByText("Original title");
    const onHistoryChange = ownedSurfaceHistoryChangeMock.mock.calls.at(
      -1,
    )?.[0] as (history: unknown) => void;
    const updated = {
      id: "chat-1",
      name: "Sidebar title",
      user_id: "default",
      channel: "console",
      created_at: null,
      updated_at: null,
    };
    const listener = vi.fn();
    const unsubscribe = subscribeConversationNavigation(listener);
    try {
      act(() =>
        publishConversationMetadata(
          { scope: "project", projectId: "project-a", chatId: "chat-1" },
          updated,
        ),
      );
      expect(screen.getByText("Original title")).toBeVisible();
      act(() =>
        publishConversationMetadata(
          { scope: "agent", agentId: "another-agent", chatId: "chat-1" },
          updated,
        ),
      );
      expect(screen.getByText("Original title")).toBeVisible();
      listener.mockClear();
      const historyReadCount = chatApiMock.getChat.mock.calls.length;
      act(() =>
        publishConversationMetadata(
          { scope: "agent", agentId: "default", chatId: "chat-1" },
          updated,
        ),
      );
      expect(screen.getByText("Sidebar title")).toBeVisible();
      expect(listener).toHaveBeenCalledOnce();
      expect(chatApiMock.getChat).toHaveBeenCalledTimes(historyReadCount);
      act(() =>
        onHistoryChange({
          ...history,
          messages: [
            ...history.messages,
            { role: "assistant", content: "partial answer" },
          ],
        }),
      );
      expect(screen.getByText("Sidebar title")).toBeVisible();
      act(() =>
        publishConversationMetadata(
          { scope: "agent", agentId: "default", chatId: "chat-1" },
          { ...updated, name: "Second sidebar title" },
        ),
      );
      act(() => onHistoryChange({ ...history, title: "Sidebar title" }));
      expect(screen.getByText("Second sidebar title")).toBeVisible();
      act(() => onHistoryChange(history));
      expect(screen.getByText("Second sidebar title")).toBeVisible();
      act(() =>
        onHistoryChange({ ...history, title: "Later authoritative title" }),
      );
      expect(screen.getByText("Later authoritative title")).toBeVisible();
      for (const updates of [{ pinned: true }, { tag: "red" as const }]) {
        let mutation!: ReturnType<typeof beginConversationMetadataUpdate>;
        act(() => {
          mutation = beginConversationMetadataUpdate(
            { scope: "agent", agentId: "default", chatId: "chat-1" },
            updated,
            updates,
          );
        });
        expect(screen.getByText("Later authoritative title")).toBeVisible();
        act(() => mutation.commit({ ...updated, ...updates }));
        expect(screen.getByText("Later authoritative title")).toBeVisible();
      }
    } finally {
      unsubscribe();
    }
  });

  it("refreshes navigation only for materialized history metadata changes", async () => {
    const listener = vi.fn();
    const unsubscribe = subscribeConversationNavigation(listener);
    try {
      renderChat("/agents/default/chats/chat-1");
      await act(async () => {});
      const onHistoryChange = ownedSurfaceHistoryChangeMock.mock.calls.at(
        -1,
      )?.[0] as (history: unknown) => void;
      expect(listener).not.toHaveBeenCalled();
      const history = {
        messages: [{ role: "user", content: "Hello" }],
        title: "First question",
        status: "running",
      };
      act(() => onHistoryChange(history));
      expect(listener).toHaveBeenLastCalledWith({
        scope: "agent",
        agentId: "default",
        chatId: "chat-1",
      });
      listener.mockClear();
      act(() =>
        onHistoryChange({
          ...history,
          messages: [
            ...history.messages,
            { role: "assistant", content: "partial answer" },
          ],
        }),
      );
      expect(listener).not.toHaveBeenCalled();
      act(() => onHistoryChange({ ...history, title: "Updated title" }));
      expect(listener).toHaveBeenCalledOnce();
    } finally {
      unsubscribe();
    }
  });

  it("passes /agents/default/chats/:id directly to the owned conversation surface", () => {
    renderChat("/agents/default/chats/chat-1");

    expect(screen.getByTestId("owned-surface")).toHaveTextContent("chat-1");
    expect(screen.getByTestId("model-selector")).toHaveTextContent("chat-1");
  });

  it("opens Chat-scoped Task activity without synthesizing transcript cards", async () => {
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByRole("button", { name: label("chat.tasks.title") }),
    );

    const drawer = await screen.findByRole("dialog");
    expect(within(drawer).getByText(label("chat.tasks.title"))).toBeVisible();
    await waitFor(() =>
      expect(taskApiMock.listTasks).toHaveBeenCalledWith(
        { scope: "agent", agentId: "default" },
        "chat-1",
        {
          limit: 200,
        },
      ),
    );
    expect(document.querySelector("[data-rua-task-card]")).toBeNull();
  });

  it("shows the local authority for a new draft conversation", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: {
          device_id: "device-current",
          authority_epoch: "0000000000000001000000000000000000000000",
        },
      },
    });

    renderChat("/agents/default/chats/draft-chat");

    await waitFor(() => {
      expect(ownedSurfaceRenderMock).toHaveBeenLastCalledWith(
        "draft-chat",
        "device-current",
        "device-current",
      );
    });
    expect(
      screen.getByRole("button", { name: /执行设备：本机/ }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Authority unavailable")).not.toBeInTheDocument();
  });

  it("hides the execution device chip when fewer than two devices exist", async () => {
    devicesApiMock.listDevices.mockResolvedValueOnce([
      {
        device_id: "device-current",
        public_key: "pk-current",
        friendly_name: "Current device",
        label: "Current device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: true,
      },
    ]);
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: {
          device_id: "device-current",
          authority_epoch: "0000000000000001000000000000000000000000",
        },
      },
    });

    renderChat("/agents/default/chats/local-only");

    await waitFor(() => expect(devicesApiMock.listDevices).toHaveBeenCalled());
    expect(
      screen.queryByRole("button", { name: /执行设备/ }),
    ).not.toBeInTheDocument();
  });

  it("does not show the old cross-device notice when authority is unavailable", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      messages: [],
      meta: {
        device_activity: {
          last_active_device_id: "device-other",
          participant_device_ids: ["device-other"],
        },
      },
    });

    renderChat("/agents/default/chats/chat-remote");

    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    expect(
      screen.queryByText(/此对话曾在其他设备运行/),
    ).not.toBeInTheDocument();
  });

  it("does not show the old remote-target notice banner", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: {
          device_id: "device-other",
        },
        device_activity: {
          last_active_device_id: "device-current",
          participant_device_ids: ["device-current"],
        },
      },
    });

    renderChat("/agents/default/chats/chat-remote-target");

    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    expect(screen.queryByText(/这是一个远程对话/)).not.toBeInTheDocument();
    expect(
      screen.queryByText(/此对话曾在其他设备运行/),
    ).not.toBeInTheDocument();
  });

  it("shows unavailable when backend authority placement is missing", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {},
    });

    renderChat("/agents/default/chats/chat-unavailable");

    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    expect(
      screen.getByRole("button", {
        name: /执行设备.*不可用|Execution device.*unavailable/i,
      }),
    ).toBeInTheDocument();
  });

  it("does not query ACP status when backend authority placement is missing", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {},
    });

    renderChat("/agents/default/chats/chat-unavailable");

    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    expect(acpApiMock.chatStatus).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", {
        name: /执行设备.*不可用|Execution device.*unavailable/i,
      }),
    ).toBeInTheDocument();
  });

  it("shows the execution chip when authority is the current device", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-current" },
      },
    });

    renderChat("/agents/default/chats/chat-target-local");

    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    expect(
      screen.getByRole("button", {
        name: /执行设备：本机/,
      }),
    ).toBeInTheDocument();
  });

  it("keeps chat submission events active after loading chat metadata", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      messages: [],
      meta: {
        device_activity: {
          last_active_device_id: "device-other",
          participant_device_ids: ["device-other"],
        },
      },
    });

    renderChat("/agents/default/chats/chat-remote-active");

    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    expect(
      screen.queryByText(/此对话曾在其他设备运行/),
    ).not.toBeInTheDocument();
    expect(screen.getByTestId("owned-surface")).toHaveTextContent(
      "chat-remote-active",
    );

    act(() => {
      window.dispatchEvent(
        new CustomEvent("rua-chat-submitted", {
          detail: {
            agentId: "default",
            chatId: "chat-remote-active",
            title: "继续处理这个会话",
          },
        }),
      );
    });

    await screen.findByText("继续处理这个会话");
  });

  it("reads the local chat index once per drawer open and ignores live wakes", async () => {
    const activityStream = controlledJsonLineResponse();
    consoleFetchMock.mockImplementation((url: RequestInfo | URL) => {
      if (String(url).includes("/console/activity")) {
        return activityStream.response;
      }
      return Promise.reject(new Error(`unexpected fetch ${String(url)}`));
    });

    renderChat("/agents/default/chats/chat-1");

    await waitFor(() =>
      expect(consoleFetchMock).toHaveBeenCalledWith(
        expect.stringContaining("/console/activity"),
        expect.objectContaining({ signal: expect.any(AbortSignal) }),
      ),
    );

    expect(chatApiMock.listChats).not.toHaveBeenCalled();

    await act(async () => {
      activityStream.send({
        type: "chat_live_wake",
        agent_id: "default",
        chat_id: "remote-new-chat",
        stream_id: "stream-remote-new-chat",
      });
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });

    expect(chatApiMock.listChats).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await waitFor(() => expect(chatApiMock.listChats).toHaveBeenCalledTimes(1));

    await act(async () => {
      activityStream.send({
        type: "chat_live_wake",
        agent_id: "default",
        chat_id: "remote-new-chat",
        stream_id: "stream-remote-new-chat",
      });
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });

    expect(chatApiMock.listChats).toHaveBeenCalledTimes(1);

    const drawer = await screen.findByRole("dialog");
    fireEvent.click(within(drawer).getByRole("button", { name: "Close" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await waitFor(() => expect(chatApiMock.listChats).toHaveBeenCalledTimes(2));
    expect(chatApiMock.listChats).toHaveBeenNthCalledWith(
      1,
      {
        offset: 0,
        limit: 10,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
      },
      "default",
    );
    expect(chatApiMock.listChats).toHaveBeenNthCalledWith(
      2,
      {
        offset: 0,
        limit: 10,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
      },
      "default",
    );
  });

  it("releases the chat activity stream while the browser tab is hidden", async () => {
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
    const activityStreams = [
      controlledJsonLineResponse(),
      controlledJsonLineResponse(),
    ];
    const activitySignals: AbortSignal[] = [];
    consoleFetchMock.mockImplementation(
      (url: RequestInfo | URL, init?: RequestInit) => {
        if (String(url).includes("/console/activity")) {
          activitySignals.push(init?.signal as AbortSignal);
          return activityStreams[activitySignals.length - 1].response;
        }
        return Promise.reject(new Error(`unexpected fetch ${String(url)}`));
      },
    );

    renderChat("/agents/default/chats/chat-1");
    await waitFor(() => expect(activitySignals).toHaveLength(1));

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    });
    act(() => document.dispatchEvent(new Event("visibilitychange")));

    expect(activitySignals[0].aborted).toBe(true);
    expect(activitySignals).toHaveLength(1);

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
    act(() => document.dispatchEvent(new Event("visibilitychange")));

    await waitFor(() => expect(activitySignals).toHaveLength(2));
    expect(activitySignals[1].aborted).toBe(false);
  });

  it("renders the complete Agent Chat route without a resolution frame", () => {
    renderChat("/agents/default/chats/paint-chat");

    expect(ownedSurfaceRenderMock.mock.calls[0]?.[0]).toBe("paint-chat");
    expect(screen.getByTestId("owned-surface")).toHaveTextContent("paint-chat");
    expect(screen.getByTestId("owned-surface")).not.toHaveTextContent("none");
  });

  it("returns to the same agent's active chat without an empty first frame", async () => {
    renderChat("/agents/default/chats/return-chat");
    await waitFor(() =>
      expect(localStorage.getItem("rua_last_chat_id_default")).toBe(
        "return-chat",
      ),
    );
    fireEvent.click(screen.getByText("go skills"));
    await screen.findByText("skills page");

    const renderCountBeforeReturn = ownedSurfaceRenderMock.mock.calls.length;
    fireEvent.click(screen.getByText("go default chat"));

    expect(
      ownedSurfaceRenderMock.mock.calls.slice(renderCountBeforeReturn)[0]?.[0],
    ).toBe("return-chat");
    expect(screen.getByTestId("owned-surface")).toHaveTextContent(
      "return-chat",
    );
    expect(screen.getByTestId("owned-surface")).not.toHaveTextContent("none");
  });

  it("remembers the active chat per selected agent", async () => {
    renderChat("/agents/default/chats/chat-1");

    await waitFor(() =>
      expect(localStorage.getItem("rua_last_chat_id_default")).toBe("chat-1"),
    );
  });

  it("uses the URL Agent even when the browser preference points elsewhere", async () => {
    useAgentStore.setState({ selectedAgent: "agent-a" });
    renderChat("/agents/agent-b/chats/agent-b-chat");

    await waitFor(() =>
      expect(chatApiMock.getChat).toHaveBeenCalledWith(
        "agent-b-chat",
        "agent-b",
      ),
    );
    expect(useAgentStore.getState().selectedAgent).toBe("agent-b");
    expect(localStorage.getItem("rua_last_chat_id_agent-b")).toBe(
      "agent-b-chat",
    );
  });

  it("keeps the current chat visible until the selected chat is prepared", async () => {
    let resolveChatB!: (value: unknown) => void;
    const chatBHistory = new Promise((resolve) => {
      resolveChatB = resolve;
    });
    chatApiMock.getChat.mockImplementation((chatId: string) =>
      chatId === "chat-b"
        ? chatBHistory
        : Promise.resolve({
            status: "idle",
            title: "Chat A title",
            messages: [],
          }),
    );
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-b",
          user_id: "default",
          channel: "console",
          name: "Chat B listed title",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-a");
    await screen.findByText("Chat A title");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Chat B listed title");
    fireEvent.click(screen.getByText("Chat B listed title"));

    expect(screen.getByTestId("location")).toHaveTextContent(
      "/agents/default/chats/chat-a",
    );
    expect(document.querySelector(".rua-owned-chat-title")).toHaveTextContent(
      "Chat A title",
    );

    await act(async () => {
      resolveChatB({
        status: "idle",
        title: "Chat B loaded title",
        messages: [],
      });
    });

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/chat-b",
      ),
    );
    await screen.findByText("Chat B loaded title");
  });

  it("ignores a stale model snapshot from the previously viewed chat", async () => {
    let resolveChatB!: (value: unknown) => void;
    const chatBHistory = new Promise((resolve) => {
      resolveChatB = resolve;
    });
    chatApiMock.getChat.mockImplementation((chatId: string) =>
      chatId === "chat-b"
        ? chatBHistory
        : Promise.resolve({
            status: "idle",
            title: "Chat A",
            model: { provider_id: "provider-a", model: "model-1" },
            messages: [],
          }),
    );
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-b",
          user_id: "default",
          channel: "console",
          name: "Chat B",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-a");
    await waitFor(() =>
      expect(screen.getByTestId("model-selector")).toHaveTextContent(
        "chat-a:model-1",
      ),
    );
    const staleHistoryChange =
      ownedSurfaceHistoryRegistrationMock.mock.calls.find(
        ([chatId]) => chatId === "chat-a",
      )?.[1] as ((history: unknown) => void) | undefined;
    expect(staleHistoryChange).toBeTypeOf("function");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    fireEvent.click(await screen.findByText("Chat B"));
    expect(screen.getByTestId("model-selector")).toHaveTextContent(
      "chat-a:model-1",
    );

    await act(async () => {
      resolveChatB({
        status: "idle",
        title: "Chat B",
        model: { provider_id: "provider-b", model: "model-2" },
        messages: [],
      });
    });
    await waitFor(() =>
      expect(screen.getByTestId("model-selector")).toHaveTextContent(
        "chat-b:model-2",
      ),
    );

    act(() => {
      staleHistoryChange?.({
        status: "idle",
        title: "Chat A late",
        model: { provider_id: "provider-a", model: "model-1" },
        messages: [],
      });
    });
    expect(screen.getByTestId("model-selector")).toHaveTextContent(
      "chat-b:model-2",
    );
  });

  it("starts a new lazy chat from the header action", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-from-header");

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("chat.newChatTooltip")));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/lazy-from-header",
      ),
    );
    expect(screen.getByTestId("owned-surface")).toHaveTextContent(
      "lazy-from-header",
    );
    expect(
      screen.getByRole("button", { name: /执行设备：本机/ }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Authority unavailable")).not.toBeInTheDocument();
  });

  it.each(["header", "sidebar"])(
    "inherits the current model through the shared %s action and keeps it selectable",
    async (entry) => {
      vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-with-model");
      chatApiMock.getChat.mockResolvedValue({
        status: "idle",
        messages: [],
        model: { provider_id: "provider-a", model: "model-a" },
      });

      renderChat("/agents/default/chats/chat-1");
      await waitFor(() =>
        expect(screen.getByTestId("model-selector")).toHaveTextContent(
          "chat-1:model-a",
        ),
      );

      if (entry === "header") {
        fireEvent.click(screen.getByLabelText(label("chat.newChatTooltip")));
      } else {
        act(() => {
          expect(
            requestNewConversation({
              scope: "agent",
              agentId: "default",
              chatId: "chat-1",
            }),
          ).toBe(true);
        });
      }

      await waitFor(() =>
        expect(screen.getByTestId("location")).toHaveTextContent(
          "/agents/default/chats/lazy-with-model",
        ),
      );
      const selector = screen.getByTestId("model-selector");
      expect(selector).toHaveAttribute("data-draft-selection", "true");
      expect(selector).toHaveTextContent("lazy-with-model:model-a");
      expect(
        JSON.parse(
          sessionStorage.getItem(
            "rua_draft_chat_model_default:lazy-with-model",
          ) || "{}",
        ),
      ).toMatchObject({ provider_id: "provider-a", model: "model-a" });

      fireEvent.click(selector);
      expect(screen.getByTestId("model-selector")).toHaveTextContent(
        "lazy-with-model:model-draft",
      );
      expect(
        JSON.parse(
          sessionStorage.getItem(
            "rua_draft_chat_model_default:lazy-with-model",
          ) || "{}",
        ),
      ).toMatchObject({
        provider_id: "provider-draft",
        model: "model-draft",
      });
    },
  );

  it("returns to the existing lazy chat without replacing its composer draft", async () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("should-not-be-used");
    localStorage.setItem("rua_draft_chat_id_default", "draft-chat");
    sessionStorage.setItem(
      "rua_input_draft_default:draft-chat",
      "unfinished input",
    );
    chatApiMock.getChat.mockImplementation((chatId: string) =>
      Promise.resolve(
        chatId === "chat-1"
          ? {
              status: "idle",
              messages: [
                { id: "user-1", role: "user", content: "materialized chat" },
              ],
            }
          : { status: "idle", messages: [] },
      ),
    );

    renderChat("/agents/default/chats/chat-1");
    fireEvent.click(screen.getByLabelText(label("chat.newChatTooltip")));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/draft-chat",
      ),
    );
    expect(randomUUID).not.toHaveBeenCalled();
    expect(localStorage.getItem("rua_draft_chat_id_default")).toBe(
      "draft-chat",
    );
    expect(sessionStorage.getItem("rua_input_draft_default:draft-chat")).toBe(
      "unfinished input",
    );
  });

  it("replaces a stale draft pointer after that Chat has materialized", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("fresh-after-materialized");
    localStorage.setItem("rua_draft_chat_id_default", "chat-1");
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [{ id: "user-1", role: "user", content: "already sent" }],
    });

    renderChat("/agents/default/chats/chat-1");

    await waitFor(() =>
      expect(localStorage.getItem("rua_draft_chat_id_default")).toBeNull(),
    );
    fireEvent.click(screen.getByLabelText(label("chat.newChatTooltip")));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/fresh-after-materialized",
      ),
    );
  });

  it("starts a new lazy chat from the history drawer and closes the drawer", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-from-drawer");
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-1",
          user_id: "default",
          channel: "console",
          name: "Existing chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Existing chat");
    fireEvent.click(screen.getByText(label("chat.createNewChat")));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/lazy-from-drawer",
      ),
    );
    await waitFor(() =>
      expect(screen.queryByText("Existing chat")).not.toBeInTheDocument(),
    );
    expect(screen.getByTestId("owned-surface")).toHaveTextContent(
      "lazy-from-drawer",
    );
  });

  it("loads owned session drawer and switches by URL", async () => {
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-2",
          user_id: "default",
          channel: "console",
          name: "Second chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Second chat");
    fireEvent.click(screen.getByText("Second chat"));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/chat-2",
      ),
    );
  });

  it("keeps the session drawer open while the target chat is prepared", async () => {
    let finishPreparation:
      | ((history: { status: string; messages: never[] }) => void)
      | undefined;
    const preparation = new Promise<{ status: string; messages: never[] }>(
      (resolve) => {
        finishPreparation = resolve;
      },
    );
    chatApiMock.getChat.mockImplementation((chatId: string) =>
      chatId === "chat-2"
        ? preparation
        : Promise.resolve({ status: "idle", messages: [] }),
    );
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-2",
          user_id: "default",
          channel: "console",
          name: "Second chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Second chat");
    const drawer = screen.getByRole("dialog");
    await waitFor(() =>
      expect(drawer.parentElement).toHaveClass("rua-ui-drawer-mask--visible"),
    );
    fireEvent.click(screen.getByText("Second chat"));

    expect(drawer.parentElement).toHaveClass("rua-ui-drawer-mask--visible");
    expect(screen.getByTestId("location")).toHaveTextContent(
      "/agents/default/chats/chat-1",
    );

    await act(
      async () => finishPreparation?.({ status: "idle", messages: [] }),
    );

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/chat-2",
      ),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("loads additional chat drawer pages from the footer action", async () => {
    const pinnedChat = {
      id: "chat-pinned",
      user_id: "default",
      channel: "console",
      name: "Pinned chat",
      pinned: true,
      created_at: "2026-06-19T00:00:00Z",
      updated_at: "2026-06-19T00:00:00Z",
    };
    chatApiMock.listChats
      .mockResolvedValueOnce({
        data: [
          pinnedChat,
          ...Array.from({ length: 10 }, (_, index) => ({
            id: `chat-${index + 1}`,
            user_id: "default",
            channel: "console",
            name: `Page one chat ${index + 1}`,
            created_at: "2026-06-18T00:00:00Z",
            updated_at: "2026-06-18T00:00:00Z",
          })),
        ],
        total: 12,
      })
      .mockResolvedValueOnce({
        data: [
          pinnedChat,
          {
            id: "chat-11",
            user_id: "default",
            channel: "console",
            name: "Page two chat 11",
            created_at: "2026-06-17T00:00:00Z",
            updated_at: "2026-06-17T00:00:00Z",
          },
        ],
        total: 12,
      });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Page one chat 1");
    fireEvent.click(
      screen.getByRole("button", { name: label("chat.loadMoreChats") }),
    );
    await screen.findByText("Page two chat 11");
    expect(screen.getByText("Page one chat 1")).toBeInTheDocument();

    expect(chatApiMock.listChats).toHaveBeenNthCalledWith(
      1,
      {
        offset: 0,
        limit: 10,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
      },
      "default",
    );
    expect(chatApiMock.listChats).toHaveBeenNthCalledWith(
      2,
      {
        offset: 10,
        limit: 10,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
      },
      "default",
    );
  });

  it("orders pinned chats before unpinned chats in the owned drawer", async () => {
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-unpinned",
          user_id: "default",
          channel: "console",
          name: "Unpinned chat",
          pinned: false,
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
        {
          id: "chat-pinned",
          user_id: "default",
          channel: "console",
          name: "Pinned chat",
          pinned: true,
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 2,
    });

    renderChat("/agents/default/chats/chat-unpinned");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    const pinned = await screen.findByText("Pinned chat");
    const unpinned = await screen.findByText("Unpinned chat");

    expect(
      pinned.compareDocumentPosition(unpinned) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("routes owned drawer list, search, and pin actions through the selected agent", async () => {
    useAgentStore.setState({
      selectedAgent: "agent-b",
      agents: [
        {
          id: "agent-b",
          name: "Agent B",
          description: "",
          workspace_dir: "",
          enabled: true,
        },
      ],
    });
    agentsApiMock.listAgents.mockResolvedValue({
      agents: [
        {
          id: "agent-b",
          name: "Agent B",
          description: "",
          workspace_dir: "",
          enabled: true,
        },
      ],
    });
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "agent-b-chat",
          user_id: "agent-b",
          channel: "console",
          name: "Agent B chat",
          pinned: false,
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });
    chatApiMock.searchChats.mockResolvedValue({
      results: [
        {
          chat_id: "agent-b-hit",
          chat_name: "Agent B hit",
          card_index: 2,
          role: "assistant",
          snippet: "scoped needle",
        },
      ],
    });

    renderChat("/agents/agent-b/chats/agent-b-chat");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Agent B chat");

    expect(chatApiMock.listChats).toHaveBeenCalledWith(
      {
        offset: 0,
        limit: 10,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
      },
      "agent-b",
    );

    fireEvent.click(screen.getByLabelText(label("chat.pinTooltip")));
    await waitFor(() =>
      expect(chatApiMock.updateChat).toHaveBeenCalledWith(
        "agent-b-chat",
        { pinned: true, pin_order: expect.any(Number) },
        "agent-b",
      ),
    );

    fireEvent.change(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
      {
        target: { value: "needle" },
      },
    );

    await screen.findByText(
      (_, element) => element?.textContent === "scoped needle",
    );
    expect(chatApiMock.searchChats).toHaveBeenCalledWith("needle", "agent-b");
  });

  it("reloads the open history drawer when the selected agent changes", async () => {
    chatApiMock.listChats
      .mockResolvedValueOnce({
        data: [
          {
            id: "agent-a-chat",
            user_id: "agent-a",
            channel: "console",
            name: "Agent A chat",
            created_at: "2026-06-18T00:00:00Z",
            updated_at: "2026-06-18T00:00:00Z",
          },
        ],
        total: 1,
      })
      .mockResolvedValueOnce({
        data: [
          {
            id: "agent-b-chat",
            user_id: "agent-b",
            channel: "console",
            name: "Agent B chat",
            created_at: "2026-06-19T00:00:00Z",
            updated_at: "2026-06-19T00:00:00Z",
          },
        ],
        total: 1,
      });
    chatApiMock.searchChats.mockResolvedValue({
      results: [
        {
          chat_id: "agent-a-hit",
          chat_name: "Agent A hit",
          card_index: 1,
          role: "assistant",
          snippet: "agent a search result",
        },
      ],
    });

    renderChat("/agents/agent-a/chats/agent-a-chat");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Agent A chat");
    fireEvent.change(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
      {
        target: { value: "agent a" },
      },
    );
    await screen.findByText(
      (_, element) => element?.textContent === "agent a search result",
    );

    fireEvent.click(screen.getByText("go agent b chat"));

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Agent B chat");
    expect(screen.queryByText("Agent A chat")).not.toBeInTheDocument();
    expect(
      screen.queryByText(
        (_, element) => element?.textContent === "agent a search result",
      ),
    ).not.toBeInTheDocument();
    expect(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
    ).toHaveValue("");
    expect(chatApiMock.listChats).toHaveBeenNthCalledWith(
      1,
      {
        offset: 0,
        limit: 10,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
      },
      "agent-a",
    );
    expect(chatApiMock.listChats).toHaveBeenNthCalledWith(
      2,
      {
        offset: 0,
        limit: 10,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
      },
      "agent-b",
    );
  });

  it("uses the metadata title from Rust chat history", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      title: "Persisted metadata title",
      messages: [
        {
          id: "sys-1",
          role: "system",
          content: "hidden",
        },
        {
          id: "user-1",
          role: "user",
          content: [
            { type: "image_url", url: "/preview/a.png" },
            { type: "text", text: "First user question" },
          ],
        },
        {
          id: "assistant-1",
          role: "assistant",
          content: "Answer",
        },
      ],
    });

    renderChat("/agents/default/chats/chat-title");

    await screen.findByText("Persisted metadata title");
    expect(screen.queryByText("First user question")).not.toBeInTheDocument();
    expect(screen.queryByText(label("chat.untitled"))).not.toBeInTheDocument();
  });

  it("does not derive the header title from message content", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [
        {
          id: "assistant-1",
          role: "assistant",
          content: "assistant preface should not become the chat title",
        },
        {
          id: "user-1",
          role: "user",
          content: "Direct string user title",
        },
      ],
    });

    renderChat("/agents/default/chats/chat-string-title");

    await screen.findByText(label("chat.untitled"));
    expect(
      screen.queryByText("assistant preface should not become the chat title"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("Direct string user title"),
    ).not.toBeInTheDocument();
  });

  it("updates a new chat title from the submitted user input", async () => {
    let resolveHistory!: (value: unknown) => void;
    chatApiMock.getChat.mockReturnValue(
      new Promise((resolve) => {
        resolveHistory = resolve;
      }),
    );

    renderChat("/agents/default/chats/lazy-chat-title");

    await screen.findByText(label("chat.untitled"));

    act(() => {
      window.dispatchEvent(
        new CustomEvent("rua-chat-submitted", {
          detail: {
            agentId: "default",
            chatId: "lazy-chat-title",
            title: "给我说个冷笑话",
          },
        }),
      );
    });

    await screen.findByText("给我说个冷笑话");
    act(() => {
      window.dispatchEvent(
        new CustomEvent("rua-chat-submitted", {
          detail: {
            agentId: "default",
            chatId: "lazy-chat-title",
            title: "第二轮不应该改标题",
          },
        }),
      );
    });
    expect(screen.getByText("给我说个冷笑话")).toBeInTheDocument();
    expect(screen.queryByText("第二轮不应该改标题")).not.toBeInTheDocument();
    await act(async () => {
      resolveHistory({ status: "idle", messages: [] });
    });
    expect(screen.getByText("给我说个冷笑话")).toBeInTheDocument();
    expect(screen.queryByText(label("chat.untitled"))).not.toBeInTheDocument();
  });

  it("ignores stale header title loads when the selected agent changes", async () => {
    let resolveAgentA!: (value: unknown) => void;
    let resolveAgentB!: (value: unknown) => void;
    const agentAHistory = new Promise((resolve) => {
      resolveAgentA = resolve;
    });
    const agentBHistory = new Promise((resolve) => {
      resolveAgentB = resolve;
    });
    chatApiMock.getChat.mockImplementation(
      (_chatId: string, agentId: string) =>
        agentId === "agent-b" ? agentBHistory : agentAHistory,
    );

    renderChat("/agents/agent-a/chats/agent-a-chat");

    fireEvent.click(screen.getByText("go agent b chat"));
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/agent-b/chats/agent-a-chat",
      ),
    );
    await waitFor(() =>
      expect(chatApiMock.getChat).toHaveBeenCalledWith(
        "agent-a-chat",
        "agent-b",
      ),
    );

    await act(async () => {
      resolveAgentA({
        status: "idle",
        title: "Stale title",
        messages: [{ id: "stale", role: "user", content: "Stale title" }],
      });
    });
    expect(screen.queryByText("Stale title")).not.toBeInTheDocument();

    await act(async () => {
      resolveAgentB({
        status: "idle",
        title: "Fresh title",
        messages: [{ id: "fresh", role: "user", content: "Fresh title" }],
      });
    });
    await screen.findByText("Fresh title");
    expect(chatApiMock.getChat).toHaveBeenCalledWith("agent-a-chat", "agent-a");
  });

  it("keeps media-only user content untitled instead of stringifying blocks", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [
            { type: "image", image: { url: "/preview/a.png" } },
            { type: "text", text: 123 },
            null,
          ],
        },
      ],
    });

    renderChat("/agents/default/chats/chat-media-only");

    await screen.findByText(label("chat.untitled"));
    expect(screen.queryByText(/preview\/a\.png/)).not.toBeInTheDocument();
    expect(screen.queryByText(/"type":"image"/)).not.toBeInTheDocument();
    expect(screen.queryByText(/\[object Object\]/)).not.toBeInTheDocument();
  });

  it("renders the mobile chat header with compact actions and keeps model configuration in the composer", async () => {
    document.documentElement.classList.add("mobile-mode");
    chatApiMock.listChats.mockResolvedValue({ data: [], total: 0 });

    renderChat("/agents/default/chats/chat-1?_mobile=1");

    await waitFor(() =>
      expect(
        screen.getByLabelText(label("agent.switchAgent")),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText("小Rua")).toBeInTheDocument();
    const modelSelector = screen.getByTestId("model-selector");
    expect(modelSelector).toHaveTextContent("chat-1");
    expect(
      document.querySelector(".rua-owned-chat-header"),
    ).not.toContainElement(modelSelector);
    expect(
      screen.getByLabelText(label("chat.newChatTooltip")),
    ).toBeInTheDocument();
    expect(
      screen.getByLabelText(label("navigation.manageConversations")),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(label("common.more"))).toBeInTheDocument();
    expect(document.documentElement).toHaveClass("mobile-mode");
  });

  it("keeps native environment and permissions in More", async () => {
    const openEnvironment = vi.fn();
    const openPermissions = vi.fn();
    window.MobileRuntime = { openEnvironment, openPermissions };
    try {
      renderChat("/agents/default/chats/chat-1?_mobile=1");
      expect(screen.queryByText(label("common.mobileRuntime"))).not.toBeInTheDocument();
      fireEvent.click(screen.getByLabelText(label("common.more")));
      fireEvent.click(await screen.findByText(label("common.mobileRuntime")));
      expect(openEnvironment).toHaveBeenCalledOnce();
      fireEvent.click(screen.getByLabelText(label("common.more")));
      fireEvent.click(await screen.findByText(label("common.mobilePermissions")));
      expect(openPermissions).toHaveBeenCalledOnce();
    } finally {
      delete window.MobileRuntime;
    }
  });

  it("does not offer native environment management in a browser", async () => {
    renderChat("/agents/default/chats/chat-1");
    fireEvent.click(screen.getByLabelText(label("common.more")));
    expect(screen.queryByText(label("common.mobileRuntime"))).not.toBeInTheDocument();
    expect(screen.queryByText(label("common.mobilePermissions"))).not.toBeInTheDocument();
  });

  it("renders one model and thinking entry through the composer tools", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: {
        provider_id: "dashscope",
        model: "glm-5.2",
        thinking_strength: "auto",
        thinking_param_style: "effort",
      },
    });

    renderChat("/agents/default/chats/chat-reasoning");

    const selector = await screen.findByTestId("model-selector");
    expect(selector).toHaveTextContent("chat-reasoning:glm-5.2");
    expect(screen.getByTestId("owned-surface")).toContainElement(selector);
    expect(
      document.querySelector(".rua-owned-chat-header"),
    ).not.toContainElement(selector);
  });

  it("toggles pin state through Rust chat update semantics", async () => {
    vi.spyOn(Date, "now").mockReturnValue(42);
    const update = deferred<never>();
    chatApiMock.updateChat.mockReturnValue(update.promise);
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-1",
          user_id: "default",
          channel: "console",
          name: "Pin target",
          pinned: false,
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Pin target");
    fireEvent.click(screen.getByLabelText(label("chat.pinTooltip")));

    expect(
      screen.getByLabelText(label("chat.unpinTooltip")),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(chatApiMock.updateChat).toHaveBeenCalledWith(
        "chat-1",
        { pinned: true, pin_order: 42 },
        "default",
      ),
    );

    await act(async () => update.reject(new Error("offline")));
    expect(screen.getByLabelText(label("chat.pinTooltip"))).toBeInTheDocument();
  });

  it("stores chat block display defaults from the more menu", async () => {
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: label("chat.options.menu"),
      }),
    );

    fireEvent.click(await screen.findByText(label("chat.blocks.menuTitle")));

    const toolRow = screen
      .getByText(label("chat.blocks.tool"))
      .closest(".rua-owned-chat-options-row");
    expect(toolRow).toBeTruthy();
    fireEvent.click(
      within(toolRow!).getByRole("button", {
        name: label("chat.blocks.expanded"),
      }),
    );

    expect(
      JSON.parse(
        localStorage.getItem("rua:chat-block-collapse-preferences") || "{}",
      ),
    ).toMatchObject({
      aggregateExpanded: false,
      thinkingExpanded: false,
      toolExpanded: true,
    });
  });

  it("places refresh before conversation options in the more menu", async () => {
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));

    const menu = await screen.findByRole("menu");
    const items = within(menu).getAllByRole("menuitem");
    const refreshIndex = items.findIndex(
      (item) => item.textContent?.includes(label("common.refreshPage")),
    );
    const optionsIndex = items.findIndex(
      (item) => item.textContent?.includes(label("chat.options.menu")),
    );

    expect(refreshIndex).toBeGreaterThan(-1);
    expect(optionsIndex).toBe(refreshIndex + 1);
  });

  it("saves selected-agent chat runtime options from the conversation options card", async () => {
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: label("chat.options.menu"),
      }),
    );
    await screen.findByText(label("chat.options.title"));

    fireEvent.click(screen.getByText(label("chat.options.agentRuntime")));

    fireEvent.change(screen.getByDisplayValue("100"), {
      target: { value: "12" },
    });
    fireEvent.click(
      screen.getAllByRole("button", { name: label("common.enable") })[0],
    );
    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));
    const preview = (await screen.findByText("max_iters")).closest(
      '[role="dialog"]',
    );
    expect(chatApiMock.updateRunningConfig).not.toHaveBeenCalled();
    fireEvent.click(
      within(preview as HTMLElement).getByRole("button", {
        name: label("common.save"),
      }),
    );

    await waitFor(() => {
      expect(chatApiMock.updateRunningConfig).toHaveBeenCalledWith(
        "chat-1",
        expect.objectContaining({
          auto_continue_on_text_only: true,
          max_iters: 12,
        }),
        "default",
      );
    });
  });

  it("previews complete configuration before applying the same candidate", async () => {
    const preview = {
      current: { epoch: "one", running_config: { max_iters: 10 } },
      candidate: { epoch: "two", running_config: { max_iters: 20 } },
      applied: false,
    };
    const previewSpy = vi
      .spyOn(conversationApi, "previewConfiguration")
      .mockResolvedValue(preview);
    const applySpy = vi
      .spyOn(conversationApi, "applyConfiguration")
      .mockResolvedValue({ ...preview, applied: true });
    try {
      renderChat("/agents/default/chats/chat-1");
      fireEvent.click(screen.getByLabelText(label("common.more")));
      fireEvent.click(
        await screen.findByRole("menuitem", {
          name: label("chat.options.menu"),
        }),
      );
      await screen.findByText(label("chat.options.title"));
      const update = await screen.findByRole("button", {
        name: "更新完整配置",
      });
      await waitFor(() => expect(update).not.toBeDisabled());
      fireEvent.click(update);
      const dialog = (
        await screen.findByText("running_config.max_iters")
      ).closest('[role="dialog"]');
      expect(applySpy).not.toHaveBeenCalled();
      fireEvent.click(
        within(dialog as HTMLElement).getByRole("button", {
          name: label("common.save"),
        }),
      );
      await waitFor(() =>
        expect(applySpy).toHaveBeenCalledWith(
          { scope: "agent", agentId: "default" },
          "chat-1",
          preview,
        ),
      );
    } finally {
      previewSpy.mockRestore();
      applySpy.mockRestore();
    }
  });

  it("keeps configuration controls usable after a transient options read failure", async () => {
    chatApiMock.getRunningConfig.mockRejectedValueOnce(
      new Error("Temporary connection failure"),
    );
    const preview = {
      current: { epoch: "current", running_config: { max_iters: 17 } },
      candidate: {
        epoch: "complete",
        running_config: { max_iters: 17 },
        security_config: { tool_guard: { approval_mode: "ask_first" } },
      },
      applied: false,
    };
    const previewSpy = vi
      .spyOn(conversationApi, "previewConfiguration")
      .mockResolvedValue(preview);
    const applySpy = vi
      .spyOn(conversationApi, "applyConfiguration")
      .mockResolvedValue({ ...preview, applied: true });
    try {
      renderChat("/agents/default/chats/chat-1");
      fireEvent.click(screen.getByLabelText(label("common.more")));
      fireEvent.click(
        await screen.findByRole("menuitem", {
          name: label("chat.options.menu"),
        }),
      );
      const update = await screen.findByRole("button", {
        name: "更新完整配置",
      });
      await waitFor(() => expect(update).toBeEnabled());
      fireEvent.click(update);
      let dialog = (await screen.findByText("security_config")).closest(
        '[role="dialog"]',
      );
      expect(applySpy).not.toHaveBeenCalled();
      fireEvent.click(
        within(dialog as HTMLElement).getByRole("button", {
          name: label("common.cancel"),
        }),
      );
      expect(applySpy).not.toHaveBeenCalled();
      await waitFor(() => expect(update).toBeEnabled());
      fireEvent.click(update);
      dialog = (await screen.findByText("security_config")).closest(
        '[role="dialog"]',
      );
      fireEvent.click(
        within(dialog as HTMLElement).getByRole("button", {
          name: label("common.save"),
        }),
      );
      await waitFor(() =>
        expect(applySpy).toHaveBeenCalledWith(
          { scope: "agent", agentId: "default" },
          "chat-1",
          preview,
        ),
      );
      await waitFor(() =>
        expect(chatApiMock.getRunningConfig).toHaveBeenCalledTimes(2),
      );
      expect(chatApiMock.updateRunningConfig).not.toHaveBeenCalled();
      expect(agentsApiMock.getAgentChatRunningDefaults).not.toHaveBeenCalled();
    } finally {
      previewSpy.mockRestore();
      applySpy.mockRestore();
    }
  });

  it("resets Chat options to the current global defaults", async () => {
    const defaults = { ...runningConfig(), max_iters: 42 };
    agentsApiMock.getAgentChatRunningDefaults.mockResolvedValue(defaults);
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: label("chat.options.menu"),
      }),
    );
    await screen.findByText(label("chat.options.title"));

    fireEvent.click(
      screen.getByRole("button", {
        name: label("chat.options.resetToGlobalDefaults"),
      }),
    );
    const confirmDialog = (
      await screen.findByText(label("chat.options.resetTitle"))
    ).closest('[role="dialog"]');
    expect(confirmDialog).not.toBeNull();
    fireEvent.click(
      within(confirmDialog as HTMLElement).getByRole("button", {
        name: label("common.reset"),
      }),
    );

    await waitFor(() => {
      expect(agentsApiMock.getAgentChatRunningDefaults).toHaveBeenCalledWith(
        "default",
      );
      expect(chatApiMock.updateRunningConfig).toHaveBeenCalledWith(
        "chat-1",
        defaults,
        "default",
      );
    });
  });

  it("keeps remote chat options open after authority update failure", async () => {
    chatApiMock.updateRunningConfig.mockRejectedValueOnce(
      new Error("authority unavailable"),
    );
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: label("chat.options.menu"),
      }),
    );
    await screen.findByText(label("chat.options.title"));
    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));

    await waitFor(() =>
      expect(chatApiMock.updateRunningConfig).toHaveBeenCalled(),
    );
    expect(screen.getByText(label("chat.options.title"))).toBeInTheDocument();
  });

  it("groups maximum context length with context compaction", async () => {
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: label("chat.options.menu"),
      }),
    );
    await screen.findByText(label("chat.options.title"));

    fireEvent.click(screen.getByText(label("chat.options.agentRuntime")));
    expect(screen.getByText(label("agentConfig.maxIters"))).toBeInTheDocument();
    expect(
      screen.queryByText(label("agentConfig.maxContextLength")),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByText(label("chat.options.contextCompaction")));
    expect(
      screen.getByText(label("agentConfig.maxContextLength")),
    ).toBeInTheDocument();
    const compactRatioRow = screen
      .getByText(label("agentConfig.contextCompactRatio"))
      .closest(".rua-owned-chat-options-row");
    expect(
      within(compactRatioRow as HTMLElement).getByRole("slider"),
    ).toHaveAttribute("aria-valuemax", "0.95");
  });

  it("gates only second-pass tool controls behind their independent switch", async () => {
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: label("chat.options.menu"),
      }),
    );
    await screen.findByText(label("chat.options.title"));

    expect(
      screen.queryByText(label("agentConfig.toolResultCompactRecentN")),
    ).toBeNull();

    fireEvent.click(screen.getByText(label("chat.options.aggressiveContext")));

    const aggressiveSection = screen
      .getByText(label("chat.options.aggressiveContext"))
      .closest(".rua-owned-chat-options-section");
    expect(
      Array.from(
        aggressiveSection?.querySelectorAll(
          ".rua-owned-chat-options-label-main > span:first-child",
        ) ?? [],
      ).map((node) => node.textContent),
    ).toEqual([
      label("agentConfig.historyReplacementEnabled"),
      label("agentConfig.activeWorkPlanInjectionEnabled"),
      label("agentConfig.activeWorkPlanInjectionInterval"),
      label("agentConfig.toolResultProjectionTruncationEnabled"),
      label("agentConfig.toolResultProjectionTruncationBatchSize"),
      label("agentConfig.toolResultCompactRecentN"),
      label("agentConfig.toolResultCompactOldMaxBytes"),
      label("agentConfig.thinkingTurnsToKeep"),
      label("agentConfig.earlyBackgroundCompactionEnabled"),
      label("agentConfig.earlyCompactionTargetRatio"),
    ]);

    const historyReplacementRow = screen
      .getByText(label("agentConfig.historyReplacementEnabled"))
      .closest(".rua-owned-chat-options-row");
    expect(historyReplacementRow).not.toHaveAttribute("data-disabled");
    fireEvent.click(
      within(historyReplacementRow as HTMLElement).getByRole("button", {
        name: label("common.enable"),
      }),
    );
    const activePlanRow = screen
      .getByText(label("agentConfig.activeWorkPlanInjectionEnabled"))
      .closest(".rua-owned-chat-options-row");
    fireEvent.click(
      within(activePlanRow as HTMLElement).getByRole("button", {
        name: label("common.enable"),
      }),
    );

    const secondPassRow = (
      await screen.findByText(label("agentConfig.toolResultCompactRecentN"))
    ).closest(".rua-owned-chat-options-row");
    expect(secondPassRow).toHaveAttribute("data-disabled", "true");
    const batchSize = screen.getByRole("spinbutton", {
      name: label("agentConfig.toolResultProjectionTruncationBatchSize"),
    });
    expect(batchSize).toBeDisabled();
    expect(batchSize).toHaveValue(1);
    expect(
      screen
        .getByText(label("agentConfig.thinkingTurnsToKeep"))
        .closest(".rua-owned-chat-options-row"),
    ).not.toHaveAttribute("data-disabled");

    const projectionRow = screen
      .getByText(label("agentConfig.toolResultProjectionTruncationEnabled"))
      .closest(".rua-owned-chat-options-row");
    fireEvent.click(
      within(projectionRow as HTMLElement).getByRole("button", {
        name: label("common.enable"),
      }),
    );
    await waitFor(() => {
      const row = screen
        .getByText(label("agentConfig.toolResultCompactRecentN"))
        .closest(".rua-owned-chat-options-row");
      expect(row).not.toHaveAttribute("data-disabled");
    });

    const recentRounds = within(secondPassRow as HTMLElement).getByRole(
      "spinbutton",
    );
    expect(recentRounds).toHaveValue(2);
    fireEvent.change(recentRounds, { target: { value: "3" } });
    expect(recentRounds).toHaveValue(3);
    expect(batchSize).not.toBeDisabled();
    fireEvent.change(batchSize, { target: { value: "10" } });
    expect(batchSize).toHaveValue(10);

    const thinkingRetention = screen.getByRole("slider", {
      name: label("agentConfig.thinkingTurnsToKeep"),
    });
    fireEvent.keyDown(thinkingRetention, { key: "Home" });
    await waitFor(() =>
      expect(thinkingRetention).toHaveAttribute("aria-valuenow", "-1"),
    );

    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));
    const preview = (
      await screen.findByText(
        "tool_result_compact.projection_truncation_batch_size",
      )
    ).closest('[role="dialog"]');
    expect(chatApiMock.updateRunningConfig).not.toHaveBeenCalled();
    fireEvent.click(
      within(preview as HTMLElement).getByRole("button", {
        name: label("common.cancel"),
      }),
    );
    expect(chatApiMock.updateRunningConfig).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));
    const confirmedPreview = (
      await screen.findByText(
        "tool_result_compact.projection_truncation_batch_size",
      )
    ).closest('[role="dialog"]');
    fireEvent.click(
      within(confirmedPreview as HTMLElement).getByRole("button", {
        name: label("common.save"),
      }),
    );
    await waitFor(() => {
      expect(chatApiMock.updateRunningConfig).toHaveBeenCalledWith(
        "chat-1",
        expect.objectContaining({
          context_compact: expect.objectContaining({
            active_work_plan_injection_enabled: true,
            history_replacement_enabled: true,
          }),
          tool_result_compact: expect.objectContaining({
            projection_truncation_enabled: true,
            projection_truncation_batch_size: 10,
            recent_n: 3,
          }),
        }),
        "default",
      );
    });
  });

  it("saves the complete Chat configuration snapshot", async () => {
    const config = runningConfig();
    chatApiMock.getRunningConfig.mockResolvedValue(config);
    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: label("chat.options.menu"),
      }),
    );
    await screen.findByText(label("chat.options.title"));

    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));

    await waitFor(() => {
      expect(chatApiMock.updateRunningConfig).toHaveBeenCalledWith(
        "chat-1",
        config,
        "default",
      );
    });
  });

  it("preserves disabled Chat settings unless explicitly changed", async () => {
    const config = {
      ...runningConfig(),
      context_compact: {
        ...runningConfig().context_compact,
        context_compact_enabled: false,
      },
      tool_result_compact: {
        ...runningConfig().tool_result_compact,
        enabled: false,
      },
    };
    chatApiMock.getRunningConfig.mockResolvedValue(config);

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: label("chat.options.menu"),
      }),
    );
    await screen.findByText(label("chat.options.title"));

    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));

    await waitFor(() => {
      expect(chatApiMock.updateRunningConfig).toHaveBeenCalledWith(
        "chat-1",
        config,
        "default",
      );
    });
  });

  it("selects the remote authority placement from the authority device dialog", async () => {
    devicesApiMock.listDevices.mockResolvedValue([
      {
        device_id: "device-current",
        public_key: "pk-current",
        friendly_name: "Current device",
        label: "Current device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: true,
      },
      {
        device_id: "device-remote",
        public_key: "pk-remote",
        friendly_name: "Remote laptop",
        label: "Remote laptop",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: false,
      },
    ]);

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: new RegExp(label("chat.authorityPlacement.menu")),
      }),
    );
    await screen.findByText(label("chat.authorityPlacement.title"));
    expect(
      screen.queryByText(label("chat.authorityPlacement.unspecified")),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText(label("chat.authorityPlacement.hint")),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: /Remote laptop/ }));
    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));

    await waitFor(() =>
      expect(chatApiMock.setAuthorityPlacement).toHaveBeenCalledWith(
        "chat-1",
        "device-remote",
        "default",
      ),
    );
    await waitFor(() =>
      expect(ownedSurfaceRenderMock).toHaveBeenLastCalledWith(
        "chat-1",
        "device-remote",
        "device-current",
      ),
    );
    expect(
      await screen.findByText(
        label("chat.authorityPlacement.cacheInvalidatedNotice"),
      ),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: label("chat.authorityPlacement.cacheInvalidatedDontRemind"),
      }),
    );
    expect(
      localStorage.getItem("rua:authority-switch-cache-notice-disabled"),
    ).toBe("true");
  });

  it("preserves the draft model when selecting authority for a lazy chat", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-authority-chat");
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: { provider_id: "provider-a", model: "model-a" },
    });

    renderChat("/agents/default/chats/chat-1");
    await waitFor(() =>
      expect(screen.getByTestId("model-selector")).toHaveTextContent(
        "chat-1:model-a",
      ),
    );

    fireEvent.click(screen.getByLabelText(label("chat.newChatTooltip")));
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/lazy-authority-chat",
      ),
    );
    fireEvent.click(screen.getByTestId("model-selector"));

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: new RegExp(label("chat.authorityPlacement.menu")),
      }),
    );
    fireEvent.click(screen.getByRole("radio", { name: /Remote device/ }));
    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));

    expect(chatApiMock.setAuthorityPlacement).not.toHaveBeenCalled();
    expect(chatApiMock.materialize).not.toHaveBeenCalled();
    expect(
      sessionStorage.getItem(
        "rua_draft_chat_device_default:lazy-authority-chat",
      ),
    ).toBe("device-remote");
    expect(screen.getByTestId("model-selector")).toHaveTextContent(
      "model-draft",
    );
    expect(screen.getByTestId("model-selector")).toHaveAttribute(
      "data-draft-selection",
      "true",
    );
  });

  it("keeps draft choices until first input acceptance without calling materialize", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-authority-chat");
    useAgentStore.getState().setSelectedAgent("default");
    localStorage.setItem("rua_draft_chat_id_default", "lazy-authority-chat");
    sessionStorage.setItem(
      "rua_draft_chat_model_default:lazy-authority-chat",
      JSON.stringify({ provider_id: "provider-draft", model: "model-draft" }),
    );
    sessionStorage.setItem(
      "rua_draft_chat_device_default:lazy-authority-chat",
      "device-remote",
    );
    chatApiMock.getChat.mockResolvedValue({ status: "idle", messages: [] });
    renderChat("/agents/default/chats/lazy-authority-chat");
    await waitFor(() =>
      expect(screen.getByTestId("model-selector")).toHaveAttribute(
        "data-draft-selection",
        "true",
      ),
    );
    expect(chatApiMock.materialize).not.toHaveBeenCalled();
    expect(
      sessionStorage.getItem(
        "rua_draft_chat_model_default:lazy-authority-chat",
      ),
    ).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "accept submission" }));
    await waitFor(() =>
      expect(screen.getByTestId("model-selector")).toHaveAttribute(
        "data-draft-selection",
        "false",
      ),
    );
    expect(
      sessionStorage.getItem(
        "rua_draft_chat_model_default:lazy-authority-chat",
      ),
    ).toBeNull();
    expect(chatApiMock.materialize).not.toHaveBeenCalled();
  });

  it("replaces a stale control placement with the acknowledged authority", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: {
          device_id: "device-current",
          authority_epoch: "epoch-current",
        },
      },
      control: {
        revision: 7,
        authority_device_id: "device-current",
        placement: {
          authority_device_id: "device-current",
          authority_epoch: "epoch-current",
          switchable: true,
          lock_reason: null,
        },
        active_turn: null,
        queue: { entries: [], paused: false },
        acp: { active: false, binding: null },
        allowed_controls: {
          stop: false,
          queue: true,
          interject: false,
          edit_queued_input: true,
          delete_queued_input: true,
          switch_authority: true,
          enter_acp: true,
          exit_acp: false,
        },
      },
    });
    chatApiMock.setAuthorityPlacement.mockResolvedValue({
      id: "chat-1",
      user_id: "default",
      channel: "console",
      created_at: null,
      updated_at: null,
      meta: {
        authority_placement: {
          device_id: "device-remote",
          authority_epoch: "epoch-remote",
        },
      },
    });

    renderChat("/agents/default/chats/chat-1");

    await waitFor(() =>
      expect(ownedSurfaceRenderMock).toHaveBeenLastCalledWith(
        "chat-1",
        "device-current",
        "device-current",
      ),
    );
    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: new RegExp(label("chat.authorityPlacement.menu")),
      }),
    );
    fireEvent.click(
      await screen.findByRole("radio", {
        name: /Remote device/,
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));

    await waitFor(() =>
      expect(chatApiMock.setAuthorityPlacement).toHaveBeenCalledWith(
        "chat-1",
        "device-remote",
        "default",
      ),
    );
    await waitFor(() =>
      expect(ownedSurfaceRenderMock).toHaveBeenLastCalledWith(
        "chat-1",
        "device-remote",
        "device-current",
      ),
    );
    expect(
      screen.getByRole("button", { name: /Remote device/ }),
    ).toBeInTheDocument();
  });

  it("shows the pending target after closing the authority dialog", async () => {
    let resolveSwitch!: (value: unknown) => void;
    chatApiMock.setAuthorityPlacement.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveSwitch = resolve;
      }),
    );

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: new RegExp(label("chat.authorityPlacement.menu")),
      }),
    );
    await screen.findByText(label("chat.authorityPlacement.title"));
    fireEvent.click(screen.getByRole("radio", { name: /Remote device/ }));
    const save = screen.getByRole("button", { name: label("common.save") });
    fireEvent.click(save);

    await waitFor(() =>
      expect(chatApiMock.setAuthorityPlacement).toHaveBeenCalledWith(
        "chat-1",
        "device-remote",
        "default",
      ),
    );
    await waitFor(() =>
      expect(
        screen.queryByText(label("chat.authorityPlacement.title")),
      ).not.toBeInTheDocument(),
    );
    const pendingChip = screen.getByRole("button", {
      name: "正在切换执行设备",
    });
    expect(pendingChip).toHaveTextContent("Remote device");
    expect(pendingChip.querySelector(".rua-spinner")).toBeInTheDocument();

    await act(async () => {
      resolveSwitch({
        id: "chat-1",
        user_id: "default",
        channel: "console",
        created_at: null,
        updated_at: null,
        meta: {
          authority_placement: { device_id: "device-remote" },
        },
      });
    });

    await waitFor(() =>
      expect(ownedSurfaceRenderMock).toHaveBeenLastCalledWith(
        "chat-1",
        "device-remote",
        "device-current",
      ),
    );
    expect(
      screen.getByRole("button", { name: /Remote device/ }),
    ).not.toContainHTML("rua-spinner");
  });

  it("switches authority from one explicit device to another", async () => {
    localStorage.setItem(
      "rua:authority-switch-cache-notice-disabled",
      "true",
    );
    devicesApiMock.listDevices.mockResolvedValue([
      {
        device_id: "device-current",
        public_key: "pk-current",
        friendly_name: "Current device",
        label: "Current device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: true,
      },
      {
        device_id: "device-remote",
        public_key: "pk-remote",
        friendly_name: "Remote laptop",
        label: "Remote laptop",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: false,
      },
    ]);
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-remote" },
      },
    });
    chatApiMock.setAuthorityPlacement.mockResolvedValue({
      id: "chat-1",
      user_id: "default",
      channel: "console",
      created_at: null,
      updated_at: null,
      meta: {
        authority_placement: { device_id: "device-current" },
      },
    });

    renderChat("/agents/default/chats/chat-1");

    await waitFor(() =>
      expect(ownedSurfaceRenderMock).toHaveBeenLastCalledWith(
        "chat-1",
        "device-remote",
        "device-current",
      ),
    );
    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: new RegExp(label("chat.authorityPlacement.menu")),
      }),
    );
    fireEvent.click(
      await screen.findByRole("radio", {
        name: /Current device/,
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));

    await waitFor(() =>
      expect(chatApiMock.setAuthorityPlacement).toHaveBeenCalledWith(
        "chat-1",
        "device-current",
        "default",
      ),
    );
    await waitFor(() =>
      expect(ownedSurfaceRenderMock).toHaveBeenLastCalledWith(
        "chat-1",
        "device-current",
        "device-current",
      ),
    );
    expect(
      screen.queryByText(
        label("chat.authorityPlacement.cacheInvalidatedNotice"),
      ),
    ).not.toBeInTheDocument();
  });

  it("blocks authority placement changes while ACP mode is active", async () => {
    devicesApiMock.listDevices.mockResolvedValue([
      {
        device_id: "device-current",
        public_key: "pk-current",
        friendly_name: "Current device",
        label: "Current device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: true,
      },
      {
        device_id: "device-remote",
        public_key: "pk-remote",
        friendly_name: "Remote laptop",
        label: "Remote laptop",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: false,
      },
    ]);
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-current" },
        acp_binding: {
          kind: "acp_passthrough",
          device_id: "device-current",
          runner: "codex",
          cwd: "/workspace",
          runner_session_id: "runner-session-1",
        },
      },
    });
    renderChat("/agents/default/chats/chat-1");

    await screen.findByRole("status");
    fireEvent.click(screen.getByLabelText(label("common.more")));
    const authorityPlacementItem = await screen.findByRole("menuitem", {
      name: new RegExp(label("chat.authorityPlacement.menu")),
    });
    const acpEnterItem = await screen.findByRole("menuitem", {
      name: new RegExp(label("chat.acpEnter.menu")),
    });

    expect(authorityPlacementItem).toHaveAttribute("aria-disabled", "true");
    expect(acpEnterItem).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(authorityPlacementItem);

    expect(
      screen.queryByText(label("chat.authorityPlacement.title")),
    ).not.toBeInTheDocument();
  });

  it("uses persisted ACP binding to show banner and lock authority placement", async () => {
    devicesApiMock.listDevices.mockResolvedValue([
      {
        device_id: "device-current",
        public_key: "pk-current",
        friendly_name: "Current device",
        label: "Current device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: true,
      },
      {
        device_id: "device-remote",
        public_key: "pk-remote",
        friendly_name: "Remote laptop",
        label: "Remote laptop",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: false,
      },
    ]);
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-remote" },
        acp_binding: {
          kind: "acp_passthrough",
          device_id: "device-remote",
          runner: "codex",
          cwd: "/workspace",
          runner_session_id: "runner-session-1",
        },
      },
    });
    acpApiMock.chatStatus.mockResolvedValue({
      active: false,
      runner: null,
      session_id: null,
      cwd: null,
      chat_id: "chat-1",
      workspace_dir: "/workspace",
      target_device_id: "device-remote",
    });

    renderChat("/agents/default/chats/chat-1");

    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveClass(
        "rua-owned-chat-acp-banner",
      ),
    );
    expect(screen.getByText("codex")).toBeInTheDocument();
    expect(screen.getByTestId("active-acp-runner")).toHaveAttribute(
      "data-runner",
      "codex",
    );
    expect(ownedSurfaceRenderMock).toHaveBeenLastCalledWith(
      "chat-1",
      "device-remote",
      "device-current",
    );

    fireEvent.click(screen.getByLabelText(label("common.more")));
    expect(
      screen.getByRole("menuitem", {
        name: new RegExp(label("chat.authorityPlacement.menu")),
      }),
    ).toHaveAttribute("aria-disabled", "true");

    fireEvent.click(screen.getByText(label("chat.acpBanner.exit")));
    await waitFor(() =>
      expect(acpApiMock.exitChat).toHaveBeenCalledWith(
        "default",
        "chat-1",
        "device-remote",
      ),
    );
  });

  it("applies a remote ACP control event without polling", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-remote" },
      },
    });

    renderChat("/agents/default/chats/chat-1");
    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(acpApiMock.chatStatus).not.toHaveBeenCalled();

    const onHistoryChange = ownedSurfaceHistoryChangeMock.mock.calls.at(
      -1,
    )?.[0] as ((history: unknown) => void) | undefined;
    expect(onHistoryChange).toBeTypeOf("function");
    act(() => {
      onHistoryChange?.({
        status: "idle",
        messages: [],
        meta: {
          authority_placement: { device_id: "device-remote" },
        },
        control: {
          authority_device_id: "device-remote",
          placement: {
            authority_device_id: "device-remote",
            switchable: false,
            lock_reason: "acp_passthrough",
          },
          active_turn: null,
          queue: { entries: [] },
          acp: {
            active: true,
            binding: {
              kind: "acp_passthrough",
              device_id: "device-remote",
              runner: "codex",
              cwd: "/workspace",
              runner_session_id: "runner-session-1",
            },
          },
          allowed_controls: {
            stop: false,
            queue: true,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: false,
            enter_acp: false,
            exit_acp: true,
          },
        },
      });
    });

    expect(screen.getByRole("status")).toHaveClass(
      "rua-owned-chat-acp-banner",
    );
    expect(screen.getByTestId("active-acp-runner")).toHaveAttribute(
      "data-runner",
      "codex",
    );
    expect(acpApiMock.chatStatus).not.toHaveBeenCalled();
  });

  it("shows the ACP banner from authority-owned metadata", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-current" },
        acp_binding: {
          kind: "acp_passthrough",
          device_id: "device-current",
          runner: "codex",
          cwd: "/workspace",
          runner_session_id: "runner-session-1",
        },
      },
    });

    renderChat("/agents/default/chats/chat-1");

    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveClass(
        "rua-owned-chat-acp-banner",
      ),
    );
    expect(screen.getByText("ACP")).toBeInTheDocument();
    expect(screen.getByText("codex")).toBeInTheDocument();
  });

  it("loads ACP runners through the chat authority placement when entering ACP", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-remote" },
      },
    });
    devicesApiMock.listDevices.mockResolvedValue([
      {
        device_id: "device-current",
        public_key: "pk-current",
        friendly_name: "Current device",
        label: "Current device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: true,
      },
      {
        device_id: "device-remote",
        public_key: "pk-remote",
        friendly_name: "Remote laptop",
        label: "Remote laptop",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: false,
      },
    ]);
    acpApiMock.listChatRunners.mockResolvedValue([
      {
        key: "codex",
        enabled: true,
        command: "codex",
        args: [],
        env: {},
        trusted: true,
        tool_parse_mode: "call_detail",
        stdio_buffer_limit_bytes: 1048576,
        is_builtin: true,
      },
    ]);

    renderChat("/agents/default/chats/chat-target-remote");

    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    fireEvent.click(screen.getByLabelText(label("common.more")));
    const acpEnterItem = await screen.findByRole("menuitem", {
      name: new RegExp(label("chat.acpEnter.menu")),
    });
    expect(acpEnterItem).not.toHaveAttribute("aria-disabled", "true");

    fireEvent.click(acpEnterItem);

    await waitFor(() =>
      expect(acpApiMock.listChatRunners).toHaveBeenCalledWith(
        "default",
        "chat-target-remote",
        "device-remote",
      ),
    );
    expect(acpApiMock.chatStatus).not.toHaveBeenCalled();
    expect(acpApiMock.listRunners).not.toHaveBeenCalled();
  });

  it("shows the current specified authority device inside the more menu", async () => {
    devicesApiMock.listDevices.mockResolvedValue([
      {
        device_id: "device-current",
        public_key: "pk-current",
        friendly_name: "Current device",
        label: "Current device",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: true,
      },
      {
        device_id: "device-remote",
        public_key: "pk-remote",
        friendly_name: "Remote laptop",
        label: "Remote laptop",
        added_at_ms: 1,
        last_seen_ms: 2,
        is_self: false,
      },
    ]);
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-remote" },
      },
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    expect(
      await screen.findByRole("menuitem", {
        name: new RegExp(
          `${label("chat.authorityPlacement.menu")}.*Remote laptop`,
        ),
      }),
    ).toBeInTheDocument();
  });

  it("archives and restores a standalone chat in its owning agent without leaving the current chat", async () => {
    let chat = {
      id: "agent-b-chat",
      name: "Archive candidate",
      user_id: "agent-b",
      channel: "console",
      pinned: true,
      archived: false,
      created_at: "2026-06-18T00:00:00Z",
      updated_at: "2026-06-18T00:00:00Z",
    };
    chatApiMock.listChats.mockImplementation(async (params) => ({
      data: params.archived === chat.archived ? [chat] : [],
      total: params.archived === chat.archived ? 1 : 0,
    }));
    chatApiMock.updateChat.mockImplementation(async (_id, patch) => {
      chat = { ...chat, ...patch, pinned: false };
      return chat;
    });
    const listener = vi.fn();
    const unsubscribe = subscribeConversationNavigation(listener);
    try {
      renderChat("/agents/agent-b/chats/agent-b-chat");
      fireEvent.click(
        screen.getByLabelText(label("navigation.manageConversations")),
      );
      const drawer = await screen.findByRole("dialog");
      await within(drawer).findByText("Archive candidate");
      expect(chatApiMock.listChats).toHaveBeenLastCalledWith(
        expect.objectContaining({ archived: false }),
        "agent-b",
      );
      fireEvent.click(
        within(drawer).getByLabelText(label("projects.chat.archive")),
      );
      await waitFor(() =>
        expect(chatApiMock.updateChat).toHaveBeenCalledWith(
          "agent-b-chat",
          { archived: true },
          "agent-b",
        ),
      );
      await waitFor(() =>
        expect(
          within(drawer).queryByText("Archive candidate"),
        ).not.toBeInTheDocument(),
      );
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/agent-b/chats/agent-b-chat",
      );
      expect(listener).toHaveBeenCalledWith({
        scope: "agent",
        agentId: "agent-b",
        chatId: "agent-b-chat",
      });

      fireEvent.click(
        within(drawer).getByRole("tab", {
          name: label("projects.chat.archiveList"),
        }),
      );
      await within(drawer).findByText("Archive candidate");
      expect(chatApiMock.listChats).toHaveBeenLastCalledWith(
        expect.objectContaining({ archived: true, view: "all" }),
        "agent-b",
      );
      expect(
        within(drawer).queryByLabelText(label("projects.chat.clearArchive")),
      ).not.toBeInTheDocument();
      expect(
        within(drawer).queryByLabelText(label("chat.pinTooltip")),
      ).not.toBeInTheDocument();
      fireEvent.click(
        within(drawer).getByLabelText(label("projects.chat.restore")),
      );
      await waitFor(() =>
        expect(chatApiMock.updateChat).toHaveBeenLastCalledWith(
          "agent-b-chat",
          { archived: false },
          "agent-b",
        ),
      );
      await waitFor(() =>
        expect(
          within(drawer).queryByText("Archive candidate"),
        ).not.toBeInTheDocument(),
      );
      fireEvent.click(
        within(drawer).getByRole("tab", {
          name: label("chat.myConversations"),
        }),
      );
      await within(drawer).findByText("Archive candidate");
      expect(chatApiMock.listChats).toHaveBeenLastCalledWith(
        expect.objectContaining({ archived: false }),
        "agent-b",
      );
      expect(
        within(drawer).getByLabelText(label("chat.pinTooltip")),
      ).toBeInTheDocument();
    } finally {
      unsubscribe();
    }
  });

  it("searches archive titles with archive filters and ignores a late archive page after returning to active chats", async () => {
    const archivePage = deferred<{ data: unknown[]; total: number }>();
    const archivedChat = {
      id: "archived-chat",
      name: "Archived needle",
      archived: true,
      user_id: "default",
      channel: "console",
      created_at: null,
      updated_at: null,
    };
    chatApiMock.listChats.mockImplementation(async (params) => {
      if (params.archived && params.search) return archivePage.promise;
      if (params.archived) return { data: [archivedChat], total: 1 };
      return { data: [], total: 0 };
    });
    renderChat("/agents/default/chats/chat-1");
    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    const drawer = await screen.findByRole("dialog");
    fireEvent.click(
      within(drawer).getByRole("tab", {
        name: label("projects.chat.archiveList"),
      }),
    );
    await within(drawer).findByText("Archived needle");
    fireEvent.change(
      within(drawer).getByPlaceholderText(
        label("chat.chatListSearchPlaceholder"),
      ),
      {
        target: { value: "needle" },
      },
    );
    await waitFor(() =>
      expect(chatApiMock.listChats).toHaveBeenLastCalledWith(
        expect.objectContaining({
          archived: true,
          view: "all",
          search: "needle",
        }),
        "default",
      ),
    );
    expect(
      within(drawer).getByPlaceholderText(
        label("chat.chatListSearchPlaceholder"),
      ),
    ).toHaveValue("needle");
    expect(chatApiMock.searchChats).not.toHaveBeenCalled();
    fireEvent.click(
      within(drawer).getByRole("tab", { name: label("chat.myConversations") }),
    );
    await waitFor(() =>
      expect(chatApiMock.listChats).toHaveBeenLastCalledWith(
        expect.objectContaining({ archived: false }),
        "default",
      ),
    );
    await act(async () =>
      archivePage.resolve({ data: [archivedChat], total: 1 }),
    );
    expect(
      within(drawer).queryByText("Archived needle"),
    ).not.toBeInTheDocument();
    expect(
      within(drawer).getByPlaceholderText(
        label("chat.chatListSearchPlaceholder"),
      ),
    ).toHaveValue("");
  });

  it("uses full chat search inside the all chats drawer", async () => {
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-1",
          user_id: "default",
          channel: "console",
          name: "Active chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });
    chatApiMock.searchChats.mockResolvedValue({
      results: [
        {
          chat_id: "chat-hit",
          chat_name: "Hit chat",
          card_index: 3,
          role: "user",
          snippet: "文件 search result",
        },
      ],
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Active chat");
    fireEvent.change(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
      {
        target: { value: "文件" },
      },
    );

    const drawerResult = await screen.findByText(
      (_, element) => element?.textContent === "文件 search result",
    );
    expect(chatApiMock.searchChats).toHaveBeenCalledWith("文件", "default");
    expect(screen.queryByText("Active chat")).not.toBeInTheDocument();

    fireEvent.click(drawerResult);
    expect(
      JSON.parse(
        sessionStorage.getItem("rua_search_scroll_target") ?? "null",
      ),
    ).toMatchObject({
      cardIndex: 3,
      query: "文件",
      conversationId: "chat-hit",
      ownerKey: "default",
    });
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/chat-hit",
      ),
    );
  });

  it("resets history drawer search when the drawer is reopened", async () => {
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-1",
          user_id: "default",
          channel: "console",
          name: "Active chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });
    chatApiMock.searchChats.mockResolvedValue({
      results: [
        {
          chat_id: "chat-hit",
          chat_name: "Hit chat",
          card_index: 3,
          role: "assistant",
          snippet: "needle result",
        },
      ],
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Active chat");
    fireEvent.change(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
      {
        target: { value: "needle" },
      },
    );

    const result = await screen.findByText(
      (_, element) => element?.textContent === "needle result",
    );
    fireEvent.click(result);
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/chat-hit",
      ),
    );

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Active chat");

    expect(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
    ).toHaveValue("");
    expect(chatApiMock.searchChats).toHaveBeenCalledWith("needle", "default");
    expect(chatApiMock.listChats).toHaveBeenCalledTimes(2);
  });

  it("shows the owned no-results state for an empty chat search response", async () => {
    chatApiMock.searchChats.mockResolvedValue({ results: [] });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    fireEvent.change(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
      {
        target: { value: "missing" },
      },
    );

    await screen.findByText(label("chat.noSearchResults"));
    expect(chatApiMock.searchChats).toHaveBeenCalledWith("missing", "default");
  });

  it("ignores stale chat search responses when the query changes", async () => {
    let resolveOld!: (value: unknown) => void;
    let resolveFresh!: (value: unknown) => void;
    const oldSearch = new Promise((resolve) => {
      resolveOld = resolve;
    });
    const freshSearch = new Promise((resolve) => {
      resolveFresh = resolve;
    });
    chatApiMock.searchChats
      .mockReturnValueOnce(oldSearch)
      .mockReturnValueOnce(freshSearch);

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    fireEvent.change(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
      {
        target: { value: "old" },
      },
    );
    await waitFor(() =>
      expect(chatApiMock.searchChats).toHaveBeenCalledWith("old", "default"),
    );

    fireEvent.change(
      screen.getByPlaceholderText(label("chat.chatListSearchPlaceholder")),
      {
        target: { value: "fresh" },
      },
    );
    await waitFor(() =>
      expect(chatApiMock.searchChats).toHaveBeenCalledWith("fresh", "default"),
    );

    await act(async () => {
      resolveFresh({
        results: [
          {
            chat_id: "fresh-chat",
            chat_name: "Fresh chat",
            card_index: 2,
            role: "assistant",
            snippet: "fresh result",
          },
        ],
      });
    });
    await screen.findByText(
      (_, element) => element?.textContent === "fresh result",
    );

    await act(async () => {
      resolveOld({
        results: [
          {
            chat_id: "old-chat",
            chat_name: "Old chat",
            card_index: 1,
            role: "user",
            snippet: "old result",
          },
        ],
      });
    });

    expect(
      screen.queryByText((_, element) => element?.textContent === "old result"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText((_, element) => element?.textContent === "fresh result"),
    ).toBeInTheDocument();
  });

  it("exports the current chat through owned DOM state and backend history", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        { role: "assistant", type: "message", content: "Answer" },
      ],
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByLabelText(label("common.more")));
    fireEvent.click(await screen.findByText(label("chat.export.tooltip")));

    await waitFor(() =>
      expect(saveTextFileMock).toHaveBeenCalledWith(
        expect.stringContaining("Question"),
        "chat-1.md",
        "text/markdown;charset=utf-8",
      ),
    );
  });

  it("handles sidebar deletion only for the active Agent scope and replaces the deleted route with an active sibling", async () => {
    renderChat("/agents/default/chats/chat-1");
    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    const remaining = [
      {
        id: "archived-chat",
        archived: true,
        user_id: "default",
        channel: "console",
        created_at: null,
        updated_at: null,
      },
      {
        id: "chat-2",
        archived: false,
        user_id: "default",
        channel: "console",
        created_at: null,
        updated_at: null,
      },
    ];
    act(() => {
      expect(
        publishConversationDeleted(
          { scope: "project", projectId: "project-1", chatId: "chat-1" },
          remaining,
        ),
      ).toBe(false);
      expect(
        publishConversationDeleted(
          { scope: "agent", agentId: "other", chatId: "chat-1" },
          remaining,
        ),
      ).toBe(false);
      expect(
        publishConversationDeleted(
          { scope: "agent", agentId: "default", chatId: "other-chat" },
          remaining,
        ),
      ).toBe(false);
    });
    expect(screen.getByTestId("location")).toHaveTextContent(
      "/agents/default/chats/chat-1",
    );
    sessionStorage.setItem("rua_input_draft_default:chat-1", "deleted draft");
    act(() => {
      expect(
        publishConversationDeleted(
          { scope: "agent", agentId: "default", chatId: "chat-1" },
          remaining,
        ),
      ).toBe(true);
    });
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/chat-2",
      ),
    );
    expect(screen.getByTestId("location")).toHaveAttribute(
      "data-navigation-type",
      "REPLACE",
    );
    expect(
      sessionStorage.getItem("rua_input_draft_default:chat-1"),
    ).toBeNull();
    expect(chatApiMock.deleteChat).not.toHaveBeenCalled();
  });

  it("deletes the active chat by URL id and advances to a fresh lazy chat", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-after-delete");
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-1",
          user_id: "default",
          channel: "console",
          name: "Active chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Active chat");
    fireEvent.click(screen.getByLabelText(label("common.delete")));
    const dialog = (
      await screen.findByText(
        label("chat.deleteConfirm", { name: "Active chat" }),
      )
    ).closest('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(
      within(dialog!).queryByText(i18n.getFixedT("en")("common.delete")),
    ).not.toBeInTheDocument();
    fireEvent.click(
      within(dialog!).getByRole("button", { name: label("common.delete") }),
    );

    await waitFor(() =>
      expect(chatApiMock.deleteChat).toHaveBeenCalledWith("chat-1", "default"),
    );
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/lazy-after-delete",
      ),
    );
    expect(window.confirm).not.toHaveBeenCalled();
  });

  it("deletes the active chat and falls back to the next known recent chat", async () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("lazy-after-delete");
    localStorage.setItem("rua_last_chat_id_default", "active-chat");
    localStorage.setItem(
      "rua_recent_chat_ids_default",
      JSON.stringify(["active-chat", "older-chat"]),
    );
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "active-chat",
          user_id: "default",
          channel: "console",
          name: "Active chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/active-chat");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Active chat");
    fireEvent.click(screen.getByLabelText(label("common.delete")));
    const dialog = (
      await screen.findByText(
        label("chat.deleteConfirm", { name: "Active chat" }),
      )
    ).closest('[role="dialog"]');
    expect(dialog).not.toBeNull();
    fireEvent.click(
      within(dialog!).getByRole("button", { name: label("common.delete") }),
    );

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/agents/default/chats/older-chat",
      ),
    );
    expect(randomUUID).not.toHaveBeenCalled();
  });

  it("renames chats through the owned modal instead of a browser prompt", async () => {
    const update = deferred<never>();
    chatApiMock.updateChat.mockReturnValue(update.promise);
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-1",
          user_id: "default",
          channel: "console",
          name: "Active chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Active chat");
    fireEvent.click(screen.getByLabelText(label("common.rename")));
    const input = await screen.findByLabelText(label("chat.renameChatInput"));
    fireEvent.change(input, { target: { value: "Renamed chat" } });
    fireEvent.click(screen.getByRole("button", { name: label("common.save") }));

    expect((await screen.findAllByText("Renamed chat")).length).toBeGreaterThan(
      0,
    );
    expect(
      screen.queryByLabelText(label("chat.renameChatInput")),
    ).not.toBeInTheDocument();
    await waitFor(() =>
      expect(chatApiMock.updateChat).toHaveBeenCalledWith(
        "chat-1",
        { name: "Renamed chat" },
        "default",
      ),
    );
    await act(async () => update.reject(new Error("offline")));
    expect((await screen.findAllByText("Active chat")).length).toBeGreaterThan(
      1,
    );
    expect(window.prompt).not.toHaveBeenCalled();
  });

  it("confirms drawer chat rename with Enter", async () => {
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-1",
          user_id: "default",
          channel: "console",
          name: "Active chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Active chat");
    fireEvent.click(screen.getByLabelText(label("common.rename")));
    const input = await screen.findByLabelText(label("chat.renameChatInput"));
    fireEvent.change(input, { target: { value: "Renamed by Enter" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() =>
      expect(chatApiMock.updateChat).toHaveBeenCalledWith(
        "chat-1",
        { name: "Renamed by Enter" },
        "default",
      ),
    );
  });

  it("cancels drawer chat rename with Escape", async () => {
    chatApiMock.listChats.mockResolvedValue({
      data: [
        {
          id: "chat-1",
          user_id: "default",
          channel: "console",
          name: "Active chat",
          created_at: "2026-06-18T00:00:00Z",
          updated_at: "2026-06-18T00:00:00Z",
        },
      ],
      total: 1,
    });

    renderChat("/agents/default/chats/chat-1");

    fireEvent.click(
      screen.getByLabelText(label("navigation.manageConversations")),
    );
    await screen.findByText("Active chat");
    fireEvent.click(screen.getByLabelText(label("common.rename")));
    const input = await screen.findByLabelText(label("chat.renameChatInput"));
    const drawer = screen.getByText("Active chat").closest('[role="dialog"]');
    expect(drawer).not.toBeNull();
    expect(drawer).toHaveTextContent(label("navigation.manageConversations"));
    const drawerClose = within(drawer!).getByRole("button", { name: "Close" });
    fireEvent.change(input, { target: { value: "Should not save" } });
    fireEvent.keyDown(document, { key: "Escape" });

    await waitFor(() =>
      expect(
        screen.queryByLabelText(label("chat.renameChatInput")),
      ).not.toBeInTheDocument(),
    );
    expect(drawerClose).toBeInTheDocument();
    expect(screen.getByText("Active chat")).toBeInTheDocument();
    expect(chatApiMock.updateChat).not.toHaveBeenCalled();
  });
});
