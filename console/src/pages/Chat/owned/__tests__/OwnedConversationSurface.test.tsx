import { __resetConversationViewportForTests } from "../useConversationViewport";
import { useState } from "react";
import type { ChatHistory } from "@/api/types";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearByPrefix as clearDrafts, save as saveDraft } from "../../drafts";
import { MemoryRouter, useLocation } from "react-router-dom";
import OwnedConversationSurface, {
  PendingQueue,
} from "../OwnedConversationSurface";
import { __resetChatViewCacheForTests } from "../useConversationViewRuntime";
import { useAgentStore } from "@/stores/agentStore";
import { ModalProvider } from "@/ui";
import { createCanonicalStreamFixture } from "./canonicalMessageFixtures";

const mockGetChat = vi.fn();
const mockStopChat = vi.fn();
const mockUploadFile = vi.fn();
const mockGetApprovalSnapshot = vi.fn();
const mockDecideApproval = vi.fn();
const mockGetAcpPermissionSnapshot = vi.fn();
const mockDecideAcpPermission = vi.fn();
const openTestStreams = new Set<ReadableStreamDefaultController<Uint8Array>>();

const configuredModel = {
  provider_id: "dashscope",
  model: "qwen-plus",
};

function LocationProbe() {
  return <output data-testid="source-route">{useLocation().pathname}</output>;
}

function idleHistory() {
  return { status: "idle" as const, messages: [], model: configuredModel };
}

function backgroundWorkControl(
  backgroundWork: Record<string, unknown>,
  activeTurn: Record<string, unknown> | null = null,
  stopAllowed = Boolean(activeTurn),
) {
  return {
    revision: 1,
    authority_device_id: "device-local",
    placement: {
      authority_device_id: "device-local",
      authority_epoch: "epoch-local",
      switchable: true,
      lock_reason: activeTurn ? "active_turn" : null,
    },
    active_turn: activeTurn,
    queue: { entries: [], paused: false },
    acp: { active: false, binding: null },
    allowed_controls: {
      stop: stopAllowed,
      queue: true,
      interject: false,
      edit_queued_input: true,
      delete_queued_input: true,
      switch_authority: true,
      enter_acp: true,
      exit_acp: false,
    },
    background_work: backgroundWork,
  };
}

vi.mock("@/hooks/useSlashCommands", () => ({
  useSlashCommands: () => [
    {
      command: "/hello",
      value: "hello",
      description: "Say hello",
    },
  ],
}));

vi.mock("@/hooks/useTTS", () => ({
  useTTS: () => ({
    speak: vi.fn(),
    stop: vi.fn(),
    isSpeaking: false,
    isLoading: false,
    isDisabled: true,
    error: null,
  }),
}));

vi.mock("@/api/modules/chat", () => ({
  chatApi: {
    getChat: (...args: unknown[]) => mockGetChat(...args),
    submitPath: (chatId: string, agentId: string) =>
      `/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
        chatId,
      )}/console/submit`,
    livePath: (chatId: string, agentId: string) =>
      `/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
        chatId,
      )}/console/live`,
    stopChat: (...args: unknown[]) => mockStopChat(...args),
    selectVariant: async (
      chatId: string,
      turnId: string,
      variantId: string,
      agentId: string,
    ) => {
      await fetch(
        `/api/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
          chatId,
        )}`,
        {
          method: "PUT",
          body: JSON.stringify({
            turn_id: turnId,
            selected_variant_id: variantId,
          }),
        },
      );
      return { id: chatId };
    },
    rollback: (
      chatId: string,
      body: { before_turn_id: string },
      agentId: string,
    ) =>
      fetch(
        `/api/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
          chatId,
        )}/rollback`,
        { method: "POST", body: JSON.stringify(body) },
      ).then((response) => response.json()),
    branch: (
      chatId: string,
      body: { before_turn_id: string },
      agentId: string,
    ) =>
      fetch(
        `/api/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
          chatId,
        )}/branch`,
        { method: "POST", body: JSON.stringify(body) },
      ).then((response) => response.json()),
    uploadFile: (...args: unknown[]) => mockUploadFile(...args),
    mediaUrl: (_scope: unknown, _chatId: string, mediaId: string) =>
      `/api/agents/default/chats/chat-1/media/${mediaId}`,
    filePreviewUrl: (path: string) => `/preview${path}`,
  },
}));

vi.mock("@/api/modules/console", () => ({
  consoleApi: {
    getApprovalSnapshot: (...args: unknown[]) =>
      mockGetApprovalSnapshot(...args),
    decideApproval: (...args: unknown[]) => mockDecideApproval(...args),
  },
}));

vi.mock("@/api/modules/acp", () => ({
  acpApi: {
    getPermissionSnapshot: (...args: unknown[]) =>
      mockGetAcpPermissionSnapshot(...args),
    decidePermission: (...args: unknown[]) => mockDecideAcpPermission(...args),
  },
}));

vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({}),
}));

function sseResponse(payloads: unknown[]): Response {
  const encoder = new TextEncoder();
  const fixture = createCanonicalStreamFixture();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const payload of payloads) {
        controller.enqueue(
          encoder.encode(
            `data: ${JSON.stringify(fixture.normalize(payload))}\n\n`,
          ),
        );
      }
      controller.close();
    },
  });
  return new Response(body, {
    status: 200,
    headers: { "Content-Type": "text/event-stream" },
  });
}

function heldSseResponse(
  payloads: unknown[],
  closeSignal: Promise<unknown>,
): Response {
  const encoder = new TextEncoder();
  const fixture = createCanonicalStreamFixture();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const payload of payloads) {
        controller.enqueue(
          encoder.encode(
            `data: ${JSON.stringify(fixture.normalize(payload))}\n\n`,
          ),
        );
      }
      closeSignal.finally(() => controller.close());
    },
  });
  return new Response(body, {
    status: 200,
    headers: { "Content-Type": "text/event-stream" },
  });
}

function trackedSseResponse() {
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const fixture = createCanonicalStreamFixture();
  const body = new ReadableStream<Uint8Array>({
    start(streamController) {
      controller = streamController;
      openTestStreams.add(streamController);
    },
    cancel() {
      openTestStreams.delete(controller);
    },
  });
  return {
    body,
    response: new Response(body, {
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
    }),
    controller: () => ({
      enqueue(chunk: Uint8Array) {
        const normalized = decoder
          .decode(chunk)
          .split("\n")
          .map((line) => {
            if (!line) return line;
            const prefix = line.startsWith("data: ") ? "data: " : "";
            const payload = prefix ? line.slice(prefix.length) : line;
            try {
              return `${prefix}${JSON.stringify(
                fixture.normalize(JSON.parse(payload)),
              )}`;
            } catch {
              return line;
            }
          })
          .join("\n");
        controller.enqueue(encoder.encode(normalized));
      },
    }),
    send(payload: unknown) {
      controller.enqueue(
        encoder.encode(
          `data: ${JSON.stringify(fixture.normalize(payload))}\n\n`,
        ),
      );
    },
    close() {
      if (openTestStreams.delete(controller)) {
        controller.close();
      }
    },
  };
}

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function submitAcceptedResponse(overrides: Record<string, unknown> = {}) {
  return jsonResponse({ accepted: true, mode: "started", ...overrides });
}

type FetchRouteHandler = (
  url: string,
  init?: RequestInit,
) => Response | Promise<Response> | undefined;

function routeFetch(handler?: FetchRouteHandler) {
  return vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const handled = handler?.(url, init);
    if (handled) return Promise.resolve(handled);
    if (url.includes("/console/submit")) {
      return Promise.resolve(submitAcceptedResponse());
    }
    if (url.includes("/console/live")) {
      return Promise.resolve(
        sseResponse([{ object: "response", status: "completed", output: [] }]),
      );
    }
    if (url.includes("/models/active")) {
      return Promise.resolve(
        jsonResponse({
          default_llm: { provider_id: "dashscope", model: "qwen-plus" },
        }),
      );
    }
    if (url.includes("/sync/cycle/target")) {
      return Promise.resolve(jsonResponse({ ok: true }));
    }
    if (url.includes("/sync/repair/scan")) {
      return Promise.resolve(jsonResponse({ ok: true }));
    }
    return Promise.resolve(new Response(null, { status: 200 }));
  });
}

function submittedChatBody(fetchMock: ReturnType<typeof vi.fn>) {
  const call = fetchMock.mock.calls.find(([url]) =>
    String(url).includes("/console/submit"),
  );
  if (!call) throw new Error("missing console chat submit request");
  return JSON.parse(String(call[1]?.body || "{}"));
}

