import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAgentStore } from "@/stores/agentStore";
import { saveDraftConversationAcp } from "../../conversationLifecycle";
import { loadSubmitted, clearSubmitted, saveSubmitted } from "../../drafts";
import {
  CHAT_SUBMITTED_EVENT,
  __resetChatViewCacheForTests,
  buildUserContent,
  primeChatViewCache,
  readChatViewScrollCache,
  readChatViewTitleCache,
  writeChatViewScrollCache,
  useConversationViewRuntime,
} from "../useConversationViewRuntime";

function isTurnActive(result: {
  current: ReturnType<typeof useConversationViewRuntime>;
}) {
  return result.current.projection.turnActivity.kind === "active";
}

import {
  canonicalHistoryFixture,
  createCanonicalStreamFixture,
} from "./canonicalMessageFixtures";

const chatApiMock = vi.hoisted(() => ({
  getChat: vi.fn(),
  submitPath: vi.fn(
    (chatId: string, agentId: string) =>
      `/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
        chatId,
      )}/console/submit`,
  ),
  livePath: vi.fn(
    (chatId: string, agentId: string) =>
      `/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
        chatId,
      )}/console/live`,
  ),
  stopChat: vi.fn(),
  selectVariant: vi.fn(),
  rollback: vi.fn(),
  branch: vi.fn(),
  persistAcpHistoryPreview: vi.fn(),
  deleteQueuedInput: vi.fn(),
  editQueuedInput: vi.fn(),
  sendNowQueuedInput: vi.fn(),
}));

const consoleApiMock = vi.hoisted(() => ({
  getApprovalSnapshot: vi.fn(),
  decideApproval: vi.fn(),
}));

const acpApiMock = vi.hoisted(() => ({
  getPermissionSnapshot: vi.fn(),
  decidePermission: vi.fn(),
}));

const projectChatsApiMock = vi.hoisted(() => ({
  getChat: vi.fn(),
  livePath: vi.fn(
    (projectId: string, chatId: string) =>
      `/projects/${projectId}/chats/${chatId}/console/live`,
  ),
  submitPath: vi.fn(
    (projectId: string, chatId: string) =>
      `/projects/${projectId}/chats/${chatId}/console/submit`,
  ),
  stopChat: vi.fn(),
  persistAcpHistoryPreview: vi.fn(),
  deleteQueuedInput: vi.fn(),
  editQueuedInput: vi.fn(),
  sendNowQueuedInput: vi.fn(),
  getApprovalSnapshot: vi.fn(),
  decideApproval: vi.fn(),
  getAcpPermissionSnapshot: vi.fn(),
  decideAcpPermission: vi.fn(),
  selectVariant: vi.fn(),
  rollback: vi.fn(),
  branch: vi.fn(),
}));

const providerApiMock = vi.hoisted(() => ({
  getActiveModels: vi.fn(),
}));

const ownedApiMock = vi.hoisted(() => ({
  getPendingSelectedVariant: vi.fn(),
  setPendingSelectedVariant: vi.fn(),
  clearPendingSelectedVariant: vi.fn(),
}));

const cacheHostMock = vi.hoisted(() => ({
  isMobile: vi.fn(() => true),
  isDesktopShell: vi.fn(() => false),
}));

vi.mock("@/utils/isMobile", () => ({ isMobile: cacheHostMock.isMobile }));
vi.mock("@/desktop/runtime", () => ({
  isDesktopShell: cacheHostMock.isDesktopShell,
}));

const configuredModel = {
  provider_id: "dashscope",
  model: "qwen-plus",
};

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      key === "modelConfig.modelNotConfigured"
        ? "translated model missing message"
        : key,
  }),
}));

vi.mock("@/api/modules/chat", () => ({
  chatApi: {
    ...chatApiMock,
    getChat: async (...args: unknown[]) =>
      canonicalHistoryFixture(await chatApiMock.getChat(...args)),
  },
}));

vi.mock("@/api/modules/console", () => ({
  consoleApi: consoleApiMock,
}));

vi.mock("@/api/modules/acp", () => ({
  acpApi: acpApiMock,
}));

vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: {
    ...projectChatsApiMock,
    getChat: async (...args: unknown[]) =>
      canonicalHistoryFixture(await projectChatsApiMock.getChat(...args)),
  },
}));

vi.mock("@/api/modules/provider", () => ({
  providerApi: providerApiMock,
}));

vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer test-token" }),
}));

vi.mock("@/api/config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));

vi.mock("../api", () => ownedApiMock);

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

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function submitAcceptedResponse(overrides: Record<string, unknown> = {}) {
  return jsonResponse({
    accepted: true,
    mode: "started",
    control: {
      authority_device_id: "device-current",
      placement: {
        authority_device_id: "device-current",
        authority_epoch: "0000000000000001000000000000000000000000",
        switchable: false,
      },
      active_turn: {
        turn_id: "turn-stop",
        authority_epoch: "0000000000000001000000000000000000000000",
        phase: "streaming",
        started_at: "2026-07-20T00:00:00Z",
        queued_count: 0,
        driver: "rua",
      },
      queue: { entries: [] },
      acp: { active: false },
      allowed_controls: {
        stop: true,
        queue: true,
        interject: true,
        edit_queued_input: false,
        delete_queued_input: false,
        switch_authority: false,
        enter_acp: false,
        exit_acp: false,
      },
    },
    ...overrides,
  });
}

function submitStartedResponse() {
  return submitAcceptedResponse();
}

function submitQueuedResponse(
  inputId: string,
  text: string,
  entries: Array<{
    input_id: string;
    submission_id?: string;
    text: string;
    device_delivery_id?: string;
  }> = [{ input_id: inputId, text }],
) {
  return submitAcceptedResponse({
    submission_id: `submission-${inputId}`,
    mode: "queued",
    input_id: inputId,
    control: {
      authority_device_id: "device-current",
      placement: {
        authority_device_id: "device-current",
        authority_epoch: "0000000000000001000000000000000000000000",
        switchable: false,
      },
      active_turn: {
        phase: "streaming",
        started_at: "2026-07-20T00:00:00Z",
        queued_count: 1,
        driver: "rua",
      },
      queue: {
        entries: entries.map((entry, index) => ({
          input_id: entry.input_id,
          submission_id: entry.submission_id || `submission-${entry.input_id}`,
          state: "queued",
          text: entry.text,
          created_at: `2026-07-20T00:00:0${index + 1}Z`,
          updated_at: `2026-07-20T00:00:0${index + 1}Z`,
          ...(entry.device_delivery_id
            ? { device_delivery_id: entry.device_delivery_id }
            : {}),
          editable: true,
          deletable: true,
        })),
      },
      acp: { active: false },
      allowed_controls: {
        stop: true,
        queue: true,
        interject: true,
        edit_queued_input: false,
        delete_queued_input: false,
        switch_authority: false,
        enter_acp: false,
        exit_acp: false,
      },
    },
  });
}

function submitStartedWithEmptyQueueResponse() {
  return submitAcceptedResponse({
    mode: "started",
    control: {
      authority_device_id: "device-current",
      placement: {
        authority_device_id: "device-current",
        switchable: true,
      },
      active_turn: null,
      queue: { entries: [] },
      acp: { active: false },
      allowed_controls: {
        stop: false,
        queue: true,
        interject: false,
        edit_queued_input: false,
        delete_queued_input: false,
        switch_authority: true,
        enter_acp: true,
        exit_acp: false,
      },
    },
  });
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
    if (url.includes("/sync/cycle/target")) {
      return Promise.resolve(jsonResponse({ ok: true }));
    }
    if (url.includes("/sync/repair/scan")) {
      return Promise.resolve(jsonResponse({ ok: true }));
    }
    return Promise.resolve(new Response(null, { status: 200 }));
  });
}

function routeFetchWithLiveStreams(...responses: Response[]) {
  let index = 0;
  return routeFetch((url) => {
    if (!url.includes("/console/live")) return undefined;
    const response = responses[index];
    index += 1;
    return (
      response ||
      sseResponse([{ object: "response", status: "completed", output: [] }])
    );
  });
}