function submitCalls(fetchMock: ReturnType<typeof vi.fn>) {
  return fetchMock.mock.calls.filter(([url]) =>
    String(url).includes("/console/submit"),
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("OwnedConversationSurface", () => {
  describe("pending queue editor", () => {
    it("edits a server queued message in a compact modal with Enter", async () => {
      const onEdit = vi.fn().mockResolvedValue(undefined);
      render(
        <PendingQueue
          items={[
            {
              id: "input-1",
              text: "First line\nSecond line",
              attachments: [],
              serverQueued: true,
              editable: true,
              deletable: true,
            },
          ]}
          onSendNow={vi.fn()}
          onReclaim={vi.fn()}
          onEdit={onEdit}
          onCancel={vi.fn()}
        />,
      );
      fireEvent.click(screen.getByRole("button", { name: "编辑" }));

      const editor = screen.getByRole("textbox", { name: "排队消息内容" });
      expect(editor.tagName).toBe("TEXTAREA");
      expect(editor).toHaveAttribute("rows", "4");
      expect(editor).toHaveValue("First line\nSecond line");
      expect(editor).toHaveFocus();
      expect(screen.getByRole("heading", { name: "编辑消息" })).toBeVisible();
      expect(screen.getByRole("dialog")).toHaveStyle({ width: "480px" });

      fireEvent.change(editor, {
        target: { value: "Updated line one\nUpdated line two" },
      });
      fireEvent.keyDown(editor, { key: "Enter", shiftKey: true });
      expect(onEdit).not.toHaveBeenCalled();

      fireEvent.keyDown(editor, { key: "Enter", isComposing: true });
      expect(onEdit).not.toHaveBeenCalled();

      fireEvent.keyDown(editor, { key: "Enter" });

      await waitFor(() =>
        expect(onEdit).toHaveBeenCalledWith(
          "input-1",
          "Updated line one\nUpdated line two",
        ),
      );
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    });
  });

  beforeEach(() => {
    __resetConversationViewportForTests();
    vi.clearAllMocks();

    mockGetChat.mockReset();
    __resetChatViewCacheForTests();
    mockUploadFile.mockResolvedValue({
      media_ref: "media/00000000-0000-4000-8000-000000000001",
      media_id: "00000000-0000-4000-8000-000000000001",
      mime_type: "image/png",
      byte_size: 4,
      authority_epoch: "0000000000000001000000000000000000000000",
      original_filename: "picture.png",
    });
    mockGetApprovalSnapshot.mockResolvedValue({ pending: [], resolved: [] });
    mockGetAcpPermissionSnapshot.mockResolvedValue({ pending: [] });
    mockDecideApproval.mockResolvedValue({
      id: "approval-1",
      status: "approved",
    });
    mockDecideAcpPermission.mockResolvedValue({});
    localStorage.clear();
    clearDrafts("");
    sessionStorage.clear();
    document.documentElement.style.removeProperty("--rua-chat-visual-height");
    Object.defineProperty(window, "visualViewport", {
      value: undefined,
      configurable: true,
    });
    useAgentStore.setState({ selectedAgent: "default" });
    vi.stubGlobal("VITE_API_BASE_URL", "");
    vi.stubGlobal("TOKEN", "");
    vi.stubGlobal("fetch", routeFetch());
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => "uuid-1" },
      configurable: true,
    });
  });

  afterEach(() => {
    for (const controller of Array.from(openTestStreams)) {
      openTestStreams.delete(controller);
      try {
        controller.close();
      } catch (error) {
        if (!(error instanceof TypeError)) throw error;
      }
    }
    vi.unstubAllGlobals();
  });

  it("does not remeasure historical bubbles as the live reply grows", async () => {
    mockGetChat.mockResolvedValue({
      status: "running",
      model: configuredModel,
      active_turn: {
        turn_id: "turn-2",
        user_message_id: "user-2",
        phase: "streaming",
        authority_device_id: "device-a",
        driver: "rua",
        started_at: "2026-09-19T00:00:00Z",
        queued_count: 0,
      },
      messages: [
        {
          id: "user-1",
          turn_id: "turn-1",
          role: "user",
          content: "Earlier prompt",
        },
        {
          id: "answer-1",
          turn_id: "turn-1",
          role: "assistant",
          content: "Historical answer",
        },
        { id: "user-2", turn_id: "turn-2", role: "user", content: "Continue" },
      ],
    });
    const stream = trackedSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    const { container, unmount } = render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-perf" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );
    await screen.findByText("Historical answer");
    await act(async () => {
      stream.send({ type: "text_delta", delta: "First chunk" });
    });
    await screen.findByText("First chunk");
    const oldBubble = container.querySelector<HTMLElement>(
      ".rua-chat-bubble--assistant",
    )!;
    const measure = vi.spyOn(oldBubble, "getBoundingClientRect");
    try {
      await act(async () => {
        stream.send({ type: "text_delta", delta: " and the rest" });
      });
      await screen.findByText("First chunk and the rest");
      expect(measure).not.toHaveBeenCalled();
      expect(screen.getByText("Historical answer")).toBeInTheDocument();
    } finally {
      measure.mockRestore();
      unmount();
    }
  });

  it("toggles the composer expanded height from the input control", async () => {
    mockGetChat.mockResolvedValue(idleHistory());

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const input = await screen.findByPlaceholderText("Message");
    const composer = input.closest(".rua-owned-composer");
    expect(composer).not.toHaveClass("rua-owned-composer--expanded");

    fireEvent.click(screen.getByRole("button", { name: "Expand input" }));
    expect(composer).toHaveClass("rua-owned-composer--expanded");
    expect(
      screen.getByRole("button", { name: "Collapse input" }),
    ).toBeInTheDocument();
    await waitFor(() => expect(input).toHaveStyle({ height: "164px" }));

    fireEvent.click(screen.getByRole("button", { name: "Collapse input" }));
    expect(composer).not.toHaveClass("rua-owned-composer--expanded");
    expect(
      screen.getByRole("button", { name: "Expand input" }),
    ).toBeInTheDocument();
  });

  describe("background work status", () => {
    it("shows an explicit waiting indicator when idle with pending items", async () => {
      mockGetChat.mockResolvedValue({
        ...idleHistory(),
        control: backgroundWorkControl({
          suppressed: false,
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "waiting_approval",
              chat: {
                scope: { kind: "agent", agent_id: "default" },
                chat_id: "chat-1",
              },
            },
            {
              kind: "collaboration",
              id: "req-1",
              status: "delivery_failed",
              chat: {
                scope: { kind: "agent", agent_id: "default" },
                chat_id: "chat-other",
              },
            },
          ],
        }),
      });

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
            currentDeviceId="device-local"
          />
          <LocationProbe />
        </MemoryRouter>,
      );

      const status = await screen.findByRole("status", {
        name: "Waiting for background work (2)",
      });
      const strip = status.closest<HTMLElement>(".rua-owned-background-work")!;
      expect(strip).toHaveAttribute("data-rua-background-work", "waiting");
      expect(strip).toHaveClass("rua-owned-background-work--waiting");
      expect(within(strip).queryByRole("list")).toBeNull();
      fireEvent.click(
        within(strip).getByRole("button", { name: "Background work details" }),
      );
      expect(within(strip).getByText("Needs approval")).toBeInTheDocument();
      expect(within(strip).getByText("Delivery failed")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: /stop/i })).toBeNull();
      expect(screen.getByPlaceholderText("Message")).toBeEnabled();

      // Only the collaboration item in another chat is navigable.
      const navigable = within(strip).getAllByRole("button", {
        name: /Open conversation/,
      });
      expect(navigable).toHaveLength(1);
      fireEvent.click(navigable[0]);
      await waitFor(() =>
        expect(screen.getByTestId("source-route")).toHaveTextContent(
          "/agents/default/chats/chat-other",
        ),
      );
    });

    it("shows the parallel count without the wait list while a turn is active", async () => {
      mockGetChat.mockResolvedValue({
        ...idleHistory(),
        status: "running",
        active_turn: {
          turn_id: "turn-1",
          phase: "streaming",
          authority_device_id: "device-local",
          authority_epoch: "epoch-local",
          driver: "rua",
          started_at: "2026-10-27T00:00:00Z",
        },
        control: backgroundWorkControl(
          {
            suppressed: false,
            items: [
              {
                kind: "task",
                id: "task-1",
                status: "running",
                chat: {
                  scope: { kind: "agent", agent_id: "default" },
                  chat_id: "chat-1",
                },
              },
            ],
          },
          {
            turn_id: "turn-1",
            phase: "streaming",
            authority_device_id: "device-local",
            authority_epoch: "epoch-local",
            driver: "rua",
            started_at: "2026-10-27T00:00:00Z",
          },
        ),
      });

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
            currentDeviceId="device-local"
          />
        </MemoryRouter>,
      );

      const status = await screen.findByRole("status", {
        name: "Background work running in parallel (1)",
      });
      const strip = status.closest<HTMLElement>(".rua-owned-background-work")!;
      expect(strip).toHaveAttribute("data-rua-background-work", "parallel");
      expect(strip.querySelector(".rua-owned-background-work-list")).toBeNull();
    });

    it("reports suppressed work as stopped auto-resume and snapshot errors as unavailable", async () => {
      mockGetChat.mockResolvedValue({
        ...idleHistory(),
        control: backgroundWorkControl({
          suppressed: true,
          items: [],
          error: "task store locked",
        }),
      });

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
            currentDeviceId="device-local"
          />
        </MemoryRouter>,
      );

      const status = await screen.findByRole("status");
      const strip = status.closest<HTMLElement>(".rua-owned-background-work")!;
      expect(status).toHaveTextContent("Auto-resume stopped");
      expect(status).toHaveTextContent("Background work unavailable");
      expect(status).toHaveTextContent("task store locked");
      expect(strip).toHaveClass("rua-owned-background-work--error");
    });

    it("renders no indicator when the control snapshot has no background work", async () => {
      mockGetChat.mockResolvedValue({
        ...idleHistory(),
        messages: [
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: [
              {
                type: "text",
                text: "Waiting for background work, result ready soon",
              },
            ],
          },
        ],
        control: backgroundWorkControl({ suppressed: false, items: [] }),
      });

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
            currentDeviceId="device-local"
          />
        </MemoryRouter>,
      );

      // The assistant text mentions waiting, but the indicator must not infer it.
      await screen.findByText(/Waiting for background work, result ready/);
      expect(document.querySelector(".rua-owned-background-work")).toBeNull();
    });

    it("does not expose stopping automatic continuation from idle background work", async () => {
      mockStopChat.mockResolvedValue({ stopped: true });
      mockGetChat.mockResolvedValue({
        ...idleHistory(),
        control: backgroundWorkControl(
          {
            suppressed: false,
            items: [
              {
                kind: "task",
                id: "task-1",
                status: "running",
                chat: {
                  scope: { kind: "agent", agent_id: "default" },
                  chat_id: "chat-1",
                },
              },
            ],
          },
          null,
          true,
        ),
      });

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
            currentDeviceId="device-local"
          />
        </MemoryRouter>,
      );

      await screen.findByRole("status", {
        name: "Waiting for background work (1)",
      });
      expect(screen.queryByRole("button", { name: /stop/i })).toBeNull();
      expect(mockStopChat).not.toHaveBeenCalled();
    });

    it("hides the idle stop action once continuation is already suppressed", async () => {
      mockGetChat.mockResolvedValue({
        ...idleHistory(),
        control: backgroundWorkControl(
          {
            suppressed: true,
            items: [
              {
                kind: "task",
                id: "task-1",
                status: "running",
                chat: {
                  scope: { kind: "agent", agent_id: "default" },
                  chat_id: "chat-1",
                },
              },
            ],
          },
          null,
          true,
        ),
      });

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
            currentDeviceId="device-local"
          />
        </MemoryRouter>,
      );

      const status = await screen.findByRole("status", {
        name: "Waiting for background work (1), Auto-resume stopped",
      });
      const strip = status.closest<HTMLElement>(".rua-owned-background-work")!;
      expect(
        within(status).queryByRole("button", {
          name: /Stop automatic continuation/,
        }),
      ).toBeNull();
      expect(
        within(strip).getByText(/Auto-resume stopped/),
      ).toBeInTheDocument();
    });
  });

  it("shows only actionable collaboration status while keeping direct input available", async () => {
    mockGetChat.mockResolvedValue({
      ...idleHistory(),
      meta: {
        collaboration: {
          source: {
            scope: { kind: "agent", agent_id: "default" },
            chat_id: "source",
          },
          executor: { kind: "acp", runner: "mock_agent" },
          executor_agent_id: "default",
          configuration: {
            desired_source_epoch: "epoch-2",
            applied_source_epoch: "epoch-2",
            status: "ready",
          },
          source_available: false,
        },
      },
    });
    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "child-chat" }}
          chatModel={configuredModel}
        />
        <LocationProbe />
      </MemoryRouter>,
    );

    const relationship = await screen.findByRole("complementary", {
      name: "Collaboration relationship",
    });
    expect(relationship).toHaveTextContent("Source unavailable");
    expect(relationship).not.toHaveTextContent("ACP · mock_agent");
    expect(relationship).not.toHaveTextContent("Fixed executor");
    expect(relationship).not.toHaveTextContent("epoch-2");
    expect(screen.getByPlaceholderText("Message")).toBeEnabled();
    expect(
      screen.queryByRole("button", { name: /switch executor/i }),
    ).toBeNull();
  });

  it("preserves the loaded collaboration model through parent history feedback", async () => {
    const initialHistory: ChatHistory = {
      ...idleHistory(),
      meta: {
        collaboration: {
          source: {
            scope: { kind: "project", project_id: "project-1" },
            chat_id: "source",
          },
          executor: { kind: "agent" },
          executor_agent_id: "implementation",
          configuration: {
            desired_source_epoch: "epoch-1",
            applied_source_epoch: "epoch-1",
            status: "ready",
          },
        },
      },
    };
    mockGetChat.mockResolvedValue(initialHistory);
    function ConversationPage() {
      const [history, setHistory] = useState<ChatHistory | null>(
        initialHistory,
      );
      return (
        <>
          <output data-testid="selected-model">{history?.model?.model}</output>
          <OwnedConversationSurface
            address={{
              scope: "agent",
              agentId: "implementation",
              chatId: "child-chat",
            }}
            chatModel={history === null ? undefined : history.model ?? null}
            onHistoryChange={setHistory}
          />
        </>
      );
    }
    render(
      <MemoryRouter>
        <ConversationPage />
      </MemoryRouter>,
    );
    await screen.findByRole("complementary", {
      name: "Collaboration relationship",
    });
    expect(screen.getByTestId("selected-model")).toHaveTextContent("qwen-plus");
  });

  it("shows source navigation but no configuration identifiers for healthy collaboration", async () => {
    mockGetChat.mockResolvedValue({
      ...idleHistory(),
      meta: {
        collaboration: {
          source: {
            scope: { kind: "agent", agent_id: "default" },
            chat_id: "source",
          },
          executor: { kind: "acp", runner: "mock_agent" },
          executor_agent_id: "default",
          configuration: {
            desired_source_epoch: "epoch-2",
            applied_source_epoch: "epoch-2",
            status: "ready",
          },
          source_available: true,
        },
      },
    });
    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "child-chat" }}
          chatModel={configuredModel}
        />
        <LocationProbe />
      </MemoryRouter>,
    );

    await screen.findByPlaceholderText("Message");
    const relation = screen.getByRole("complementary", {
      name: "Collaboration relationship",
    });
    expect(relation).not.toHaveTextContent("epoch-2");
    expect(
      screen.getByRole("button", { name: "Back to source conversation" }),
    ).toBeEnabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Back to source conversation" }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("source-route")).toHaveTextContent(
        "/agents/default/chats/source",
      ),
    );
  });

  it("submits, streams output, and clears generating state without refresh", async () => {
    mockGetChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValue({
        status: "idle",
        model: configuredModel,
        messages: [
          {
            id: "user-1",
            role: "user",
            content: [{ type: "text", text: "Hello" }],
          },
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "Hello done" }],
          },
        ],
      });
    const encoder = new TextEncoder();
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );
    await act(async () => {});

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Hello" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));

    live.controller().enqueue(
      encoder.encode(
        `data: ${JSON.stringify({
          type: "user_message",
          message: {
            id: "user-1",
            role: "user",
            content: [{ type: "text", text: "Hello" }],
          },
        })}\n\n`,
      ),
    );
    live.controller().enqueue(
      encoder.encode(
        `data: ${JSON.stringify({
          object: "response",
          status: "completed",
          output: [
            {
              id: "assistant-1",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "Hello done" }],
            },
          ],
        })}\n\n`,
      ),
    );
    live.close();

    await screen.findByText("Hello done");
    expect(screen.queryByText("Generating")).not.toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/agents/default/chats/chat-1/console/submit",
      expect.objectContaining({
        method: "POST",
      }),
    );
    const body = submittedChatBody(fetchMock);
    expect(body.chat_id).toBe("chat-1");
    expect(body.input[0].content[0].text).toBe("Hello");
    expect(body.regenerate_turn_id).toBeUndefined();
  });

  it("reconciles a terminal-only automatic continuation without a new user message", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const live = trackedSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.endsWith("/console/live") ? live.response : undefined,
      ),
    );
    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
        />
      </MemoryRouter>,
    );
    await screen.findByPlaceholderText("Message");
    await waitFor(() => expect(fetch).toHaveBeenCalled());
    mockGetChat.mockResolvedValue({
      ...idleHistory(),
      messages: [
        {
          id: "automatic-answer",
          role: "assistant",
          type: "message",
          turn_id: "auto-turn",
          status: "completed",
          content: [{ type: "text", text: "Automatic continuation result" }],
        },
      ],
    });
    await act(async () => {
      live.send({
        type: "turn_terminal",
        turn_id: "auto-turn",
        status: "completed",
      });
    });
    expect(
      await screen.findByText("Automatic continuation result"),
    ).toBeInTheDocument();
  });

  it("submits without treating the viewer model snapshot as authority", async () => {
    mockGetChat.mockResolvedValue({ status: "idle", messages: [] });
    const fetchMock = routeFetch();
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
        />
      </MemoryRouter>,
    );

    const input = await screen.findByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Do not lose this" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));

    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
    await waitFor(() => expect(input).toHaveValue(""));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    const body = submittedChatBody(fetchMock);
    expect(body.model).toBeUndefined();
    expect(body.chat_model_slot).toBeUndefined();
  });

  it("opens address-based reads and live activity while preserving the draft model", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const fetchMock = routeFetch();
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "lazy-chat" }}
          unmaterialized
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const input = await screen.findByPlaceholderText("Message");
    await waitFor(() =>
      expect(mockGetChat).toHaveBeenCalledWith("lazy-chat", "default", {
        turnLimit: 20,
      }),
    );
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(
          ([input, init]) =>
            String(input).includes(
              "/api/agents/default/chats/lazy-chat/console/live",
            ) && JSON.parse(String(init?.body)).chat_id === "lazy-chat",
        ),
      ).toBe(true),
    );
    fireEvent.change(input, { target: { value: "Use the inherited model" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));

    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
    expect(submittedChatBody(fetchMock).model).toEqual(configuredModel);
  });

  it("keeps submission enabled before remote conversation control arrives", async () => {
    mockGetChat.mockResolvedValue({
      messages: [],
      model: configuredModel,
    });
    vi.stubGlobal("fetch", routeFetch());

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          authorityPlacementDeviceId="device-remote"
        />
      </MemoryRouter>,
    );

    const input = await screen.findByPlaceholderText("Message");
    expect(input).not.toBeDisabled();
  });

  it("does not let the submit body override backend authority placement", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const fetchMock = routeFetch((url) =>
      url.includes("/console/submit")
        ? submitAcceptedResponse({
            mode: "started",
            device_delivery_id: "device-delivery-1",
            target_device_id: "device-remote",
            device_delivery_sequence: 1,
            accepted_at: "2026-06-27T00:00:00Z",
          })
        : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
          authorityPlacementDeviceId="device-remote"
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Run there" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    const body = submittedChatBody(fetchMock);
    expect(body.authority_placement).toBeUndefined();
    await screen.findByText("Delivered, waiting for result");
    expect(
      JSON.parse(
        localStorage.getItem("rua:remote-pending-submissions:default:chat-1") ||
          "[]",
      )[0].device_delivery_id,
    ).toBe("device-delivery-1");
  });

  it.each([
    {
      placement: "known local target",
      target: "device-local",
      current: "device-local",
      remote: false,
    },
    {
      placement: "known remote target",
      target: "device-remote",
      current: "device-local",
      remote: true,
    },
    {
      placement: "unselected draft target",
      target: null,
      current: "device-local",
      remote: false,
    },
    {
      placement: "unresolved current identity",
      target: "device-remote",
      current: null,
      remote: false,
    },
  ])(
    "uses pending delivery placement for $placement",
    async ({ target, current, remote }) => {
      mockGetChat.mockResolvedValue(idleHistory());
      const submitResponse = deferred<Response>();
      const fetchMock = routeFetch((url) =>
        url.includes("/console/submit") ? submitResponse.promise : undefined,
      );
      vi.stubGlobal("fetch", fetchMock);

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
            authorityPlacementDeviceId={target}
            currentDeviceId={current}
          />
        </MemoryRouter>,
      );

      const input = await screen.findByPlaceholderText("Message");
      await waitFor(() => expect(input).not.toBeDisabled());
      fireEvent.change(input, { target: { value: "placement pending input" } });
      fireEvent.keyDown(input, { key: "Enter" });
      await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
      await screen.findByText("placement pending input");

      try {
        if (remote) {
          expect(
            screen.getByText("Delivering to remote device..."),
          ).toBeInTheDocument();
        } else {
          expect(
            screen.queryByText("Delivering to remote device..."),
          ).not.toBeInTheDocument();
        }
      } finally {
        await act(async () => {
          submitResponse.resolve(
            submitAcceptedResponse(
              remote
                ? {
                    mode: "started",
                    device_delivery_id: "device-delivery-placement",
                    target_device_id: "device-remote",
                    device_delivery_sequence: 1,
                    accepted_at: "2026-06-27T00:00:00Z",
                  }
                : {},
            ),
          );
        });
      }

      if (remote) {
        await screen.findByText("Delivered, waiting for result");
      }
    },
  );

  it("shows remote delivery progress after the target acknowledges", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const fetchMock = routeFetch((url) =>
      url.includes("/console/submit")
        ? submitAcceptedResponse({
            mode: "started",
            device_delivery_id: "device-delivery-1",
            target_device_id: "device-remote",
            device_delivery_sequence: 1,
            accepted_at: "2026-06-27T00:00:00Z",
          })
        : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
          authorityPlacementDeviceId="device-remote"
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Run there" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await screen.findByText("Delivered, waiting for result");
  });

  it("clears remote pending once the official synced turn arrives", async () => {
    mockGetChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValue({
        status: "idle",
        model: configuredModel,
        messages: [
          {
            id: "official-user-1",
            role: "user",
            content: [{ type: "text", text: "Run there" }],
            metadata: {
              device_delivery_id: "device-delivery-1",
            },
          },
          {
            id: "official-assistant-1",
            role: "assistant",
            content: [{ type: "text", text: "Done there" }],
          },
        ],
        meta: {
          authority_placement: { device_id: "device-remote" },
        },
      });
    const fetchMock = routeFetch((url) =>
      url.includes("/console/submit")
        ? submitAcceptedResponse({
            mode: "started",
            device_delivery_id: "device-delivery-1",
            target_device_id: "device-remote",
            device_delivery_sequence: 1,
            accepted_at: "2026-06-27T00:00:00Z",
          })
        : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
          authorityPlacementDeviceId="device-remote"
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Run there" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await waitFor(() =>
      expect(mockGetChat.mock.calls.length).toBeGreaterThanOrEqual(2),
    );
    await waitFor(() =>
      expect(
        localStorage.getItem("rua:remote-pending-submissions:default:chat-1"),
      ).toBeNull(),
    );
    const fetchCalls = fetchMock.mock.calls.map(([url, init]) => ({
      url: String(url),
      init: init as RequestInit | undefined,
    }));
    const targetedSyncCall = fetchCalls.find((call) =>
      call.url.includes("/sync/cycle/target"),
    );
    expect(targetedSyncCall).toBeTruthy();
    expect(JSON.parse(String(targetedSyncCall?.init?.body || "{}"))).toEqual({
      target_device_id: "device-remote",
    });
    expect(
      fetchCalls.some((call) => call.url.includes("/sync/repair/scan")),
    ).toBe(false);
    expect(screen.queryByText("Delivered, waiting for result")).toBeNull();
  });

  it("allows follow-up input while a stale remote pending projection is display-only", async () => {
    mockGetChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [],
      meta: {
        authority_placement: { device_id: "device-remote" },
      },
    });
    localStorage.setItem(
      "rua:remote-pending-submissions:default:chat-1",
      JSON.stringify([
        {
          device_delivery_id: "device-delivery-1",
          target_device_id: "device-remote",
          device_delivery_sequence: 1,
          accepted_at: "2026-06-27T00:00:00Z",
          message: {
            id: "user-pending-1",
            role: "user",
            content: [{ type: "text", text: "First remote" }],
          },
        },
      ]),
    );
    const fetchMock = routeFetch();
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("Delivered, waiting for result");
    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Follow-up remote" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
    await waitFor(() => expect(input).toHaveValue(""));
    expect(screen.queryByText("Follow-up remote")).not.toBeInTheDocument();
  });

  it("reconnects and streams a target-side remote conversation input before the result is written", async () => {
    const completedHistory = deferred<{
      status: "idle";
      messages: Array<Record<string, unknown>>;
      meta: Record<string, unknown>;
    }>();
    const remoteUser = {
      id: "official-user-1",
      role: "user",
      content: [{ type: "text", text: "Run on target" }],
      metadata: {
        device_delivery_id: "device-delivery-1",
        source_device_id: "phone",
        target_device_id: "desktop",
      },
    };
    mockGetChat
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [remoteUser],
        meta: {
          authority_placement: { device_id: "desktop" },
        },
      })
      .mockReturnValue(completedHistory.promise);
    const streamClose = deferred<void>();
    const fetchMock = vi.fn().mockResolvedValue(
      heldSseResponse(
        [
          {
            type: "user_message",
            message: remoteUser,
          },
          {
            object: "response",
            status: "in_progress",
            id: "resp-chat-1",
            chat_id: "chat-1",
          },
          {
            object: "message",
            status: "in_progress",
            id: "assistant-live",
            type: "message",
            role: "assistant",
            content: null,
          },
          {
            object: "content",
            status: "in_progress",
            type: "text",
            delta: true,
            msg_id: "assistant-live",
            text: "Live target token",
          },
        ],
        streamClose.promise,
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
          authorityPlacementDeviceId="desktop"
          currentDeviceId="desktop"
        />
      </MemoryRouter>,
    );

    await screen.findByText("Run on target");
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/agents/default/chats/chat-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    await screen.findByText("Live target token");
    streamClose.resolve();

    completedHistory.resolve({
      status: "idle",
      model: configuredModel,
      messages: [
        remoteUser,
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Target result" }],
        },
      ],
      meta: {
        authority_placement: { device_id: "desktop" },
      },
    });
  });

  it("discovers target-side remote conversation inputs that arrive after the page is already open", async () => {
    const intervalCallbacks: Array<() => void> = [];
    const setIntervalSpy = vi
      .spyOn(window, "setInterval")
      .mockImplementation((handler: TimerHandler) => {
        if (typeof handler === "function") intervalCallbacks.push(handler);
        return intervalCallbacks.length as unknown as number;
      });
    const clearIntervalSpy = vi
      .spyOn(window, "clearInterval")
      .mockImplementation(() => undefined);
    try {
      const completedHistory = deferred<{
        status: "idle";
        messages: Array<Record<string, unknown>>;
        meta: Record<string, unknown>;
      }>();
      const oldHistory = {
        status: "idle",
        model: configuredModel,
        messages: [
          {
            id: "previous-assistant",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "Previous result" }],
          },
        ],
        meta: {
          authority_placement: { device_id: "desktop" },
        },
      };
      const remoteUser = {
        id: "official-user-1",
        role: "user",
        content: [{ type: "text", text: "Run after open" }],
        metadata: {
          device_delivery_id: "device-delivery-1",
          source_device_id: "phone",
          target_device_id: "desktop",
        },
      };
      mockGetChat
        .mockResolvedValueOnce(oldHistory)
        .mockResolvedValueOnce({
          status: "idle",
          model: configuredModel,
          messages: [remoteUser],
          meta: {
            authority_placement: { device_id: "desktop" },
          },
        })
        .mockReturnValueOnce(completedHistory.promise);
      const streamClose = deferred<void>();
      const fetchMock = vi.fn().mockResolvedValue(
        heldSseResponse(
          [
            {
              type: "user_message",
              message: remoteUser,
            },
            {
              object: "response",
              status: "in_progress",
              id: "resp-chat-1",
              chat_id: "chat-1",
            },
            {
              object: "message",
              status: "in_progress",
              id: "assistant-live",
              type: "message",
              role: "assistant",
              content: null,
            },
            {
              object: "content",
              status: "in_progress",
              type: "text",
              delta: true,
              msg_id: "assistant-live",
              text: "Live after open token",
            },
          ],
          streamClose.promise,
        ),
      );
      vi.stubGlobal("fetch", fetchMock);

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
            authorityPlacementDeviceId="desktop"
            currentDeviceId="desktop"
          />
        </MemoryRouter>,
      );

      await screen.findByText("Previous result");
      await waitFor(() => expect(intervalCallbacks.length).toBeGreaterThan(0));

      await act(async () => {
        intervalCallbacks.forEach((callback) => callback());
        await Promise.resolve();
      });
      await screen.findByText("Run after open");
      await waitFor(() =>
        expect(fetchMock).toHaveBeenCalledWith(
          "/api/agents/default/chats/chat-1/console/live",
          expect.objectContaining({ method: "POST" }),
        ),
      );
      await screen.findByText("Live after open token");
      streamClose.resolve();
    } finally {
      setIntervalSpy.mockRestore();
      clearIntervalSpy.mockRestore();
    }
  });

  it("keeps later queued input blocked after a remote delivery failure", async () => {
    mockGetChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [],
      meta: {
        authority_placement: { device_id: "device-remote" },
      },
    });
    const fetchMock = routeFetch((url) =>
      url.includes("/console/submit")
        ? submitAcceptedResponse({
            accepted: false,
            mode: "started",
            detail: "remote delivery failed",
          })
        : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
          authorityPlacementDeviceId="device-remote"
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "First remote" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "remote delivery failed",
    );

    fireEvent.change(input, { target: { value: "Second remote" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(2));
  });

  it("does not render the new-chat welcome while an existing conversation is loading", async () => {
    const pendingHistory = deferred<{
      status: string;
      messages: Array<{
        id: string;
        role: string;
        content: Array<{ type: string; text: string }>;
      }>;
    }>();
    mockGetChat.mockReturnValue(pendingHistory.promise);

    const { container } = render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    expect(container.querySelector("[data-rua-chat-empty]")).toBeNull();
    expect(container.querySelector(".rua-owned-surface-loading")).toBeTruthy();

    pendingHistory.resolve({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "Loaded question" }],
        },
      ],
    });

    await screen.findByText("Loaded question");
    expect(container.querySelector(".rua-owned-surface-loading")).toBeNull();
  });

  it("renders pending approvals in the composer tray without message-card actions", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    mockGetApprovalSnapshot.mockResolvedValue({
      pending: [
        {
          id: "approval-1",
          agent_id: "default",
          chat_id: "chat-1",
          turn_id: "turn-1",
          tool_call_id: "call-1",
          tool_name: "Bash",
          status: "pending",
          trigger: {
            source: "approval_mode",
            message: "Tool 'Bash' requires approval (Balanced)",
          },
          tool_call: {
            display: "du -sh /Users/example/projects/rua",
            args: { command: "du -sh /Users/example/projects/rua" },
          },
          created_at: "2026-06-21T00:00:00Z",
          reuse_options: [],
        },
      ],
      resolved: [],
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const tray = await screen.findByLabelText("Approval queue");
    expect(tray.tagName).toBe("DIV");
    expect(within(tray).getByText("Bash")).toBeInTheDocument();
    expect(within(tray).getByText("1 / 1")).toBeInTheDocument();
    expect(
      within(tray).getByText(
        "Balanced mode asks before shell, write/edit, MCP/plugin, and web tool calls.",
      ),
    ).toBeInTheDocument();
    expect(
      within(tray).queryByText("Tool 'Bash' requires approval (Balanced)"),
    ).not.toBeInTheDocument();
    expect(
      within(tray).getByRole("button", { name: "Run once" }),
    ).toBeInTheDocument();
    expect(
      within(tray).getByRole("button", { name: "Reject" }),
    ).toBeInTheDocument();
    expect(within(tray).queryByLabelText(/approval options/i)).toBeNull();
  });

  it("decides run-once approvals without reloading or replaying the chat", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    mockGetApprovalSnapshot.mockResolvedValue({
      pending: [
        {
          id: "approval-1",
          agent_id: "default",
          chat_id: "chat-1",
          turn_id: "turn-1",
          tool_call_id: "call-1",
          tool_name: "Bash",
          status: "pending",
          trigger: {
            source: "approval_mode",
            message: "Tool 'Bash' requires approval (Balanced)",
          },
          tool_call: {
            display: "Run git status --short",
            args: { command: "git status --short" },
          },
          created_at: "2026-06-21T00:00:00Z",
          reuse_options: [],
        },
      ],
      resolved: [],
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const tray = await screen.findByLabelText("Approval queue");
    fireEvent.click(within(tray).getByRole("button", { name: "Run once" }));

    await waitFor(() =>
      expect(mockDecideApproval).toHaveBeenCalledWith("default", "approval-1", {
        decision: "run_once",
        decided_by: "webui",
      }),
    );
    expect(mockGetChat).toHaveBeenCalledTimes(1);
  });

  it("renders and submits exact ACP runner permission options", async () => {
    mockGetChat.mockResolvedValue({
      ...idleHistory(),
      control: {
        ...backgroundWorkControl({}),
        acp: { active: true, binding: { runner: "codex" } },
      },
    });
    mockGetAcpPermissionSnapshot.mockResolvedValue({
      pending: [
        {
          id: "acp-permission-1",
          agent_id: "default",
          chat_id: "chat-1",
          session_id: "session-1",
          runner: "codex",
          tool_call_id: "call-1",
          tool_name: "mcp.knowledge.search",
          tool_input: {
            server: "knowledge",
            tool: "search",
            arguments: { query: "ACP" },
          },
          options: [
            {
              option_id: "allow_once",
              name: "Allow once",
              kind: "allow_once",
            },
            {
              option_id: "allow_session",
              name: "Allow for session",
              kind: "allow_always",
            },
            {
              option_id: "allow_always",
              name: "Always allow",
              kind: "allow_always",
            },
            {
              option_id: "decline",
              name: "Decline",
              kind: "reject_once",
            },
          ],
        },
      ],
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const tray = await screen.findByLabelText("Approval queue");
    expect(within(tray).getByText("ACP · codex")).toBeInTheDocument();
    expect(
      within(tray).queryByRole("button", { name: "Run once" }),
    ).not.toBeInTheDocument();
    expect(
      within(tray).queryByRole("button", { name: "Reject" }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      within(tray).getByRole("button", { name: "Allow for session" }),
    );

    await waitFor(() =>
      expect(mockDecideAcpPermission).toHaveBeenCalledWith(
        "default",
        "chat-1",
        "acp-permission-1",
        "allow_session",
      ),
    );
    expect(mockDecideApproval).not.toHaveBeenCalled();
  });

  it("renders approval reuse options as direct tray actions", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    mockGetApprovalSnapshot.mockResolvedValue({
      pending: [
        {
          id: "approval-1",
          agent_id: "default",
          chat_id: "chat-1",
          turn_id: "turn-1",
          tool_call_id: "call-1",
          tool_name: "Bash",
          status: "pending",
          trigger: {
            source: "approval_mode",
            message: "Tool 'Bash' requires approval (Balanced)",
          },
          tool_call: {
            display: "Run git status --short",
            args: { command: "git status --short" },
          },
          created_at: "2026-06-21T00:00:00Z",
          reuse_options: [
            {
              scope: "chat",
              label:
                "Allow Bash(du -sh /Users/example/projects/rua) in this chat",
              matcher: {
                type: "bash_command_pattern",
                pattern: "du -sh /Users/example/projects/rua",
              },
            },
            {
              scope: "agent",
              label: "Always allow Bash(du -sh *) for this agent",
              matcher: {
                type: "bash_command_pattern",
                pattern: "du -sh *",
              },
            },
          ],
        },
      ],
      resolved: [],
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const tray = await screen.findByLabelText("Approval queue");
    expect(within(tray).getByText("Allow in this chat")).toBeInTheDocument();
    expect(
      within(tray).getByText("Always allow for this agent"),
    ).toBeInTheDocument();
    expect(
      within(tray).getByRole("button", {
        name: "Always allow Bash(du -sh *) for this agent",
      }),
    ).toBeInTheDocument();
    fireEvent.click(
      within(tray).getByRole("button", {
        name: "Allow Bash(du -sh /Users/example/projects/rua) in this chat",
      }),
    );

    await waitFor(() =>
      expect(mockDecideApproval).toHaveBeenCalledWith("default", "approval-1", {
        decision: "allow_chat",
        decided_by: "webui",
        matcher: {
          type: "bash_command_pattern",
          pattern: "du -sh /Users/example/projects/rua",
        },
      }),
    );
  });

  it("advances the approval tray after deciding the active item", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    mockGetApprovalSnapshot.mockResolvedValue({
      pending: [
        {
          id: "approval-1",
          agent_id: "default",
          chat_id: "chat-1",
          turn_id: "turn-1",
          tool_call_id: "call-1",
          tool_name: "Bash",
          status: "pending",
          trigger: { source: "approval_mode", message: "First approval" },
          tool_call: {
            display: "Run first command",
            args: { command: "echo first" },
          },
          created_at: "2026-06-21T00:00:00Z",
          reuse_options: [],
        },
        {
          id: "approval-2",
          agent_id: "default",
          chat_id: "chat-1",
          turn_id: "turn-1",
          tool_call_id: "call-2",
          tool_name: "Read",
          status: "pending",
          trigger: { source: "file_guard", message: "Sensitive file read" },
          tool_call: {
            display: "Read ~/.ssh/id_rsa",
            args: { path: "~/.ssh/id_rsa" },
          },
          created_at: "2026-06-21T00:00:01Z",
          reuse_options: [],
        },
      ],
      resolved: [],
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    let tray = await screen.findByLabelText("Approval queue");
    expect(within(tray).getByText("1 / 2")).toBeInTheDocument();
    expect(within(tray).getByText("Bash")).toBeInTheDocument();

    fireEvent.click(within(tray).getByRole("button", { name: "Run once" }));

    await waitFor(() =>
      expect(mockDecideApproval).toHaveBeenCalledWith("default", "approval-1", {
        decision: "run_once",
        decided_by: "webui",
      }),
    );
    tray = await screen.findByLabelText("Approval queue");
    expect(within(tray).getByText("1 / 1")).toBeInTheDocument();
    expect(within(tray).getByText("Read")).toBeInTheDocument();
    expect(within(tray).queryByText("Bash")).not.toBeInTheDocument();
  });

  it("refreshes the approval tray when the active stream emits a pending approval", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    mockGetApprovalSnapshot
      .mockResolvedValueOnce({ pending: [], resolved: [] })
      .mockResolvedValueOnce({
        pending: [
          {
            id: "approval-1",
            agent_id: "default",
            chat_id: "chat-1",
            turn_id: "turn-1",
            tool_call_id: "call-1",
            tool_name: "Bash",
            status: "pending",
            trigger: {
              source: "approval_mode",
              message: "Tool 'Bash' requires approval (Balanced)",
            },
            tool_call: {
              display: "Run git status --short",
              args: { command: "git status --short" },
            },
            created_at: "2026-06-21T00:00:00Z",
            reuse_options: [],
          },
        ],
        resolved: [],
      });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live")
          ? sseResponse([
              {
                type: "tool_approval_request",
                id: "approval-1",
                object: "message",
                status: "in_progress",
                content: [
                  {
                    type: "tool_approval_request",
                    data: {
                      id: "approval-1",
                      approval_request_id: "approval-1",
                      call_id: "call-1",
                      name: "Bash",
                      prompt: "Tool 'Bash' requires approval (Balanced)",
                      status: "waiting",
                    },
                  },
                ],
              },
            ])
          : undefined,
      ),
    );

    const { container } = render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Run git status" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      const tray = screen.getByLabelText("Approval queue");
      expect(within(tray).getByText("Bash")).toBeInTheDocument();
      expect(
        within(tray).getByText("Run git status --short"),
      ).toBeInTheDocument();
    });
    expect(mockGetApprovalSnapshot.mock.calls.length).toBeGreaterThanOrEqual(2);
    const messageList = container.querySelector(".rua-chat-message-list");
    if (messageList) {
      expect(
        within(messageList as HTMLElement).queryByText(
          "Tool 'Bash' requires approval (Balanced)",
        ),
      ).toBeNull();
    }
  });

  it("refreshes the approval tray during reconnect without rendering approval messages as cards", async () => {
    mockGetChat
      .mockResolvedValueOnce({ status: "running", messages: [] })
      .mockResolvedValue({
        status: "idle",
        messages: [],
        model: configuredModel,
      });
    mockGetApprovalSnapshot
      .mockResolvedValueOnce({ pending: [], resolved: [] })
      .mockResolvedValue({
        pending: [
          {
            id: "approval-1",
            agent_id: "default",
            chat_id: "chat-1",
            turn_id: "turn-1",
            tool_call_id: "call-1",
            tool_name: "Bash",
            status: "pending",
            trigger: {
              source: "approval_mode",
              message: "Tool 'Bash' requires approval (Balanced)",
            },
            tool_call: {
              display: "du -sh /Users/example/projects/rua",
              args: { command: "du -sh /Users/example/projects/rua" },
            },
            created_at: "2026-06-21T00:00:00Z",
            reuse_options: [],
          },
        ],
        resolved: [],
      });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live")
          ? sseResponse([
              {
                type: "tool_approval_request",
                id: "approval-1",
                request_id: "approval-1",
                call_id: "call-1",
                tool_name: "Bash",
                prompt: "Tool 'Bash' requires approval (Balanced)",
              },
            ])
          : undefined,
      ),
    );

    const { container } = render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const tray = await screen.findByLabelText("Approval queue");
    expect(within(tray).getByText("Bash")).toBeInTheDocument();
    expect(
      within(tray).getByText("du -sh /Users/example/projects/rua"),
    ).toBeInTheDocument();
    const messageList = container.querySelector(".rua-chat-message-list");
    if (messageList) {
      expect(
        within(messageList as HTMLElement).queryByText(
          "Tool 'Bash' requires approval (Balanced)",
        ),
      ).toBeNull();
    }
  });

  it("shows an empty-output fallback when the stream completes without assistant text", async () => {
    mockGetChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValue({
        status: "idle",
        model: configuredModel,
        messages: [
          { id: "user-1", role: "user", content: "Hello" },
          { id: "assistant-1", role: "assistant", content: [] },
        ],
      });
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Hello" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
    live.controller().enqueue(
      new TextEncoder().encode(
        `data: ${JSON.stringify({
          type: "user_message",
          message: { id: "user-1", role: "user", content: "Hello" },
        })}\n\n`,
      ),
    );
    live.controller().enqueue(
      new TextEncoder().encode(
        `data: ${JSON.stringify({
          type: "turn_terminal",
          turn_id: "turn-1",
          status: "completed",
        })}\n\n`,
      ),
    );
    live.close();

    await screen.findByText("The assistant returned no output.");
    await waitFor(() =>
      expect(screen.queryByText("Generating")).not.toBeInTheDocument(),
    );
  });

  it("submits an empty-state prompt", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const encoder = new TextEncoder();
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByText("Let's start a new journey!"));
    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
    live.controller().enqueue(
      encoder.encode(
        `data: ${JSON.stringify({
          type: "user_message",
          message: {
            id: "user-1",
            role: "user",
            content: [{ type: "text", text: "Let's start a new journey!" }],
          },
        })}\n\n`,
      ),
    );
    live.controller().enqueue(
      encoder.encode(
        `data: ${JSON.stringify({
          object: "response",
          status: "completed",
          output: [
            {
              id: "assistant-1",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "Prompt answer" }],
            },
          ],
        })}\n\n`,
      ),
    );
    live.close();
    await screen.findByText("Prompt answer");

    const body = submittedChatBody(fetchMock);
    expect(body.input[0].content[0].text).toBe("Let's start a new journey!");
  });

  it("renders backend text_delta frames before the stream completes", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Hello" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));

    live.send({ type: "start", chat_id: "chat-1" });
    live.send({ type: "text_delta", delta: "Hel" });
    live.send({ type: "text_delta", delta: "lo" });

    await screen.findByText("Hello");

    live.send({ type: "done", finish_reason: "stop" });
    live.close();

    await waitFor(() =>
      expect(screen.queryByText("Generating")).not.toBeInTheDocument(),
    );
  });

  it("pauses auto-follow when the user scrolls up and resumes from the latest button", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);
    const scrollTo = vi.fn(function (
      this: HTMLDivElement,
      options?: ScrollToOptions,
    ) {
      if (typeof options?.top === "number") this.scrollTop = options.top;
    });
    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    Object.defineProperty(scroller, "scrollHeight", {
      value: 1000,
      configurable: true,
    });
    Object.defineProperty(scroller, "clientHeight", {
      value: 300,
      configurable: true,
    });
    scroller.scrollTo = scrollTo;

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Hello" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
    live.send({ type: "text_delta", delta: "Hel" });
    await screen.findByText("Hel");
    await waitFor(() => expect(scrollTo).toHaveBeenCalled());

    scroller.scrollTop = 100;
    fireEvent.scroll(scroller);
    const jumpToBottom = await screen.findByRole("button", {
      name: "到最下",
    });
    expect(jumpToBottom).toBeInTheDocument();
    expect(screen.queryByText("到最下")).not.toBeInTheDocument();

    const callsBeforePausedUpdate = scrollTo.mock.calls.length;
    live.send({ type: "text_delta", delta: "lo" });
    await screen.findByText("Hello");
    expect(scrollTo).toHaveBeenCalledTimes(callsBeforePausedUpdate);

    fireEvent.click(jumpToBottom);
    expect(scrollTo).toHaveBeenCalledWith({
      top: 1000,
      behavior: "auto",
    });

    const callsAfterResume = scrollTo.mock.calls.length;
    live.send({ type: "text_delta", delta: "!" });
    await screen.findByText("Hello!");
    expect(scrollTo.mock.calls.length).toBeGreaterThan(callsAfterResume);
  });

  it("shows the scroll controls after scrolling up even when idle", async () => {
    mockGetChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "Question" }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Answer" }],
        },
      ],
    });
    const scrollTo = vi.fn(function (
      this: HTMLDivElement,
      options?: ScrollToOptions,
    ) {
      if (typeof options?.top === "number") this.scrollTop = options.top;
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("Answer");
    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    Object.defineProperty(scroller, "scrollHeight", {
      value: 1000,
      configurable: true,
    });
    Object.defineProperty(scroller, "clientHeight", {
      value: 300,
      configurable: true,
    });
    scroller.scrollTo = scrollTo;

    scroller.scrollTop = 100;
    fireEvent.scroll(scroller);

    const controls = await screen.findByRole("toolbar", {
      name: "聊天滚动导航",
    });
    const jumpToTop = within(controls).getByRole("button", {
      name: "到顶部",
    });
    const jumpToBottom = within(controls).getByRole("button", {
      name: "到最下",
    });
    expect(jumpToBottom).toBeInTheDocument();
    expect(screen.queryByText("到最下")).not.toBeInTheDocument();
    fireEvent.click(jumpToTop);
    expect(scrollTo).toHaveBeenCalledWith({
      top: 0,
      behavior: "smooth",
    });
    fireEvent.click(jumpToBottom);
    expect(scrollTo).toHaveBeenCalledWith({
      top: 1000,
      behavior: "auto",
    });
  });

  it("jumps previous and next controls between user messages", async () => {
    mockGetChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "First question" }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "First answer" }],
        },
        {
          id: "user-2",
          role: "user",
          content: [{ type: "text", text: "Second question" }],
        },
        {
          id: "assistant-2",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Second answer" }],
        },
        {
          id: "user-3",
          role: "user",
          content: [{ type: "text", text: "Third question" }],
        },
        {
          id: "assistant-3",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Third answer" }],
        },
      ],
    });
    const scrollTo = vi.fn(function (
      this: HTMLDivElement,
      options?: ScrollToOptions,
    ) {
      if (typeof options?.top === "number") this.scrollTop = options.top;
    });
    const scrollBy = vi.fn();

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("Third question");
    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    Object.defineProperty(scroller, "scrollHeight", {
      value: 1600,
      configurable: true,
    });
    Object.defineProperty(scroller, "clientHeight", {
      value: 300,
      configurable: true,
    });
    Object.defineProperty(scroller, "getBoundingClientRect", {
      value: () => ({ top: 0, bottom: 300, height: 300 }),
      configurable: true,
    });
    scroller.scrollTo = scrollTo;
    Object.defineProperty(scroller, "scrollBy", {
      value: scrollBy,
      configurable: true,
    });

    const userMessages = Array.from(
      scroller.querySelectorAll<HTMLElement>('[data-rua-chat-role="user"]'),
    );
    [40, 360, 720].forEach((top, index) => {
      Object.defineProperty(userMessages[index], "getBoundingClientRect", {
        value: () => ({
          top: top - scroller.scrollTop,
          bottom: top - scroller.scrollTop + 80,
          height: 80,
        }),
        configurable: true,
      });
    });

    scroller.scrollTop = 500;
    fireEvent.scroll(scroller);

    const controls = await screen.findByRole("toolbar", {
      name: "聊天滚动导航",
    });
    fireEvent.click(within(controls).getByRole("button", { name: "到上一条" }));
    expect(scrollTo).toHaveBeenCalledWith({
      top: 360,
      behavior: "smooth",
    });

    fireEvent.click(within(controls).getByRole("button", { name: "到下一条" }));
    expect(scrollTo).toHaveBeenCalledWith({
      top: 720,
      behavior: "smooth",
    });
    expect(scrollBy).not.toHaveBeenCalled();
  });

  it("keeps the bottom anchored when the visual viewport changes", async () => {
    mockGetChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "Question" }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Answer" }],
        },
      ],
    });
    const listeners = new Map<string, Set<EventListener>>();
    const viewport = {
      height: 620,
      addEventListener: vi.fn((type: string, listener: EventListener) => {
        const bucket = listeners.get(type) || new Set<EventListener>();
        bucket.add(listener);
        listeners.set(type, bucket);
      }),
      removeEventListener: vi.fn((type: string, listener: EventListener) => {
        listeners.get(type)?.delete(listener);
      }),
    };
    Object.defineProperty(window, "visualViewport", {
      value: viewport,
      configurable: true,
    });
    const requestAnimationFrameSpy = vi
      .spyOn(window, "requestAnimationFrame")
      .mockImplementation((callback: FrameRequestCallback) => {
        callback(0);
        return 1;
      });
    const cancelAnimationFrameSpy = vi
      .spyOn(window, "cancelAnimationFrame")
      .mockImplementation(() => undefined);
    const scrollTo = vi.fn(function (
      this: HTMLDivElement,
      options?: ScrollToOptions,
    ) {
      if (typeof options?.top === "number") this.scrollTop = options.top;
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("Answer");
    await waitFor(() =>
      expect(viewport.addEventListener).toHaveBeenCalledWith(
        "resize",
        expect.any(Function),
      ),
    );
    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    Object.defineProperty(scroller, "scrollHeight", {
      value: 1000,
      configurable: true,
    });
    Object.defineProperty(scroller, "clientHeight", {
      value: 300,
      configurable: true,
    });
    scroller.scrollTo = scrollTo;
    scrollTo.mockClear();

    act(() => {
      viewport.height = 420;
      listeners
        .get("resize")
        ?.forEach((listener) => listener(new Event("resize")));
    });

    expect(
      document.documentElement.style.getPropertyValue(
        "--rua-chat-visual-height",
      ),
    ).toBe("420px");
    await waitFor(() =>
      expect(scrollTo).toHaveBeenCalledWith({
        top: 1000,
        behavior: "auto",
      }),
    );
    requestAnimationFrameSpy.mockRestore();
    cancelAnimationFrameSpy.mockRestore();
  });

  it("keeps the visible history anchored when the composer reduces the message viewport", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const resizeObservers: ResizeObserverCallback[] = [];
    vi.stubGlobal(
      "ResizeObserver",
      vi.fn().mockImplementation(function (callback: ResizeObserverCallback) {
        resizeObservers.push(callback);
        return { observe: vi.fn(), unobserve: vi.fn(), disconnect: vi.fn() };
      }),
    );
    const requestAnimationFrameSpy = vi
      .spyOn(window, "requestAnimationFrame")
      .mockImplementation((callback: FrameRequestCallback) => {
        callback(0);
        return 1;
      });
    const cancelAnimationFrameSpy = vi
      .spyOn(window, "cancelAnimationFrame")
      .mockImplementation(() => undefined);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await waitFor(() => expect(mockGetChat).toHaveBeenCalled());
    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    let clientHeight = 300;
    Object.defineProperty(scroller, "clientHeight", {
      get: () => clientHeight,
      configurable: true,
    });
    Object.defineProperty(scroller, "scrollHeight", {
      value: 1200,
      configurable: true,
    });

    act(() => {
      resizeObservers.forEach((callback) => callback([], {} as ResizeObserver));
    });
    scroller.scrollTop = 400;
    fireEvent.scroll(scroller);

    act(() => {
      clientHeight = 180;
      resizeObservers.forEach((callback) => callback([], {} as ResizeObserver));
    });

    expect(scroller.scrollTop).toBe(520);
    requestAnimationFrameSpy.mockRestore();
    cancelAnimationFrameSpy.mockRestore();
  });

  it("shows scroll navigation after scrolling and hides it after inactivity", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await waitFor(() => expect(mockGetChat).toHaveBeenCalled());
    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    expect(
      screen.queryByRole("toolbar", { name: "聊天滚动导航" }),
    ).not.toBeInTheDocument();

    vi.useFakeTimers();
    try {
      fireEvent.scroll(scroller);
      expect(
        screen.getByRole("toolbar", { name: "聊天滚动导航" }),
      ).toBeInTheDocument();

      act(() => vi.advanceTimersByTime(2999));
      expect(
        screen.getByRole("toolbar", { name: "聊天滚动导航" }),
      ).toBeInTheDocument();
      act(() => vi.advanceTimersByTime(1));
      expect(
        screen.queryByRole("toolbar", { name: "聊天滚动导航" }),
      ).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("pauses bottom follow as soon as touch scrolling starts", async () => {
    mockGetChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "Question" }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Answer" }],
        },
      ],
    });
    const listeners = new Map<string, Set<EventListener>>();
    const viewport = {
      height: 620,
      addEventListener: vi.fn((type: string, listener: EventListener) => {
        const bucket = listeners.get(type) || new Set<EventListener>();
        bucket.add(listener);
        listeners.set(type, bucket);
      }),
      removeEventListener: vi.fn((type: string, listener: EventListener) => {
        listeners.get(type)?.delete(listener);
      }),
    };
    Object.defineProperty(window, "visualViewport", {
      value: viewport,
      configurable: true,
    });
    const requestAnimationFrameSpy = vi
      .spyOn(window, "requestAnimationFrame")
      .mockImplementation((callback: FrameRequestCallback) => {
        callback(0);
        return 1;
      });
    const cancelAnimationFrameSpy = vi
      .spyOn(window, "cancelAnimationFrame")
      .mockImplementation(() => undefined);
    const scrollTo = vi.fn(function (
      this: HTMLDivElement,
      options?: ScrollToOptions,
    ) {
      if (typeof options?.top === "number") this.scrollTop = options.top;
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("Answer");
    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    Object.defineProperty(scroller, "scrollHeight", {
      value: 1000,
      configurable: true,
    });
    Object.defineProperty(scroller, "clientHeight", {
      value: 300,
      configurable: true,
    });

    scroller.scrollTop = 400;
    scroller.scrollTo = scrollTo;
    scrollTo.mockClear();

    fireEvent.touchStart(scroller, {
      touches: [{ clientX: 20, clientY: 100 }],
    });
    fireEvent.touchMove(scroller, {
      touches: [{ clientX: 20, clientY: 120 }],
    });
    fireEvent.scroll(scroller);

    expect(
      await screen.findByRole("button", { name: "到最下" }),
    ).toBeInTheDocument();
    scrollTo.mockClear();

    act(() => {
      viewport.height = 420;
      listeners
        .get("resize")
        ?.forEach((listener) => listener(new Event("resize")));
    });

    expect(scrollTo).not.toHaveBeenCalled();
    requestAnimationFrameSpy.mockRestore();
    cancelAnimationFrameSpy.mockRestore();
  });

  it("pauses bottom follow as soon as desktop scrolling starts upward", async () => {
    mockGetChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "Question" }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Answer" }],
        },
      ],
    });
    const listeners = new Map<string, Set<EventListener>>();
    const viewport = {
      height: 620,
      addEventListener: vi.fn((type: string, listener: EventListener) => {
        const bucket = listeners.get(type) || new Set<EventListener>();
        bucket.add(listener);
        listeners.set(type, bucket);
      }),
      removeEventListener: vi.fn((type: string, listener: EventListener) => {
        listeners.get(type)?.delete(listener);
      }),
    };
    Object.defineProperty(window, "visualViewport", {
      value: viewport,
      configurable: true,
    });
    const requestAnimationFrameSpy = vi
      .spyOn(window, "requestAnimationFrame")
      .mockImplementation((callback: FrameRequestCallback) => {
        callback(0);
        return 1;
      });
    const cancelAnimationFrameSpy = vi
      .spyOn(window, "cancelAnimationFrame")
      .mockImplementation(() => undefined);
    const scrollTo = vi.fn(function (
      this: HTMLDivElement,
      options?: ScrollToOptions,
    ) {
      if (typeof options?.top === "number") this.scrollTop = options.top;
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("Answer");
    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    Object.defineProperty(scroller, "scrollHeight", {
      value: 1000,
      configurable: true,
    });
    Object.defineProperty(scroller, "clientHeight", {
      value: 300,
      configurable: true,
    });

    scroller.scrollTop = 400;
    scroller.scrollTo = scrollTo;
    scrollTo.mockClear();

    fireEvent.wheel(scroller, { deltaY: -10 });
    fireEvent.scroll(scroller);

    expect(
      await screen.findByRole("button", { name: "到最下" }),
    ).toBeInTheDocument();
    scrollTo.mockClear();

    act(() => {
      viewport.height = 420;
      listeners
        .get("resize")
        ?.forEach((listener) => listener(new Event("resize")));
    });

    expect(scrollTo).not.toHaveBeenCalled();
    requestAnimationFrameSpy.mockRestore();
    cancelAnimationFrameSpy.mockRestore();
  });

  it("resumes auto-follow when submitting after scrolling up", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);
    const scrollTo = vi.fn(function (
      this: HTMLDivElement,
      options?: ScrollToOptions,
    ) {
      if (typeof options?.top === "number") this.scrollTop = options.top;
    });

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const scroller = document.querySelector(
      ".rua-owned-surface-messages",
    ) as HTMLDivElement;
    Object.defineProperty(scroller, "scrollHeight", {
      value: 1000,
      configurable: true,
    });
    Object.defineProperty(scroller, "clientHeight", {
      value: 300,
      configurable: true,
    });
    scroller.scrollTo = scrollTo;

    scroller.scrollTop = 100;
    fireEvent.scroll(scroller);
    scrollTo.mockClear();

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Next question" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));
    live.controller().enqueue(
      new TextEncoder().encode(
        `data: ${JSON.stringify({
          type: "user_message",
          message: {
            id: "user-next",
            role: "user",
            content: [{ type: "text", text: "Next question" }],
          },
        })}\n\n`,
      ),
    );

    await screen.findByText("Next question");
    await waitFor(() =>
      expect(scrollTo).toHaveBeenCalledWith({
        top: 1000,
        behavior: "auto",
      }),
    );
  });

  it("renders AgentScope content deltas live and keeps thinking separate", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
          blockPreferences={{
            aggregateExpanded: true,
            thinkingExpanded: true,
            toolExpanded: true,
          }}
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Hello" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(submitCalls(fetchMock)).toHaveLength(1));

    const write = (payload: unknown) => {
      live.send(payload);
    };

    write({
      object: "message",
      status: "in_progress",
      id: "reasoning-1",
      type: "reasoning",
      role: "assistant",
      content: null,
    });
    write({
      object: "content",
      status: "in_progress",
      type: "text",
      index: 0,
      delta: true,
      msg_id: "reasoning-1",
      text: "Need answer",
    });

    await screen.findByText("Need answer");

    write({
      object: "message",
      status: "in_progress",
      id: "message-1",
      type: "message",
      role: "assistant",
      content: null,
    });
    write({
      object: "content",
      status: "in_progress",
      type: "text",
      index: 0,
      delta: true,
      msg_id: "message-1",
      text: "Partial answer",
    });

    await screen.findByText("Partial answer");

    const thinkingDetails = await screen.findByText("Thinking");
    const thinkingPanel = thinkingDetails.closest("details");
    expect(thinkingPanel).toHaveTextContent("Need answer");
    expect(thinkingPanel).not.toHaveTextContent("Partial answer");

    write({
      object: "response",
      status: "completed",
      output: [
        {
          object: "message",
          status: "completed",
          id: "reasoning-1",
          type: "reasoning",
          role: "assistant",
          content: [{ type: "text", text: "Need answer" }],
        },
        {
          object: "message",
          status: "completed",
          id: "message-1",
          type: "message",
          role: "assistant",
          content: [{ type: "text", text: "Final answer" }],
        },
      ],
    });
    live.close();

    await screen.findByText("Final answer");
    await waitFor(() =>
      expect(screen.queryByText("Generating")).not.toBeInTheDocument(),
    );
  });

  it("turns EOF without terminal into an interrupted non-loading response", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live")
          ? sseResponse([
              {
                object: "response",
                status: "in_progress",
                output: [
                  {
                    id: "assistant-1",
                    role: "assistant",
                    type: "message",
                    content: [{ type: "text", text: "partial" }],
                  },
                ],
              },
            ])
          : undefined,
      ),
    );

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Hello" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await screen.findByText("partial");
    await waitFor(() =>
      expect(screen.queryByText("Generating")).not.toBeInTheDocument(),
    );
  });

  it("keeps the spinner and Stop button consistent when reopening an active Chat before content recovers", async () => {
    const activeTurn = {
      turn_id: "turn-reopening",
      authority_epoch: "epoch-local",
      authority_device_id: "device-local",
      phase: "streaming",
      driver: "rua",
      started_at: "2026-09-01T00:00:00Z",
    };
    mockGetChat.mockResolvedValue({
      ...idleHistory(),
      control: backgroundWorkControl({}, activeTurn),
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live")
          ? trackedSseResponse().response
          : undefined,
      ),
    );
    const mount = () =>
      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{
              scope: "agent",
              agentId: "default",
              chatId: "chat-reopening",
            }}
            chatModel={configuredModel}
          />
        </MemoryRouter>,
      );
    const expectActive = async () => {
      expect(await screen.findByLabelText("Stop generating")).toBeEnabled();
      expect(document.querySelectorAll("[data-rua-chat-running]")).toHaveLength(
        1,
      );
      expect(
        document.querySelector("[data-rua-chat-running]")?.textContent,
      ).toBe("");
      expect(
        document.querySelector("[data-rua-chat-role='assistant']"),
      ).toBeNull();
    };
    const first = mount();
    await expectActive();
    first.unmount();
    mount();
    await expectActive();
    expect(mockStopChat).not.toHaveBeenCalled();
  });

  it.each(["accepted", "stale", "failed", "unobserved"])(
    "stops an in-flight run through the owned runtime: %s",
    async (outcome) => {
      mockGetChat.mockResolvedValue({
        ...idleHistory(),
        active_turn:
          outcome === "unobserved"
            ? null
            : {
                turn_id: "turn-stop",
                authority_epoch: "epoch-stop",
                phase: "streaming",
                authority_device_id: "device-a",
                driver: "rua",
                started_at: "2026-09-01T00:00:00Z",
              },
      });
      if (outcome === "failed")
        mockStopChat.mockRejectedValue(new Error("Stop request failed"));
      else
        mockStopChat.mockResolvedValue({
          stopped: outcome === "accepted",
          detail: "Observed Turn changed",
        });
      const live = trackedSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/console/live") ? live.response : undefined,
        ),
      );

      render(
        <MemoryRouter>
          <OwnedConversationSurface
            address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
            chatModel={configuredModel}
          />
        </MemoryRouter>,
      );

      const input = screen.getByPlaceholderText("Message");
      fireEvent.change(input, { target: { value: "Slow" } });
      fireEvent.keyDown(input, { key: "Enter" });

      await act(async () => {
        live.send({
          type: "stream_replay",
          complete: true,
          events: [
            {
              type: "user_message",
              message: {
                id: "user-shell",
                role: "user",
                content: [{ type: "text", text: "Slow" }],
              },
            },
            {
              type: "tool_call",
              message_id: "shell-call",
              id: "call-shell",
              name: "bash",
              arguments: { command: "sleep 90" },
            },
          ],
        });
      });

      const stop = await screen.findByLabelText("Stop generating");
      await waitFor(() =>
        expect(
          document.querySelector(".rua-chat-inline-spinner"),
        ).not.toBeNull(),
      );
      fireEvent.click(stop);

      if (outcome === "unobserved") {
        await waitFor(() =>
          expect(mockGetChat.mock.calls.length).toBeGreaterThan(1),
        );
        expect(mockStopChat).not.toHaveBeenCalled();
        return;
      }
      await waitFor(() =>
        expect(mockStopChat).toHaveBeenCalledWith(
          "chat-1",
          { turn_id: "turn-stop", authority_epoch: "epoch-stop" },
          "default",
        ),
      );
      if (outcome !== "accepted") {
        expect(
          await screen.findByText(
            outcome === "failed"
              ? "Stop request failed"
              : "Observed Turn changed",
          ),
        ).toBeInTheDocument();
        expect(screen.getByLabelText("Stop generating")).toBeEnabled();
      }
    },
  );

  it("selects variants and sends selected metadata on next submit", async () => {
    sessionStorage.clear();
    mockGetChat
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [
          { id: "user-1", role: "user", content: "Question" },
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: "First",
            turn_id: "turn-1",
            selected_variant_id: "variant-1",
            assistant_response_variants: [
              {
                variant_id: "variant-1",
                output: [{ id: "a1", role: "assistant", content: "First" }],
              },
              {
                variant_id: "variant-2",
                output: [{ id: "a2", role: "assistant", content: "Second" }],
              },
            ],
          },
        ],
      })
      .mockResolvedValue({
        status: "idle",
        messages: [],
        model: configuredModel,
      });
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("First");
    fireEvent.click(screen.getByLabelText("Next variant"));
    await screen.findByText("Second");

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Next question" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(([url]) =>
          String(url).includes("/console/submit"),
        ),
      ).toBe(true),
    );
    const submitCall = fetchMock.mock.calls.find(([url]) =>
      String(url).includes("/console/submit"),
    );
    const body = JSON.parse(submitCall![1].body);
    expect(body.turn_id).toBe("turn-1");
    expect(body.selected_variant_id).toBe("variant-2");
    live.close();
  });

  it("regenerates as an assistant response variant and reloads in place", async () => {
    mockGetChat
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [
          { id: "user-1", role: "user", content: "Question" },
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: "First",
            turn_id: "turn-1",
            selected_variant_id: "variant-1",
            assistant_response_variants: [
              {
                variant_id: "variant-1",
                output: [{ id: "a1", role: "assistant", content: "First" }],
              },
            ],
          },
        ],
      })
      .mockResolvedValue({
        status: "idle",
        model: configuredModel,
        messages: [
          { id: "user-1", role: "user", content: "Question" },
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: "Regenerated",
            turn_id: "turn-1",
            selected_variant_id: "variant-2",
            assistant_response_variants: [
              {
                variant_id: "variant-1",
                output: [{ id: "a1", role: "assistant", content: "First" }],
              },
              {
                variant_id: "variant-2",
                output: [
                  { id: "a2", role: "assistant", content: "Regenerated" },
                ],
              },
            ],
          },
        ],
      });
    const live = trackedSseResponse();
    const fetchMock = routeFetch((url) =>
      url.includes("/console/live") ? live.response : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("First");
    fireEvent.click(screen.getByLabelText("Regenerate"));
    live.controller().enqueue(
      new TextEncoder().encode(
        `data: ${JSON.stringify({
          object: "response",
          status: "completed",
          output: [{ role: "assistant", content: "Regenerated" }],
        })}\n\n`,
      ),
    );
    live.controller().enqueue(
      new TextEncoder().encode(
        `data: ${JSON.stringify({
          type: "turn_terminal",
          turn_id: "turn-1",
          status: "completed",
        })}\n\n`,
      ),
    );
    live.close();

    await screen.findByText("Regenerated");
    expect(screen.getByText("2/2")).toBeInTheDocument();
    expect(screen.getAllByText("Question")).toHaveLength(1);

    const body = submittedChatBody(fetchMock);
    expect(body.chat_id).toBe("chat-1");
    expect(body.input[0].content[0].text).toBe("Question");
    expect(body.regenerate_turn_id).toBe("turn-1");
  });

  it("streams regenerated output into the existing assistant response", async () => {
    mockGetChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        { id: "user-1", role: "user", content: "Question" },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: "First",
          turn_id: "turn-1",
          selected_variant_id: "variant-1",
          assistant_response_variants: [
            {
              variant_id: "variant-1",
              output: [
                { id: "a1", role: "assistant", content: "First" },
                {
                  id: "assistant-old-final",
                  role: "assistant",
                  type: "message",
                  content: "Old final answer",
                },
              ],
            },
          ],
        },
        {
          id: "assistant-old-tool",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: { call_id: "call-1", name: "Bash", arguments: "{}" },
            },
          ],
        },
        {
          id: "assistant-old-final",
          role: "assistant",
          type: "message",
          content: "Old final answer",
        },
      ],
    });
    const encoder = new TextEncoder();
    const live = trackedSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? live.response : undefined,
      ),
    );

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    await screen.findByText("First");
    await screen.findByText("Old final answer");
    fireEvent.click(screen.getByLabelText("Regenerate"));
    await waitFor(() =>
      expect(screen.queryByText("Old final answer")).not.toBeInTheDocument(),
    );

    live.send({
      type: "thinking_delta",
      delta: "Thinking about replacement",
    });

    await screen.findByText("Thinking about replacement");
    expect(screen.queryByText("Old final answer")).not.toBeInTheDocument();

    live.send({ type: "text_delta", delta: "Regenerating" });

    await screen.findByText("Regenerating");
    expect(screen.getAllByText("Question")).toHaveLength(1);
    expect(screen.queryByText("2/2")).not.toBeInTheDocument();

    live
      .controller()
      .enqueue(
        encoder.encode(
          `${JSON.stringify({ type: "done", finish_reason: "stop" })}\n`,
        ),
      );
    live.close();
  });

  it.each([
    [false, 0],
    [true, 0],
    [false, 1],
    [true, 1],
    [false, 2],
    [true, 2],
  ] as const)(
    "rolls back and restores the complete input with durable coverage (image: %s, retained turns: %s)",
    async (withImage, retainedTurns) => {
      const retainedMessages = Array.from(
        { length: retainedTurns },
        (_, index) => [
          {
            id: `retained-user-${index}`,
            role: "user",
            turn_id: `retained-turn-${index}`,
            content: `Retained question ${index}`,
          },
          {
            id: `retained-assistant-${index}`,
            role: "assistant",
            turn_id: `retained-turn-${index}`,
            content: `Retained answer ${index}`,
          },
        ],
      ).flat();
      const mediaRef = "media/00000000-0000-4000-8000-000000000001";
      mockGetChat
        .mockResolvedValueOnce({
          status: "idle",
          model: configuredModel,
          durable_coverage: {
            latest_message_sequence: retainedMessages.length + 4,
            manifest_hash: "before-rollback",
          },
          messages: [
            ...retainedMessages,
            {
              id: "user-1",
              role: "user",
              turn_id: "turn-1",
              content: withImage
                ? [
                    { type: "text", text: "First question\n" },
                    { type: "image", media_ref: mediaRef },
                    { type: "text", text: "\nDescribe it" },
                  ]
                : "First question",
            },
            {
              id: "assistant-1",
              role: "assistant",
              turn_id: "turn-1",
              selected_variant_id: "variant-1",
              content: "First answer",
            },
            {
              id: "later-user",
              role: "user",
              turn_id: "later-turn",
              content: "Later question",
            },
            {
              id: "later-assistant",
              role: "assistant",
              turn_id: "later-turn",
              content: "Later answer",
            },
          ],
        })
        .mockResolvedValue({
          status: "idle",
          model: configuredModel,
          durable_coverage: {
            latest_message_sequence: retainedMessages.length,
            manifest_hash: "after-rollback",
          },
          messages: retainedMessages,
        });
      const fetchMock = routeFetch((url) =>
        url.includes("/chats/chat-1/rollback")
          ? jsonResponse({ chat: { id: "chat-1" }, cancelled_tasks: 0 })
          : undefined,
      );
      vi.stubGlobal("fetch", fetchMock);

      render(
        <MemoryRouter>
          <ModalProvider>
            <OwnedConversationSurface
              address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
              chatModel={configuredModel}
            />
          </ModalProvider>
        </MemoryRouter>,
      );

      await screen.findByText("First answer");
      fireEvent.click(
        screen.getAllByLabelText("More message actions")[retainedTurns],
      );
      fireEvent.click(screen.getByRole("menuitem", { name: "Rollback" }));
      const dialog = await screen.findByRole("dialog");
      fireEvent.click(within(dialog).getByRole("button", { name: "Rollback" }));

      await waitFor(() =>
        expect(screen.getByPlaceholderText("Message")).toHaveValue(
          withImage
            ? "First question\n[Image #1]\nDescribe it"
            : "First question",
        ),
      );
      await waitFor(() =>
        expect(screen.queryByText("First answer")).not.toBeInTheDocument(),
      );
      await waitFor(() => expect(mockGetChat).toHaveBeenCalledTimes(2));
      expect(screen.queryByText("First answer")).not.toBeInTheDocument();
      expect(screen.queryByText("Later answer")).not.toBeInTheDocument();
      for (let index = 0; index < retainedTurns; index += 1) {
        expect(
          screen.getByText(`Retained answer ${index}`),
        ).toBeInTheDocument();
      }
      if (withImage) {
        fireEvent.keyDown(screen.getByPlaceholderText("Message"), {
          key: "Enter",
        });
        await waitFor(() =>
          expect(submittedChatBody(fetchMock).input[0].content).toEqual([
            { type: "text", text: "First question\n" },
            { type: "image", media_ref: mediaRef },
            { type: "text", text: "\nDescribe it" },
          ]),
        );
        expect(mockUploadFile).not.toHaveBeenCalled();
      }
    },
  );

  it("uploads attachments and submits multimodal content parts", async () => {
    mockGetChat.mockResolvedValue(idleHistory());
    const fetchMock = vi.fn().mockResolvedValue(
      sseResponse([
        {
          object: "response",
          status: "completed",
          output: [{ role: "assistant", content: "ok" }],
        },
      ]),
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const fileInput = document.querySelector(
      '.rua-owned-composer input[type="file"]',
    ) as HTMLInputElement;
    const file = new File(["fake"], "picture.png", { type: "image/png" });
    fireEvent.change(fileInput, { target: { files: [file] } });

    await screen.findByAltText("picture.png");
    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, {
      target: { value: "Describe this\n[Image #1]" },
    });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    const body = submittedChatBody(fetchMock);
    expect(body.input[0].content).toEqual([
      { type: "text", text: "Describe this\n" },
      {
        type: "image",
        media_ref: "media/00000000-0000-4000-8000-000000000001",
      },
    ]);
  });

  it("restores and saves drafts per conversation id", async () => {
    saveDraft("default:chat-1", "saved draft");
    mockGetChat.mockResolvedValue(idleHistory());

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const input = (await screen.findByPlaceholderText(
      "Message",
    )) as HTMLTextAreaElement;
    expect(input.value).toBe("saved draft");

    fireEvent.change(input, { target: { value: "changed draft" } });
    expect(input).toHaveValue("changed draft");
    await waitFor(() =>
      expect(sessionStorage.getItem("rua_input_draft_default:chat-1")).toBe(
        "changed draft",
      ),
    );
  });

  it("uses canonical Project submit and live endpoints with the default Agent", async () => {
    const onSubmitAccepted = vi.fn();
    const fetchMock = routeFetch((url, init) => {
      if (url.includes("/projects/project-1/chats/chat-1/console/submit")) {
        return submitAcceptedResponse();
      }
      if (url.includes("/projects/project-1/chats/chat-1/console/live")) {
        expect(JSON.parse(String(init?.body))).toEqual({
          viewer_id: expect.any(String),
        });
        return sseResponse([
          { object: "response", status: "completed", output: [] },
        ]);
      }
      return undefined;
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{
            scope: "project",
            projectId: {
              scope: "project",
              projectId: "project-1",
              agentId: "default",
            }.projectId,
            chatId: "chat-1",
          }}
          lazy
          draftKey="project:project-1:chat-1"
          authorityPlacementDeviceId="device-local"
          currentDeviceId="device-local"
          onSubmitAccepted={onSubmitAccepted}
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Project work" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/projects/project-1/chats/chat-1/console/submit",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    const submitCall = fetchMock.mock.calls.find(([url]) =>
      String(url).includes("/projects/project-1/chats/chat-1/console/submit"),
    );
    expect(JSON.parse(String(submitCall?.[1]?.body))).toEqual(
      expect.objectContaining({
        chat_id: "chat-1",
        agent_id: "default",
        input: expect.any(Array),
      }),
    );
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/projects/project-1/chats/chat-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    await waitFor(() => expect(onSubmitAccepted).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(input).toHaveValue(""));
  });

  it("loads a materialized Project Chat from its canonical history endpoint", async () => {
    const fetchMock = routeFetch((url) => {
      if (url.includes("/projects/project-1/chats/chat-saved?turn_limit=20")) {
        return jsonResponse({
          status: "idle",
          messages: [],
          model: configuredModel,
        });
      }
      if (url.includes("/projects/project-1/chats/chat-saved/console/live")) {
        return sseResponse([
          { object: "response", status: "completed", output: [] },
        ]);
      }
      return undefined;
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{
            scope: "project",
            projectId: {
              scope: "project",
              projectId: "project-1",
              agentId: "default",
            }.projectId,
            chatId: "chat-saved",
          }}
          authorityPlacementDeviceId="device-local"
          currentDeviceId="device-local"
        />
      </MemoryRouter>,
    );

    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(
          ([url, init]) =>
            String(url).startsWith(
              "/api/projects/project-1/chats/chat-saved?",
            ) &&
            (!init?.method || init.method === "GET"),
        ),
      ).toBe(true),
    );
  });

  it("keeps a Project draft and uses the existing failed queue item when Submit is rejected", async () => {
    const fetchMock = routeFetch((url) => {
      if (url.includes("/projects/project-1/chats/chat-1/console/submit")) {
        return submitAcceptedResponse({
          accepted: false,
          detail: "Project directory is unavailable",
        });
      }
      if (url.includes("/projects/project-1/chats/chat-1?turn_limit=20")) {
        return jsonResponse(idleHistory());
      }
      if (url.includes("/approvals/snapshot")) {
        return jsonResponse({ pending: [], resolved: [] });
      }
      if (url.includes("/acp/permissions")) {
        return jsonResponse({ pending: [] });
      }
      if (url.includes("/projects/project-1/chats/chat-1/console/live")) {
        return sseResponse([
          { object: "response", status: "completed", output: [] },
        ]);
      }
      return undefined;
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{
            scope: "project",
            projectId: {
              scope: "project",
              projectId: "project-1",
              agentId: "default",
            }.projectId,
            chatId: "chat-1",
          }}
          lazy
          draftKey="project:project-1:chat-1"
          authorityPlacementDeviceId="device-local"
          currentDeviceId="device-local"
        />
      </MemoryRouter>,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Preserve this work" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Project directory is unavailable",
    );
    expect(input).toHaveValue("Preserve this work");
    expect(
      sessionStorage.getItem("rua_input_draft_project:project-1:chat-1"),
    ).toBe("Preserve this work");
    expect(
      document.querySelector(".rua-owned-pending-queue-item"),
    ).not.toBeNull();
  });

  it("renders slash suggestions and inserts the selected command", async () => {
    mockGetChat.mockResolvedValue(idleHistory());

    render(
      <MemoryRouter>
        <OwnedConversationSurface
          address={{ scope: "agent", agentId: "default", chatId: "chat-1" }}
          chatModel={configuredModel}
        />
      </MemoryRouter>,
    );

    const input = (await screen.findByPlaceholderText(
      "Message",
    )) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "/he" } });

    fireEvent.mouseDown(await screen.findByText("/hello"));
    expect(input.value).toBe("/hello ");
  });
});

// Domain stream fixtures enter through the shared transport seam.
vi.mock("@/api/consoleTransport", () => ({
  consoleFetch: (path: string, options?: RequestInit) =>
    fetch(`/api${path}`, options),
}));