function controlledSseResponse() {
  const encoder = new TextEncoder();
  const fixture = createCanonicalStreamFixture();
  const cancel = vi.fn();
  let controller: ReadableStreamDefaultController<Uint8Array> | null = null;
  const body = new ReadableStream<Uint8Array>({
    start(nextController) {
      controller = nextController;
    },
    cancel,
  });
  return {
    cancel,
    response: new Response(body, {
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
    }),
    send(payload: unknown) {
      controller?.enqueue(
        encoder.encode(
          `data: ${JSON.stringify(fixture.normalize(payload))}\n\n`,
        ),
      );
    },
    close() {
      controller?.close();
    },
    error(reason: unknown) {
      controller?.error(reason);
    },
  };
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

function regenerationHistory(
  variants: Array<{ variantId: string; messageId: string; text: string }>,
  selectedVariantId: string,
  includeLaterTurn = false,
) {
  const selected = variants.find(
    (variant) => variant.variantId === selectedVariantId,
  );
  if (!selected) throw new Error("selected regeneration variant is missing");
  const messages = [
    {
      id: "user-1",
      role: "user",
      turn_id: "turn-1",
      content: [{ type: "text", text: "inspect" }],
    },
    {
      id: selected.messageId,
      role: "assistant",
      type: "message",
      turn_id: "turn-1",
      selected_variant_id: selectedVariantId,
      assistant_response_variants: variants.map((variant) => ({
        variant_id: variant.variantId,
        output: [
          {
            id: variant.messageId,
            role: "assistant",
            type: "message",
            turn_id: "turn-1",
            content: [{ type: "text", text: variant.text }],
          },
        ],
      })),
      content: [{ type: "text", text: selected.text }],
    },
  ];
  if (includeLaterTurn) {
    messages.push(
      {
        id: "user-2",
        role: "user",
        turn_id: "turn-2",
        content: [{ type: "text", text: "later" }],
      },
      {
        id: "assistant-later",
        role: "assistant",
        type: "message",
        turn_id: "turn-2",
        content: [{ type: "text", text: "later answer" }],
      },
    );
  }
  return { status: "idle", model: configuredModel, messages };
}

describe("useConversationViewRuntime", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    cacheHostMock.isMobile.mockReturnValue(true);
    cacheHostMock.isDesktopShell.mockReturnValue(false);
    __resetChatViewCacheForTests();
    localStorage.clear();
    localStorage.setItem("rua-terminal-viewer-id", "browser-test");
    sessionStorage.clear();
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
    useAgentStore.setState({ selectedAgent: "agent-a" });
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: { provider_id: "dashscope", model: "qwen-plus" },
    });
    chatApiMock.stopChat.mockResolvedValue({ stopped: true });
    chatApiMock.selectVariant.mockResolvedValue({ id: "chat-1" });
    chatApiMock.rollback.mockResolvedValue({
      chat: { id: "chat-1" },
      cancelled_tasks: 0,
    });
    chatApiMock.branch.mockResolvedValue({
      chat: { id: "branch-chat" },
    });
    chatApiMock.persistAcpHistoryPreview.mockResolvedValue({
      turn_id: "acp-history-preview-1",
      message_id: "acp-history-preview-message-1",
      created: true,
    });
    chatApiMock.deleteQueuedInput.mockResolvedValue({
      deleted_input_id: "input-1",
      control: {
        active_turn: null,
        queue: { entries: [] },
        acp: { active: false },
        allowed_controls: {
          stop: false,
          queue: false,
          interject: false,
          edit_queued_input: false,
          delete_queued_input: false,
          switch_authority: true,
          enter_acp: true,
          exit_acp: false,
        },
      },
    });
    chatApiMock.editQueuedInput.mockResolvedValue({
      entry: {
        input_id: "input-1",
        state: "queued",
        text: "edited server queued",
        created_at: "2026-07-16T00:00:01Z",
        updated_at: "2026-07-16T00:00:02Z",
        editable: true,
        deletable: true,
      },
      control: {
        active_turn: {
          phase: "streaming",
          driver: "rua",
          started_at: "2026-07-16T00:00:00Z",
          queued_count: 1,
        },
        queue: {
          entries: [
            {
              input_id: "input-1",
              state: "queued",
              text: "edited server queued",
              created_at: "2026-07-16T00:00:01Z",
              updated_at: "2026-07-16T00:00:02Z",
              editable: true,
              deletable: true,
            },
          ],
        },
        acp: { active: false },
        allowed_controls: {
          stop: true,
          queue: true,
          interject: true,
          edit_queued_input: true,
          delete_queued_input: true,
          switch_authority: false,
          enter_acp: false,
          exit_acp: false,
        },
      },
    });
    chatApiMock.sendNowQueuedInput.mockImplementation(
      async (_chatId: string, inputId: string) => ({
        input_id: inputId,
        applied: true,
        control: {
          authority_device_id: "device-current",
          placement: {
            authority_device_id: "device-current",
            authority_epoch: "0000000000000001000000000000000000000000",
            switchable: false,
          },
          active_turn: {
            turn_id: "turn-active",
            phase: "stopping",
            authority_device_id: "device-current",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-16T00:00:00Z",
            queued_count: 1,
          },
          queue: {
            paused: false,
            entries: [
              {
                input_id: inputId,
                submission_id: `submission-${inputId}`,
                state: "queued",
                text: "second",
                created_at: "2026-07-16T00:00:01Z",
                updated_at: "2026-07-16T00:00:01Z",
              },
            ],
          },
          acp: { active: false },
          allowed_controls: {
            stop: true,
            queue: true,
            interject: true,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: false,
            enter_acp: false,
            exit_acp: false,
          },
        },
      }),
    );
    providerApiMock.getActiveModels.mockResolvedValue({
      default_llm: { provider_id: "dashscope", model: "qwen-plus" },
    });
    consoleApiMock.getApprovalSnapshot.mockResolvedValue({
      pending: [],
      resolved: [],
    });
    acpApiMock.getPermissionSnapshot.mockResolvedValue({ pending: [] });
    acpApiMock.decidePermission.mockResolvedValue({});
    consoleApiMock.decideApproval.mockResolvedValue({
      id: "approval-1",
      status: "approved",
    });
    projectChatsApiMock.getChat.mockImplementation(
      async (_projectId: string, chatId: string) =>
        chatApiMock.getChat(chatId, "agent-a"),
    );
    projectChatsApiMock.stopChat.mockResolvedValue({ stopped: true });
    projectChatsApiMock.persistAcpHistoryPreview.mockResolvedValue({
      turn_id: "acp-history-preview-1",
      message_id: "acp-history-preview-message-1",
      created: true,
    });
    projectChatsApiMock.deleteQueuedInput.mockImplementation(
      async (_projectId: string, chatId: string, inputId: string) =>
        chatApiMock.deleteQueuedInput(chatId, inputId, "agent-a"),
    );
    projectChatsApiMock.editQueuedInput.mockImplementation(
      async (
        _projectId: string,
        chatId: string,
        inputId: string,
        message: string,
      ) => chatApiMock.editQueuedInput(chatId, inputId, message, "agent-a"),
    );
    projectChatsApiMock.sendNowQueuedInput.mockImplementation(
      async (
        _projectId: string,
        chatId: string,
        inputId: string,
        body: unknown,
      ) => chatApiMock.sendNowQueuedInput(chatId, inputId, body, "agent-a"),
    );
    projectChatsApiMock.getApprovalSnapshot.mockResolvedValue({
      pending: [],
      resolved: [],
    });
    projectChatsApiMock.getAcpPermissionSnapshot.mockResolvedValue({
      pending: [],
    });
    projectChatsApiMock.decideApproval.mockResolvedValue({
      id: "approval-1",
      status: "approved",
    });
    projectChatsApiMock.decideAcpPermission.mockResolvedValue({});
    projectChatsApiMock.selectVariant.mockResolvedValue({
      turn_id: "turn-1",
      selected_variant_id: "variant-2",
    });
    projectChatsApiMock.rollback.mockResolvedValue({
      chat: { id: "chat-project-1" },
      cancelled_tasks: 0,
    });
    projectChatsApiMock.branch.mockResolvedValue({
      chat: { id: "branch-project-chat" },
    });
    ownedApiMock.getPendingSelectedVariant.mockReturnValue(null);
    vi.stubGlobal("fetch", routeFetch());
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => "user-uuid" },
      configurable: true,
    });
  });

  it("attaches remote Live without waiting for an availability preflight", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "running",
      messages: [{ id: "stable-1", role: "assistant", content: "cached" }],
    });
    const fetchMock = routeFetch((url) =>
      url.includes("device-network-availability")
        ? new Response("Authentication required", { status: 401 })
        : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-b",
        currentDeviceId: "device-a",
      }),
    );

    await waitFor(() =>
      expect(result.current.history?.messages[0]?.content).toBe("cached"),
    );
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toBe(true),
    );
    const liveCall = fetchMock.mock.calls.find(([url]) =>
      String(url).includes("/console/live"),
    );
    expect(
      fetchMock.mock.calls.some(([url]) =>
        String(url).includes("device-network-availability"),
      ),
    ).toBe(false);
    expect(JSON.parse(String(liveCall?.[1]?.body))).toMatchObject({
      chat_id: "chat-1",
      viewer_id: "browser-test",
    });
  });

  it("closes the Chat Live stream when the document becomes hidden", async () => {
    const live = controlledSseResponse();
    let liveSignal: AbortSignal | undefined;
    vi.stubGlobal(
      "fetch",
      routeFetch((url, init) => {
        if (!url.includes("/console/live")) return undefined;
        liveSignal = init?.signal || undefined;
        return live.response;
      }),
    );

    renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(liveSignal).toBeDefined());

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    });
    act(() => document.dispatchEvent(new Event("visibilitychange")));

    expect(liveSignal?.aborted).toBe(true);
    live.close();
  });

  it("builds ordered user content from one placeholder per image", () => {
    expect(
      buildUserContent("before\n[Image #1]\nmiddle\n[Image #2]\nafter", [
        {
          id: "a1",
          name: "photo.png",
          type: "image",
          url: "/media/photo.png",
          marker: "[Image #1]",
          storedName: "00000000-0000-4000-8000-000000000111",
          authorityEpoch: "000001a00343124c00000000f8e40a95f10d7f0e",
          mimeType: "image/png",
          size: 42,
        },
        {
          id: "a2",
          name: "diagram.png",
          type: "image",
          url: "/media/diagram.png",
          marker: "[Image #2]",
          storedName: "00000000-0000-4000-8000-000000000222",
        },
      ]),
    ).toEqual([
      { type: "text", text: "before\n" },
      {
        type: "image",
        media_ref: "media/00000000-0000-4000-8000-000000000111",
      },
      { type: "text", text: "\nmiddle\n" },
      {
        type: "image",
        media_ref: "media/00000000-0000-4000-8000-000000000222",
      },
      { type: "text", text: "\nafter" },
    ]);
  });

  it("builds ordered user content from a converted text attachment marker", () => {
    expect(
      buildUserContent("before [File #1] after", [
        {
          id: "file-1",
          name: "pasted-text.txt",
          type: "file",
          url: "/media/pasted-text.txt",
          marker: "[File #1]",
          storedName: "00000000-0000-4000-8000-000000000333",
        },
      ]),
    ).toEqual([
      { type: "text", text: "before " },
      {
        type: "file",
        media_ref: "media/00000000-0000-4000-8000-000000000333",
      },
      { type: "text", text: " after" },
    ]);
  });

  it("still submits an attached image when its placeholder was edited away", () => {
    expect(
      buildUserContent("Describe this", [
        {
          id: "a1",
          name: "photo.png",
          type: "image",
          url: "/media/photo.png",
          marker: "[Image #1]",
          storedName: "00000000-0000-4000-8000-000000000111",
          authorityEpoch: "000001a00343124c00000000f8e40a95f10d7f0e",
        },
      ]),
    ).toEqual([
      { type: "text", text: "Describe this" },
      {
        type: "image",
        media_ref: "media/00000000-0000-4000-8000-000000000111",
      },
    ]);
  });

  it("does not submit when there is no owned conversation id", async () => {
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: undefined },
      }),
    );

    await act(async () => {
      await result.current.submit("hello");
    });

    expect(fetch).not.toHaveBeenCalled();
  });

  it.each(["agent", "project"] as const)(
    "does not query ACP permissions for an ordinary %s conversation",
    async (scope) => {
      const error = new Error(
        "remote ACP control failed: Device removed-device is not paired",
      );
      acpApiMock.getPermissionSnapshot.mockRejectedValue(error);
      projectChatsApiMock.getAcpPermissionSnapshot.mockRejectedValue(error);
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address:
            scope === "agent"
              ? { scope, agentId: "agent-a", chatId: "chat-1" }
              : { scope, projectId: "project-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(result.current.history).not.toBeNull());
      await waitFor(() =>
        expect(
          scope === "agent"
            ? consoleApiMock.getApprovalSnapshot
            : projectChatsApiMock.getApprovalSnapshot,
        ).toHaveBeenCalled(),
      );
      expect(acpApiMock.getPermissionSnapshot).not.toHaveBeenCalled();
      expect(
        projectChatsApiMock.getAcpPermissionSnapshot,
      ).not.toHaveBeenCalled();
      expect(result.current.error).toBeNull();
    },
  );

  it("still reports ACP permission failures when ACP is enabled", async () => {
    acpApiMock.getPermissionSnapshot.mockRejectedValueOnce(
      new Error("ACP device is offline"),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        activeAcpRunner: "codex",
      }),
    );
    await waitFor(() =>
      expect(result.current.error).toBe("ACP device is offline"),
    );
    expect(acpApiMock.getPermissionSnapshot).toHaveBeenCalledWith(
      "agent-a",
      "chat-1",
    );
  });

  it("uses inactive server ACP control over a stale runner selection", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      control: { acp: { active: false } },
    });
    const { result, rerender } = renderHook(
      ({ runner }: { runner: string | null }) =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          activeAcpRunner: runner,
        }),
      { initialProps: { runner: null } },
    );
    await waitFor(() =>
      expect(result.current.history?.control?.acp.active).toBe(false),
    );
    rerender({ runner: "codex" });
    expect(acpApiMock.getPermissionSnapshot).not.toHaveBeenCalled();
  });

  it("renders stable local history without waiting for approval state", async () => {
    const approval = deferred<{
      pending: never[];
      resolved: never[];
    }>();
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      messages: [
        { id: "stable-1", role: "assistant", content: "local history" },
      ],
    });
    consoleApiMock.getApprovalSnapshot.mockReturnValueOnce(approval.promise);

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );

    await waitFor(() =>
      expect(result.current.history?.messages).toEqual([
        { id: "stable-1", role: "assistant", content: "local history" },
      ]),
    );
    expect(result.current.loading).toBe(false);

    approval.resolve({ pending: [], resolved: [] });
  });

  it("keeps stable local history when the passive live viewer fails", async () => {
    const firstLiveStream = controlledSseResponse();
    const secondLiveStream = controlledSseResponse();
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      messages: [
        { id: "stable-1", role: "assistant", content: "local history" },
      ],
    });
    vi.stubGlobal(
      "fetch",
      routeFetchWithLiveStreams(
        firstLiveStream.response,
        secondLiveStream.response,
      ),
    );

    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() =>
      expect(result.current.history?.messages).toEqual([
        { id: "stable-1", role: "assistant", content: "local history" },
      ]),
    );
    firstLiveStream.send({
      type: "conversation_control",
      activity_generation: "generation-a",
      activity_sequence: 41,
    });
    await act(async () => {
      await Promise.resolve();
    });

    await act(async () => {
      firstLiveStream.error(new Error("remote Live unavailable"));
    });
    await waitFor(
      () =>
        expect(
          (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
            String(url).includes("/console/live"),
          ).length,
        ).toBe(2),
      { timeout: 2_500 },
    );
    expect(result.current.history?.messages).toEqual([
      { id: "stable-1", role: "assistant", content: "local history" },
    ]);
    const liveRequests = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
      ([url]) => String(url).includes("/console/live"),
    );
    expect(JSON.parse(String(liveRequests[0]?.[1]?.body))).not.toHaveProperty(
      "activity_generation",
    );
    expect(JSON.parse(String(liveRequests[1]?.[1]?.body))).toMatchObject({
      activity_generation: "generation-a",
      next_activity_sequence: 42,
    });

    unmount();
    secondLiveStream.close();
  });

  it("keeps newer live control when initial stable history resolves later", async () => {
    const stableHistory = deferred<{
      messages: Array<{ id: string; role: string; content: string }>;
    }>();
    const liveStream = controlledSseResponse();
    chatApiMock.getChat.mockReturnValueOnce(stableHistory.promise);
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );

    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toBe(true),
    );

    await act(async () => {
      liveStream.send({
        type: "conversation_control",
        reason: "viewer_attached",
        control: {
          revision: 3,
          authority_device_id: "device-b",
          placement: {
            authority_device_id: "device-b",
            authority_epoch: "0000000000000002000000000000000000000000",
            switchable: false,
          },
          active_turn: {
            turn_id: "turn-live",
            phase: "streaming",
            authority_device_id: "device-b",
            authority_epoch: "0000000000000002000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-30T00:00:00Z",
          },
          queue: { entries: [], paused: false },
          acp: { active: false },
          allowed_controls: {
            stop: true,
            queue: true,
            interject: true,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: false,
            enter_acp: false,
            exit_acp: false,
          },
        },
      });
    });
    await waitFor(() =>
      expect(result.current.history?.control?.revision).toBe(3),
    );

    await act(async () => {
      stableHistory.resolve({
        messages: [
          { id: "stable-1", role: "assistant", content: "local history" },
        ],
      });
    });
    await waitFor(() =>
      expect(result.current.history?.messages).toEqual([
        { id: "stable-1", role: "assistant", content: "local history" },
      ]),
    );
    expect(result.current.history?.control?.revision).toBe(3);
    expect(result.current.history?.active_turn?.turn_id).toBe("turn-live");
    expect(result.current.history?.status).toBe("running");

    liveStream.close();
    unmount();
  });

  describe("durable Turn terminal precedence", () => {
    const authorityEpoch = "0000000000000002000000000000000000000000";
    const terminalHistory = {
      messages: [
        {
          id: "user-stopped",
          role: "user",
          turn_id: "turn-stopped",
          turn_status: "interrupted",
          content: "Inspect the order",
        },
        {
          id: "assistant-stopped",
          role: "assistant",
          turn_id: "turn-stopped",
          turn_status: "interrupted",
          status: "completed",
          started_at: "2026-07-30T00:00:01Z",
          finished_at: "2026-07-30T00:01:00Z",
          content: "Saved output before Stop",
        },
        {
          id: "tool-stopped",
          role: "tool",
          type: "plugin_call_output",
          turn_id: "turn-stopped",
          turn_status: "interrupted",
          status: "completed",
          finished_at: "2026-07-30T00:01:00Z",
          content: "Saved tool output",
        },
      ],
    };
    const runningControl = (revision: number, startedAt: string) => ({
      type: "conversation_control",
      reason: "viewer_attached",
      control: {
        revision,
        authority_device_id: "device-b",
        placement: {
          authority_device_id: "device-b",
          authority_epoch: authorityEpoch,
          switchable: false,
        },
        active_turn: {
          turn_id: "turn-stopped",
          user_message_id: "user-stopped",
          phase: "streaming",
          authority_device_id: "device-b",
          authority_epoch: authorityEpoch,
          driver: "rua",
          started_at: startedAt,
        },
        queue: { entries: [], paused: false },
        acp: { active: false },
        allowed_controls: {
          stop: true,
          queue: true,
          interject: true,
          edit_queued_input: false,
          delete_queued_input: false,
          switch_authority: false,
          enter_acp: false,
          exit_acp: false,
        },
      },
    });

    it("closes synced interrupted execution without a live idle update and rejects late running control", async () => {
      const history = deferred<typeof terminalHistory>();
      const live = controlledSseResponse();
      chatApiMock.getChat.mockReturnValue(history.promise);
      vi.stubGlobal("fetch", routeFetchWithLiveStreams(live.response));
      const { result, unmount } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          currentDeviceId: "device-a",
        }),
      );
      try {
        await waitFor(() =>
          expect(
            (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
              String(url).includes("/console/live"),
            ),
          ).toBe(true),
        );
        await act(async () => {
          live.send(runningControl(3, "2026-07-30T00:00:00Z"));
        });
        await waitFor(() => expect(isTurnActive(result)).toBe(true));
        await act(async () => {
          live.send({
            type: "user_message",
            message: {
              id: "user-stopped",
              role: "user",
              turn_id: "turn-stopped",
              content: "Inspect the order",
            },
          });
          live.send({
            type: "text_delta",
            turn_id: "turn-stopped",
            message_id: "assistant-stopped",
            delta: "Saved output",
          });
        });
        await waitFor(() =>
          expect(JSON.stringify(result.current.projection.items)).toContain(
            "Saved output",
          ),
        );

        await act(async () => history.resolve(terminalHistory));
        await waitFor(() => {
          expect(result.current.history?.messages[1]?.id).toBe(
            "assistant-stopped",
          );
          expect(isTurnActive(result)).toBe(false);
        });
        expect(result.current.history?.messages[1]?.content).toBe(
          "Saved output before Stop",
        );
        expect(result.current.history?.active_turn).toBeNull();
        expect(result.current.history?.control?.active_turn).toBeNull();
        expect(
          result.current.projection.items.find(
            (item) => item.kind === "assistant",
          ),
        ).toMatchObject({ status: "interrupted" });
        expect(result.current.history?.messages[2]?.content).toBe(
          "Saved tool output",
        );
        expect(JSON.stringify(result.current.projection.items)).toContain(
          "Saved output before Stop",
        );
        expect(JSON.stringify(result.current.projection.items)).toContain(
          "Saved tool output",
        );

        await act(async () => {
          live.send(runningControl(4, "2026-07-30T00:00:00Z"));
        });
        await waitFor(() =>
          expect(result.current.history?.control?.revision).toBe(4),
        );
        expect(isTurnActive(result)).toBe(false);
        expect(result.current.history?.control?.active_turn).toBeNull();
        expect(result.current.history?.messages[1]?.content).toBe(
          "Saved output before Stop",
        );
        const historyReadsBeforeReplay = chatApiMock.getChat.mock.calls.length;
        await act(async () => {
          live.send({
            type: "user_message",
            message: {
              id: "user-stopped",
              role: "user",
              turn_id: "turn-stopped",
              content: "Inspect the order",
            },
          });
          live.send({
            type: "text_delta",
            turn_id: "turn-stopped",
            message_id: "assistant-stopped",
            delta: "Late stale output",
          });
        });
        await waitFor(() =>
          expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThan(
            historyReadsBeforeReplay,
          ),
        );
        await waitFor(() => expect(isTurnActive(result)).toBe(false));
        expect(JSON.stringify(result.current.projection.items)).toContain(
          "Saved output before Stop",
        );
        expect(JSON.stringify(result.current.projection.items)).not.toContain(
          "Late stale output",
        );
        expect(chatApiMock.stopChat).not.toHaveBeenCalled();
      } finally {
        unmount();
      }
    });

    it("keeps newer regeneration active when earlier terminal history arrives for the same Turn", async () => {
      const history = deferred<typeof terminalHistory>();
      const live = controlledSseResponse();
      chatApiMock.getChat.mockReturnValueOnce(history.promise);
      vi.stubGlobal("fetch", routeFetchWithLiveStreams(live.response));
      const { result, unmount } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          currentDeviceId: "device-a",
        }),
      );
      try {
        await waitFor(() =>
          expect(
            (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
              String(url).includes("/console/live"),
            ),
          ).toBe(true),
        );
        await act(async () => {
          live.send(runningControl(5, "2026-07-30T00:02:00Z"));
        });
        await waitFor(() => expect(isTurnActive(result)).toBe(true));
        await act(async () => history.resolve(terminalHistory));
        await waitFor(() =>
          expect(result.current.history?.messages[1]?.id).toBe(
            "assistant-stopped",
          ),
        );
        expect(isTurnActive(result)).toBe(true);
        expect(result.current.history?.control?.active_turn?.started_at).toBe(
          "2026-07-30T00:02:00Z",
        );
        expect(result.current.history?.messages[1]?.content).toBe(
          "Saved output before Stop",
        );
        expect(chatApiMock.stopChat).not.toHaveBeenCalled();
      } finally {
        unmount();
      }
    });
  });

  it("submits input without treating the viewer model snapshot as authority", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({ status: "idle", messages: [] });
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("hello");
    });

    expect(providerApiMock.getActiveModels).not.toHaveBeenCalled();
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(1);
    const submitCall = (fetch as ReturnType<typeof vi.fn>).mock.calls.find(
      ([url]) => String(url).includes("/console/submit"),
    );
    const body = JSON.parse(String(submitCall?.[1]?.body || "{}"));
    expect(body.model).toBeUndefined();
    expect(body.chat_model_slot).toBeUndefined();
    expect(result.current.error).toBeNull();
  });

  it("reconciles a missed terminal state after activity recovery while Live remains attached", async () => {
    const live = controlledSseResponse();
    vi.stubGlobal("fetch", routeFetchWithLiveStreams(live.response));
    chatApiMock.getChat.mockResolvedValue({
      status: "running",
      messages: [],
      control: {
        revision: 1,
        placement: { authority_epoch: "epoch" },
        active_turn: {
          turn_id: "turn-recovery",
          phase: "streaming",
          authority_epoch: "epoch",
          driver: "rua",
        },
      },
    });
    const { result, rerender, unmount } = renderHook(
      ({ recoverySequence, wakeAgentId }) =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          activityWake: {
            type: "activity_reconnected",
            sequence: recoverySequence,
            recoverySequence,
            chat_id: "chat-1",
            address: {
              scope: { kind: "agent", agent_id: wakeAgentId },
              chat_id: "chat-1",
            },
          },
        }),
      { initialProps: { recoverySequence: 0, wakeAgentId: "agent-a" } },
    );
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    const reads = chatApiMock.getChat.mock.calls.length;
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      control: {
        revision: 2,
        placement: { authority_epoch: "epoch" },
        active_turn: null,
      },
    });
    await act(async () => {
      rerender({ recoverySequence: 1, wakeAgentId: "another-agent" });
    });
    expect(chatApiMock.getChat).toHaveBeenCalledTimes(reads);
    expect(isTurnActive(result)).toBe(true);
    rerender({ recoverySequence: 1, wakeAgentId: "agent-a" });
    await waitFor(() =>
      expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThan(reads),
    );
    await waitFor(() => expect(isTurnActive(result)).toBe(false));
    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
    unmount();
  });

  it("submits after the committed chat model is synchronized from the canonical update", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({ status: "idle", messages: [] });
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => {
      result.current.applyCommittedModel({
        provider_id: "dashscope",
        model: "qwen-plus",
        thinking_strength: "auto",
        thinking_param_style: "budget",
      });
    });

    await act(async () => {
      await result.current.submit("hello");
    });

    expect(providerApiMock.getActiveModels).not.toHaveBeenCalled();
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(1);
  });

  it("submits an unmaterialized Chat without loading history first", async () => {
    const selection = {
      runner: "codex",
      cwd: "/workspace",
      resume_session_id: "resume-1",
    };
    saveDraftConversationAcp("agent-a", "lazy-chat", selection);
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "lazy-chat" },
        activityWake: 0,
        unmaterialized: true,
      }),
    );

    act(() => {
      result.current.applyCommittedModel(configuredModel);
    });
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => {
      await result.current.submit("use the inherited model");
    });

    expect(chatApiMock.getChat).not.toHaveBeenCalled();
    const submitCall = (fetch as ReturnType<typeof vi.fn>).mock.calls.find(
      ([url]) => String(url).includes("/console/submit"),
    );
    expect(JSON.parse(String(submitCall?.[1]?.body || "{}"))).toMatchObject({
      chat_id: "lazy-chat",
      model: configuredModel,
      initial_acp: selection,
    });
    expect(JSON.parse(String(submitCall?.[1]?.body || "{}")).model).not.toEqual(
      configuredModel.model,
    );
  });

  it("retains unknown submissions across remount without creating another identity", async () => {
    let identity = 0;
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => `unknown-${identity++}` },
      configurable: true,
    });
    const key = "agent-a:unknown-chat";
    clearSubmitted(key);
    vi.stubGlobal(
      "fetch",
      vi.fn().mockRejectedValue(new TypeError("network lost")),
    );
    const first = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "unknown-chat" },
        activityWake: 0,
        unmaterialized: true,
      }),
    );
    act(() => first.result.current.applyCommittedModel(configuredModel));
    await act(async () => {
      expect(await first.result.current.submit("uncertain input")).toBe(
        "unknown",
      );
    });
    const saved = loadSubmitted(key);
    expect(saved?.submissionId).toBeTruthy();
    expect(saved?.text).toBe("uncertain input");
    first.unmount();
    const restored = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "unknown-chat" },
        activityWake: 0,
        unmaterialized: true,
      }),
    );
    await waitFor(() =>
      expect(restored.result.current.queuedSubmissions).toHaveLength(1),
    );
    expect(restored.result.current.queuedSubmissions[0]).toMatchObject({
      submissionId: saved?.submissionId,
      deliveryState: "unknown",
      editable: false,
      deletable: false,
    });
    const calls = (fetch as ReturnType<typeof vi.fn>).mock.calls.length;
    await act(async () => {
      expect(await restored.result.current.submit("another input")).toBe(
        "unknown",
      );
    });
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.length,
    ).toBeGreaterThan(calls);
    expect(loadSubmitted(key, saved?.submissionId)?.text).toBe(
      "uncertain input",
    );
    restored.unmount();
    clearSubmitted(key);
  });

  it.each(["agent", "project"] as const)(
    "retains an explicit unconfirmed %s input until history confirms its original identity",
    async (scope) => {
      const address =
        scope === "agent"
          ? { scope, agentId: "agent-a", chatId: "unconfirmed-chat" }
          : { scope, projectId: "project-1", chatId: "unconfirmed-chat" };
      const key = `${
        scope === "agent" ? "agent-a" : "project:project-1"
      }:unconfirmed-chat`;
      const getHistory =
        scope === "agent" ? chatApiMock.getChat : projectChatsApiMock.getChat;
      getHistory.mockResolvedValue({ status: "idle", messages: [] });
      vi.stubGlobal(
        "fetch",
        routeFetch((url, init) => {
          if (!url.includes("/console/submit")) return undefined;
          const request = JSON.parse(String(init?.body));
          return jsonResponse({
            accepted: false,
            mode: "unconfirmed",
            failure_kind: "delivery_unknown",
            detail: "Acceptance reply was not received",
            chat_id: "unconfirmed-chat",
            submission_id: request.submission_id,
            device_delivery_id: "delivery-unconfirmed",
            device_delivery_sequence: 1,
            target_device_id: "device-remote",
            input_id: null,
            accepted_at: null,
            control: null,
          });
        }),
      );
      const options = {
        address,
        unmaterialized: true,
        authorityPlacementDeviceId: "device-remote",
        projectExecutionAgentId: "agent-a",
      };
      const first = renderHook(() => useConversationViewRuntime(options));
      act(() => first.result.current.applyCommittedModel(configuredModel));
      await act(async () => {
        expect(
          await first.result.current.submit("wait for this acceptance"),
        ).toBe("unknown");
      });
      const saved = loadSubmitted(key);
      expect(saved?.submissionId).toBeTruthy();
      expect(first.result.current.queuedSubmissions).toEqual([
        expect.objectContaining({
          submissionId: saved?.submissionId,
          deliveryState: "unknown",
          editable: false,
          deletable: false,
        }),
      ]);
      first.unmount();

      getHistory.mockResolvedValue({
        status: "idle",
        messages: [
          {
            id: "durable-unconfirmed-user",
            role: "user",
            content: [{ type: "text", text: "wait for this acceptance" }],
            metadata: { submission_id: saved?.submissionId },
          },
        ],
      });
      const restored = renderHook(() => useConversationViewRuntime(options));
      await waitFor(() => expect(loadSubmitted(key)).toBeNull());
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
          String(url).includes("/console/submit"),
        ),
      ).toHaveLength(1);
      restored.unmount();
      clearSubmitted(key);
    },
  );

  it("confirms a restored submission from read-only Chat history and clears only its record", async () => {
    const key = "agent-a:confirmed-chat";
    saveSubmitted(key, {
      submissionId: "submission-confirmed",
      text: "accepted input",
      attachments: [],
      request: { submission_id: "submission-confirmed" },
      targetDeviceId: null,
    });
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [
        {
          id: "durable-user",
          role: "user",
          content: [{ type: "text", text: "accepted input" }],
          metadata: { submission_id: "submission-confirmed" },
        },
      ],
    });
    const view = renderHook(() =>
      useConversationViewRuntime({
        address: {
          scope: "agent",
          agentId: "agent-a",
          chatId: "confirmed-chat",
        },
        activityWake: 0,
        unmaterialized: true,
      }),
    );
    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalled());
    await waitFor(() => expect(loadSubmitted(key)).toBeNull());
    expect(view.result.current.queuedSubmissions).toEqual([]);
    view.unmount();
  });

  it("starts Project history only after an unmaterialized Submit is accepted", async () => {
    sessionStorage.setItem(
      "rua_project_chat_directory_project-1:chat-1",
      "obsolete-directory",
    );
    const materializedHistory = deferred<{
      status: "idle";
      messages: never[];
    }>();
    projectChatsApiMock.getChat.mockReturnValue(materializedHistory.promise);
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/projects/project-1/chats/chat-1/console/submit")) {
          return submitAcceptedResponse();
        }
        if (url.includes("/projects/project-1/chats/chat-1/console/live")) {
          return new Response(null, { status: 200 });
        }
        return undefined;
      }),
    );

    const { result, rerender, unmount } = renderHook(
      ({ unmaterialized }: { unmaterialized: boolean }) =>
        useConversationViewRuntime({
          address: {
            scope: "project",
            projectId: {
              scope: "project",
              projectId: "project-1",
            }.projectId,
            chatId: "chat-1",
          },
          activityWake: 0,
          projectExecutionAgentId: "agent-a",
          unmaterialized: unmaterialized,
        }),
      { initialProps: { unmaterialized: true } },
    );

    try {
      act(() => {
        result.current.applyCommittedModel(configuredModel);
      });
      expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
          String(url).includes("/projects/project-1/chats/chat-1/console/live"),
        ),
      ).toHaveLength(0);
      await act(async () => {
        await result.current.submit("keep this visible");
      });

      expect(JSON.stringify(result.current.projection.items)).not.toContain(
        "keep this visible",
      );
      expect(result.current.queuedSubmissions).toHaveLength(0);
      const submitCall = (fetch as ReturnType<typeof vi.fn>).mock.calls.find(
        ([url]) =>
          String(url).includes(
            "/projects/project-1/chats/chat-1/console/submit",
          ),
      );
      expect(JSON.parse(String(submitCall?.[1]?.body || "{}"))).toMatchObject({
        agent_id: "agent-a",
        model: configuredModel,
      });
      expect(
        JSON.parse(String(submitCall?.[1]?.body || "{}")),
      ).not.toHaveProperty("project_working_directory_id");
      rerender({ unmaterialized: false });

      await waitFor(() =>
        expect(projectChatsApiMock.getChat).toHaveBeenCalledWith(
          "project-1",
          "chat-1",
          { turnLimit: 20 },
        ),
      );
      expect(JSON.stringify(result.current.projection.items)).not.toContain(
        "keep this visible",
      );

      await act(async () => {
        materializedHistory.resolve({ status: "idle", messages: [] });
        await materializedHistory.promise;
      });
      expect(result.current.history?.model).toEqual(configuredModel);
      expect(JSON.stringify(result.current.projection.items)).not.toContain(
        "keep this visible",
      );
    } finally {
      unmount();
    }
  });

  it("submits text and attachments to the selected agent console-chat route", async () => {
    ownedApiMock.getPendingSelectedVariant.mockReturnValue({
      turn_id: "turn-1",
      selected_variant_id: "variant-2",
    });
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(chatApiMock.getChat).toHaveBeenCalledWith("chat-1", "agent-a", {
      turnLimit: 20,
    });

    await act(async () => {
      await result.current.submit("  hello  ", [
        {
          id: "attachment-1",
          name: "photo.png",
          type: "image",
          url: "/uploads/photo.png",
          storedName: "00000000-0000-4000-8000-000000000111",
          authorityEpoch: "000001a00343124c00000000f8e40a95f10d7f0e",
        },
      ]);
    });

    expect(fetch).toHaveBeenCalledWith(
      "/api/agents/agent-a/chats/chat-1/console/submit",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          "Content-Type": "application/json",
          Authorization: "Bearer test-token",
        }),
      }),
    );
    const submitCall = (fetch as ReturnType<typeof vi.fn>).mock.calls.find(
      ([url]) => String(url).includes("/console/submit"),
    );
    if (!submitCall) throw new Error("missing submit call");
    const body = JSON.parse(submitCall[1].body);
    expect(body).toMatchObject({
      chat_id: "chat-1",
      user_id: "default",
      channel: "console",
      stream: true,
      turn_id: "turn-1",
      selected_variant_id: "variant-2",
      expected_authority_epoch: "000001a00343124c00000000f8e40a95f10d7f0e",
    });
    expect(body.input[0]).toMatchObject({
      id: "user-user-uuid",
      role: "user",
    });
    expect(body.input[0].content).toEqual([
      { type: "text", text: "hello" },
      {
        type: "image",
        media_ref: "media/00000000-0000-4000-8000-000000000111",
      },
    ]);
    expect(ownedApiMock.clearPendingSelectedVariant).toHaveBeenCalledWith(
      "agent-a:chat-1",
    );
  });

  it("rejects pending attachments from different Authority epochs before submit", async () => {
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    const submitCallsBefore = (
      fetch as ReturnType<typeof vi.fn>
    ).mock.calls.filter(([url]) =>
      String(url).includes("/console/submit"),
    ).length;
    let accepted = true;
    await act(async () => {
      accepted = await result.current.submit("hello", [
        {
          id: "attachment-1",
          name: "one.png",
          type: "image",
          url: "/media/one",
          storedName: "00000000-0000-4000-8000-000000000112",
          authorityEpoch: "epoch-one",
        },
        {
          id: "attachment-2",
          name: "two.png",
          type: "image",
          url: "/media/two",
          storedName: "00000000-0000-4000-8000-000000000113",
          authorityEpoch: "epoch-two",
        },
      ]);
    });

    expect(accepted).toBe(false);
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(submitCallsBefore);
  });

  it("preserves attachments through Project authority submission", async () => {
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (
          url.includes(
            "/projects/project-1/chats/chat-project-1/console/submit",
          )
        ) {
          return submitAcceptedResponse();
        }
        if (
          url.includes("/projects/project-1/chats/chat-project-1/console/live")
        ) {
          return sseResponse([
            { object: "response", status: "completed", output: [] },
          ]);
        }
        return undefined;
      }),
    );
    const target = {
      scope: "project" as const,
      projectId: "project-1",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("inspect image", [
        {
          id: "attachment-project-1",
          name: "project.png",
          type: "image",
          url: "/uploads/project.png",
          storedName: "stored-project.png",
        },
      ]);
    });

    const submitCall = (fetch as ReturnType<typeof vi.fn>).mock.calls.find(
      ([url]) =>
        String(url).includes(
          "/projects/project-1/chats/chat-project-1/console/submit",
        ),
    );
    if (!submitCall) throw new Error("missing Project submit call");
    const body = JSON.parse(submitCall[1].body);
    expect(body).toMatchObject({
      chat_id: "chat-project-1",
      agent_id: "agent-a",
    });
    expect(body.input[0].content).toEqual([
      { type: "text", text: "inspect image" },
      {
        type: "image",
        media_ref: "media/stored-project.png",
      },
    ]);
  });

  it("emits a submitted event only after the submit is accepted", async () => {
    const submitResponse = deferred<Response>();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/submit") ? submitResponse.promise : undefined,
      ),
    );
    const onSubmitted = vi.fn();
    window.addEventListener(CHAT_SUBMITTED_EVENT, onSubmitted);

    try {
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));

      let submitPromise: Promise<boolean> | undefined;
      act(() => {
        submitPromise = result.current.submit("hello");
      });
      await waitFor(() =>
        expect(
          (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
            String(url).includes("/console/submit"),
          ),
        ).toBe(true),
      );
      expect(onSubmitted).not.toHaveBeenCalled();
      expect(result.current.queuedSubmissions).toHaveLength(0);

      submitResponse.resolve(submitAcceptedResponse());
      await act(async () => {
        await submitPromise;
      });

      expect(onSubmitted).toHaveBeenCalledTimes(1);
      expect(onSubmitted.mock.calls[0][0].detail).toMatchObject({
        agentId: "agent-a",
        chatId: "chat-1",
        title: "hello",
      });
    } finally {
      window.removeEventListener(CHAT_SUBMITTED_EVENT, onSubmitted);
    }
  });

  it("does not emit a submitted event when the submit is rejected", async () => {
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/submit")
          ? new Response("nope", { status: 500 })
          : undefined,
      ),
    );
    const onSubmitted = vi.fn();
    window.addEventListener(CHAT_SUBMITTED_EVENT, onSubmitted);

    try {
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));

      await act(async () => {
        await result.current.submit("hello");
      });

      expect(onSubmitted).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener(CHAT_SUBMITTED_EVENT, onSubmitted);
    }
  });

  it("tracks concurrent submit acknowledgements independently by submission id", async () => {
    let uuidSequence = 0;
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => `concurrent-${uuidSequence++}` },
      configurable: true,
    });
    const firstResponse = deferred<Response>();
    const secondResponse = deferred<Response>();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (!url.includes("/console/submit")) return undefined;
        submitCount += 1;
        return submitCount === 1
          ? firstResponse.promise
          : secondResponse.promise;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<boolean> | undefined;
    let secondSubmit: Promise<boolean> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
      secondSubmit = result.current.submit("second");
    });
    await waitFor(() => expect(submitCount).toBe(2));

    const submitCalls = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
      ([url]) => String(url).includes("/console/submit"),
    );
    const firstBody = JSON.parse(String(submitCalls[0][1].body));
    const secondBody = JSON.parse(String(submitCalls[1][1].body));
    expect(firstBody.submission_id).toMatch(/^submission-/);
    expect(secondBody.submission_id).toMatch(/^submission-/);
    expect(secondBody.submission_id).not.toBe(firstBody.submission_id);

    secondResponse.resolve(
      submitAcceptedResponse({ submission_id: secondBody.submission_id }),
    );
    await expect(secondSubmit).resolves.toBe(true);

    firstResponse.resolve(
      submitAcceptedResponse({ submission_id: firstBody.submission_id }),
    );
    await expect(firstSubmit).resolves.toBe(true);
  });

  it("does not apply an old chat submit callback to the newly viewed chat", async () => {
    const submitResponse = deferred<Response>();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/submit") ? submitResponse.promise : undefined,
      ),
    );
    const { result, rerender } = renderHook(
      ({ chatId }) =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: chatId },
        }),
      { initialProps: { chatId: "chat-1" } },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let submitPromise: Promise<boolean> | undefined;
    act(() => {
      submitPromise = result.current.submit("old chat input");
    });
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/submit"),
        ),
      ).toBe(true),
    );

    rerender({ chatId: "chat-2" });
    submitResponse.resolve(new Response("offline", { status: 503 }));
    await expect(submitPromise).resolves.toBe(false);
    expect(result.current.error).toBeNull();
  });

  it("locates an old turn without discarding current history and fills the intervening gap", async () => {
    const page = (first: number, last: number) => ({
      status: "idle",
      model: configuredModel,
      oldest_turn_ordinal: first,
      newest_turn_ordinal: last,
      has_more_before: first > 1,
      messages: Array.from({ length: last - first + 1 }, (_, index) => [
        {
          id: "u" + (first + index),
          turn_id: "t" + (first + index),
          role: "user",
          content: "Question " + (first + index),
        },
        {
          id: "a" + (first + index),
          turn_id: "t" + (first + index),
          role: "assistant",
          content: "Answer " + (first + index),
        },
      ]).flat(),
    });
    chatApiMock.getChat.mockResolvedValueOnce(page(21, 40));
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    chatApiMock.getChat.mockResolvedValueOnce(page(1, 10));
    await act(async () => {
      await result.current.locateTurn("t5");
    });
    expect(chatApiMock.getChat).toHaveBeenLastCalledWith("chat-1", "agent-a", {
      aroundTurnId: "t5",
      turnLimit: 20,
    });
    expect(
      result.current.projection.items.some((item) => item.id === "u5"),
    ).toBe(true);
    expect(
      result.current.projection.items.some((item) => item.id === "u40"),
    ).toBe(true);
    expect(result.current.historyGap).toMatchObject({
      beforeMessageId: "u21",
      count: 10,
    });
    chatApiMock.getChat.mockResolvedValueOnce(page(1, 20));
    await act(async () => {
      await result.current.loadHistoryGap();
    });
    expect(result.current.historyGap).toBeNull();
    expect(
      result.current.history?.messages.filter((item) => item.role === "user"),
    ).toHaveLength(40);
  });

  it("keeps already loaded older turns when a latest-page refresh arrives", async () => {
    const page = (first: number, last: number) => ({
      status: "idle",
      model: configuredModel,
      oldest_turn_ordinal: first,
      newest_turn_ordinal: last,
      has_more_before: first > 1,
      messages: Array.from({ length: last - first + 1 }, (_, index) => [
        {
          id: "u" + (first + index),
          turn_id: "t" + (first + index),
          role: "user",
          content: "Question " + (first + index),
        },
        {
          id: "a" + (first + index),
          turn_id: "t" + (first + index),
          role: "assistant",
          content: "Answer " + (first + index),
        },
      ]).flat(),
    });
    chatApiMock.getChat.mockResolvedValueOnce(page(21, 40));
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    chatApiMock.getChat.mockResolvedValueOnce(page(1, 20));
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.history?.messages).toHaveLength(80);
    chatApiMock.getChat.mockResolvedValueOnce(page(22, 41));
    act(() => result.current.reload());
    await waitFor(() =>
      expect(
        result.current.history?.messages.some((item) => item.id === "u41"),
      ).toBe(true),
    );
    expect(result.current.history?.messages[0].id).toBe("u1");
    expect(result.current.history?.messages).toHaveLength(82);
  });

  it("loads older complete turn pages before the current history", async () => {
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        has_more_before: true,
        oldest_turn_ordinal: 3,
        newest_turn_ordinal: 4,
        messages: [
          { id: "user-3", role: "user", content: "third" },
          { id: "assistant-3", role: "assistant", content: "third answer" },
          { id: "user-4", role: "user", content: "fourth" },
          { id: "assistant-4", role: "assistant", content: "fourth answer" },
        ],
      })
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        has_more_before: false,
        oldest_turn_ordinal: 1,
        newest_turn_ordinal: 2,
        messages: [
          { id: "user-1", role: "user", content: "first" },
          { id: "assistant-1", role: "assistant", content: "first answer" },
          { id: "user-2", role: "user", content: "second" },
          { id: "assistant-2", role: "assistant", content: "second answer" },
        ],
      });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.loadOlder();
    });

    expect(chatApiMock.getChat).toHaveBeenNthCalledWith(
      1,
      "chat-1",
      "agent-a",
      {
        turnLimit: 20,
      },
    );
    expect(chatApiMock.getChat).toHaveBeenNthCalledWith(
      2,
      "chat-1",
      "agent-a",
      {
        turnLimit: 20,
        beforeTurnOrdinal: 3,
      },
    );
    expect(
      result.current.history?.messages.map((message) => message.id),
    ).toEqual([
      "user-1",
      "assistant-1",
      "user-2",
      "assistant-2",
      "user-3",
      "assistant-3",
      "user-4",
      "assistant-4",
    ]);
    expect(result.current.hasMoreBefore).toBe(false);
  });

  it("shows cached chat history across remounts while revalidating in the background", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      model: configuredModel,
      title: "Cached chat",
      has_more_before: false,
      oldest_turn_ordinal: 1,
      newest_turn_ordinal: 1,
      messages: [{ id: "user-1", role: "user", content: "cached message" }],
    });

    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(
      result.current.history?.messages.map((message) => message.id),
    ).toEqual(["user-1"]);

    unmount();
    chatApiMock.getChat.mockReturnValueOnce(new Promise(() => undefined));
    consoleApiMock.getApprovalSnapshot.mockReturnValueOnce(
      new Promise(() => undefined),
    );

    const { result: remount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );

    expect(remount.current.loading).toBe(false);
    expect(remount.current.history?.title).toBe("Cached chat");
    expect(
      remount.current.history?.messages.map((message) => message.id),
    ).toEqual(["user-1"]);
    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalledTimes(2));
  });

  it("keeps the latest eight cached conversations when older entries are replaced", () => {
    for (let index = 0; index < 8; index += 1) {
      primeChatViewCache("agent-a", `cached-${index}`, {
        title: `Chat ${index}`,
        messages: [],
      });
    }
    primeChatViewCache("agent-a", "cached-0", {
      title: "Updated chat",
      messages: [],
    });
    primeChatViewCache("agent-a", "cached-8", {
      title: "Newest chat",
      messages: [],
    });

    expect(readChatViewTitleCache("agent-a", "cached-0")).toBe("Updated chat");
    expect(readChatViewTitleCache("agent-a", "cached-1")).toBeNull();
    expect(readChatViewTitleCache("agent-a", "cached-8")).toBe("Newest chat");
  });

  it.each([
    [
      "Android browser",
      "Mozilla/5.0 (Linux; Android 15) Chrome/125",
      "Linux armv8l",
      5,
      false,
    ],
    [
      "iPhone browser",
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)",
      "iPhone",
      5,
      false,
    ],
    [
      "iPad desktop browser",
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) Version/18 Safari/605",
      "MacIntel",
      5,
      false,
    ],
    [
      "Windows touch laptop",
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
      "Win32",
      10,
      true,
    ],
    [
      "Mac browser",
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)",
      "MacIntel",
      0,
      true,
    ],
    [
      "Linux browser",
      "Mozilla/5.0 (X11; Linux x86_64)",
      "Linux x86_64",
      0,
      true,
    ],
    [
      "ChromeOS browser",
      "Mozilla/5.0 (X11; CrOS x86_64 16181.0.0)",
      "Linux x86_64",
      0,
      true,
    ],
    ["unknown host", "Mozilla/5.0", "", 0, false],
  ])(
    "uses the appropriate cache budget for %s",
    (_label, userAgent, platform, maxTouchPoints, desktop) => {
      cacheHostMock.isMobile.mockReturnValue(false);
      const originalNavigator = navigator;
      vi.stubGlobal("navigator", { userAgent, platform, maxTouchPoints });
      try {
        // The current estimator counts each retained reference. Reuse a small
        // payload to exercise large budgets without allocating 192 MiB in tests.
        const payload = "x".repeat(1024 * 1024);
        for (const id of ["first", "second"]) {
          primeChatViewCache("agent-a", id, {
            title: id,
            messages: [],
            meta: { payloads: Array(48).fill(payload) },
          });
        }
        expect(readChatViewTitleCache("agent-a", "first")).toBe(
          desktop ? "first" : null,
        );
        expect(readChatViewTitleCache("agent-a", "second")).toBe("second");
        expect(cacheHostMock.isMobile).toHaveBeenCalledTimes(1);
      } finally {
        vi.stubGlobal("navigator", originalNavigator);
      }
    },
  );

  it("retains eight project-sized snapshots and their reading positions on mobile", () => {
    const payload = "x".repeat(512 * 1024);
    // Rounded MiB weights from the rua-rust Project's latest eight snapshots.
    const sizes = [61, 2, 5, 3, 3, 10, 19, 17];
    for (const [index, size] of sizes.entries()) {
      primeChatViewCache("project:rua-rust", `chat-${index}`, {
        title: `Chat ${index}`,
        messages: [],
        meta: { payloads: Array(size).fill(payload) },
      });
      writeChatViewScrollCache("project:rua-rust", `chat-${index}`, 640, false);
    }
    for (const index of sizes.keys()) {
      expect(readChatViewTitleCache("project:rua-rust", `chat-${index}`)).toBe(
        `Chat ${index}`,
      );
      expect(
        readChatViewScrollCache("project:rua-rust", `chat-${index}`),
      ).toEqual({
        scrollTop: 640,
        followLatest: false,
      });
    }
    expect(cacheHostMock.isMobile).toHaveBeenCalledTimes(1);
  });

  it.each([false, true])(
    "accounts for replacement, eviction and reset within the cache byte budget (desktop: %s)",
    (desktop) => {
      cacheHostMock.isMobile.mockReturnValue(!desktop);
      cacheHostMock.isDesktopShell.mockReturnValue(desktop);
      const payload = "x".repeat(1024 * 1024);
      const cacheHistory = (id: string, chunks = desktop ? 40 : 20) =>
        primeChatViewCache("agent-a", id, {
          title: id,
          messages: [],
          meta: { payloads: Array(chunks).fill(payload) },
        });
      cacheHistory("first");
      cacheHistory("second");
      cacheHistory("third");
      for (let index = 0; index < 3; index += 1) cacheHistory("second");
      expect(readChatViewCacheTitles()).toEqual([
        "first",
        "second",
        "third",
        null,
        null,
      ]);

      cacheHistory("fourth");
      expect(readChatViewCacheTitles()).toEqual([
        null,
        "second",
        "third",
        "fourth",
        null,
      ]);

      cacheHistory("second", 0);
      cacheHistory("fifth");
      expect(readChatViewCacheTitles()).toEqual([
        null,
        "second",
        "third",
        "fourth",
        "fifth",
      ]);

      __resetChatViewCacheForTests();
      for (const id of ["first", "second", "third"]) cacheHistory(id);
      expect(readChatViewCacheTitles()).toEqual([
        "first",
        "second",
        "third",
        null,
        null,
      ]);

      function readChatViewCacheTitles() {
        return ["first", "second", "third", "fourth", "fifth"].map((id) =>
          readChatViewTitleCache("agent-a", id),
        );
      }
    },
  );

  it("counts growing live output while preserving an unfinished conversation", async () => {
    const stream = controlledSseResponse();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    primeChatViewCache("agent-a", "cached-chat", {
      title: "Other cached chat",
      messages: [],
    });
    chatApiMock.getChat.mockResolvedValueOnce({
      title: "Active chat",
      model: configuredModel,
      messages: [],
      // Keep the live delta small while total retained data crosses 128 MiB.
      meta: { payloads: Array(124).fill("x".repeat(512 * 1024)) },
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    try {
      await waitFor(() => expect(result.current.loading).toBe(false));
      await act(async () => {
        await result.current.submit("Continue");
      });
      expect(readChatViewTitleCache("agent-a", "cached-chat")).toBe(
        "Other cached chat",
      );
      await act(async () => {
        stream.send({
          type: "text_delta",
          message_id: "live-answer",
          delta: "x".repeat(2_200_000),
        });
      });
      await waitFor(() =>
        expect(readChatViewTitleCache("agent-a", "cached-chat")).toBeNull(),
      );
      expect(readChatViewTitleCache("agent-a", "chat-1")).toBe("Active chat");
      expect(warn).toHaveBeenCalledWith(
        "conversation_view_runtime_capacity_unavailable",
        expect.objectContaining({
          retainedConversations: 1,
          estimatedBytes: expect.any(Number),
        }),
      );
    } finally {
      unmount();
      stream.close();
      warn.mockRestore();
    }
  });

  it("does not reread unchanged history or other cached chats when live text grows", async () => {
    const stream = controlledSseResponse();
    const cachedPayload = vi.fn(() => "cached tool output".repeat(1_000));
    const activePayload = vi.fn(() => "earlier tool output".repeat(1_000));
    primeChatViewCache("agent-a", "cached-chat", {
      title: "Other cached chat",
      messages: [],
      meta: {
        diagnostic: {
          get payload() {
            return cachedPayload();
          },
        },
      },
    });
    chatApiMock.getChat.mockResolvedValueOnce({
      title: "Active chat",
      model: configuredModel,
      messages: [{ id: "old-user", role: "user", content: "Earlier question" }],
      meta: {
        diagnostic: {
          get payload() {
            return activePayload();
          },
        },
      },
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );

    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    try {
      await waitFor(() => expect(result.current.loading).toBe(false));
      await act(async () => {
        await result.current.submit("Continue");
      });
      expect(cachedPayload).toHaveBeenCalled();
      expect(activePayload).toHaveBeenCalled();
      cachedPayload.mockClear();
      activePayload.mockClear();

      for (const text of ["A", "A few", "A few more words"]) {
        await act(async () => {
          stream.send({
            object: "response",
            status: "in_progress",
            output: [
              {
                id: "live-answer",
                role: "assistant",
                type: "message",
                content: [{ type: "text", text }],
              },
            ],
          });
        });
        await waitFor(() => {
          const last = result.current.projection.items.at(-1);
          expect(
            last?.kind === "assistant" ? last.output.at(-1)?.content : [],
          ).toEqual([{ type: "text", text }]);
        });
      }

      writeChatViewScrollCache("agent-a", "chat-1", 480, false);
      writeChatViewScrollCache("agent-a", "chat-1", 960, false);
      expect(readChatViewScrollCache("agent-a", "chat-1")).toEqual({
        scrollTop: 960,
        followLatest: false,
      });
      expect(readChatViewTitleCache("agent-a", "cached-chat")).toBe(
        "Other cached chat",
      );
      expect(cachedPayload).not.toHaveBeenCalled();
      expect(activePayload).not.toHaveBeenCalled();
    } finally {
      unmount();
      stream.close();
    }
  });

  it("does not restore cached authority activity across remounts", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "running",
      active_turn: {
        turn_id: "turn-1",
        phase: "streaming",
        authority_device_id: "device-a",
        authority_epoch: "0000000000000001000000000000000000000000",
        driver: "rua",
        started_at: "2026-07-22T00:00:00Z",
        queued_count: 0,
      },
      control: {
        revision: 1,
        placement: {
          authority_device_id: "device-a",
          authority_epoch: "0000000000000001000000000000000000000000",
          switchable: false,
        },
        active_turn: {
          turn_id: "turn-1",
          phase: "streaming",
          authority_device_id: "device-a",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-07-22T00:00:00Z",
          queued_count: 0,
        },
        queue: { entries: [] },
        acp: { active: false },
        allowed_controls: {
          stop: true,
          queue: true,
          interject: true,
          edit_queued_input: false,
          delete_queued_input: false,
          switch_authority: false,
          enter_acp: false,
          exit_acp: false,
        },
      },
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "cached running message" }],
        },
      ],
    });

    const first = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(isTurnActive(first.result)).toBe(true));
    first.unmount();

    chatApiMock.getChat.mockReturnValueOnce(new Promise(() => undefined));
    consoleApiMock.getApprovalSnapshot.mockReturnValueOnce(
      new Promise(() => undefined),
    );
    const remount = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );

    expect(
      JSON.stringify(remount.result.current.history?.messages || []),
    ).toContain("cached running message");
    expect(isTurnActive(remount.result)).toBe(false);
    expect(remount.result.current.history?.active_turn).toBeNull();

    remount.unmount();
  });

  it("does not write optimistic active turns into the stable chat view cache", async () => {
    const stream = controlledSseResponse();
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      title: "Stable chat",
      model: { provider_id: "dashscope", model: "qwen-plus" },
      messages: [{ id: "user-1", role: "user", content: "stable message" }],
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );

    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => {
      void result.current.submit("optimistic message");
    });
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/submit"),
        ),
      ).toBe(true),
    );
    expect(JSON.stringify(result.current.history?.messages || [])).toContain(
      "stable message",
    );
    expect(
      JSON.stringify(result.current.history?.messages || []),
    ).not.toContain("optimistic message");

    unmount();
    chatApiMock.getChat.mockReturnValueOnce(new Promise(() => undefined));
    consoleApiMock.getApprovalSnapshot.mockReturnValueOnce(
      new Promise(() => undefined),
    );

    const { result: remount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );

    expect(remount.current.loading).toBe(false);
    const cachedMessages = JSON.stringify(
      remount.current.history?.messages || [],
    );
    expect(cachedMessages).toContain("stable message");
    expect(cachedMessages).not.toContain("optimistic message");
  });

  it("reconnects when terminal content arrives before authority marks the Turn idle", async () => {
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValueOnce({
        status: "running",
        active_turn: {
          turn_id: "turn-1",
          user_message_id: "user-1",
          phase: "streaming",
          authority_device_id: "device-current",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-07-20T00:00:00Z",
        },
        control: {
          revision: 2,
          placement: null,
          active_turn: {
            turn_id: "turn-1",
            user_message_id: "user-1",
            phase: "streaming",
            authority_device_id: "device-current",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-20T00:00:00Z",
          },
          input_queue: [],
        },
        model: configuredModel,
        messages: [
          {
            id: "user-1",
            role: "user",
            content: [{ type: "text", text: "hello" }],
          },
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "done" }],
          },
        ],
      });
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toBe(true),
    );

    await act(async () => {
      await result.current.submit("hello");
    });
    await act(async () => {
      liveStream.send({
        object: "response",
        status: "completed",
        output: [
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "done" }],
          },
        ],
      });
      liveStream.close();
    });

    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(1);
    await waitFor(
      () =>
        expect(
          (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
            String(url).includes("/console/live"),
          ),
        ).toHaveLength(2),
      { timeout: 2_000 },
    );
    expect(isTurnActive(result)).toBe(true);
  });

  it("mirrors authority queue from live conversation control events", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      liveStream.send({
        type: "conversation_control",
        reason: "queue_accepted",
        control: {
          active_turn: {
            phase: "streaming",
            driver: "rua",
            started_at: "2026-07-16T00:00:00Z",
            queued_count: 1,
          },
          queue: {
            entries: [
              {
                input_id: "input-1",
                state: "queued",
                text: "queued from authority",
                created_at: "2026-07-16T00:00:01Z",
                updated_at: "2026-07-16T00:00:01Z",
                editable: true,
                deletable: true,
              },
            ],
          },
          acp: { active: false },
          allowed_controls: {
            stop: true,
            queue: true,
            interject: true,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: false,
            enter_acp: false,
            exit_acp: false,
          },
        },
      });
    });

    await waitFor(() => {
      expect(isTurnActive(result)).toBe(true);
      expect(result.current.queuedSubmissions).toEqual([
        expect.objectContaining({
          id: "input-1",
          text: "queued from authority",
          serverQueued: true,
        }),
      ]);
    });
  });

  it("applies the authority model from live conversation control", async () => {
    const liveStream = controlledSseResponse();
    let liveSignal: AbortSignal | undefined;
    vi.stubGlobal(
      "fetch",
      routeFetch((url, init) => {
        if (!url.includes("/console/live")) return undefined;
        liveSignal = init?.signal || undefined;
        return liveStream.response;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toBe(true),
    );
    expect(liveSignal?.aborted).toBe(false);

    await act(async () => {
      liveStream.send({
        type: "conversation_control",
        reason: "model_updated",
        control: {
          revision: 2,
          placement: {
            authority_device_id: "device-b",
            authority_epoch: "0000000000000003000000000000000000000000",
            switchable: true,
          },
          active_turn: null,
          queue: { entries: [], paused: false },
          acp: { active: false },
          model: {
            provider_id: "fixture",
            model: "target-model",
            thinking_strength: "high",
            thinking_param_style: "effort",
            reasoning_effort_options: ["low", "medium", "high"],
          },
          allowed_controls: {
            stop: false,
            queue: false,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: true,
            enter_acp: true,
            exit_acp: false,
          },
        },
      });
      liveStream.close();
    });

    await waitFor(() =>
      expect(result.current.history?.model).toEqual({
        provider_id: "fixture",
        model: "target-model",
        thinking_strength: "high",
        thinking_param_style: "effort",
        reasoning_effort_options: ["low", "medium", "high"],
      }),
    );
  });

  it("applies the initial live control after stable history renders without runtime state", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValue({
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.history?.control).toBeUndefined();

    await act(async () => {
      liveStream.send({
        type: "conversation_control",
        reason: "viewer_attached",
        control: {
          revision: 0,
          placement: {
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000007000000000000000000000000",
            switchable: true,
          },
          active_turn: null,
          queue: { entries: [], paused: false },
          acp: { active: false },
          allowed_controls: {
            stop: false,
            queue: false,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: true,
            enter_acp: true,
            exit_acp: false,
          },
        },
      });
    });

    await waitFor(() =>
      expect(result.current.history?.control?.revision).toBe(0),
    );
    liveStream.close();
  });

  it("ignores stale control revisions from the same authority Epoch", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    const control = (revision: number, inputId: string, text: string) => ({
      revision,
      placement: {
        authority_device_id: "device-current",
        authority_epoch: "0000000000000004000000000000000000000000",
        switchable: false,
      },
      active_turn: {
        turn_id: "turn-active",
        phase: "streaming",
        authority_device_id: "device-current",
        authority_epoch: "0000000000000004000000000000000000000000",
        driver: "rua",
        started_at: "2026-07-16T00:00:00Z",
        queued_count: 1,
      },
      queue: {
        paused: false,
        entries: [
          {
            input_id: inputId,
            submission_id: `submission-${inputId}`,
            state: "queued",
            text,
            created_at: "2026-07-16T00:00:01Z",
            updated_at: "2026-07-16T00:00:01Z",
          },
        ],
      },
      acp: { active: false },
      allowed_controls: {
        stop: true,
        queue: true,
        interject: true,
        edit_queued_input: false,
        delete_queued_input: false,
        switch_authority: false,
        enter_acp: false,
        exit_acp: false,
      },
    });

    await act(async () => {
      liveStream.send({
        type: "conversation_control",
        reason: "queue_accepted",
        control: control(2, "input-new", "new"),
      });
      liveStream.send({
        type: "conversation_control",
        reason: "queue_accepted",
        control: control(1, "input-old", "old"),
      });
    });

    await waitFor(() =>
      expect(result.current.queuedSubmissions).toEqual([
        expect.objectContaining({ id: "input-new", text: "new" }),
      ]),
    );
  });

  it("deletes authority queued input through backend control endpoint", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "running",
      model: configuredModel,
      messages: [],
      control: {
        active_turn: {
          phase: "streaming",
          driver: "rua",
          started_at: "2026-07-16T00:00:00Z",
          queued_count: 1,
        },
        queue: {
          entries: [
            {
              input_id: "input-1",
              state: "queued",
              text: "server queued",
              created_at: "2026-07-16T00:00:01Z",
              updated_at: "2026-07-16T00:00:01Z",
              editable: true,
              deletable: true,
            },
          ],
        },
        acp: { active: false },
        allowed_controls: {
          stop: true,
          queue: true,
          interject: true,
          edit_queued_input: false,
          delete_queued_input: true,
          switch_authority: false,
          enter_acp: false,
          exit_acp: false,
        },
      },
    });
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(result.current.queuedSubmissions).toHaveLength(1),
    );

    act(() => {
      result.current.cancelQueuedSubmission("input-1");
    });

    await waitFor(() => {
      expect(chatApiMock.deleteQueuedInput).toHaveBeenCalledWith(
        "chat-1",
        "input-1",
        "agent-a",
      );
      expect(result.current.queuedSubmissions).toHaveLength(0);
    });
  });

  it("edits authority queued input through backend control endpoint", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "running",
      model: configuredModel,
      messages: [],
      control: {
        active_turn: {
          phase: "streaming",
          driver: "rua",
          started_at: "2026-07-16T00:00:00Z",
          queued_count: 1,
        },
        queue: {
          entries: [
            {
              input_id: "input-1",
              state: "queued",
              text: "server queued",
              created_at: "2026-07-16T00:00:01Z",
              updated_at: "2026-07-16T00:00:01Z",
              editable: true,
              deletable: true,
            },
          ],
        },
        acp: { active: false },
        allowed_controls: {
          stop: true,
          queue: true,
          interject: true,
          edit_queued_input: true,
          delete_queued_input: true,
          switch_authority: false,
          enter_acp: false,
          exit_acp: false,
        },
      },
    });
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(result.current.queuedSubmissions).toHaveLength(1),
    );

    await act(async () => {
      await result.current.editQueuedSubmission(
        "input-1",
        "edited server queued",
      );
    });

    expect(chatApiMock.editQueuedInput).toHaveBeenCalledWith(
      "chat-1",
      "input-1",
      "edited server queued",
      "agent-a",
    );
    await waitFor(() => {
      expect(result.current.queuedSubmissions[0].text).toBe(
        "edited server queued",
      );
    });
  });

  it("derives authority placement lock from conversation control view", async () => {
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "running",
      model: configuredModel,
      messages: [],
      meta: {},
      control: {
        active_turn: {
          phase: "streaming",
          driver: "rua",
          started_at: "2026-07-16T00:00:00Z",
          queued_count: 0,
        },
        placement: {
          authority_device_id: "device-b",
          switchable: false,
        },
        queue: { entries: [] },
        acp: { active: false },
        allowed_controls: {
          stop: true,
          queue: true,
          interject: true,
          edit_queued_input: false,
          delete_queued_input: false,
          switch_authority: false,
          enter_acp: false,
          exit_acp: false,
        },
      },
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.authorityPlacementDeviceId).toBe("device-b");
    expect(result.current.authorityPlacementLocked).toBe(true);
  });

  it("does not duplicate a local submit after canonical live user message converges with history", async () => {
    const canonicalUser = {
      id: "msg-user-1",
      role: "user",
      turn_id: "turn-1",
      content: [{ type: "text", text: "hello" }],
    };
    const canonicalAssistant = {
      id: "msg-assistant-1",
      role: "assistant",
      turn_id: "turn-1",
      type: "message",
      content: [{ type: "text", text: "done" }],
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValueOnce({
        status: "running",
        model: configuredModel,
        messages: [canonicalUser],
      })
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [canonicalUser, canonicalAssistant],
      });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live")
          ? sseResponse([
              { type: "user_message", message: canonicalUser },
              {
                object: "response",
                status: "completed",
                output: [canonicalAssistant],
              },
              {
                type: "turn_terminal",
                turn_id: "turn-1",
                status: "completed",
              },
            ])
          : undefined,
      ),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("hello");
    });

    await waitFor(() => expect(isTurnActive(result)).toBe(false));
    await waitFor(() =>
      expect(
        result.current.history?.messages.map((message) => message.id),
      ).toEqual(["msg-user-1", "msg-assistant-1"]),
    );
    expect(result.current.projection.items.map((item) => item.id)).toEqual([
      "msg-user-1",
      "chat-1:assistant:msg-user-1",
    ]);
  });

  it("uses the mounted live watcher as the assistant output reader for local submit", async () => {
    const canonicalUser = {
      id: "msg-user-1",
      role: "user",
      turn_id: "turn-1",
      content: [{ type: "text", text: "hello" }],
    };
    const liveStream = controlledSseResponse();
    const submitResponse = deferred<Response>();
    let liveSignal: AbortSignal | undefined;
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: { provider_id: "dashscope", model: "qwen-plus" },
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url, init) => {
        if (url.includes("/console/live")) {
          liveSignal = init?.signal || undefined;
          return liveStream.response;
        }
        if (url.includes("/console/submit")) return submitResponse.promise;
        return undefined;
      }),
    );
    expect(fetch).not.toHaveBeenCalled();

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toHaveLength(1),
    );

    let submitPromise: Promise<void>;
    await act(async () => {
      submitPromise = result.current.submit("hello");
    });
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/submit"),
        ),
      ).toBe(true),
    );
    expect(liveSignal?.aborted).toBe(false);

    await act(async () => {
      liveStream.send({ type: "user_message", message: canonicalUser });
      liveStream.send({ type: "text_delta", delta: "mounted watcher output" });
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "mounted watcher output",
      ),
    );

    submitResponse.resolve(submitAcceptedResponse());
    await act(async () => {
      await submitPromise!;
    });

    const rendered = JSON.stringify(result.current.projection.items);
    expect(rendered).not.toContain("submit reader output");
    expect(fetch).toHaveBeenCalledWith(
      "/api/agents/agent-a/chats/chat-1/console/submit",
      expect.objectContaining({ method: "POST" }),
    );
    expect(fetch).toHaveBeenCalledWith(
      "/api/agents/agent-a/chats/chat-1/console/live",
      expect.objectContaining({ method: "POST" }),
    );
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/live"),
      ),
    ).toHaveLength(1);
  });

  it("shows blocking compaction after the active user and clears every terminal status", async () => {
    const liveStream = controlledSseResponse();
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: { provider_id: "dashscope", model: "qwen-plus" },
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toBe(true),
    );

    await act(async () => {
      liveStream.send({
        type: "conversation_control",
        reason: "started",
        control: {
          active_turn: {
            phase: "streaming",
            turn_id: "turn-compaction",
            user_message_id: "msg-user-compaction",
            driver: "rua",
            started_at: "2026-07-16T00:00:00Z",
            queued_count: 0,
          },
          queue: { entries: [] },
          acp: { active: false },
          allowed_controls: {
            stop: true,
            queue: true,
            interject: true,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: false,
            enter_acp: false,
            exit_acp: false,
          },
        },
      });
      liveStream.send({
        type: "user_message",
        message: {
          id: "msg-user-compaction",
          role: "user",
          turn_id: "turn-compaction",
          content: [{ type: "text", text: "inspect the tool output" }],
        },
      });
      liveStream.send({
        type: "context_compaction",
        status: "started",
        mode: "blocking",
      });
    });

    await waitFor(() =>
      expect(result.current.contextCompactionStatus).toEqual({ active: true }),
    );
    expect(isTurnActive(result)).toBe(true);
    expect(
      result.current.projection.items.filter(
        (item) => item.kind === "assistant",
      ),
    ).toEqual([
      expect.objectContaining({
        kind: "assistant",
        output: [],
      }),
    ]);

    for (const status of ["completed", "skipped", "failed"]) {
      await act(async () => {
        liveStream.send({
          type: "context_compaction",
          status: "started",
          mode: "blocking",
        });
      });
      await waitFor(() =>
        expect(result.current.contextCompactionStatus?.active).toBe(true),
      );
      await act(async () => {
        liveStream.send({
          type: "context_compaction",
          status,
          mode: "blocking",
        });
      });
      await waitFor(() =>
        expect(result.current.contextCompactionStatus).toBeNull(),
      );
    }

    expect(
      result.current.projection.items
        .filter((item) => item.kind === "assistant")
        .flatMap((item) => item.output),
    ).toHaveLength(0);
    liveStream.close();
  });

  it("submits while streaming and displays the authority queue projection", async () => {
    const firstStream = controlledSseResponse();
    const onSubmitted = vi.fn();
    let submitCount = 0;
    window.addEventListener(CHAT_SUBMITTED_EVENT, onSubmitted);
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 1) return submitStartedResponse();
          if (submitCount === 2)
            return submitQueuedResponse("input-second", "second");
          if (submitCount === 3) return submitStartedWithEmptyQueueResponse();
        }
        return undefined;
      }),
    );
    try {
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));

      let firstSubmit: Promise<boolean> | undefined;
      act(() => {
        firstSubmit = result.current.submit("first");
      });
      await waitFor(() => expect(isTurnActive(result)).toBe(true));
      await waitFor(() => expect(onSubmitted).toHaveBeenCalledTimes(1));
      onSubmitted.mockClear();

      await act(async () => {
        await result.current.submit("second");
      });

      expect(result.current.queuedSubmissions).toHaveLength(1);
      expect(result.current.queuedSubmissions[0].text).toBe("second");
      expect(result.current.queuedSubmissions[0]).toMatchObject({
        id: "input-second",
        serverQueued: true,
      });
      expect(onSubmitted).toHaveBeenCalledTimes(1);
      expect(onSubmitted.mock.calls[0][0].detail).toMatchObject({
        agentId: "agent-a",
        chatId: "chat-1",
        title: "second",
      });

      firstStream.send({ object: "response", status: "completed", output: [] });
      firstStream.close();
      await act(async () => {
        await firstSubmit;
      });

      const submitCalls = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
        ([url]) => String(url).includes("/console/submit"),
      );
      expect(submitCalls).toHaveLength(2);
      const secondBody = JSON.parse(submitCalls[1][1].body);
      expect(secondBody.input[0].content).toEqual([
        { type: "text", text: "second" },
      ]);
    } finally {
      window.removeEventListener(CHAT_SUBMITTED_EVENT, onSubmitted);
    }
  });

  it("queues ACP submits through the unified submit projection", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 1) return submitStartedResponse();
          if (submitCount === 2)
            return submitQueuedResponse("input-second", "second");
          if (submitCount === 3) return submitStartedWithEmptyQueueResponse();
        }
        return undefined;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: null,
        currentDeviceId: null,
        activeAcpRunner: "codex",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
    });

    expect(result.current.queuedSubmissions).toHaveLength(1);
    expect(result.current.queuedSubmissions[0].text).toBe("second");
    expect(result.current.queuedSubmissions[0]).toMatchObject({
      id: "input-second",
      serverQueued: true,
    });

    firstStream.send({ object: "response", status: "completed", output: [] });
    firstStream.close();
    await act(async () => {
      await firstSubmit;
    });

    const submitCalls = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
      ([url]) => String(url).includes("/console/submit"),
    );
    expect(submitCalls).toHaveLength(2);
    const secondBody = JSON.parse(submitCalls[1][1].body);
    expect(secondBody.input[0].content).toEqual([
      { type: "text", text: "second" },
    ]);
  });

  it("keeps queued submissions after stop without auto-draining them", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2)
            return submitQueuedResponse("input-second", "second");
          if (submitCount === 3) return submitStartedWithEmptyQueueResponse();
        }
        return undefined;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
    });
    expect(result.current.queuedSubmissions).toHaveLength(1);

    await act(async () => {
      await result.current.stop();
      firstStream.close();
      await firstSubmit;
    });

    expect(result.current.queuedSubmissions).toHaveLength(1);
    expect(result.current.queuedSubmissions[0].text).toBe("second");

    await act(async () => {
      await result.current.submitQueuedNow(
        result.current.queuedSubmissions[0].id,
      );
    });

    expect(result.current.queuedSubmissions).toHaveLength(1);
    expect(chatApiMock.sendNowQueuedInput).toHaveBeenCalledWith(
      "chat-1",
      "input-second",
      expect.objectContaining({
        authority_epoch: "0000000000000001000000000000000000000000",
      }),
      "agent-a",
    );
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(2);
  });

  it("sends queued input now through the dedicated authority control", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2)
            return submitQueuedResponse("input-second", "second");
        }
        return undefined;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    await act(async () => {
      firstStream.send({
        type: "user_message",
        message: {
          id: "user-first",
          role: "user",
          content: [{ type: "text", text: "first" }],
        },
      });
      firstStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-first",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "first partial" }],
          },
        ],
      });
    });

    await act(async () => {
      await result.current.submit("second");
    });
    const queuedId = result.current.queuedSubmissions[0].id;
    await waitFor(() =>
      expect(result.current.queuedSubmissions[0].serverQueued).toBe(true),
    );

    await act(async () => {
      await result.current.submitQueuedNow(queuedId);
    });

    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
    const submitCalls = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
      ([url]) => String(url).includes("/console/submit"),
    );
    expect(submitCalls).toHaveLength(2);
    const queueBody = JSON.parse(submitCalls[1][1].body);
    expect(queueBody.chat_id).toBe("chat-1");
    expect(queueBody.input[0].content).toEqual([
      { type: "text", text: "second" },
    ]);
    expect(chatApiMock.sendNowQueuedInput).toHaveBeenCalledWith(
      "chat-1",
      "input-second",
      expect.objectContaining({
        authority_epoch: "0000000000000001000000000000000000000000",
      }),
      "agent-a",
    );

    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      model: configuredModel,
      messages: [
        { id: "user-first", role: "user", content: "first" },
        {
          id: "assistant-first",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "first partial" }],
          status: "interrupted",
        },
        {
          id: "turn-interjected-first",
          role: "system",
          type: "turn_interjected",
          content:
            "<turn_interjected>\nThe user inserted a new message, so the previous turn was ended before it could continue.\n</turn_interjected>",
          metadata: { model_visible_marker: "turn_interjected" },
        },
      ],
    });

    await act(async () => {
      firstStream.send({
        type: "interrupted",
        reason: "interjected",
      });
      firstStream.send({
        type: "user_message",
        message: {
          id: "user-second",
          role: "user",
          content: [{ type: "text", text: "second" }],
        },
      });
      firstStream.send({
        type: "text_delta",
        delta: "second answer",
      });
      firstStream.send({
        object: "response",
        status: "completed",
        output: [],
      });
    });

    await waitFor(() => {
      const rendered = JSON.stringify(result.current.history?.messages || []);
      expect(rendered).toContain("first");
      expect(rendered).toContain("first partial");
      expect(rendered).toContain("<turn_interjected>");
      expect(rendered).toContain("second");
      expect(rendered).toContain("second answer");
      expect(rendered).not.toContain("<turn_aborted>");
    });
    expect(
      result.current.projection.items.filter((item) => item.kind === "user"),
    ).toHaveLength(2);
    const assistantItems = result.current.projection.items.filter(
      (item) => item.kind === "assistant",
    );
    expect(assistantItems).toHaveLength(2);
    expect(assistantItems[0]).toMatchObject({
      kind: "assistant",
      status: "interrupted",
    });
    expect(JSON.stringify(assistantItems[0])).toContain("first partial");
    expect(JSON.stringify(assistantItems[1])).toContain("second answer");

    await act(async () => {
      firstStream.close();
      await firstSubmit;
    });
  });

  it("queues submit from backend Active Turn even when stable status is idle", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      active_turn: {
        phase: "streaming",
        turn_id: "turn-1",
        user_message_id: "user-1",
        authority_device_id: null,
        driver: "rua",
        started_at: "2026-07-16T00:00:00Z",
        queued_count: 0,
      },
      messages: [
        {
          id: "user-1",
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "first" }],
        },
      ],
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/submit")) {
          return submitQueuedResponse("input-second", "second");
        }
        return undefined;
      }),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(isTurnActive(result)).toBe(true);

    await act(async () => {
      await result.current.submit("second");
    });

    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toBe(true);
    expect(result.current.queuedSubmissions).toHaveLength(1);
    expect(result.current.queuedSubmissions[0]).toMatchObject({
      id: "input-second",
      text: "second",
      serverQueued: true,
    });
  });

  it("sends ACP queued input now through authority control", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2)
            return submitQueuedResponse("input-second", "second");
        }
        return undefined;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: null,
        currentDeviceId: null,
        activeAcpRunner: "codex",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
    });
    const queuedId = result.current.queuedSubmissions[0].id;

    await act(async () => {
      await result.current.submitQueuedNow(queuedId);
    });

    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
    expect(result.current.queuedSubmissions).toHaveLength(1);

    firstStream.send({
      type: "user_message",
      message: {
        id: "input-second",
        role: "user",
        content: [{ type: "text", text: "second" }],
        metadata: { submission_id: "submission-input-second" },
      },
    });
    firstStream.send({ object: "response", status: "completed", output: [] });
    firstStream.close();
    await act(async () => {
      await firstSubmit;
    });

    await waitFor(() =>
      expect(result.current.queuedSubmissions).toHaveLength(0),
    );
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(2);
    expect(chatApiMock.sendNowQueuedInput).toHaveBeenCalledWith(
      "chat-1",
      "input-second",
      expect.objectContaining({
        authority_epoch: "0000000000000001000000000000000000000000",
      }),
      "agent-a",
    );
  });

  it("remote ACP Send Now submits the queued input id to the authority", async () => {
    const firstStream = controlledSseResponse();
    let liveCalls = 0;
    let submitCalls = 0;
    let stableHistoryReady = false;
    chatApiMock.getChat.mockImplementation(async () =>
      stableHistoryReady
        ? {
            status: "idle",
            model: configuredModel,
            messages: [
              {
                id: "target-user-1",
                role: "user",
                content: [{ type: "text", text: "first" }],
                metadata: {
                  device_delivery_id: "device-delivery-1",
                },
              },
              {
                id: "target-assistant-1",
                role: "assistant",
                content: [{ type: "text", text: "first stopped" }],
              },
            ],
          }
        : { status: "idle", messages: [], model: configuredModel },
    );
    vi.stubGlobal(
      "fetch",
      routeFetch((url, _init) => {
        if (url.includes("/console/live")) {
          liveCalls += 1;
          return liveCalls === 1
            ? firstStream.response
            : sseResponse([
                { object: "response", status: "completed", output: [] },
              ]);
        }
        if (url.includes("/console/submit")) {
          submitCalls += 1;
          if (submitCalls === 2) {
            return submitQueuedResponse("input-second", "second", [
              {
                input_id: "input-second",
                text: "second",
                device_delivery_id: "device-delivery-2",
              },
            ]);
          }
          return submitAcceptedResponse({
            mode: "started",
            device_delivery_id: `device-delivery-${submitCalls}`,
            target_device_id: "device-remote",
            device_delivery_sequence: submitCalls,
            accepted_at: `2026-07-05T00:00:0${submitCalls}Z`,
          });
        }
        return undefined;
      }),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
        currentDeviceId: null,
        activeAcpRunner: "codex",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("first");
    });
    stableHistoryReady = true;
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toBe(true),
    );
    await act(async () => {
      firstStream.send({
        type: "user_message",
        message: {
          id: "target-user-1",
          role: "user",
          content: [{ type: "text", text: "first" }],
          metadata: {
            device_delivery_id: "device-delivery-1",
          },
        },
      });
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
    });
    expect(result.current.queuedSubmissions).toHaveLength(1);
    const queuedId = result.current.queuedSubmissions[0].id;

    await act(async () => {
      await result.current.submitQueuedNow(queuedId);
    });

    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
    expect(result.current.queuedSubmissions).toHaveLength(1);

    await act(async () => {
      firstStream.send({
        object: "response",
        status: "interrupted",
        output: [],
      });
      firstStream.send({
        type: "user_message",
        message: {
          id: "input-second",
          role: "user",
          content: [{ type: "text", text: "second" }],
          metadata: {
            device_delivery_id: "device-delivery-2",
          },
        },
      });
      firstStream.close();
    });

    expect(result.current.queuedSubmissions).toHaveLength(1);
    const chatCalls = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
      ([url]) =>
        String(url).includes("/console/submit") &&
        !String(url).includes("/console/live"),
    );
    expect(chatCalls).toHaveLength(2);
    expect(chatCalls[0][0]).toBe(
      "/api/agents/agent-a/chats/chat-1/console/submit",
    );
    expect(chatCalls[1][0]).toBe(
      "/api/agents/agent-a/chats/chat-1/console/submit",
    );
    expect(chatApiMock.sendNowQueuedInput).toHaveBeenCalledWith(
      "chat-1",
      "input-second",
      expect.objectContaining({
        authority_epoch: "0000000000000001000000000000000000000000",
      }),
      "agent-a",
    );
  });

  it("passively queues input and starts it after the active turn completes", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2)
            return submitQueuedResponse("input-second", "second");
        }
        return undefined;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    await act(async () => {
      firstStream.send({
        type: "user_message",
        message: {
          id: "user-first",
          role: "user",
          content: [{ type: "text", text: "first" }],
          metadata: { turn_id: "turn-first" },
        },
      });
    });

    await act(async () => {
      await result.current.submit("second");
    });
    await waitFor(() =>
      expect(result.current.queuedSubmissions[0].serverQueued).toBe(true),
    );
    const secondSubmissionId = result.current.queuedSubmissions[0].submissionId;
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      model: configuredModel,
      messages: [
        { id: "user-first", role: "user", content: "first" },
        {
          id: "assistant-first",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "first complete" }],
        },
      ],
    });

    await act(async () => {
      firstStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-first",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "first complete" }],
          },
        ],
      });
      firstStream.send({
        object: "response",
        status: "completed",
        output: [],
      });
      firstStream.send({
        type: "turn_terminal",
        turn_id: "turn-first",
        status: "completed",
      });
      firstStream.send({
        type: "user_message",
        message: {
          id: "server-second",
          role: "user",
          content: [{ type: "text", text: "second" }],
          metadata: { submission_id: secondSubmissionId },
        },
      });
      firstStream.send({
        type: "text_delta",
        delta: "second answer",
      });
    });

    await waitFor(() => {
      const rendered = JSON.stringify(result.current.history?.messages || []);
      expect(rendered).toContain("first complete");
      expect(rendered).toContain("second");
      expect(rendered).toContain("second answer");
    });
    expect(result.current.queuedSubmissions).toHaveLength(0);
    expect(
      result.current.projection.items.filter((item) => item.kind === "user"),
    ).toHaveLength(2);
    expect(
      result.current.projection.items.filter(
        (item) => item.kind === "assistant",
      ),
    ).toHaveLength(2);

    await act(async () => {
      firstStream.close();
      await firstSubmit;
    });
  });

  it("passively queued inputs become active in FIFO order", async () => {
    let uuidSeq = 0;
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => `queued-fifo-${uuidSeq++}` },
      configurable: true,
    });
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2) {
            return submitQueuedResponse("input-second", "second");
          }
          if (submitCount === 3) {
            return submitQueuedResponse("input-third", "third", [
              { input_id: "input-second", text: "second" },
              { input_id: "input-third", text: "third" },
            ]);
          }
        }
        return undefined;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
      await result.current.submit("third");
    });
    await waitFor(() =>
      expect(result.current.queuedSubmissions.map((item) => item.text)).toEqual(
        ["second", "third"],
      ),
    );
    await waitFor(() =>
      expect(
        result.current.queuedSubmissions.every((item) => item.serverQueued),
      ).toBe(true),
    );
    await act(async () => {
      firstStream.send({
        object: "response",
        status: "completed",
        output: [],
      });
      firstStream.send({
        type: "user_message",
        message: {
          id: "server-second",
          role: "user",
          content: [{ type: "text", text: "second" }],
          metadata: { submission_id: "submission-input-second" },
        },
      });
      firstStream.send({ type: "text_delta", delta: "second answer" });
    });

    await waitFor(() => {
      const rendered = JSON.stringify(result.current.history?.messages || []);
      expect(rendered).toContain("second");
      expect(rendered).toContain("second answer");
      expect(rendered).not.toContain("third answer");
    });
    expect(result.current.queuedSubmissions.map((item) => item.text)).toEqual([
      "third",
    ]);

    await act(async () => {
      firstStream.close();
      await firstSubmit;
    });
  });

  it("keeps identical text submissions distinct by submission id", async () => {
    let uuidSeq = 0;
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => `same-text-${uuidSeq++}` },
      configurable: true,
    });
    const stream = controlledSseResponse();
    const queuedEntries: Array<{
      input_id: string;
      submission_id: string;
      text: string;
    }> = [];
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url, init) => {
        if (url.includes("/console/live")) return stream.response;
        if (!url.includes("/console/submit")) return undefined;
        submitCount += 1;
        if (submitCount === 1) return submitStartedResponse();
        const body = JSON.parse(String(init?.body || "{}"));
        const inputId = `input-repeat-${submitCount - 1}`;
        queuedEntries.push({
          input_id: inputId,
          submission_id: body.submission_id,
          text: "repeat",
        });
        return submitQueuedResponse(
          inputId,
          "repeat",
          queuedEntries.map((entry) => ({ ...entry })),
        );
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<boolean> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("repeat");
      await result.current.submit("repeat");
    });
    expect(result.current.queuedSubmissions).toHaveLength(2);
    expect(result.current.queuedSubmissions.map((item) => item.text)).toEqual([
      "repeat",
      "repeat",
    ]);
    expect(
      new Set(result.current.queuedSubmissions.map((item) => item.submissionId))
        .size,
    ).toBe(2);

    await act(async () => {
      stream.send({ object: "response", status: "completed", output: [] });
      stream.send({
        type: "user_message",
        message: {
          id: "server-repeat-1",
          role: "user",
          content: [{ type: "text", text: "repeat" }],
          metadata: {
            submission_id: queuedEntries[0].submission_id,
          },
        },
      });
    });

    await waitFor(() =>
      expect(result.current.queuedSubmissions).toHaveLength(1),
    );
    expect(result.current.queuedSubmissions[0].submissionId).toBe(
      queuedEntries[1].submission_id,
    );

    await act(async () => {
      stream.close();
      await firstSubmit;
    });
  });

  it("retains a completed remote live turn when stable history lags behind the next turn", async () => {
    const staleHistory = {
      status: "idle" as const,
      model: configuredModel,
      messages: [
        {
          id: "historical-user",
          role: "user",
          turn_id: "turn-historical",
          content: [{ type: "text", text: "Historical question" }],
        },
        {
          id: "historical-assistant",
          role: "assistant",
          type: "message",
          turn_id: "turn-historical",
          content: [{ type: "text", text: "Historical answer" }],
        },
      ],
    };
    const staleRefresh = deferred<typeof staleHistory>();
    chatApiMock.getChat
      .mockResolvedValueOnce(staleHistory)
      .mockReturnValueOnce(staleRefresh.promise)
      .mockResolvedValue(staleHistory);
    const firstStream = controlledSseResponse();
    const restoredStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetchWithLiveStreams(firstStream.response, restoredStream.response),
    );
    const firstView = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-b",
        currentDeviceId: "device-a",
      }),
    );
    const { result } = firstView;
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        "/api/agents/agent-a/chats/chat-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );

    await act(async () => {
      firstStream.send({
        type: "user_message",
        message: {
          id: "live-user-1",
          role: "user",
          turn_id: "turn-live-1",
          content: [{ type: "text", text: "First remote question" }],
        },
      });
      firstStream.send({ type: "text_delta", delta: "First remote answer" });
      firstStream.send({
        type: "turn_terminal",
        turn_id: "turn-live-1",
        status: "completed",
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "First remote answer",
      ),
    );

    await act(async () => {
      firstStream.send({
        type: "user_message",
        message: {
          id: "live-user-2",
          role: "user",
          turn_id: "turn-live-2",
          content: [{ type: "text", text: "Second remote question" }],
        },
      });
    });
    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalledTimes(2));
    await act(async () => {
      staleRefresh.resolve(staleHistory);
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "Second remote question",
      ),
    );
    await waitFor(() => {
      const rendered = JSON.stringify(result.current.projection.items);
      expect(rendered).toContain("Historical question");
      expect(rendered).toContain("Historical answer");
      expect(rendered).toContain("First remote question");
      expect(rendered).toContain("First remote answer");
      expect(rendered).toContain("Second remote question");
    });

    await act(async () => {
      firstStream.close();
    });
    firstView.unmount();

    const restoredView = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-b",
        currentDeviceId: "device-a",
      }),
    );
    await waitFor(() =>
      expect(restoredView.result.current.loading).toBe(false),
    );
    await waitFor(() => {
      const rendered = JSON.stringify(
        restoredView.result.current.projection.items,
      );
      expect(rendered).toContain("Historical question");
      expect(rendered).toContain("Historical answer");
      expect(rendered).toContain("First remote question");
      expect(rendered).toContain("First remote answer");
      expect(rendered).toContain("Second remote question");
    });

    await act(async () => {
      restoredStream.close();
    });
    restoredView.unmount();
  });

  it("renders consecutive server-announced queued turns without rewriting earlier users", async () => {
    let uuidSeq = 0;
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => `queued-${uuidSeq++}` },
      configurable: true,
    });
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2) {
            return submitQueuedResponse("input-second", "second");
          }
          if (submitCount === 3) {
            return submitQueuedResponse("input-third", "third", [
              { input_id: "input-second", text: "second" },
              { input_id: "input-third", text: "third" },
            ]);
          }
        }
        return undefined;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
      await result.current.submit("third");
    });
    await waitFor(() =>
      expect(
        result.current.queuedSubmissions.every((item) => item.serverQueued),
      ).toBe(true),
    );
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [
          { id: "user-first", role: "user", content: "first" },
          {
            id: "assistant-first",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "first answer" }],
          },
        ],
      })
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [
          { id: "user-first", role: "user", content: "first" },
          {
            id: "assistant-first",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "first answer" }],
          },
          { id: "server-second", role: "user", content: "second" },
          {
            id: "assistant-second",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "second answer" }],
          },
        ],
      });

    await act(async () => {
      firstStream.send({
        type: "user_message",
        message: {
          id: "user-first",
          role: "user",
          content: [{ type: "text", text: "first" }],
        },
      });
      firstStream.send({ type: "text_delta", delta: "first answer" });
      firstStream.send({
        object: "response",
        status: "completed",
        output: [],
      });
      firstStream.send({
        type: "user_message",
        message: {
          id: "server-second",
          role: "user",
          content: [{ type: "text", text: "second" }],
          metadata: { submission_id: "submission-input-second" },
        },
      });
      firstStream.send({ type: "text_delta", delta: "second answer" });
      firstStream.send({
        object: "response",
        status: "completed",
        output: [],
      });
      firstStream.send({
        type: "user_message",
        message: {
          id: "server-third",
          role: "user",
          content: [{ type: "text", text: "third" }],
          metadata: { submission_id: "submission-input-third" },
        },
      });
    });

    await waitFor(() =>
      expect(
        result.current.projection.items.some(
          (item) => item.kind === "user" && item.id === "server-third",
        ),
      ).toBe(true),
    );
    await act(async () => {
      firstStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-third",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "third answer" }],
          },
        ],
      });
    });

    await waitFor(() => {
      expect(result.current.queuedSubmissions).toHaveLength(0);
      const itemTexts = result.current.projection.items.map((item) =>
        JSON.stringify(item),
      );
      expect(itemTexts).toHaveLength(6);
      expect(itemTexts[0]).toContain("first");
      expect(itemTexts[1]).toContain("first answer");
      expect(itemTexts[2]).toContain("second");
      expect(itemTexts[3]).toContain("second answer");
      expect(itemTexts[4]).toContain("third");
      expect(itemTexts[5]).toContain("third answer");
    });

    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      model: configuredModel,
      messages: [
        { id: "user-first", role: "user", content: "first" },
        {
          id: "assistant-first",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "first answer" }],
        },
        { id: "server-second", role: "user", content: "second" },
        {
          id: "assistant-second",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "second answer" }],
        },
        { id: "server-third", role: "user", content: "third" },
        {
          id: "assistant-third",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "third answer" }],
        },
      ],
    });
    await act(async () => {
      firstStream.send({
        object: "response",
        status: "completed",
        output: [],
      });
      firstStream.close();
      await firstSubmit;
    });
    await waitFor(() => {
      const itemTexts = result.current.projection.items.map((item) =>
        JSON.stringify(item),
      );
      expect(itemTexts).toHaveLength(6);
      expect(itemTexts.join("\n")).toContain("first answer");
      expect(itemTexts.join("\n")).toContain("second answer");
      expect(itemTexts.join("\n")).toContain("third answer");
    });
  });

  it("does not reclaim a backend authority queued submit locally", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2)
            return submitQueuedResponse("input-second", "second");
        }
        return undefined;
      }),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second", [
        {
          id: "attachment-1",
          name: "notes.txt",
          type: "file",
          url: "/uploads/notes.txt",
          storedName: "stored-notes.txt",
        },
      ]);
    });

    await waitFor(() =>
      expect(result.current.queuedSubmissions).toHaveLength(1),
    );
    const queuedId = result.current.queuedSubmissions[0].id;
    let reclaimed:
      | ReturnType<typeof result.current.reclaimQueuedSubmission>
      | undefined;
    act(() => {
      reclaimed = result.current.reclaimQueuedSubmission(queuedId);
    });

    expect(reclaimed).toBeNull();
    expect(result.current.queuedSubmissions).toHaveLength(1);

    firstStream.send({
      type: "user_message",
      message: {
        id: "input-second",
        role: "user",
        content: [{ type: "text", text: "second" }],
      },
    });
    firstStream.send({ object: "response", status: "completed", output: [] });
    firstStream.send({
      type: "turn_terminal",
      turn_id: "turn-second",
      status: "completed",
    });
    firstStream.close();
    await act(async () => {
      await firstSubmit;
    });

    await waitFor(() => expect(isTurnActive(result)).toBe(false));
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(2);
  });

  it("uses the authoritative epoch when sending a queued input now", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url, _init) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2)
            return submitQueuedResponse("input-second", "second");
          return submitAcceptedResponse();
        }
        return undefined;
      }),
    );
    const { result, rerender } = renderHook(
      ({ target }: { target: string | null }) =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          authorityPlacementDeviceId: target,
        }),
      { initialProps: { target: null } },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void> | undefined;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
    });
    expect(result.current.queuedSubmissions).toHaveLength(1);

    rerender({ target: "device-remote" });
    await act(async () => {
      await result.current.submitQueuedNow(
        result.current.queuedSubmissions[0].id,
      );
    });

    await waitFor(() =>
      expect(chatApiMock.sendNowQueuedInput).toHaveBeenCalled(),
    );
    const chatCalls = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
      ([url]) => String(url).includes("/console/submit"),
    );
    expect(chatCalls).toHaveLength(2);
    expect(chatApiMock.sendNowQueuedInput).toHaveBeenCalledWith(
      "chat-1",
      "input-second",
      expect.objectContaining({
        authority_epoch: "0000000000000001000000000000000000000000",
      }),
      "agent-a",
    );
    firstStream.close();
    await act(async () => {
      await firstSubmit;
    });
  });

  it("does not stop background continuation when there is no active turn", async () => {
    const epoch = "0000000000000001000000000000000000000000";
    chatApiMock.getChat.mockResolvedValue({
      messages: [],
      status: "idle",
      model: configuredModel,
      control: {
        revision: 1,
        placement: { authority_epoch: epoch },
        queue: { entries: [], paused: false },
        background_work: {
          suppressed: false,
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "running",
              chat: {
                scope: { kind: "agent", agent_id: "agent-a" },
                chat_id: "chat-1",
              },
            },
          ],
        },
      },
    });
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() =>
      expect(
        result.current.history?.control?.background_work?.items,
      ).toHaveLength(1),
    );
    await act(async () => {
      await result.current.stop();
    });
    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
    expect(result.current.history?.active_turn).toBeFalsy();
  });

  it("stops through the selected agent without reloading the conversation", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "running",
      messages: [],
      model: configuredModel,
      active_turn: {
        turn_id: "turn-stop",
        authority_epoch: "0000000000000001000000000000000000000000",
        phase: "streaming",
        driver: "rua",
      },
    });
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.stop();
    });

    expect(chatApiMock.stopChat).toHaveBeenCalledWith(
      "chat-1",
      {
        turn_id: "turn-stop",
        authority_epoch: "0000000000000001000000000000000000000000",
      },
      "agent-a",
    );
    expect(chatApiMock.getChat).toHaveBeenCalledTimes(1);
  });

  it("preserves partial output until the server confirms stop", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: { provider_id: "dashscope", model: "qwen-plus" },
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let submitPromise: Promise<void>;
    await act(async () => {
      submitPromise = result.current.submit("Hello");
    });
    await act(async () => {
      stream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-stream",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "partial" }],
          },
        ],
      });
    });
    await waitFor(() =>
      expect(result.current.history?.messages).toHaveLength(2),
    );

    await act(async () => {
      await result.current.stop();
    });
    expect(chatApiMock.getChat).toHaveBeenCalledTimes(1);
    expect(stream.cancel).not.toHaveBeenCalled();
    expect(JSON.stringify(result.current.history?.messages || [])).toContain(
      "partial",
    );
    expect(
      JSON.stringify(result.current.history?.messages || []),
    ).not.toContain("<turn_aborted>");
    await act(async () => {
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "interrupted",
      });
    });
    await waitFor(() =>
      expect(
        result.current.projection.items.find(
          (item) => item.kind === "assistant",
        )?.status,
      ).toBe("interrupted"),
    );
    const messages = result.current.history?.messages || [];
    expect(messages.filter((message) => message.role === "user")).toHaveLength(
      1,
    );
    expect(
      messages.filter((message) => message.role === "assistant"),
    ).toHaveLength(1);
    expect(
      messages.filter((message) => message.role === "system"),
    ).toHaveLength(1);
    expect(JSON.stringify(messages)).toContain("partial");
    expect(JSON.stringify(messages)).toContain("<turn_aborted>");
    expect(JSON.stringify(messages)).not.toContain(
      "The assistant returned no output.",
    );
    expect(
      result.current.projection.items.find((item) => item.kind === "assistant")
        ?.status,
    ).toBe("interrupted");
    await act(async () => {
      stream.close();
      await submitPromise;
    });
  });

  it.each(["rejected", "error"])(
    "keeps Live output and reports a %s stop request",
    async (failure) => {
      const stream = controlledSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/console/live") ? stream.response : undefined,
        ),
      );
      if (failure === "error") {
        chatApiMock.stopChat.mockRejectedValueOnce(
          new Error("Stop unavailable"),
        );
      } else {
        chatApiMock.stopChat.mockResolvedValueOnce({
          stopped: false,
          mode: "local",
          detail: "Stop unavailable",
        });
      }
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));
      await act(async () => {
        await result.current.submit("Hello");
      });
      await waitFor(() => expect(isTurnActive(result)).toBe(true));
      await act(async () => {
        await result.current.stop();
      });
      expect(result.current.error).toBe("Stop unavailable");
      expect(stream.cancel).not.toHaveBeenCalled();
      expect(isTurnActive(result)).toBe(true);
      await act(async () => {
        stream.send({
          object: "response",
          status: "in_progress",
          output: [
            {
              id: "assistant-1",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "still receiving output" }],
            },
          ],
        });
      });
      await waitFor(() =>
        expect(JSON.stringify(result.current.history?.messages)).toContain(
          "still receiving output",
        ),
      );
      expect(JSON.stringify(result.current.history?.messages)).not.toContain(
        "<turn_aborted>",
      );
      await act(async () => stream.close());
    },
  );

  it("keeps the remote live stream open after a targeted stop request", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValue({
      active_turn: {
        turn_id: "turn-stop",
        authority_epoch: "0000000000000001000000000000000000000000",
        phase: "streaming",
        driver: "rua",
      },
      status: "idle",
      messages: [],
      model: configuredModel,
    });
    chatApiMock.stopChat.mockResolvedValueOnce({
      stopped: true,
      mode: "remote_control",
      control: "remote_stop",
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let submitPromise: Promise<void>;
    act(() => {
      submitPromise = result.current.submit("Hello");
    });
    await act(async () => {
      stream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-stream",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "partial" }],
          },
        ],
      });
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages)).toContain(
        "partial",
      ),
    );

    await act(async () => {
      await result.current.stop();
    });

    expect(chatApiMock.stopChat).toHaveBeenCalledWith(
      "chat-1",
      {
        turn_id: "turn-stop",
        authority_epoch: "0000000000000001000000000000000000000000",
      },
      "agent-a",
    );
    expect(isTurnActive(result)).toBe(true);
    expect(JSON.stringify(result.current.history?.messages)).not.toContain(
      "<turn_aborted>",
    );

    await act(async () => {
      stream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-stream",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "after remote stop" }],
          },
        ],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages)).toContain(
        "after remote stop",
      ),
    );

    await act(async () => {
      stream.close();
      await submitPromise;
    });
  });

  it("keeps the Project live stream open through the existing remote stop path", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/projects/project-1/chats/chat-1/console/live")
          ? stream.response
          : undefined,
      ),
    );
    projectChatsApiMock.getChat.mockResolvedValue({
      status: "running",
      active_turn: {
        turn_id: "turn-project-stop",
        user_message_id: "user-project-stop",
        phase: "streaming",
        authority_device_id: "device-controller",
        authority_epoch: "0000000000000001000000000000000000000000",
        driver: "rua",
        started_at: "2026-07-20T00:00:00Z",
      },
      control: {
        revision: 1,
        placement: null,
        active_turn: {
          turn_id: "turn-project-stop",
          user_message_id: "user-project-stop",
          phase: "streaming",
          authority_device_id: "device-controller",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-07-20T00:00:00Z",
        },
        input_queue: [],
      },
      messages: [],
      model: configuredModel,
    });
    projectChatsApiMock.stopChat.mockResolvedValueOnce({
      stopped: true,
      mode: "remote_control",
      control: "remote_stop",
    });
    const target = {
      scope: "project" as const,
      projectId: "project-1",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? { scope: "project", projectId: target.projectId, chatId: "chat-1" }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );

    await act(async () => {
      stream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-stream",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "partial" }],
          },
        ],
      });
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages)).toContain(
        "partial",
      ),
    );

    await act(async () => {
      await result.current.stop();
    });

    expect(projectChatsApiMock.stopChat).toHaveBeenCalledWith(
      "project-1",
      "chat-1",
      {
        turn_id: "turn-project-stop",
        authority_epoch: "0000000000000001000000000000000000000000",
      },
    );
    expect(isTurnActive(result)).toBe(true);
    expect(JSON.stringify(result.current.history?.messages)).not.toContain(
      "<turn_aborted>",
    );

    await act(async () => {
      stream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-stream",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "after remote stop" }],
          },
        ],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages)).toContain(
        "after remote stop",
      ),
    );

    await act(async () => {
      stream.close();
    });
  });

  it("keeps completed remote live output visible until refreshed history covers it", async () => {
    const coveredHistory = deferred<unknown>();
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    const user = {
      id: "user-user-uuid",
      role: "user",
      content: [{ type: "text", text: "Hello remote" }],
    };
    const assistant = {
      id: "assistant-remote",
      role: "assistant",
      type: "message",
      content: [{ type: "text", text: "remote final" }],
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [user],
      })
      .mockImplementation(() => coveredHistory.promise);

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let submitPromise: Promise<void>;
    act(() => {
      submitPromise = result.current.submit("Hello remote");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      stream.send({
        object: "response",
        status: "in_progress",
        output: [assistant],
      });
      stream.send({ object: "response", status: "completed", output: [] });
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-remote",
        status: "completed",
      });
      stream.close();
      await submitPromise;
    });

    await waitFor(() =>
      expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThanOrEqual(2),
    );
    expect(JSON.stringify(result.current.history?.messages || [])).toContain(
      "remote final",
    );

    await act(async () => {
      coveredHistory.resolve({
        status: "idle",
        model: configuredModel,
        messages: [user, assistant],
      });
      result.current.reload();
    });
    await waitFor(() => {
      const rendered = JSON.stringify(result.current.history?.messages || []);
      expect(rendered.match(/remote final/g) || []).toHaveLength(1);
    });
  });

  it("does not poll a fixed-target viewer while the live stream stays open", async () => {
    vi.useFakeTimers();
    try {
      vi.stubGlobal(
        "fetch",
        vi.fn().mockResolvedValue(
          new Response("{}", {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        ),
      );
      const initialHistory = {
        status: "idle",
        model: configuredModel,
        messages: [
          {
            id: "old-user",
            role: "user",
            content: [{ type: "text", text: "old prompt" }],
          },
        ],
        meta: {
          authority_placement: { device_id: "device-remote" },
        },
      };
      chatApiMock.getChat.mockResolvedValue(initialHistory);

      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          authorityPlacementDeviceId: "device-remote",
          currentDeviceId: "device-c",
        }),
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2500);
      });

      expect(result.current.loading).toBe(false);
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "old prompt",
      );
      expect(chatApiMock.getChat).toHaveBeenCalledTimes(1);
      const syncCall = (fetch as ReturnType<typeof vi.fn>).mock.calls.find(
        ([url]) => String(url).includes("/sync/cycle/target"),
      );
      expect(syncCall).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows ACP load history preview before the next prompt", async () => {
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [
          {
            id: "acp-history-preview-message-1",
            role: "ui",
            type: "acp_history_preview",
            content: [{ type: "text", text: "previous ACP transcript" }],
            metadata: {
              ui_only: true,
              model_visible: false,
              acp_history_preview: true,
              origin: { kind: "acp", runner: "codex" },
            },
          },
        ],
      });

    const { result, rerender } = renderHook(
      ({ preview }: { preview: { id: number; text: string } | null }) =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          activeAcpRunner: "codex",
          acpHistoryPreview: preview,
        }),
      { initialProps: { preview: null } },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    rerender({ preview: { id: 1, text: "previous ACP transcript" } });

    await waitFor(() =>
      expect(chatApiMock.persistAcpHistoryPreview).toHaveBeenCalledWith(
        "chat-1",
        {
          preview_id: 1,
          text: "previous ACP transcript",
          runner: "codex",
        },
        "agent-a",
      ),
    );
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "previous ACP transcript",
      ),
    );
    expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("queues submits while ACP history preview persistence is preparing", async () => {
    const persistence = deferred<{
      turn_id: string;
      message_id: string;
      created: boolean;
    }>();
    chatApiMock.persistAcpHistoryPreview.mockReturnValue(persistence.promise);
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        activeAcpRunner: "codex",
        acpHistoryPreview: {
          id: 1,
          text: "previous ACP transcript",
        },
      }),
    );
    await waitFor(() =>
      expect(chatApiMock.persistAcpHistoryPreview).toHaveBeenCalled(),
    );
    expect(isTurnActive(result)).toBe(false);

    await act(async () => {
      await result.current.submit("next prompt");
    });

    expect(result.current.queuedSubmissions).toHaveLength(1);
    expect(result.current.queuedSubmissions[0].text).toBe("next prompt");
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toBe(false);

    await act(async () => {
      persistence.resolve({
        turn_id: "acp-history-preview-1",
        message_id: "acp-history-preview-message-1",
        created: true,
      });
      await persistence.promise;
    });

    expect(result.current.queuedSubmissions).toHaveLength(1);
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toBe(false);
  });

  it("mirrors remote authority queued input metadata from submit control projection", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2) {
            return submitQueuedResponse("input-second", "second", [
              {
                input_id: "input-second",
                text: "second",
                device_delivery_id: "device-delivery-2",
              },
            ]);
          }
        }
        return undefined;
      }),
    );
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void>;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
    });

    await waitFor(() =>
      expect(result.current.queuedSubmissions[0]).toMatchObject({
        text: "second",
        serverQueued: true,
        deviceDeliveryId: "device-delivery-2",
      }),
    );
    const rendered = JSON.stringify(result.current.history?.messages || []);
    expect(rendered).not.toContain("device-delivery-2");
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(2);

    await act(async () => {
      firstStream.close();
      await firstSubmit;
    });
  });

  it("does not render both pending and live user boundaries for the same device delivery", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return liveStream.response;
        if (url.includes("/console/submit")) {
          return submitAcceptedResponse({
            mode: "started",
            device_delivery_id: "device-delivery-1",
            target_device_id: "device-remote",
            device_delivery_sequence: 1,
            accepted_at: "2026-07-05T00:00:00Z",
          });
        }
        return undefined;
      }),
    );
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("first remote");
    });
    await waitFor(() =>
      expect(
        result.current.projection.items.filter((item) => item.kind === "user"),
      ).toHaveLength(1),
    );
    expect(result.current.projection.items[0]).toMatchObject({
      kind: "user",
      remote_delivery_state: "pending",
    });
    expect(
      result.current.projection.items.filter((item) => item.kind === "user"),
    ).toHaveLength(1);
    const placeholder = result.current.projection.items.find(
      (item) => item.kind === "assistant",
    );
    if (placeholder?.kind === "assistant")
      expect(placeholder.output).toEqual([]);
    expect(isTurnActive(result)).toBe(true);
    expect(result.current.projection.items[0]).not.toMatchObject({
      remote_delivery_status: "delivering",
    });

    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toBe(true),
    );
    await act(async () => {
      liveStream.send({
        type: "user_message",
        message: {
          id: "target-user-1",
          role: "user",
          content: [{ type: "text", text: "first remote" }],
          metadata: {
            device_delivery_id: "device-delivery-1",
          },
        },
      });
    });

    expect(
      result.current.projection.items.filter((item) => item.kind === "user"),
    ).toHaveLength(1);

    await act(async () => {
      liveStream.close();
    });
  });

  it("clears remote pending delivery and reloads official history when live turn_terminal arrives", async () => {
    const liveStream = controlledSseResponse();
    let submitCalls = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return liveStream.response;
        if (url.includes("/console/submit")) {
          submitCalls += 1;
          return submitAcceptedResponse({
            mode: "started",
            device_delivery_id: `device-delivery-${submitCalls}`,
            target_device_id: "device-remote",
            device_delivery_sequence: submitCalls,
            accepted_at: "2026-07-05T00:00:00Z",
          });
        }
        return undefined;
      }),
    );
    let officialHistoryAvailable = false;
    const officialHistory = {
      status: "idle" as const,
      model: configuredModel,
      messages: [
        {
          id: "official-user-1",
          role: "user",
          content: [{ type: "text", text: "first remote" }],
          metadata: {
            device_delivery_id: "device-delivery-1",
          },
        },
        {
          id: "official-assistant-1",
          role: "assistant",
          content: [{ type: "text", text: "official answer" }],
        },
      ],
    };
    chatApiMock.getChat.mockImplementation(async () =>
      officialHistoryAvailable
        ? officialHistory
        : { status: "idle", messages: [], model: configuredModel },
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("first remote");
    });
    await waitFor(() =>
      expect(result.current.projection.items[0]).toMatchObject({
        kind: "user",
        remote_delivery_state: "pending",
      }),
    );

    officialHistoryAvailable = true;
    await act(async () => {
      liveStream.send({
        type: "turn_terminal",
        turn_id: "turn-remote-1",
        status: "completed",
        device_delivery_id: "device-delivery-1",
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "official answer",
      ),
    );
    expect(JSON.stringify(result.current.projection.items || [])).not.toContain(
      "Delivered, waiting for result",
    );

    await act(async () => {
      await result.current.submit("second remote");
    });

    expect(submitCalls).toBe(2);
    expect(result.current.queuedSubmissions).toHaveLength(0);
  });

  it("reconciles remote pending delivery when idle control arrives without turn_terminal", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return liveStream.response;
        if (url.includes("/console/submit")) {
          return submitAcceptedResponse({
            mode: "started",
            device_delivery_id: "device-delivery-1",
            target_device_id: "device-remote",
            device_delivery_sequence: 1,
            accepted_at: "2026-07-05T00:00:00Z",
          });
        }
        return undefined;
      }),
    );
    let officialHistoryAvailable = false;
    const officialHistory = {
      status: "idle" as const,
      model: configuredModel,
      messages: [
        {
          id: "official-user-1",
          role: "user",
          content: [{ type: "text", text: "first remote" }],
          metadata: {
            device_delivery_id: "device-delivery-1",
          },
        },
        {
          id: "official-assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "official answer" }],
        },
      ],
    };
    chatApiMock.getChat.mockImplementation(() =>
      Promise.resolve(
        officialHistoryAvailable
          ? officialHistory
          : { status: "idle", messages: [], model: configuredModel },
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("first remote");
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "pending",
      ),
    );

    officialHistoryAvailable = true;
    await act(async () => {
      liveStream.send({
        type: "conversation_control",
        reason: "turn_finished",
        control: {
          revision: 5,
          placement: {
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000002000000000000000000000000",
            switchable: true,
          },
          active_turn: null,
          queue: { entries: [], paused: false },
          acp: { active: false },
          allowed_controls: {
            stop: false,
            queue: false,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: true,
            enter_acp: true,
            exit_acp: false,
          },
        },
      });
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "official answer",
      ),
    );
    expect(JSON.stringify(result.current.projection.items)).not.toContain(
      "pending",
    );
  });

  it("reloads official history when live history hydration completes", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return liveStream.response;
        return undefined;
      }),
    );
    let officialHistoryAvailable = false;
    chatApiMock.getChat.mockImplementation(async () =>
      officialHistoryAvailable
        ? {
            status: "idle",
            model: configuredModel,
            messages: [
              {
                id: "official-user-1",
                role: "user",
                content: [{ type: "text", text: "hydrated prompt" }],
              },
              {
                id: "official-assistant-1",
                role: "assistant",
                content: [{ type: "text", text: "hydrated answer" }],
              },
            ],
          }
        : { status: "idle", messages: [], model: configuredModel },
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-local",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/live"),
        ),
      ).toBe(true),
    );

    officialHistoryAvailable = true;
    await act(async () => {
      liveStream.send({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        agent_id: "agent-a",
        chat_id: "chat-1",
        authority_device_id: "device-remote",
      });
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "hydrated answer",
      ),
    );
    const targetedSyncCalls = (
      fetch as ReturnType<typeof vi.fn>
    ).mock.calls.filter(([url]) => String(url).includes("/sync/cycle/target"));
    expect(targetedSyncCalls).toHaveLength(1);
    expect(JSON.parse(String(targetedSyncCalls[0]?.[1]?.body))).toEqual({
      target_device_id: "device-remote",
    });
  });

  it("clears a live spinner when hydrated stable history covers the turn without a terminal frame", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const user = {
      id: "remote-user-1",
      turn_id: "turn-1",
      role: "user",
      content: [{ type: "text", text: "inspect" }],
    };
    const assistant = {
      id: "remote-assistant-1",
      turn_id: "turn-1",
      role: "assistant",
      type: "message",
      content: [{ type: "text", text: "stable final" }],
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "running",
        active_turn: {
          turn_id: "turn-1",
          user_message_id: "remote-user-1",
          phase: "streaming",
          authority_device_id: "device-remote",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-07-20T00:00:00Z",
        },
        control: {
          revision: 1,
          placement: null,
          active_turn: {
            turn_id: "turn-1",
            user_message_id: "remote-user-1",
            phase: "streaming",
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-20T00:00:00Z",
          },
          input_queue: [],
        },
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValueOnce({
        status: "idle",
        active_turn: null,
        control: {
          revision: 2,
          placement: null,
          active_turn: null,
          input_queue: [],
        },
        messages: [user, assistant],
        model: configuredModel,
      });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      liveStream.send({ type: "user_message", message: user });
      liveStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            ...assistant,
            id: "live-assistant-1",
            content: [{ type: "text", text: "partial" }],
          },
        ],
      });
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      liveStream.send({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        agent_id: "agent-a",
        chat_id: "chat-1",
        authority_device_id: "device-remote",
      });
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "stable final",
      ),
    );
    await waitFor(() => expect(isTurnActive(result)).toBe(false));
    expect(
      (result.current.history?.messages || []).filter(
        (message) => message.role === "assistant",
      ),
    ).toEqual([assistant]);
    liveStream.close();
  });

  it("keeps stable history converged when final response frames trail the idle control", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const user = {
      id: "stable-user-1",
      turn_id: "turn-1",
      role: "user",
      content: [{ type: "text", text: "inspect" }],
    };
    const stableAssistant = {
      id: "stable-assistant-1",
      turn_id: "turn-1",
      role: "assistant",
      type: "message",
      content: [{ type: "text", text: "final answer" }],
    };
    const stableHistory = {
      messages: [user, stableAssistant],
      model: configuredModel,
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValueOnce(stableHistory)
      .mockResolvedValue(stableHistory);

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      liveStream.send({ type: "user_message", message: user });
      liveStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            ...stableAssistant,
            id: "stable-assistant-1",
            turn_id: undefined,
            content: [{ type: "text", text: "partial" }],
          },
        ],
      });
      liveStream.send({
        type: "conversation_control",
        reason: "turn_handoff_completed",
        control: {
          revision: 2,
          placement: {
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000002000000000000000000000000",
            switchable: true,
          },
          active_turn: null,
          queue: { entries: [], paused: false },
          acp: { active: false },
          allowed_controls: {
            stop: false,
            queue: false,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: true,
            enter_acp: true,
            exit_acp: false,
          },
        },
      });
    });

    await waitFor(() =>
      expect(
        (result.current.history?.messages || []).filter(
          (message) => message.role === "assistant",
        ),
      ).toEqual([stableAssistant]),
    );

    await act(async () => {
      liveStream.send({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "completed",
        device_delivery_id: "device-delivery-1",
      });
    });

    await waitFor(() =>
      expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThanOrEqual(3),
    );
    await waitFor(() =>
      expect(
        (result.current.history?.messages || []).filter(
          (message) => message.role === "assistant",
        ),
      ).toEqual([stableAssistant]),
    );

    await act(async () => {
      liveStream.send({
        object: "response",
        status: "completed",
        output: [
          {
            ...stableAssistant,
            id: "stable-assistant-1",
            turn_id: undefined,
          },
        ],
      });
    });

    expect(
      (result.current.history?.messages || []).filter(
        (message) => message.role === "assistant",
      ),
    ).toEqual([stableAssistant]);
    liveStream.close();
  });

  it("does not reactivate a completed live turn after stable history takes ownership", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const user = {
      id: "stable-user-1",
      turn_id: "turn-1",
      role: "user",
      content: [{ type: "text", text: "What day is it?" }],
    };
    const stableToolCall = {
      id: "stable-tool-call-1",
      turn_id: "turn-1",
      role: "assistant",
      type: "plugin_call",
      content: [
        {
          type: "data",
          data: {
            call_id: "call-time-1",
            name: "time_now",
            arguments: '{"timezone":"Asia/Shanghai"}',
          },
        },
      ],
    };
    const stableToolResult = {
      id: "stable-tool-result-1",
      turn_id: "turn-1",
      role: "system",
      type: "plugin_call_output",
      content: [
        {
          type: "data",
          data: {
            call_id: "call-time-1",
            name: "time_now",
            output: "Wednesday",
          },
        },
      ],
    };
    const stableAssistant = {
      id: "stable-assistant-1",
      turn_id: "turn-1",
      role: "assistant",
      type: "message",
      content: [{ type: "text", text: "Today is Wednesday." }],
    };
    const stableHistory = {
      status: "idle" as const,
      messages: [user, stableToolCall, stableToolResult, stableAssistant],
      model: configuredModel,
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValue(stableHistory);

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    const liveOutput = [
      {
        ...stableToolCall,
        turn_id: undefined,
      },
      {
        ...stableToolResult,
        turn_id: undefined,
        role: "tool",
      },
      {
        ...stableAssistant,
        turn_id: undefined,
      },
    ];

    await act(async () => {
      liveStream.send({ type: "user_message", message: user });
      liveStream.send({
        object: "response",
        status: "in_progress",
        output: liveOutput,
      });
      liveStream.send({
        type: "conversation_control",
        reason: "turn_completed",
        control: {
          revision: 2,
          placement: {
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000002000000000000000000000000",
            switchable: true,
          },
          active_turn: null,
          queue: { entries: [], paused: false },
          acp: { active: false },
          allowed_controls: {
            stop: false,
            queue: false,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: true,
            enter_acp: true,
            exit_acp: false,
          },
        },
      });
    });

    await waitFor(() =>
      expect(result.current.history?.messages).toEqual(stableHistory.messages),
    );
    await waitFor(() =>
      expect(
        result.current.projection.items.filter(
          (item) => item.kind === "assistant",
        ),
      ).toHaveLength(1),
    );
    expect(
      result.current.projection.items.find((item) => item.kind === "assistant")
        ?.output,
    ).toHaveLength(3);

    await act(async () => {
      liveStream.send({
        type: "usage",
        usage: {
          prompt_tokens: 100,
          completion_tokens: 10,
          total_tokens: 110,
        },
      });
      liveStream.send({
        object: "response",
        status: "completed",
        output: liveOutput,
      });
      liveStream.send({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "completed",
      });
    });

    await waitFor(() => {
      const assistant = result.current.projection.items.find(
        (item) => item.kind === "assistant",
      );
      expect(assistant?.output).toHaveLength(3);
      expect(
        (JSON.stringify(assistant).match(/Today is Wednesday\./g) || []).length,
      ).toBe(1);
    });
    liveStream.close();
  });

  it("ignores duplicate activity sequences without dropping the next event", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    const activity = (sequence: number) => ({
      activity_generation: "generation-1",
      activity_sequence: sequence,
    });
    await act(async () => {
      liveStream.send({
        ...activity(1),
        type: "user_message",
        message: {
          id: "user-1",
          turn_id: "turn-1",
          role: "user",
          content: [{ type: "text", text: "question" }],
        },
      });
      liveStream.send({
        ...activity(2),
        type: "text_delta",
        delta: "answer",
      });
      liveStream.send({
        ...activity(2),
        type: "text_delta",
        delta: "answer",
      });
      liveStream.send({
        ...activity(3),
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "completed",
      });
    });

    await waitFor(() => {
      const rendered = JSON.stringify(result.current.projection.items);
      expect(rendered.match(/answer/g) || []).toHaveLength(1);
    });
    liveStream.close();
  });

  it("clears a replay-only spinner when stable history covers output without a replayed user message", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const stableAssistant = {
      id: "stable-assistant-1",
      turn_id: "turn-1",
      role: "assistant",
      type: "message",
      content: [{ type: "text", text: "stable replay result" }],
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "running",
        active_turn: {
          turn_id: "turn-1",
          user_message_id: "stable-user-1",
          phase: "streaming",
          authority_device_id: "device-remote",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-07-20T00:00:00Z",
        },
        control: {
          revision: 1,
          placement: null,
          active_turn: {
            turn_id: "turn-1",
            user_message_id: "stable-user-1",
            phase: "streaming",
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-20T00:00:00Z",
          },
          input_queue: [],
        },
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValueOnce({
        status: "idle",
        active_turn: null,
        control: {
          revision: 2,
          placement: null,
          active_turn: null,
          input_queue: [],
        },
        messages: [
          {
            id: "stable-user-1",
            turn_id: "turn-1",
            role: "user",
            content: [{ type: "text", text: "started before reconnect" }],
          },
          stableAssistant,
        ],
        model: configuredModel,
      });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      liveStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            ...stableAssistant,
            content: [{ type: "text", text: "partial replay" }],
          },
        ],
      });
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      liveStream.send({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        agent_id: "agent-a",
        chat_id: "chat-1",
        authority_device_id: "device-remote",
      });
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "stable replay result",
      ),
    );
    await waitFor(() => expect(isTurnActive(result)).toBe(false));
    liveStream.close();
  });

  it.each([
    [
      "failed",
      {
        id: "stable-failed-1",
        role: "assistant",
        type: "message",
        status: "failed",
        content: [{ type: "text", text: "provider failed" }],
      },
    ],
    [
      "interrupted",
      {
        id: "stable-aborted-1",
        role: "system",
        type: "turn_aborted",
        content: [{ type: "text", text: "<turn_aborted>" }],
      },
    ],
    [
      "interjected",
      {
        id: "stable-interjected-1",
        role: "system",
        type: "turn_interjected",
        content: [{ type: "text", text: "<turn_interjected>" }],
      },
    ],
  ])(
    "clears a live spinner when stable %s history arrives without a terminal frame",
    async (_outcome, stableResponse) => {
      const liveStream = controlledSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/console/live") ? liveStream.response : undefined,
        ),
      );
      const user = {
        id: "stable-terminal-user-1",
        turn_id: "turn-stable-terminal-1",
        role: "user",
        content: [{ type: "text", text: "finish this turn" }],
      };
      chatApiMock.getChat
        .mockResolvedValueOnce({
          status: "running",
          active_turn: {
            turn_id: "turn-stable-terminal-1",
            user_message_id: "stable-terminal-user-1",
            phase: "streaming",
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-20T00:00:00Z",
          },
          control: {
            revision: 1,
            placement: null,
            active_turn: {
              turn_id: "turn-stable-terminal-1",
              user_message_id: "stable-terminal-user-1",
              phase: "streaming",
              authority_device_id: "device-remote",
              authority_epoch: "0000000000000001000000000000000000000000",
              driver: "rua",
              started_at: "2026-07-20T00:00:00Z",
            },
            input_queue: [],
          },
          messages: [],
          model: configuredModel,
        })
        .mockResolvedValueOnce({
          status: "idle",
          active_turn: null,
          control: {
            revision: 2,
            placement: null,
            active_turn: null,
            input_queue: [],
          },
          messages: [
            user,
            { ...stableResponse, turn_id: "turn-stable-terminal-1" },
          ],
          model: configuredModel,
        });

      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          authorityPlacementDeviceId: "device-remote",
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));

      await act(async () => {
        liveStream.send({ type: "user_message", message: user });
        liveStream.send({
          object: "response",
          status: "in_progress",
          output: [
            {
              id: "live-terminal-assistant-1",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "partial" }],
            },
          ],
        });
      });
      await waitFor(() => expect(isTurnActive(result)).toBe(true));

      await act(async () => {
        liveStream.send({
          type: "history_watermark_reached",
          durable_sequence: 1,
          manifest_hash: "test-manifest-1",
          agent_id: "agent-a",
          chat_id: "chat-1",
          authority_device_id: "device-remote",
        });
      });

      await waitFor(() => expect(isTurnActive(result)).toBe(false));
      liveStream.close();
    },
  );

  it("keeps uncovered live output when stable history has only matching text", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? liveStream.response : undefined,
      ),
    );
    const liveUser = {
      id: "live-user-1",
      role: "user",
      content: [{ type: "text", text: "same text" }],
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "running",
        active_turn: {
          turn_id: "turn-live-1",
          user_message_id: "live-user-1",
          phase: "streaming",
          authority_device_id: "device-remote",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-07-20T00:00:00Z",
        },
        control: {
          revision: 1,
          placement: null,
          active_turn: {
            turn_id: "turn-live-1",
            user_message_id: "live-user-1",
            phase: "streaming",
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-20T00:00:00Z",
          },
          input_queue: [],
        },
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValueOnce({
        status: "running",
        active_turn: {
          turn_id: "turn-live-1",
          user_message_id: "live-user-1",
          phase: "streaming",
          authority_device_id: "device-remote",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-07-20T00:00:00Z",
        },
        control: {
          revision: 1,
          placement: null,
          active_turn: {
            turn_id: "turn-live-1",
            user_message_id: "live-user-1",
            phase: "streaming",
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-20T00:00:00Z",
          },
          input_queue: [],
        },
        messages: [
          {
            id: "different-user-1",
            role: "user",
            content: [{ type: "text", text: "same text" }],
          },
          {
            id: "different-assistant-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "unrelated stable answer" }],
          },
        ],
        model: configuredModel,
      });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      liveStream.send({ type: "user_message", message: liveUser });
      liveStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "live-assistant-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "live partial" }],
          },
        ],
      });
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "live partial",
      ),
    );

    await act(async () => {
      liveStream.send({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        agent_id: "agent-a",
        chat_id: "chat-1",
        authority_device_id: "device-remote",
      });
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "unrelated stable answer",
      ),
    );
    expect(isTurnActive(result)).toBe(true);
    expect(JSON.stringify(result.current.projection.items)).toContain(
      "live partial",
    );
    liveStream.close();
  });

  it("keeps consuming Project live output when running stable history covers only a partial Turn", async () => {
    const liveStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/projects/project-1/chats/chat-1/console/live")
          ? liveStream.response
          : undefined,
      ),
    );
    const user = {
      id: "project-user-1",
      turn_id: "turn-1",
      role: "user",
      content: [{ type: "text", text: "inspect" }],
    };
    const partialAssistant = {
      id: "project-assistant-1",
      turn_id: "turn-1",
      role: "assistant",
      type: "message",
      content: [{ type: "text", text: "partial" }],
    };
    projectChatsApiMock.getChat
      .mockResolvedValueOnce({
        status: "running",
        active_turn: {
          turn_id: "turn-1",
          user_message_id: "project-user-1",
          phase: "streaming",
          authority_device_id: "device-a",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-08-02T00:00:00Z",
        },
        messages: [user],
        model: configuredModel,
      })
      .mockResolvedValueOnce({
        status: "running",
        active_turn: {
          turn_id: "turn-1",
          user_message_id: "project-user-1",
          phase: "streaming",
          authority_device_id: "device-a",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-08-02T00:00:00Z",
        },
        messages: [user, partialAssistant],
        model: configuredModel,
      });
    const target = { scope: "project" as const, projectId: "project-1" };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? { scope: "project", projectId: target.projectId, chatId: "chat-1" }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-a",
        currentDeviceId: "device-a",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      liveStream.send({ type: "user_message", message: user });
      liveStream.send({
        object: "response",
        status: "in_progress",
        output: [partialAssistant],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "partial",
      ),
    );

    await act(async () => {
      liveStream.send({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        agent_id: "agent-a",
        chat_id: "chat-1",
        authority_device_id: "device-a",
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "partial",
      ),
    );
    expect(isTurnActive(result)).toBe(true);

    await act(async () => {
      liveStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            ...partialAssistant,
            content: [{ type: "text", text: "partial continued" }],
          },
        ],
      });
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "partial continued",
      ),
    );
    expect(isTurnActive(result)).toBe(true);
    liveStream.close();
  });

  it.each([
    ["remote", "device-remote", "device-local", "remote_control"],
    ["local", undefined, "device-remote", "local"],
    ["local with placement", "device-remote", "device-remote", "local"],
  ])(
    "clears stopping when authority marks the %s Turn idle",
    async (_name, deviceId, currentDeviceId, mode) => {
      const liveStream = controlledSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/console/live") ? liveStream.response : undefined,
        ),
      );
      const user = {
        id: "remote-stop-user-1",
        turn_id: "turn-remote-stop-1",
        role: "user",
        content: [{ type: "text", text: "stop this" }],
      };
      const assistant = {
        id: "remote-stop-assistant-1",
        role: "assistant",
        type: "message",
        content: [{ type: "text", text: "stopped final" }],
      };
      chatApiMock.getChat
        .mockResolvedValueOnce({
          status: "running",
          active_turn: {
            turn_id: "turn-remote-stop-1",
            user_message_id: "remote-stop-user-1",
            phase: "streaming",
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-20T00:00:00Z",
          },
          control: {
            revision: 1,
            placement: null,
            active_turn: {
              turn_id: "turn-remote-stop-1",
              user_message_id: "remote-stop-user-1",
              phase: "streaming",
              authority_device_id: "device-remote",
              authority_epoch: "0000000000000001000000000000000000000000",
              driver: "rua",
              started_at: "2026-07-20T00:00:00Z",
            },
            input_queue: [],
          },
          messages: [],
          model: configuredModel,
        })
        .mockResolvedValueOnce({
          status: "idle",
          active_turn: null,
          control: {
            revision: 2,
            placement: null,
            active_turn: null,
            input_queue: [],
          },
          messages: [user, { ...assistant, turn_id: "turn-remote-stop-1" }],
          model: configuredModel,
        });
      let resolveStop:
        | ((value: { stopped: boolean; mode: string; control: string }) => void)
        | undefined;
      chatApiMock.stopChat.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveStop = resolve;
          }),
      );

      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          authorityPlacementDeviceId: deviceId,
          currentDeviceId: currentDeviceId,
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));

      await act(async () => {
        liveStream.send({ type: "user_message", message: user });
        liveStream.send({
          object: "response",
          status: "in_progress",
          output: [
            {
              ...assistant,
              content: [{ type: "text", text: "partial" }],
            },
          ],
        });
      });
      await waitFor(() => expect(isTurnActive(result)).toBe(true));

      act(() => {
        void result.current.stop();
      });
      expect(result.current.projection.turnActivity.kind).toBe("active");

      await act(async () => {
        resolveStop?.({ stopped: true, mode, control: "stop" });
      });
      expect(isTurnActive(result)).toBe(true);

      await act(async () => {
        liveStream.send({
          type: "history_watermark_reached",
          durable_sequence: 1,
          manifest_hash: "test-manifest-1",
          agent_id: "agent-a",
          chat_id: "chat-1",
          authority_device_id: "device-remote",
        });
      });

      await waitFor(() => expect(isTurnActive(result)).toBe(false));

      expect(liveStream.cancel).not.toHaveBeenCalled();
      await act(async () => {
        liveStream.send({
          type: "conversation_control",
          control: {
            revision: 3,
            placement: null,
            active_turn: {
              turn_id: "next-turn",
              user_message_id: "next-user",
              phase: "streaming",
              authority_device_id: "device-remote",
              authority_epoch: "0000000000000001000000000000000000000000",
              driver: "rua",
              started_at: "2026-07-20T00:01:00Z",
            },
            input_queue: [],
          },
        });
      });
      await waitFor(() => expect(isTurnActive(result)).toBe(true));
      await act(async () => liveStream.close());
    },
  );

  it("treats stale remote pending projection as display-only for idle submit", async () => {
    localStorage.setItem(
      "rua:remote-pending-submissions:agent-a:chat-1",
      JSON.stringify([
        {
          device_delivery_id: "device-delivery-1",
          target_device_id: "device-remote",
          device_delivery_sequence: 1,
          accepted_at: "2026-07-05T00:00:00Z",
          message: {
            id: "stale-user-1",
            role: "user",
            content: [{ type: "text", text: "stale remote" }],
          },
        },
      ]),
    );
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/submit")) {
          return submitAcceptedResponse({
            mode: "started",
            device_delivery_id: "device-delivery-2",
            target_device_id: "device-remote",
            device_delivery_sequence: 2,
            accepted_at: "2026-07-05T00:00:02Z",
          });
        }
        return undefined;
      }),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "stale remote",
      ),
    );

    await act(async () => {
      await result.current.submit("second remote");
    });

    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toBe(true);
    expect(result.current.queuedSubmissions).toHaveLength(0);
    expect(JSON.stringify(result.current.projection.items)).not.toContain(
      "stale remote",
    );
    expect(JSON.stringify(result.current.projection.items)).toContain(
      "second remote",
    );
  });

  it.each([true, false])(
    "reconciles an old pending delivery outside the current history page only by identity (matched: %s)",
    async (matched) => {
      const key = "rua:remote-pending-submissions:agent-a:chat-1";
      localStorage.setItem(
        key,
        JSON.stringify([
          {
            device_delivery_id: "old-delivery",
            target_device_id: "device-remote",
            device_delivery_sequence: 1,
            turn_id: "old-turn",
            submission_id: "old-submission",
            message: {
              id: "optimistic-user",
              role: "user",
              content: [{ type: "text", text: "old input" }],
            },
          },
        ]),
      );
      chatApiMock.getChat.mockImplementation(async (...args: unknown[]) => {
        const options = args[2] as { aroundTurnId?: string } | undefined;
        return {
          status: "idle",
          messages: options?.aroundTurnId
            ? [
                {
                  id: "canonical-user",
                  role: "user",
                  turn_id: "old-turn",
                  content: [{ type: "text", text: "old input" }],
                  metadata: {
                    submission_id: matched
                      ? "old-submission"
                      : "unrelated-submission",
                  },
                },
              ]
            : [],
          model: configuredModel,
        };
      });
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          authorityPlacementDeviceId: "device-remote",
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));
      await waitFor(() =>
        expect(chatApiMock.getChat).toHaveBeenCalledWith("chat-1", "agent-a", {
          aroundTurnId: "old-turn",
          turnLimit: 1,
        }),
      );
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 30));
      });
      expect(localStorage.getItem(key) === null).toBe(matched);
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/console/submit"),
        ),
      ).toBe(false);
    },
  );

  it("keeps each failed remote delivery as a recoverable transport action", async () => {
    let uuidSequence = 0;
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => `remote-failure-${uuidSequence++}` },
      configurable: true,
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/submit")) {
          return jsonResponse({
            accepted: false,
            mode: "started",
            detail: "target unavailable",
          });
        }
        return undefined;
      }),
    );
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("second");
    });
    await waitFor(() =>
      expect(result.current.error).toBe("target unavailable"),
    );
    expect(result.current.queuedSubmissions).toEqual([
      expect.objectContaining({
        text: "second",
        source: "transport",
        deliveryState: "failed",
      }),
    ]);

    await act(async () => {
      await result.current.submit("third");
    });

    expect(result.current.queuedSubmissions).toHaveLength(2);
    expect(result.current.queuedSubmissions[1]).toMatchObject({
      text: "third",
      source: "transport",
      deliveryState: "failed",
    });
    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) =>
        String(url).includes("/console/submit"),
      ),
    ).toHaveLength(2);
  });

  it("distinguishes rejected Project input from an unconfirmed transport result", async () => {
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (!url.includes("/projects/project-1/chats/chat-1/console/submit")) {
          return undefined;
        }
        submitCount += 1;
        if (submitCount === 1) {
          return jsonResponse({
            accepted: false,
            mode: "started",
            detail: "target unavailable",
          });
        }
        throw new Error("transport unavailable");
      }),
    );
    projectChatsApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? { scope: "project", projectId: target.projectId, chatId: "chat-1" }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.submit("rejected Project input");
    });
    expect(result.current.queuedSubmissions).toEqual([
      expect.objectContaining({
        text: "rejected Project input",
        source: "transport",
        deliveryState: "failed",
      }),
    ]);

    await act(async () => {
      await result.current.submit("failed Project transport");
    });
    expect(result.current.queuedSubmissions).toEqual([
      expect.objectContaining({
        text: "failed Project transport",
        source: "transport",
        deliveryState: "unknown",
        editable: false,
        deletable: false,
      }),
    ]);
  });

  it("sends remote queued head metadata when submitting it now", async () => {
    const firstStream = controlledSseResponse();
    let submitCount = 0;
    vi.stubGlobal(
      "fetch",
      routeFetch((url, _init) => {
        if (url.includes("/console/live")) return firstStream.response;
        if (url.includes("/console/submit")) {
          submitCount += 1;
          if (submitCount === 2) {
            return submitQueuedResponse("input-second", "second", [
              {
                input_id: "input-second",
                text: "second",
                device_delivery_id: "device-delivery-2",
              },
            ]);
          }
        }
        return undefined;
      }),
    );
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void>;
    act(() => {
      firstSubmit = result.current.submit("first");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.submit("second");
    });
    await waitFor(() =>
      expect(result.current.queuedSubmissions[0]?.deviceDeliveryId).toBe(
        "device-delivery-2",
      ),
    );
    const queuedId = result.current.queuedSubmissions[0].id;

    await act(async () => {
      await result.current.submitQueuedNow(queuedId);
    });

    const chatCalls = (fetch as ReturnType<typeof vi.fn>).mock.calls.filter(
      ([url]) =>
        String(url).includes("/console/submit") &&
        !String(url).includes("/console/live"),
    );
    expect(chatCalls).toHaveLength(2);
    expect(chatApiMock.sendNowQueuedInput).toHaveBeenCalledWith(
      "chat-1",
      "input-second",
      expect.objectContaining({
        authority_epoch: "0000000000000001000000000000000000000000",
      }),
      "agent-a",
    );
    expect(result.current.queuedSubmissions).toHaveLength(1);

    await act(async () => {
      firstStream.close();
      await firstSubmit;
    });
  });

  it("hands off the local stop marker when persisted stop history loads", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    chatApiMock.getChat
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
            id: "server-user",
            role: "user",
            turn_id: "turn-1",
            content: [{ type: "text", text: "Hello" }],
            metadata: { submission_id: "submission-user-uuid" },
          },
          {
            id: "server-assistant",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "partial" }],
          },
          {
            id: "server-turn-aborted",
            role: "system",
            turn_id: "turn-1",
            type: "turn_aborted",
            content:
              "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>",
          },
        ],
      });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let submitPromise: Promise<void>;
    await act(async () => {
      submitPromise = result.current.submit("Hello");
    });
    await act(async () => {
      stream.send({
        type: "user_message",
        message: {
          id: "server-user",
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "Hello" }],
          metadata: { submission_id: "submission-user-uuid" },
        },
      });
      stream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "server-assistant",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "partial" }],
          },
        ],
      });
    });
    await act(async () => {
      await result.current.stop();
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "interrupted",
      });
      stream.close();
      await submitPromise;
    });
    await act(async () => {
      result.current.reload();
    });

    await waitFor(() =>
      expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThanOrEqual(2),
    );
    await waitFor(() =>
      expect(result.current.history?.messages || []).toHaveLength(3),
    );
    const rendered = JSON.stringify(result.current.history?.messages || []);
    expect(rendered.match(/<turn_aborted>/g) || []).toHaveLength(1);
  });

  it("retains stopped thinking through marker-only history and the next turn", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal("fetch", routeFetchWithLiveStreams(stream.response));
    const user = {
      id: "user-a",
      role: "user",
      turn_id: "turn-a",
      content: [{ type: "text", text: "First" }],
    };
    const nextUser = {
      id: "user-b",
      role: "user",
      turn_id: "turn-b",
      content: [{ type: "text", text: "Second" }],
    };
    const marker = {
      id: "stop-a",
      role: "assistant",
      turn_id: "turn-a",
      type: "turn_aborted",
      content: [],
    };
    let messages = [user];
    chatApiMock.getChat.mockImplementation(async () => ({
      status: "idle",
      messages,
      model: configuredModel,
    }));
    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "device-remote",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => {
      stream.send({ type: "user_message", message: user });
      stream.send({
        type: "thinking_delta",
        message_id: "thinking-a",
        delta: "Old thought",
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "Old thought",
      ),
    );
    messages = [user, marker];
    await act(async () => {
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-a",
        status: "interrupted",
      });
    });
    await act(async () => {
      await result.current.reload();
    });
    expect(JSON.stringify(result.current.projection.items)).toContain(
      "Old thought",
    );
    messages = [user, marker, nextUser];
    await act(async () => {
      stream.send({ type: "user_message", message: nextUser });
      stream.send({
        type: "thinking_delta",
        message_id: "thinking-b",
        delta: "New thought",
      });
    });
    await act(async () => {
      await result.current.reload();
    });
    await waitFor(() => {
      const responses = result.current.projection.items.filter(
        (item) => item.kind === "assistant",
      );
      expect(
        responses
          .find((item) => item.turn_id === "turn-a")
          ?.output.map((item) => item.id),
      ).toContain("thinking-a");
      expect(
        responses
          .find((item) => item.turn_id === "turn-b")
          ?.output.map((item) => item.id),
      ).toEqual(["thinking-b"]);
    });
    unmount();
    stream.close();
  });

  it("does not retain a stopped thinking/tool-only turn after server history covers it", async () => {
    const firstStream = controlledSseResponse();
    const secondStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetchWithLiveStreams(firstStream.response, secondStream.response),
    );
    const coveredHistory = {
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "server-user-tool",
          role: "user",
          turn_id: "turn-tool",
          content: [{ type: "text", text: "Run tool" }],
          metadata: { submission_id: "submission-user-uuid" },
        },
        {
          id: "server-assistant-tool",
          role: "assistant",
          turn_id: "turn-tool",
          content: [
            { type: "thinking", thinking: "Need tool" },
            {
              type: "tool_use",
              id: "call-1",
              name: "bash",
              input: { command: "sleep 10" },
            },
          ],
        },
        {
          id: "server-turn-aborted-tool",
          role: "system",
          turn_id: "turn-tool",
          type: "turn_aborted",
          content:
            "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>",
        },
      ],
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        messages: [],
        model: configuredModel,
      })
      .mockResolvedValue(coveredHistory);

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void>;
    await act(async () => {
      firstSubmit = result.current.submit("Run tool");
    });
    await act(async () => {
      firstStream.send({
        type: "user_message",
        message: coveredHistory.messages[0],
      });
      firstStream.send({
        type: "thinking_delta",
        message_id: "server-assistant-tool",
        delta: "Need tool",
      });
      firstStream.send({
        type: "tool_call_delta",
        id: "call-1",
        index: 0,
        name: "bash",
        arguments_delta: '{"command":"sleep 10"}',
      });
      await result.current.stop();
      firstStream.send({
        type: "turn_terminal",
        turn_id: "turn-tool",
        status: "interrupted",
      });
      firstStream.close();
      await firstSubmit;
    });

    let secondSubmit: Promise<void>;
    await act(async () => {
      secondSubmit = result.current.submit("second request");
    });

    await act(async () => {
      result.current.reload();
    });
    await waitFor(() =>
      expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThanOrEqual(2),
    );
    await waitFor(() =>
      expect(
        (
          JSON.stringify(result.current.history?.messages || []).match(
            /Run tool/g,
          ) || []
        ).length,
      ).toBe(1),
    );

    const rendered = JSON.stringify(result.current.history?.messages || []);
    expect(rendered.match(/Run tool/g) || []).toHaveLength(1);
    expect(rendered.match(/<turn_aborted>/g) || []).toHaveLength(1);
    expect(rendered).not.toContain("second request");

    await act(async () => {
      secondStream.close();
      await secondSubmit;
    });
  });

  it("keeps an interrupted visible run when the next submit starts", async () => {
    const firstStream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? firstStream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let firstSubmit: Promise<void>;
    await act(async () => {
      firstSubmit = result.current.submit("first request");
    });
    await act(async () => {
      firstStream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-first",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "partial first response" }],
          },
        ],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages)).toContain(
        "partial first response",
      ),
    );

    await act(async () => {
      await result.current.stop();
    });
    let secondSubmit: Promise<void>;
    await act(async () => {
      firstStream.send({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "interrupted",
      });
      firstStream.send({
        type: "conversation_control",
        control: {
          revision: 2,
          placement: null,
          active_turn: null,
          input_queue: [],
        },
      });
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(false));
    expect(firstStream.cancel).not.toHaveBeenCalled();
    await act(async () => {
      secondSubmit = result.current.submit("second request");
    });

    const rendered = JSON.stringify(result.current.history?.messages || []);
    expect(rendered).toContain("first request");
    expect(rendered).toContain("partial first response");
    expect(rendered).toContain("<turn_aborted>");
    expect(
      result.current.projection.items.filter((item) => item.kind === "user"),
    ).toHaveLength(1);
    expect(
      result.current.projection.items.find(
        (item) =>
          item.kind === "assistant" &&
          JSON.stringify(item.output).includes("partial first response"),
      )?.status,
    ).toBe("interrupted");

    await act(async () => {
      firstStream.send({
        type: "user_message",
        message: {
          id: "user-second",
          role: "user",
          content: [{ type: "text", text: "second request" }],
        },
      });
      firstStream.close();
      await firstSubmit;
      await secondSubmit;
    });
  });

  it("stops before assistant output without creating a false empty assistant response", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let submitPromise: Promise<void>;
    act(() => {
      submitPromise = result.current.submit("Hello");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    await act(async () => {
      await result.current.stop();
    });

    expect(stream.cancel).not.toHaveBeenCalled();
    expect(
      JSON.stringify(result.current.history?.messages || []),
    ).not.toContain("<turn_aborted>");
    await act(async () => {
      stream.send({
        type: "user_message",
        message: {
          id: "user-1",
          turn_id: "turn-1",
          role: "user",
          content: [{ type: "text", text: "Hello" }],
        },
      });
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "interrupted",
      });
    });
    await waitFor(() =>
      expect(
        result.current.projection.items.find(
          (item) => item.kind === "assistant",
        )?.status,
      ).toBe("interrupted"),
    );
    const messages = result.current.history?.messages || [];
    expect(messages.filter((message) => message.role === "user")).toHaveLength(
      1,
    );
    expect(
      messages.filter((message) => message.role === "assistant"),
    ).toHaveLength(0);
    expect(
      messages.filter((message) => message.role === "system"),
    ).toHaveLength(1);
    expect(JSON.stringify(messages)).toContain("<turn_aborted>");
    expect(JSON.stringify(messages)).not.toContain(
      "The assistant returned no output.",
    );
    expect(
      result.current.projection.items.some((item) => item.kind === "user"),
    ).toBe(true);
    expect(
      result.current.projection.items.find((item) => item.kind === "assistant")
        ?.status,
    ).toBe("interrupted");

    await act(async () => {
      stream.close();
      await submitPromise;
    });
  });

  it("does not replay a stopped Turn that was subsequently rolled back", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() => expect(fetch).toHaveBeenCalled());
    await act(async () => {
      stream.send({
        type: "stream_replay",
        complete: true,
        events: [
          {
            type: "user_message",
            message: {
              id: "user-wrong",
              turn_id: "turn-wrong",
              role: "user",
              content: [{ type: "text", text: "Wrong input" }],
            },
          },
          { object: "response", status: "interrupted", output: [] },
          {
            type: "conversation_history_mutated",
            action: "rolled_back",
            before_turn_id: "turn-wrong",
          },
        ],
      });
      stream.close();
    });
    expect(result.current.projection.items).toEqual([]);
    expect(result.current.history?.messages).toEqual([]);
  });

  it.each(["local", "terminal", "next-turn"])(
    "does not restore a stopped user-only Turn after rollback and history reload (%s)",
    async (stopMode) => {
      const stream = controlledSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/console/live") ? stream.response : undefined,
        ),
      );
      const { result, unmount } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));

      await act(async () => {
        await result.current.submit("Wrong input");
      });
      await act(async () => {
        stream.send({
          type: "user_message",
          message: {
            id: "user-wrong",
            turn_id: "turn-wrong",
            role: "user",
            content: [{ type: "text", text: "Wrong input" }],
          },
        });
      });
      await waitFor(() =>
        expect(
          result.current.projection.items.some(
            (item) => item.kind === "user" && item.turn_id === "turn-wrong",
          ),
        ).toBe(true),
      );
      await act(async () => {
        if (stopMode !== "local") {
          stream.send({
            object: "response",
            status: "interrupted",
            output: [],
          });
          if (stopMode === "next-turn") {
            stream.send({
              type: "user_message",
              message: {
                id: "user-next",
                turn_id: "turn-next",
                role: "user",
                content: [{ type: "text", text: "Next input" }],
              },
            });
            stream.send({
              object: "response",
              status: "interrupted",
              output: [],
            });
          }
        } else {
          await result.current.stop();
        }
        stream.close();
      });
      expect(
        result.current.history?.messages.some(
          (message) => message.role === "assistant",
        ),
      ).toBe(false);

      await act(async () => {
        await result.current.rollback("turn-wrong");
      });
      await waitFor(() =>
        expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThanOrEqual(2),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));
      expect(result.current.history?.messages).toEqual([]);
      expect(result.current.projection.items).toEqual([]);

      unmount();
      const reopened = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(reopened.result.current.loading).toBe(false));
      expect(reopened.result.current.history?.messages).toEqual([]);
    },
  );

  it("does not fabricate resolved approvals on stop before server confirmation", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "running",
      messages: [],
      model: configuredModel,
      active_turn: {
        turn_id: "turn-1",
        authority_epoch: "0000000000000001000000000000000000000000",
        phase: "streaming",
        driver: "rua",
      },
    });
    consoleApiMock.getApprovalSnapshot.mockResolvedValueOnce({
      pending: [
        {
          id: "approval-stop",
          agent_id: "agent-a",
          chat_id: "chat-1",
          turn_id: "turn-1",
          tool_call_id: "call-1",
          tool_name: "Bash",
          status: "pending",
          trigger: {
            source: "approval_mode",
            message: "Bash requires approval",
          },
          tool_call: {
            display: "Bash(git status)",
            args: { command: "git status" },
          },
          created_at: "2026-06-23T00:00:00Z",
          reuse_options: [],
        },
      ],
      resolved: [],
    });
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() =>
      expect(result.current.approvalSnapshot.pending).toHaveLength(1),
    );

    await act(async () => {
      await result.current.stop();
    });

    expect(result.current.approvalSnapshot.pending).toHaveLength(1);
    expect(result.current.approvalSnapshot.resolved).toHaveLength(0);
  });

  it.each(["agent", "project"] as const)(
    "silently detaches and resumes an active %s Chat when the viewer becomes hidden",
    async (scope) => {
      const first = controlledSseResponse();
      const resumed = controlledSseResponse();
      const streams = [first, resumed];
      const liveSignals: AbortSignal[] = [];
      let liveCalls = 0;
      vi.stubGlobal(
        "fetch",
        routeFetch((url, init) => {
          if (!url.includes("/console/live")) return undefined;
          const stream = streams[liveCalls++];
          const signal = init?.signal;
          if (signal) {
            liveSignals.push(signal);
            signal.addEventListener(
              "abort",
              () => stream.error(signal.reason),
              {
                once: true,
              },
            );
          }
          return stream.response;
        }),
      );
      const target =
        scope === "project"
          ? { scope, projectId: "project-1", agentId: "agent-a" }
          : undefined;
      const { result, unmount } = renderHook(() =>
        useConversationViewRuntime({
          address: target
            ? {
                scope: "project",
                projectId: target.projectId,
                chatId: "chat-1",
              }
            : { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
          activeAcpRunner: null,
          acpHistoryPreview: null,
          activityWake: 0,
          projectExecutionAgentId: "agent-a",
        }),
      );

      try {
        await waitFor(() => expect(result.current.loading).toBe(false));
        await waitFor(() => expect(liveCalls).toBe(1));
        await act(async () => {
          await result.current.submit("Hello");
        });
        await waitFor(() => expect(isTurnActive(result)).toBe(true));
        await act(async () => {
          first.send({
            object: "response",
            status: "in_progress",
            output: [
              {
                id: "assistant-stream",
                role: "assistant",
                type: "message",
                content: [{ type: "text", text: "partial" }],
              },
            ],
          });
        });
        await waitFor(() =>
          expect(JSON.stringify(result.current.projection.items)).toContain(
            "partial",
          ),
        );

        await act(async () => {
          Object.defineProperty(document, "hidden", {
            configurable: true,
            value: true,
          });
          document.dispatchEvent(new Event("visibilitychange"));
        });

        expect(liveSignals[0].aborted).toBe(true);
        expect(liveSignals[0].reason).toBe("viewer hidden");
        expect(result.current.error).toBeNull();
        expect(
          result.current.projection.items.find(
            (item) => item.kind === "assistant",
          )?.status,
        ).not.toBe("failed");
        expect(JSON.stringify(result.current.projection.items)).toContain(
          "partial",
        );
        expect(chatApiMock.stopChat).not.toHaveBeenCalled();
        expect(projectChatsApiMock.stopChat).not.toHaveBeenCalled();

        await act(async () => {
          Object.defineProperty(document, "hidden", {
            configurable: true,
            value: false,
          });
          document.dispatchEvent(new Event("visibilitychange"));
        });
        await waitFor(() => expect(liveCalls).toBe(2));
        expect(liveSignals[1].aborted).toBe(false);
        expect(result.current.error).toBeNull();
        expect(JSON.stringify(result.current.projection.items)).toContain(
          "partial",
        );
      } finally {
        unmount();
      }
    },
  );

  it.each([false, true])(
    "does not turn an accepted failed Turn into a delivery error or poison the next completed Turn (detail: %s)",
    async (hasDetail) => {
      const stream = controlledSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/console/live") ? stream.response : undefined,
        ),
      );
      chatApiMock.getChat.mockResolvedValue({
        status: "idle",
        messages: [],
        model: configuredModel,
      });
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));
      await act(async () => {
        await result.current.submit("first");
      });
      act(() => {
        stream.send({
          type: "user_message",
          message: {
            id: "first-user",
            role: "user",
            content: [{ type: "text", text: "first" }],
            turn_id: "first-turn",
          },
        });
        stream.send({
          type: "turn_terminal",
          turn_id: "first-turn",
          status: "failed",
          ...(hasDetail ? { detail: "provider disconnected" } : {}),
        });
      });
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 30));
      });
      expect(result.current.error).toBe(
        hasDetail ? "provider disconnected" : null,
      );
      act(() => {
        stream.send({
          type: "user_message",
          message: {
            id: "second-user",
            role: "user",
            content: [{ type: "text", text: "second" }],
            turn_id: "second-turn",
          },
        });
        stream.send({ object: "response", status: "completed", output: [] });
        stream.send({
          type: "turn_terminal",
          turn_id: "second-turn",
          status: "completed",
        });
      });
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 30));
      });
      expect(result.current.error).toBeNull();
      await act(async () => {
        stream.close();
      });
    },
  );

  it("preserves partial output and marks provider stream errors as failed", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let submitPromise: Promise<void>;
    act(() => {
      submitPromise = result.current.submit("Hello");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    await act(async () => {
      stream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-stream",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "partial" }],
          },
        ],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "partial",
      ),
    );
    await act(async () => {
      stream.send({
        object: "response",
        status: "failed",
        error: { message: "provider disconnected" },
        output: [],
      });
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-stop",
        status: "failed",
        detail: "provider disconnected",
      });
      await submitPromise;
    });

    await waitFor(() => {
      const assistant = result.current.projection.items.find(
        (item) => item.kind === "assistant",
      );
      expect(assistant?.status).toBe("failed");
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "partial",
      );
      expect(result.current.error).toContain("provider disconnected");
    });
  });

  it("does not send a stop request when a streaming chat view unmounts", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      model: configuredModel,
      messages: [],
    });

    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => {
      void result.current.submit("keep running");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    unmount();

    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
  });

  it("detaches the viewer stream on unmount without stopping the backend run", async () => {
    const stream = controlledSseResponse();
    let requestSignal: AbortSignal | undefined;
    vi.stubGlobal(
      "fetch",
      routeFetch((url, init) => {
        if (!url.includes("/console/live")) return undefined;
        requestSignal = init?.signal;
        return stream.response;
      }),
    );
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      model: configuredModel,
      messages: [],
    });

    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => {
      void result.current.submit("keep running");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    unmount();

    expect(requestSignal?.aborted).toBe(true);
    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
  });

  it("drops old-Agent live output when the explicit address changes", async () => {
    const stream = controlledSseResponse();
    let requestSignal: AbortSignal | undefined;
    vi.stubGlobal(
      "fetch",
      routeFetch((url, init) => {
        if (!url.includes("/console/live")) return undefined;
        requestSignal = init?.signal;
        return stream.response;
      }),
    );
    chatApiMock.getChat.mockImplementation((_chatId: string, agentId: string) =>
      Promise.resolve({
        status: "idle",
        model: configuredModel,
        messages:
          agentId === "agent-b"
            ? [
                {
                  id: "agent-b-user",
                  role: "user",
                  content: [{ type: "text", text: "agent b history" }],
                },
              ]
            : [],
      }),
    );

    const { result, rerender } = renderHook(
      ({ agentId }) =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: agentId, chatId: "chat-1" },
          unmaterialized: false,
        }),
      { initialProps: { agentId: "agent-a" } },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => {
      void result.current.submit("agent a request");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    await act(async () => {
      stream.send({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "agent-a-assistant",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "agent a live output" }],
          },
        ],
      });
    });
    expect(JSON.stringify(result.current.history?.messages || [])).toContain(
      "agent a live output",
    );

    rerender({ agentId: "agent-b" });

    await waitFor(() =>
      expect(JSON.stringify(result.current.history?.messages || [])).toContain(
        "agent b history",
      ),
    );
    const rendered = JSON.stringify(result.current.history?.messages || []);
    expect(requestSignal?.aborted).toBe(true);
    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
    expect(rendered).not.toContain("agent a request");
    expect(rendered).not.toContain("agent a live output");
  });

  it("reconnects when loaded history is still running", async () => {
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "running",
        model: configuredModel,
        messages: [
          {
            id: "user-1",
            role: "user",
            content: [{ type: "text", text: "draw" }],
          },
        ],
      })
      .mockResolvedValue({
        status: "idle",
        model: configuredModel,
        messages: [
          {
            id: "user-1",
            role: "user",
            content: [{ type: "text", text: "draw" }],
          },
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "done" }],
          },
        ],
      });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        sseResponse([
          {
            type: "user_message",
            message: {
              id: "user-1",
              role: "user",
              content: [{ type: "text", text: "draw" }],
            },
          },
          {
            object: "response",
            status: "completed",
            output: [
              {
                id: "assistant-1",
                role: "assistant",
                type: "message",
                content: [{ type: "text", text: "done" }],
              },
            ],
          },
        ]),
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );

    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        "/api/agents/agent-a/chats/chat-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    const body = JSON.parse(
      (fetch as ReturnType<typeof vi.fn>).mock.calls[0][1].body,
    );
    expect(body).toMatchObject({
      chat_id: "chat-1",
      user_id: "default",
      channel: "console",
      stream: true,
    });
    expect(body.input).toBeUndefined();
    await waitFor(() =>
      expect(
        result.current.projection.items.some((item) => {
          if (item.kind !== "assistant") return false;
          return JSON.stringify(item).includes("done");
        }),
      ).toBe(true),
    );
  });

  it("reuses settled projection cards while live text changes", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "running",
      active_turn: {
        turn_id: "turn-2",
        user_message_id: "user-2",
        phase: "streaming",
        authority_device_id: "device-a",
        authority_epoch: "0000000000000001000000000000000000000000",
        driver: "rua",
        started_at: "2026-07-27T00:00:00Z",
      },
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          turn_id: "turn-1",
          role: "user",
          content: "Old question",
        },
        {
          id: "answer-1",
          turn_id: "turn-1",
          role: "assistant",
          content: "Settled answer",
        },
        { id: "user-2", turn_id: "turn-2", role: "user", content: "Continue" },
      ],
    });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() =>
      expect(result.current.projection.items).toHaveLength(4),
    );
    const before = result.current.projection;
    await act(async () => {
      stream.send({
        type: "text_delta",
        delta: "New live text",
        activity_generation: "generation-a",
        activity_sequence: 1,
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection)).toContain(
        "New live text",
      ),
    );
    const after = result.current.projection;
    expect(after.items[0]).toBe(before.items[0]);
    expect(after.items[1]).toBe(before.items[1]);
    expect(after.items[2]).toBe(before.items[2]);
    expect(after.items[3]).not.toBe(before.items[3]);
    expect(JSON.stringify(before)).not.toContain("New live text");
    unmount();
  });

  it("publishes completed replay with a captured user without losing unrelated queued input", async () => {
    chatApiMock.getChat.mockResolvedValue({
      control: {
        queue: {
          entries: [
            {
              input_id: "queued-next",
              submission_id: "submission-next",
              state: "queued",
              text: "next task",
              created_at: "2026-07-27T00:00:00Z",
              updated_at: "2026-07-27T00:00:00Z",
              editable: true,
              deletable: true,
            },
          ],
        },
      },
      status: "running",
      active_turn: {
        turn_id: "turn-1",
        user_message_id: "user-1",
        phase: "streaming",
        authority_device_id: "device-a",
        authority_epoch: "0000000000000001000000000000000000000000",
        driver: "rua",
        started_at: "2026-07-27T00:00:00Z",
      },
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          turn_id: "turn-1",
          role: "user",
          content: [{ type: "text", text: "draw" }],
        },
      ],
    });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        "/api/agents/agent-a/chats/chat-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );

    await act(async () => {
      stream.send({
        type: "stream_replay",
        complete: false,
        events: [
          {
            type: "user_message",
            activity_generation: "generation-a",
            activity_sequence: 1,
            message: {
              id: "user-1",
              turn_id: "turn-1",
              role: "user",
              content: [{ type: "text", text: "draw" }],
            },
          },
          {
            type: "text_delta",
            delta: "already generated",
            activity_generation: "generation-a",
            activity_sequence: 2,
          },
        ],
      });
    });
    expect(JSON.stringify(result.current.projection.items)).not.toContain(
      "already generated",
    );

    await act(async () => {
      stream.send({
        type: "stream_replay",
        complete: true,
        events: [
          {
            type: "text_delta",
            delta: " before refresh",
            activity_generation: "generation-a",
            activity_sequence: 3,
          },
        ],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "already generated before refresh",
      ),
    );

    await act(async () => {
      stream.send({
        type: "text_delta",
        delta: " and still streaming",
        activity_generation: "generation-a",
        activity_sequence: 4,
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "already generated before refresh and still streaming",
      ),
    );
    expect(chatApiMock.getChat).toHaveBeenCalledTimes(1);
    expect(result.current.queuedSubmissions.map((item) => item.id)).toEqual([
      "queued-next",
    ]);
  });

  it("atomically replaces a gapped tail with an authority activity checkpoint", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "running",
      active_turn: {
        turn_id: "turn-1",
        user_message_id: "user-1",
        phase: "streaming",
        authority_device_id: "device-a",
        authority_epoch: "0000000000000001000000000000000000000000",
        driver: "rua",
        started_at: "2026-07-27T00:00:00Z",
      },
      model: configuredModel,
      messages: [],
    });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(fetch).toHaveBeenCalled());
    await act(async () => {
      stream.send({
        type: "user_message",
        activity_generation: "generation-a",
        activity_sequence: 8,
        message: {
          id: "user-1",
          turn_id: "turn-1",
          role: "user",
          content: [{ type: "text", text: "question" }],
        },
      });
      stream.send({
        type: "text_delta",
        delta: "tail",
        activity_generation: "generation-a",
        activity_sequence: 9,
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain("tail"),
    );

    await act(async () => {
      stream.send({
        type: "history_refresh_required",
        reason: "live_activity_gap",
        requested_after_sequence: 0,
        first_available_sequence: 8,
      });
      stream.send({
        type: "stream_replay",
        complete: false,
        events: [
          {
            type: "user_message",
            activity_checkpoint: true,
            activity_generation: "generation-a",
            activity_sequence: 1,
            message: {
              id: "user-1",
              turn_id: "turn-1",
              role: "user",
              content: [{ type: "text", text: "question" }],
            },
          },
          {
            type: "text_delta",
            delta: "complete prefix ",
            activity_checkpoint: true,
            activity_generation: "generation-a",
            activity_sequence: 2,
          },
        ],
      });
    });
    expect(JSON.stringify(result.current.projection.items)).toContain("tail");
    expect(JSON.stringify(result.current.projection.items)).not.toContain(
      "complete prefix",
    );

    await act(async () => {
      stream.send({
        type: "stream_replay",
        complete: true,
        events: [
          {
            type: "text_delta",
            delta: "through checkpoint",
            activity_checkpoint: true,
            activity_generation: "generation-a",
            activity_sequence: 9,
          },
        ],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "complete prefix through checkpoint",
      ),
    );

    await act(async () => {
      stream.send({
        type: "text_delta",
        delta: " and live suffix",
        activity_generation: "generation-a",
        activity_sequence: 10,
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "complete prefix through checkpoint and live suffix",
      ),
    );
  });

  it("keeps the current Project live stream attached when the next-Turn Agent changes", async () => {
    projectChatsApiMock.getChat.mockResolvedValue({
      status: "running",
      active_turn: {
        turn_id: "turn-1",
        user_message_id: "user-1",
        phase: "streaming",
        authority_device_id: "device-a",
        authority_epoch: "0000000000000001000000000000000000000000",
        driver: "rua",
        started_at: "2026-07-27T00:00:00Z",
      },
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          turn_id: "turn-1",
          role: "user",
          content: [{ type: "text", text: "draw" }],
        },
      ],
    });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/projects/project-1/chats/chat-project-1/console/live")
          ? stream.response
          : undefined,
      ),
    );
    const target = {
      scope: "project" as const,
      projectId: "project-1",
    };
    const { result, rerender } = renderHook(
      ({ executionAgentId }) =>
        useConversationViewRuntime({
          address: target
            ? {
                scope: "project",
                projectId: target.projectId,
                chatId: "chat-project-1",
              }
            : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
          authorityPlacementDeviceId: "device-resource",
          currentDeviceId: "device-controller",
          activeAcpRunner: null,
          acpHistoryPreview: null,
          activityWake: 0,
          projectExecutionAgentId: executionAgentId,
        }),
      { initialProps: { executionAgentId: "agent-a" } },
    );

    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        "/api/projects/project-1/chats/chat-project-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    await act(async () => {
      stream.send({
        type: "stream_replay",
        complete: true,
        events: [
          {
            type: "user_message",
            message: {
              id: "user-1",
              turn_id: "turn-1",
              role: "user",
              content: [{ type: "text", text: "draw" }],
            },
          },
          { type: "text_delta", delta: "before agent refresh" },
        ],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "before agent refresh",
      ),
    );

    rerender({ executionAgentId: "agent-b" });
    await act(async () => {
      stream.send({ type: "text_delta", delta: " and after agent refresh" });
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "before agent refresh and after agent refresh",
      ),
    );
    expect(isTurnActive(result)).toBe(true);

    await act(async () => {
      stream.send({ type: "completed" });
      stream.close();
    });
    expect(isTurnActive(result)).toBe(true);
    await act(async () => {
      await result.current.submit("next turn");
    });
    const submitCall = vi
      .mocked(fetch)
      .mock.calls.find(([url]) => String(url).includes("/console/submit"));
    expect(JSON.parse(String(submitCall?.[1]?.body || "{}"))).toMatchObject({
      agent_id: "agent-b",
      input: [
        {
          role: "user",
          content: [{ type: "text", text: "next turn" }],
        },
      ],
    });
  });

  it("keeps a running Turn active when a detached task wait closes the live stream without a terminal event", async () => {
    chatApiMock.getChat.mockResolvedValue({
      status: "running",
      active_turn: {
        turn_id: "turn-1",
        user_message_id: "user-1",
        phase: "streaming",
        authority_device_id: "device-a",
        authority_epoch: "0000000000000001000000000000000000000000",
        driver: "rua",
        started_at: "2026-07-27T00:00:00Z",
      },
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          turn_id: "turn-1",
          role: "user",
          content: [{ type: "text", text: "wait for the task" }],
        },
      ],
    });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );

    await act(async () => {
      stream.send({
        type: "stream_replay",
        complete: true,
        events: [
          {
            type: "user_message",
            message: {
              id: "user-1",
              turn_id: "turn-1",
              role: "user",
              content: [{ type: "text", text: "wait for the task" }],
            },
          },
          { type: "text_delta", delta: "Waiting for task_await" },
        ],
      });
      stream.close();
    });

    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "Waiting for task_await",
      ),
    );
    expect(isTurnActive(result)).toBe(true);
    expect(
      [...result.current.projection.items]
        .reverse()
        .find((item) => item.kind === "assistant"),
    ).toMatchObject({ status: "generating" });
  });

  it.each([
    ["local", 0],
    ["live", 0],
    ["local", 1],
    ["live", 1],
    ["local", 2],
    ["live", 2],
  ] as const)(
    "accepts lower durable coverage after explicit rollback (%s, retained turns: %s)",
    async (source, retainedTurns) => {
      const retainedMessages = Array.from(
        { length: retainedTurns },
        (_, index) => [
          {
            id: `user-retained-${index}`,
            role: "user",
            turn_id: `turn-retained-${index}`,
            content: `Retained question ${index}`,
          },
          {
            id: `assistant-retained-${index}`,
            role: "assistant",
            turn_id: `turn-retained-${index}`,
            content: `Retained answer ${index}`,
          },
        ],
      ).flat();
      const stream = controlledSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/console/live") ? stream.response : undefined,
        ),
      );
      chatApiMock.getChat.mockResolvedValue({
        status: "idle",
        durable_coverage: {
          latest_message_sequence: retainedMessages.length + 2,
          manifest_hash: "before",
        },
        messages: [
          ...retainedMessages,
          {
            id: "user-rollback",
            role: "user",
            turn_id: "turn-rollback",
            content: "Question",
          },
          {
            id: "assistant-rollback",
            role: "assistant",
            turn_id: "turn-rollback",
            content: "Removed answer",
          },
        ],
      });
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() =>
        expect(result.current.history?.messages).toHaveLength(
          retainedMessages.length + 2,
        ),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));
      chatApiMock.getChat.mockResolvedValue({
        status: "idle",
        durable_coverage: {
          latest_message_sequence: retainedMessages.length,
          manifest_hash: "after",
        },
        messages: retainedMessages,
      });
      await act(async () => {
        if (source === "local") await result.current.rollback("turn-rollback");
        else
          stream.send({
            type: "conversation_history_mutated",
            action: "rolled_back",
            before_turn_id: "turn-rollback",
          });
      });
      if (source === "local") expect(chatApiMock.rollback).toHaveBeenCalled();
      await waitFor(() =>
        expect(result.current.authoritativeHistory?.messages).toEqual(
          retainedMessages,
        ),
      );
      act(() => result.current.reload());
      await waitFor(() =>
        expect(result.current.history?.durable_coverage?.manifest_hash).toBe(
          "after",
        ),
      );
      expect(result.current.history?.messages).toEqual(retainedMessages);
      expect(JSON.stringify(result.current.projection.items)).not.toContain(
        "Removed answer",
      );
    },
  );

  it.each(["local", "live"])(
    "accepts a changed manifest after rollback outside the latest history page (%s)",
    async (source) => {
      const stream = controlledSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/console/live") ? stream.response : undefined,
        ),
      );
      const page = (first: number, last: number, hash: string) => ({
        status: "idle",
        durable_coverage: { latest_message_sequence: 80, manifest_hash: hash },
        oldest_turn_ordinal: first,
        newest_turn_ordinal: last,
        has_more_before: first > 1,
        messages: Array.from({ length: last - first + 1 }, (_, index) => [
          {
            id: `u${first + index}`,
            turn_id: `t${first + index}`,
            role: "user",
            content: `Question ${first + index}`,
          },
          {
            id: `a${first + index}`,
            turn_id: `t${first + index}`,
            role: "assistant",
            content: `Answer ${first + index}`,
          },
        ]).flat(),
      });
      chatApiMock.getChat.mockResolvedValue(page(21, 40, "before"));
      const { result } = renderHook(() =>
        useConversationViewRuntime({
          address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        }),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));
      chatApiMock.getChat.mockResolvedValueOnce(page(1, 10, "before"));
      await act(async () => {
        await result.current.locateTurn("t5");
      });
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "Answer 40",
      );
      chatApiMock.getChat.mockResolvedValue(page(1, 4, "after"));
      await act(async () => {
        if (source === "local") await result.current.rollback("t5");
        else
          stream.send({
            type: "conversation_history_mutated",
            action: "rolled_back",
            before_turn_id: "t5",
          });
      });
      await waitFor(() =>
        expect(result.current.authoritativeHistory?.messages).toHaveLength(8),
      );
      expect(result.current.history?.durable_coverage?.manifest_hash).toBe(
        "after",
      );
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "Answer 4",
      );
      expect(JSON.stringify(result.current.projection.items)).not.toContain(
        "Answer 40",
      );
      expect(result.current.history?.messages).toHaveLength(8);
      stream.close();
    },
  );

  it("discards history reads started before rollback", async () => {
    const stale = deferred<unknown>();
    const before = {
      status: "idle",
      durable_coverage: { latest_message_sequence: 2, manifest_hash: "before" },
      messages: [
        {
          id: "user-rollback",
          role: "user",
          turn_id: "turn-rollback",
          content: "Question",
        },
        {
          id: "assistant-rollback",
          role: "assistant",
          turn_id: "turn-rollback",
          content: "Removed answer",
        },
      ],
    };
    chatApiMock.getChat.mockResolvedValue(before);
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    chatApiMock.getChat.mockImplementationOnce(() => stale.promise);
    act(() => result.current.reload());
    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalledTimes(2));
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      durable_coverage: { latest_message_sequence: 0, manifest_hash: "after" },
      messages: [],
    });
    await act(async () => {
      await result.current.rollback("turn-rollback");
    });
    await waitFor(() => expect(result.current.history?.messages).toEqual([]));
    await act(async () => {
      stale.resolve(before);
    });
    expect(result.current.history?.messages).toEqual([]);
    expect(result.current.history?.durable_coverage?.manifest_hash).toBe(
      "after",
    );
  });

  it("delegates variant, rollback, and branch actions", async () => {
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.selectVariant("turn-1", "variant-2");
    });
    expect(chatApiMock.selectVariant).toHaveBeenCalledWith(
      "chat-1",
      "turn-1",
      "variant-2",
      "agent-a",
    );

    await act(async () => {
      await result.current.rollback("turn-2");
    });
    expect(chatApiMock.rollback).toHaveBeenCalledWith(
      "chat-1",
      { before_turn_id: "turn-2" },
      "agent-a",
    );

    await expect(result.current.branch("turn-2")).resolves.toBe("branch-chat");
    expect(chatApiMock.branch).toHaveBeenCalledWith(
      "chat-1",
      { before_turn_id: "turn-2" },
      "agent-a",
    );
  });

  it("delegates Project variant, rollback, and branch actions to Project scope", async () => {
    projectChatsApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
    });
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.selectVariant("turn-1", "variant-2");
      await result.current.rollback("turn-2");
    });
    await expect(result.current.branch("turn-2")).resolves.toBe(
      "branch-project-chat",
    );
    expect(projectChatsApiMock.selectVariant).toHaveBeenCalledWith(
      "project-1",
      "chat-project-1",
      "turn-1",
      "variant-2",
    );
    expect(projectChatsApiMock.rollback).toHaveBeenCalledWith(
      "project-1",
      "chat-project-1",
      { before_turn_id: "turn-2" },
    );
    expect(projectChatsApiMock.branch).toHaveBeenCalledWith(
      "project-1",
      "chat-project-1",
      { before_turn_id: "turn-2" },
    );
    expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(2);
  });

  it("submits Project regeneration through the Project Chat route", async () => {
    const stream = controlledSseResponse();
    projectChatsApiMock.getChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "user-1",
          role: "user",
          turn_id: "turn-1",
          content: "Question",
        },
        {
          id: "assistant-1",
          role: "assistant",
          turn_id: "turn-1",
          selected_variant_id: "variant-1",
          content: "Answer",
        },
      ],
    });
    vi.stubGlobal(
      "fetch",
      routeFetch((url) => {
        if (url.includes("/console/live")) return stream.response;
        if (url.includes("/console/submit")) return submitAcceptedResponse();
        return undefined;
      }),
    );
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let regeneratePromise: Promise<void> | undefined;
    act(() => {
      regeneratePromise = result.current.regenerate("turn-1", "Question");
    });
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        "/api/projects/project-1/chats/chat-project-1/console/submit",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    const submitCall = vi
      .mocked(fetch)
      .mock.calls.find(([url]) => String(url).includes("/console/submit"));
    expect(JSON.parse(String(submitCall?.[1]?.body))).toMatchObject({
      agent_id: "agent-a",
      regenerate_turn_id: "turn-1",
    });

    stream.send({
      type: "turn_terminal",
      turn_id: "turn-1",
      status: "completed",
    });
    stream.close();
    await act(async () => {
      await regeneratePromise;
    });
  });

  it("applies Project variant and rollback mutations from Live without history Sync", async () => {
    const stream = controlledSseResponse();
    projectChatsApiMock.getChat.mockResolvedValue(
      regenerationHistory(
        [
          {
            variantId: "variant-1",
            messageId: "assistant-1",
            text: "first answer",
          },
          {
            variantId: "variant-2",
            messageId: "assistant-2",
            text: "selected remotely",
          },
        ],
        "variant-1",
        true,
      ),
    );
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/projects/project-1/chats/chat-project-1/console/live")
          ? stream.response
          : undefined,
      ),
    );
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(1);

    await act(async () => {
      stream.send({
        type: "conversation_history_mutated",
        action: "variant_selected",
        turn_id: "turn-1",
        selected_variant_id: "variant-2",
      });
    });
    await waitFor(() => {
      const assistant = result.current.projection.items.find(
        (item) => item.kind === "assistant" && item.turn_id === "turn-1",
      );
      expect(assistant?.kind).toBe("assistant");
      if (assistant?.kind !== "assistant") return;
      expect(assistant.selected_variant_id).toBe("variant-2");
      expect(JSON.stringify(assistant.output)).toContain("selected remotely");
    });

    projectChatsApiMock.getChat.mockResolvedValue({
      ...result.current.authoritativeHistory,
      messages: result.current.authoritativeHistory!.messages.filter(
        (message) => message.turn_id !== "turn-2",
      ),
    });
    await act(async () => {
      stream.send({
        type: "conversation_history_mutated",
        action: "rolled_back",
        before_turn_id: "turn-2",
        selected_turn_id: "turn-1",
        selected_variant_id: "variant-2",
      });
    });
    await waitFor(() => {
      const rendered = JSON.stringify(result.current.projection.items);
      expect(rendered).toContain("selected remotely");
      expect(rendered).not.toContain("later answer");
      expect(rendered).not.toContain("later");
    });
    expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(2);

    stream.close();
  });

  it("preserves split selected output as a fallback variant during regeneration", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    chatApiMock.getChat.mockResolvedValueOnce({
      status: "idle",
      model: configuredModel,
      messages: [
        { id: "user-1", role: "user", turn_id: "turn-1", content: "inspect" },
        {
          id: "assistant-thinking",
          role: "assistant",
          type: "reasoning",
          turn_id: "turn-1",
          selected_variant_id: "variant-1",
          content: [{ type: "text", text: "old thinking" }],
        },
        {
          id: "assistant-final",
          role: "assistant",
          type: "message",
          turn_id: "turn-1",
          selected_variant_id: "variant-1",
          content: [{ type: "text", text: "old final" }],
        },
      ],
    });

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    let regeneratePromise: Promise<void> | undefined;
    act(() => {
      regeneratePromise = result.current.regenerate("turn-1", "inspect");
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));
    await act(async () => {
      stream.send({ type: "text_delta", delta: "new answer" });
    });

    await waitFor(() => {
      const assistant = result.current.projection.items.find(
        (item) => item.kind === "assistant",
      );
      expect(assistant?.kind).toBe("assistant");
      if (assistant?.kind !== "assistant") return;
      expect(assistant.variants).toHaveLength(2);
      const fallback = JSON.stringify(assistant.variants[0].output);
      expect(fallback).toContain("old thinking");
      expect(fallback).toContain("old final");
    });

    stream.send({ object: "response", status: "completed", output: [] });
    stream.close();
    await act(async () => {
      await regeneratePromise;
    });
  });

  it("converges a completed regeneration while the live viewer stays open", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    let officialRegenerationAvailable = false;
    chatApiMock.getChat.mockImplementation(async () =>
      officialRegenerationAvailable
        ? {
            ...regenerationHistory(
              [
                {
                  variantId: "variant-1",
                  messageId: "assistant-1",
                  text: "old answer",
                },
                {
                  variantId: "variant-2",
                  messageId: "assistant-2",
                  text: "new answer",
                },
              ],
              "variant-2",
            ),
            status: "idle",
            active_turn: null,
            control: {
              revision: 2,
              placement: null,
              active_turn: null,
              input_queue: [],
            },
          }
        : regenerationHistory(
            [
              {
                variantId: "variant-1",
                messageId: "assistant-1",
                text: "old answer",
              },
            ],
            "variant-1",
          ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.regenerate("turn-1", "inspect");
    });
    await act(async () => {
      stream.send({ type: "text_delta", delta: "new answer" });
    });
    await waitFor(() => expect(isTurnActive(result)).toBe(true));

    officialRegenerationAvailable = true;
    await act(async () => {
      stream.send({ object: "response", status: "completed", output: [] });
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "completed",
      });
      stream.send({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        agent_id: "agent-a",
        chat_id: "chat-1",
        authority_device_id: "device-local",
      });
    });

    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalledTimes(3));
    await waitFor(() => {
      const assistant = result.current.projection.items.find(
        (item) => item.kind === "assistant",
      );
      expect(assistant?.kind).toBe("assistant");
      if (assistant?.kind !== "assistant") return;
      expect(assistant.status).toBe("completed");
      expect(assistant.selected_variant_id).toBe("variant-2");
      expect(assistant.variants.map((variant) => variant.variant_id)).toEqual([
        "variant-1",
        "variant-2",
      ]);
      expect(isTurnActive(result)).toBe(false);
    });

    await act(async () => {
      stream.send({
        object: "response",
        status: "completed",
        output: [
          {
            id: "late-regeneration-output",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "late duplicate answer" }],
          },
        ],
      });
    });
    await waitFor(() => {
      const rendered = JSON.stringify(result.current.projection.items);
      expect(rendered).toContain("new answer");
      expect(rendered).not.toContain("late duplicate answer");
    });

    stream.close();
  });

  it("drops a stale regeneration overlay after rollback reloads stable history", async () => {
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );
    let rolledBack = false;
    chatApiMock.getChat.mockImplementation(async () =>
      rolledBack
        ? regenerationHistory(
            [
              {
                variantId: "variant-1",
                messageId: "assistant-1",
                text: "old answer",
              },
            ],
            "variant-1",
          )
        : regenerationHistory(
            [
              {
                variantId: "variant-1",
                messageId: "assistant-1",
                text: "old answer",
              },
              {
                variantId: "variant-2",
                messageId: "assistant-2",
                text: "selected answer",
              },
            ],
            "variant-2",
            true,
          ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.regenerate("turn-1", "inspect");
      stream.send({ type: "text_delta", delta: "stale pending answer" });
      stream.send({ object: "response", status: "completed", output: [] });
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "completed",
      });
      stream.send({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        agent_id: "agent-a",
        chat_id: "chat-1",
        authority_device_id: "device-local",
      });
    });
    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalledTimes(3));

    rolledBack = true;
    await act(async () => {
      await result.current.rollback("turn-2");
    });
    await waitFor(() => expect(chatApiMock.getChat).toHaveBeenCalledTimes(4));
    await waitFor(() => {
      const assistantItems = result.current.projection.items.filter(
        (item) => item.kind === "assistant",
      );
      expect(assistantItems).toHaveLength(1);
      const assistant = assistantItems[0];
      if (assistant.kind !== "assistant") return;
      expect(assistant.status).toBe("completed");
      expect(assistant.selected_variant_id).toBe("variant-1");
      expect(assistant.variants.map((variant) => variant.variant_id)).toEqual([
        "variant-1",
      ]);
      expect(JSON.stringify(assistant.output)).toContain("old answer");
      expect(JSON.stringify(result.current.projection.items)).not.toContain(
        "stale pending answer",
      );
      expect(isTurnActive(result)).toBe(false);
    });

    stream.close();
  });

  it("submits approval decisions to the selected agent approval control route", async () => {
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.approveTool("approval-1", true);
    });

    expect(consoleApiMock.decideApproval).toHaveBeenCalledWith(
      "agent-a",
      "approval-1",
      {
        decision: "run_once",
        decided_by: "webui",
      },
    );
    expect(fetch).not.toHaveBeenCalledWith(
      "/api/agents/agent-a/chats/chat-1/console/submit",
      expect.anything(),
    );
  });

  it("sends backend-provided reuse matchers for chat approval decisions", async () => {
    const matcher = {
      type: "bash_command_family" as const,
      program: "git",
      subcommand: "status",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.approveTool("approval-1", true, {
        decision: "allow_chat",
        matcher,
      });
    });

    expect(consoleApiMock.decideApproval).toHaveBeenCalledWith(
      "agent-a",
      "approval-1",
      {
        decision: "allow_chat",
        decided_by: "webui",
        matcher,
      },
    );
  });

  it("routes Project Stop and queue controls without touching Agent control routes", async () => {
    const control = {
      revision: 1,
      authority_device_id: "device-resource",
      placement: {
        authority_device_id: "device-resource",
        authority_epoch: "epoch-project-1",
        switchable: false,
      },
      active_turn: {
        turn_id: "turn-project-queue",
        authority_epoch: "epoch-project-1",
        phase: "streaming",
        driver: "rua",
      },
      queue: {
        paused: false,
        entries: [
          {
            input_id: "input-project-1",
            submission_id: "submission-project-1",
            state: "queued",
            text: "queued project input",
            created_at: "2026-08-01T00:00:00Z",
            updated_at: "2026-08-01T00:00:00Z",
            editable: true,
            deletable: true,
          },
        ],
      },
      acp: { active: false },
      allowed_controls: {
        stop: true,
        queue: true,
        interject: true,
        edit_queued_input: true,
        delete_queued_input: true,
        switch_authority: false,
        enter_acp: true,
        exit_acp: false,
      },
    };
    projectChatsApiMock.getChat.mockResolvedValue({
      status: "idle",
      messages: [],
      model: configuredModel,
      control,
    });
    projectChatsApiMock.sendNowQueuedInput.mockResolvedValue({
      input_id: "input-project-1",
      applied: true,
      control,
    });
    projectChatsApiMock.editQueuedInput.mockResolvedValue({
      entry: control.queue.entries[0],
      control,
    });
    projectChatsApiMock.deleteQueuedInput.mockResolvedValue({
      deleted_input_id: "input-project-1",
      control: { ...control, queue: { paused: false, entries: [] } },
    });
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() =>
      expect(result.current.queuedSubmissions).toHaveLength(1),
    );

    await act(async () => {
      await result.current.submitQueuedNow("input-project-1");
      await result.current.editQueuedSubmission(
        "input-project-1",
        "edited project input",
      );
      result.current.cancelQueuedSubmission("input-project-1");
    });
    await waitFor(() =>
      expect(result.current.history?.active_turn?.turn_id).toBe(
        "turn-project-queue",
      ),
    );
    await act(async () => {
      await result.current.stop();
    });

    expect(projectChatsApiMock.sendNowQueuedInput).toHaveBeenCalledWith(
      "project-1",
      "chat-project-1",
      "input-project-1",
      expect.objectContaining({ authority_epoch: "epoch-project-1" }),
    );
    expect(projectChatsApiMock.editQueuedInput).toHaveBeenCalledWith(
      "project-1",
      "chat-project-1",
      "input-project-1",
      "edited project input",
    );
    expect(projectChatsApiMock.deleteQueuedInput).toHaveBeenCalledWith(
      "project-1",
      "chat-project-1",
      "input-project-1",
    );
    expect(projectChatsApiMock.stopChat).toHaveBeenCalledWith(
      "project-1",
      "chat-project-1",
      { turn_id: "turn-project-queue", authority_epoch: "epoch-project-1" },
    );
    expect(chatApiMock.sendNowQueuedInput).not.toHaveBeenCalled();
    expect(chatApiMock.editQueuedInput).not.toHaveBeenCalled();
    expect(chatApiMock.deleteQueuedInput).not.toHaveBeenCalled();
    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
  });

  it("uses the existing optimistic Chat history when Project approval fails", async () => {
    projectChatsApiMock.getChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "approval-message",
          role: "assistant",
          content: [
            {
              type: "tool_approval_request",
              data: {
                approval_request_id: "approval-project-1",
                status: "pending",
              },
            },
          ],
        },
      ],
    });
    projectChatsApiMock.getApprovalSnapshot.mockResolvedValue({
      pending: [
        {
          id: "approval-project-1",
          agent_id: "agent-a",
          chat_id: "chat-project-1",
          turn_id: "turn-1",
          tool_call_id: "tool-1",
          tool_name: "shell",
          status: "pending",
          tool_call: { display: "git status", args: {} },
          created_at: "2026-08-01T00:00:00Z",
          reuse_options: [],
        },
      ],
      resolved: [],
    });
    projectChatsApiMock.decideApproval.mockRejectedValue(
      new Error("resource Device offline"),
    );
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() =>
      expect(result.current.approvalSnapshot.pending).toHaveLength(1),
    );

    await act(async () => {
      await result.current.approveTool("approval-project-1", true);
    });

    expect(projectChatsApiMock.decideApproval).toHaveBeenCalledWith(
      "project-1",
      "chat-project-1",
      "approval-project-1",
      { decision: "run_once", decided_by: "webui" },
    );
    expect(result.current.approvalSnapshot.pending).toHaveLength(1);
    expect(JSON.stringify(result.current.history)).toContain(
      '"status":"approved"',
    );
    expect(result.current.error).toBe("resource Device offline");
  });

  it("recovers Project Live after visibility and unexpected EOF", async () => {
    const first = controlledSseResponse();
    const second = controlledSseResponse();
    const third = controlledSseResponse();
    const liveResponses = [first.response, second.response, third.response];
    const liveSignals: AbortSignal[] = [];
    let liveIndex = 0;
    const fetchMock = routeFetch((url, init) => {
      if (url !== "/api/projects/project-1/chats/chat-project-1/console/live") {
        return undefined;
      }
      if (init?.signal) liveSignals.push(init.signal);
      const response = liveResponses[liveIndex];
      liveIndex += 1;
      return response;
    });
    vi.stubGlobal("fetch", fetchMock);
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );

    try {
      await waitFor(() => expect(liveIndex).toBe(1));
      expect(liveSignals[0]?.aborted).toBe(false);

      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: true,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      expect(liveSignals[0]?.aborted).toBe(true);
      await act(async () => {
        first.close();
        await Promise.resolve();
      });

      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: false,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      await waitFor(() => expect(liveIndex).toBe(2));

      await act(async () => {
        second.close();
        await Promise.resolve();
      });
      const historyReadsBeforeWake =
        projectChatsApiMock.getChat.mock.calls.length;
      await waitFor(() => expect(liveIndex).toBe(3), { timeout: 2_500 });
      expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(
        historyReadsBeforeWake,
      );

      const projectLiveCalls = fetchMock.mock.calls.filter(
        ([url]) =>
          String(url) ===
          "/api/projects/project-1/chats/chat-project-1/console/live",
      );
      expect(projectLiveCalls).toHaveLength(3);
      expect(
        fetchMock.mock.calls.some(([url]) =>
          String(url).includes("/sync/cycle/target"),
        ),
      ).toBe(false);
    } finally {
      unmount();
      third.close();
    }
  });

  it("preserves active-turn messages when Project Live resumes from a cursor suffix", async () => {
    const first = controlledSseResponse();
    const second = controlledSseResponse();
    const liveResponses = [first.response, second.response];
    const liveRequestBodies: Array<Record<string, unknown>> = [];
    let liveIndex = 0;
    const fetchMock = routeFetch((url, init) => {
      if (url !== "/api/projects/project-1/chats/chat-project-1/console/live") {
        return undefined;
      }
      liveRequestBodies.push(JSON.parse(String(init?.body)));
      const response = liveResponses[liveIndex];
      liveIndex += 1;
      return response;
    });
    vi.stubGlobal("fetch", fetchMock);
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    const activity = (sequence: number) => ({
      activity_generation: "project-generation-1",
      activity_sequence: sequence,
    });

    try {
      await waitFor(() => expect(liveIndex).toBe(1));
      await act(async () => {
        first.send({
          ...activity(1),
          type: "user_message",
          message: {
            id: "project-user-1",
            turn_id: "project-turn-1",
            role: "user",
            content: [{ type: "text", text: "project question" }],
          },
        });
        first.send({
          ...activity(2),
          type: "text_delta",
          message_id: "project-assistant-1",
          delta: "first live message",
        });
        first.send({
          ...activity(3),
          type: "text_delta",
          message_id: "project-assistant-2",
          delta: "second live message",
        });
      });
      await waitFor(() => {
        const rendered = JSON.stringify(result.current.projection.items);
        expect(rendered).toContain("first live message");
        expect(rendered).toContain("second live message");
      });

      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: true,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      await act(async () => {
        first.close();
        await Promise.resolve();
      });

      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: false,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      await waitFor(() => expect(liveIndex).toBe(2));
      expect(liveRequestBodies[1]).toMatchObject({
        activity_generation: "project-generation-1",
        next_activity_sequence: 4,
      });

      await act(async () => {
        second.send({
          type: "stream_replay",
          complete: true,
          events: [
            {
              ...activity(4),
              type: "text_delta",
              message_id: "project-assistant-3",
              delta: "third live message",
            },
          ],
        });
      });
      await waitFor(() => {
        const rendered = JSON.stringify(result.current.projection.items);
        expect(rendered).toContain("first live message");
        expect(rendered).toContain("second live message");
        expect(rendered).toContain("third live message");
      });
    } finally {
      unmount();
      second.close();
    }
  });

  it("hydrates an explicit Project replay gap once without clearing uncovered live output", async () => {
    const stream = controlledSseResponse();
    const liveUser = {
      id: "project-live-user",
      role: "user",
      turn_id: "project-live-turn",
      metadata: {
        device_delivery_id: "project-live-delivery",
      },
      content: [{ type: "text", text: "project prompt" }],
    };
    projectChatsApiMock.getChat
      .mockResolvedValueOnce({
        status: "running",
        active_turn: {
          turn_id: "project-live-turn",
          user_message_id: "project-live-user",
          phase: "streaming",
          authority_device_id: "device-remote",
          authority_epoch: "0000000000000001000000000000000000000000",
          driver: "rua",
          started_at: "2026-07-20T00:00:00Z",
        },
        control: {
          revision: 1,
          placement: null,
          active_turn: {
            turn_id: "project-live-turn",
            user_message_id: "project-live-user",
            phase: "streaming",
            authority_device_id: "device-remote",
            authority_epoch: "0000000000000001000000000000000000000000",
            driver: "rua",
            started_at: "2026-07-20T00:00:00Z",
          },
          input_queue: [],
        },
        model: configuredModel,
        messages: [],
      })
      .mockResolvedValueOnce({
        status: "idle",
        active_turn: null,
        control: {
          revision: 2,
          placement: null,
          active_turn: null,
          input_queue: [],
        },
        model: configuredModel,
        messages: [
          liveUser,
          {
            id: "project-stable-assistant",
            role: "assistant",
            type: "message",
            turn_id: "project-live-turn",
            content: [{ type: "text", text: "canonical project answer" }],
          },
        ],
      });
    const fetchMock = routeFetch((url) =>
      url.includes("/projects/project-1/") && url.endsWith("/console/live")
        ? stream.response
        : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { result, unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );

    try {
      await waitFor(() =>
        expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(1),
      );
      await act(async () => {
        stream.send({
          type: "stream_replay",
          complete: true,
          events: [
            {
              activity_generation: "project-generation",
              activity_sequence: 1,
              type: "user_message",
              message: liveUser,
            },
            {
              activity_generation: "project-generation",
              activity_sequence: 3,
              object: "response",
              status: "in_progress",
              output: [
                {
                  id: "project-stable-assistant",
                  role: "assistant",
                  type: "message",
                  turn_id: "project-live-turn",
                  content: [{ type: "text", text: "uncovered live output" }],
                },
              ],
            },
          ],
        });
      });

      await waitFor(() => {
        const rendered = JSON.stringify(result.current.projection.items);
        expect(rendered).toContain("uncovered live output");
        expect(isTurnActive(result)).toBe(true);
      });
      expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(1);
      expect(
        fetchMock.mock.calls.some(([url]) =>
          String(url).includes("/sync/cycle/target"),
        ),
      ).toBe(false);

      await act(async () => {
        stream.send({
          activity_generation: "project-generation",
          activity_sequence: 4,
          type: "history_watermark_reached",
          durable_sequence: 1,
          manifest_hash: "test-manifest-1",
          address: {
            scope: { kind: "project", project_id: "project-1" },
            chat_id: "chat-project-1",
          },
        });
        stream.send({
          activity_generation: "project-generation",
          activity_sequence: 5,
          type: "history_watermark_reached",
          durable_sequence: 1,
          manifest_hash: "test-manifest-1",
          address: {
            scope: { kind: "project", project_id: "project-1" },
            chat_id: "chat-project-1",
          },
        });
      });
      await waitFor(() =>
        expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(2),
      );
      await waitFor(() => {
        const rendered = JSON.stringify(result.current.projection.items);
        expect(rendered).toContain("canonical project answer");
        expect(rendered).not.toContain("uncovered live output");
        expect(isTurnActive(result)).toBe(false);
      });
    } finally {
      unmount();
      stream.close();
    }
  });

  it("ignores a late history watermark for a different Project Chat", async () => {
    const stream = controlledSseResponse();
    projectChatsApiMock.getChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [
        {
          id: "current-message",
          role: "assistant",
          content: [{ type: "text", text: "current durable history" }],
        },
      ],
    });
    const fetchMock = routeFetch((url) =>
      url.includes("/projects/project-1/") && url.endsWith("/console/live")
        ? stream.response
        : undefined,
    );
    vi.stubGlobal("fetch", fetchMock);
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { unmount } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );

    try {
      await waitFor(() =>
        expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(1),
      );
      await act(async () => {
        stream.send({
          activity_generation: "old-project-generation",
          activity_sequence: 99,
          type: "history_watermark_reached",
          durable_sequence: 7,
          manifest_hash: "old-project-manifest",
          authority_device_id: "old-project-device",
          address: {
            scope: { kind: "project", project_id: "project-2" },
            chat_id: "chat-project-old",
          },
        });
        await new Promise((resolve) => setTimeout(resolve, 25));
      });

      expect(projectChatsApiMock.getChat).toHaveBeenCalledTimes(1);
      expect(
        fetchMock.mock.calls.some(([url]) =>
          String(url).includes("/sync/cycle/target"),
        ),
      ).toBe(false);
    } finally {
      unmount();
      stream.close();
    }
  });

  it("converges Project history when turn completion follows the stable refresh", async () => {
    const stream = controlledSseResponse();
    let acceptedSubmissionId = "";
    const canonicalHistory = () => ({
      status: "idle" as const,
      model: configuredModel,
      messages: [
        {
          id: "stable-user-1",
          role: "user",
          turn_id: "turn-project-1",
          metadata: {
            submission_id: acceptedSubmissionId,
            input_id: "input-project-1",
            device_delivery_id: "device-delivery-project-1",
          },
          content: [{ type: "text", text: "Question" }],
        },
        {
          id: "stable-assistant-1",
          role: "assistant",
          type: "message",
          turn_id: "turn-project-1",
          content: [{ type: "text", text: "Answer" }],
        },
      ],
    });
    projectChatsApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [],
      })
      .mockImplementation(async () => canonicalHistory());
    vi.stubGlobal(
      "fetch",
      routeFetch((url, init) => {
        if (url.includes("/console/live")) return stream.response;
        if (url.includes("/console/submit")) {
          const body = JSON.parse(String(init?.body || "{}"));
          acceptedSubmissionId = body.submission_id;
          return submitAcceptedResponse({
            submission_id: acceptedSubmissionId,
            input_id: "input-project-1",
            device_delivery_id: "device-delivery-project-1",
            target_device_id: "device-resource",
            device_delivery_sequence: 1,
            control: {
              revision: 1,
              authority_device_id: "device-resource",
              placement: {
                authority_device_id: "device-resource",
                authority_epoch: "epoch-project-1",
                switchable: false,
              },
              active_turn: {
                turn_id: "turn-project-1",
                phase: "streaming",
                authority_device_id: "device-resource",
                authority_epoch: "epoch-project-1",
                driver: "rua",
                started_at: "2026-08-01T00:00:00Z",
                queued_count: 0,
              },
              queue: { entries: [], paused: false },
              acp: { active: false },
              allowed_controls: {
                stop: true,
                queue: true,
                interject: true,
                edit_queued_input: false,
                delete_queued_input: false,
                switch_authority: false,
                enter_acp: false,
                exit_acp: false,
              },
            },
          });
        }
        return undefined;
      }),
    );
    const target = {
      scope: "project" as const,
      projectId: "project-1",
      agentId: "agent-a",
    };
    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: target
          ? {
              scope: "project",
              projectId: target.projectId,
              chatId: "chat-project-1",
            }
          : { scope: "agent", agentId: "agent-a", chatId: "chat-project-1" },
        authorityPlacementDeviceId: "device-resource",
        currentDeviceId: "device-controller",
        activeAcpRunner: null,
        acpHistoryPreview: null,
        activityWake: 0,
        projectExecutionAgentId: "agent-a",
      }),
    );
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        "/api/projects/project-1/chats/chat-project-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    const liveCall = vi
      .mocked(fetch)
      .mock.calls.find(([url]) => String(url).includes("/console/live"));
    expect(JSON.parse(String(liveCall?.[1]?.body || "{}"))).not.toHaveProperty(
      "agent_id",
    );

    await act(async () => {
      await result.current.submit("Question");
    });
    await act(async () => {
      stream.send({
        object: "message",
        id: "stable-assistant-1",
        role: "assistant",
        type: "message",
        status: "in_progress",
        content: [{ type: "text", text: "Answer" }],
      });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "stable-assistant-1",
      ),
    );

    await act(async () => {
      stream.send({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        address: {
          scope: {
            kind: "project",
            project_id: "project-1",
          },
          chat_id: "chat-project-1",
        },
        authority_device_id: "device-resource",
      });
    });
    await waitFor(() =>
      expect(projectChatsApiMock.getChat.mock.calls.length).toBeGreaterThan(1),
    );
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "stable-assistant-1",
      ),
    );
    await waitFor(() => {
      expect(
        result.current.projection.items.filter((item) => item.kind === "user"),
      ).toHaveLength(1);
      expect(JSON.stringify(result.current.projection.items)).not.toContain(
        "remote-pending-device-delivery-project-1",
      );
    });

    await act(async () => {
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-project-1",
        status: "completed",
      });
    });

    await waitFor(() => {
      const assistantMessageIds = result.current.projection.items
        .filter((item) => item.kind === "assistant")
        .flatMap((item) => item.output.map((message) => message.id));
      expect(assistantMessageIds).toEqual(["stable-assistant-1"]);
    });

    stream.close();
  });

  it("reconnects open remote turns from any chat viewer device", async () => {
    const remoteUser = {
      id: "remote-user-1",
      role: "user",
      content: [{ type: "text", text: "Run on desktop" }],
      metadata: {
        device_delivery_id: "device-delivery-1",
        source_device_id: "phone",
        target_device_id: "desktop",
      },
    };
    chatApiMock.getChat
      .mockResolvedValueOnce({
        status: "idle",
        model: configuredModel,
        messages: [remoteUser],
        meta: {
          authority_placement: { device_id: "desktop" },
        },
      })
      .mockResolvedValue({
        status: "idle",
        model: configuredModel,
        messages: [
          remoteUser,
          {
            id: "assistant-remote-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "Live from desktop" }],
          },
        ],
        meta: {
          authority_placement: { device_id: "desktop" },
        },
      });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "desktop",
        currentDeviceId: "tablet",
      }),
    );

    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        "/api/agents/agent-a/chats/chat-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    await act(async () => {
      stream.send({ type: "text_delta", delta: "Live from desktop" });
    });
    await waitFor(() =>
      expect(JSON.stringify(result.current.projection.items)).toContain(
        "Live from desktop",
      ),
    );
    await act(async () => {
      stream.send({ object: "response", status: "completed", output: [] });
      stream.send({
        type: "turn_terminal",
        turn_id: "turn-remote-1",
        status: "completed",
      });
      stream.close();
    });
  });

  it("does not infer local streaming when another source device owns an open remote turn", async () => {
    const remoteUser = {
      id: "remote-user-1",
      role: "user",
      content: [{ type: "text", text: "Run on desktop" }],
      metadata: {
        device_delivery_id: "device-delivery-1",
        source_device_id: "phone",
        target_device_id: "desktop",
      },
    };
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [remoteUser],
      meta: {
        authority_placement: { device_id: "desktop" },
      },
    });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "desktop",
        currentDeviceId: "tablet",
      }),
    );

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.history?.status).toBe("idle");
    expect(isTurnActive(result)).toBe(false);

    await act(async () => {
      stream.close();
    });
  });

  it("does not infer authority activity from an open device delivery on the source device", async () => {
    const remoteUser = {
      id: "remote-user-1",
      role: "user",
      content: [{ type: "text", text: "Run on desktop" }],
      metadata: {
        device_delivery_id: "device-delivery-1",
        source_device_id: "phone",
        target_device_id: "desktop",
      },
    };
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      active_turn: null,
      model: configuredModel,
      messages: [remoteUser],
      meta: {
        authority_placement: { device_id: "desktop" },
      },
    });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "desktop",
        currentDeviceId: "phone",
      }),
    );

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.history?.status).toBe("idle");
    expect(isTurnActive(result)).toBe(false);

    await act(async () => {
      stream.close();
    });
  });

  it("keeps remote delivery pending without fabricating authority activity", async () => {
    localStorage.setItem(
      "rua:remote-pending-submissions:agent-a:chat-1",
      JSON.stringify([
        {
          device_delivery_id: "device-delivery-1",
          target_device_id: "desktop",
          device_delivery_sequence: 1,
          accepted_at: "2026-07-05T00:00:00Z",
          message: {
            id: "source-user-1",
            role: "user",
            content: [{ type: "text", text: "Run on desktop" }],
          },
        },
      ]),
    );
    const remoteUser = {
      id: "remote-user-1",
      role: "user",
      content: [{ type: "text", text: "Run on desktop" }],
      metadata: {
        device_delivery_id: "device-delivery-1",
        source_device_id: "phone",
        target_device_id: "desktop",
      },
    };
    chatApiMock.getChat.mockResolvedValue({
      status: "idle",
      model: configuredModel,
      messages: [remoteUser],
      meta: {
        authority_placement: { device_id: "desktop" },
      },
    });
    const stream = controlledSseResponse();
    vi.stubGlobal(
      "fetch",
      routeFetch((url) =>
        url.includes("/console/live") ? stream.response : undefined,
      ),
    );

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "desktop",
        currentDeviceId: "phone",
      }),
    );

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(isTurnActive(result)).toBe(false);

    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 1_100));
    });

    expect(
      (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
        String(url).includes("/sync/cycle/target"),
      ),
    ).toBe(true);
    expect(chatApiMock.getChat.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(result.current.history?.status).toBe("idle");
    expect(isTurnActive(result)).toBe(false);
    expect(
      result.current.projection.items.some(
        (item) => item.kind === "assistant" && item.status === "generating",
      ),
    ).toBe(false);
    expect(JSON.stringify(result.current.projection.items)).toContain(
      "Run on desktop",
    );

    await act(async () => {
      stream.close();
    });
  });

  it("keeps a live user boundary when the same text exists in an older completed turn", async () => {
    const runningHistory = {
      status: "running",
      model: configuredModel,
      messages: [
        {
          id: "old-user",
          role: "user",
          content: [{ type: "text", text: "Repeat this" }],
        },
        {
          id: "old-assistant",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Old answer" }],
        },
        {
          id: "previous-user",
          role: "user",
          content: [{ type: "text", text: "Different previous input" }],
        },
        {
          id: "previous-assistant",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Previous answer" }],
        },
      ],
      meta: {
        authority_placement: { device_id: "desktop" },
      },
    };
    chatApiMock.getChat
      .mockResolvedValueOnce(runningHistory)
      .mockResolvedValue({
        ...runningHistory,
        status: "idle",
        model: configuredModel,
      });
    const stream = controlledSseResponse();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(stream.response));

    const { result } = renderHook(() =>
      useConversationViewRuntime({
        address: { scope: "agent", agentId: "agent-a", chatId: "chat-1" },
        authorityPlacementDeviceId: "desktop",
        currentDeviceId: "tablet",
      }),
    );

    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        "/api/agents/agent-a/chats/chat-1/console/live",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    await act(async () => {
      stream.send({
        type: "user_message",
        message: {
          id: "live-user",
          role: "user",
          content: [{ type: "text", text: "Repeat this" }],
        },
      });
      stream.send({ type: "text_delta", delta: "Fresh answer" });
    });

    await waitFor(() => {
      const users = result.current.projection.items.filter(
        (item) => item.kind === "user",
      );
      const assistants = result.current.projection.items.filter(
        (item) => item.kind === "assistant",
      );
      expect(users).toHaveLength(3);
      expect(assistants).toHaveLength(3);
      expect(JSON.stringify(users[2])).toContain("Repeat this");
      expect(JSON.stringify(assistants[2])).toContain("Fresh answer");
      expect(JSON.stringify(assistants[1])).not.toContain("Fresh answer");
    });

    await act(async () => {
      stream.send({ object: "response", status: "completed", output: [] });
      stream.close();
    });
  });
  it.each(["execution-agent", "project-address", "scope"])(
    "keeps a pending Stop bound to its Chat Address when changing %s",
    async (change) => {
      const stream = controlledSseResponse();
      vi.stubGlobal(
        "fetch",
        routeFetch((url) =>
          url.includes("/projects/project-1/chats/chat-1/console/live")
            ? stream.response
            : undefined,
        ),
      );
      let resolveStop!: (value: { stopped: boolean; detail: string }) => void;
      projectChatsApiMock.stopChat.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveStop = resolve;
          }),
      );
      const address: import("../../conversationAddress").ConversationAddress = {
        scope: "project",
        projectId: "project-1",
        chatId: "chat-1",
      };
      const { result, rerender, unmount } = renderHook(
        ({ address, executionAgentId }) =>
          useConversationViewRuntime({
            address,
            projectExecutionAgentId: executionAgentId,
          }),
        { initialProps: { address, executionAgentId: "agent-a" } },
      );
      await waitFor(() => expect(result.current.loading).toBe(false));
      await act(async () => {
        await result.current.submit("Hello");
      });
      await waitFor(() => expect(isTurnActive(result)).toBe(true));
      const identity = result.current.identity;
      let pendingStop!: Promise<void>;
      act(() => {
        pendingStop = result.current.stop();
      });
      expect(projectChatsApiMock.stopChat).toHaveBeenCalledOnce();
      const nextAddress: import("../../conversationAddress").ConversationAddress =
        change === "project-address"
          ? { scope: "project", projectId: "project-2", chatId: "chat-1" }
          : change === "scope"
          ? { scope: "agent", agentId: "agent-a", chatId: "chat-1" }
          : { ...address };
      rerender({ address: nextAddress, executionAgentId: "agent-b" });
      await act(async () => {
        resolveStop({
          stopped: false,
          detail: "Stop response for original Chat",
        });
        await pendingStop;
      });
      if (change === "execution-agent") {
        expect(result.current.error).toBe("Stop response for original Chat");
        expect(result.current.identity).toBe(identity);
        expect(result.current.identity.mediaScope).toEqual({
          kind: "project",
          projectId: "project-1",
        });
        expect(stream.cancel).not.toHaveBeenCalled();
      } else {
        expect(result.current.error).not.toBe(
          "Stop response for original Chat",
        );
        expect(result.current.identity.viewKey).not.toBe(identity.viewKey);
      }
      unmount();
    },
  );
});

// Domain stream fixtures enter through the shared transport seam.
vi.mock("@/api/consoleTransport", () => ({
  consoleFetch: (path: string, options?: RequestInit) =>
    fetch(`/api${path}`, options),
}));
