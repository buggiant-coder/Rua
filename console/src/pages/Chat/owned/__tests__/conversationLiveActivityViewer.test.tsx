import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatHistory } from "@/api/types";
import {
  completedDeviceDeliveryIdsInHistory,
  openDeviceDeliveryId,
  deviceDeliveryIdsInHistory,
  triggerTargetedSyncCycle,
  useConversationLiveActivityViewerLifecycle,
  type ConversationLiveActivityViewerLifecycleOptions,
} from "../conversationLiveActivityViewer";

vi.mock("@/api/config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));

vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer test-token" }),
}));

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function baseOptions(
  overrides: Partial<ConversationLiveActivityViewerLifecycleOptions> = {},
): ConversationLiveActivityViewerLifecycleOptions {
  return {
    viewKey: "agent-a:chat-1",
    history: { status: "idle", messages: [] },
    currentDeviceId: "device-a",
    remotePendingSubmissions: [],
    isWatchingLiveActivity: vi.fn(() => false),
    startLiveActivityViewer: vi.fn(() => false),
    loadStableHistory: vi.fn(async () => ({ status: "idle", messages: [] })),
    applyStableHistory: vi.fn(),
    onRemotePendingStableHistory: vi.fn(),
    viewerVisible: true,
    ...overrides,
  };
}

describe("conversationLiveActivityViewer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({}))),
    );
  });

  it("extracts device delivery identity from stable history", () => {
    const history: ChatHistory = {
      status: "running",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "remote prompt",
          metadata: { device_delivery_id: "device-delivery-1" },
        },
      ],
    };

    expect(openDeviceDeliveryId(history)).toBe("device-delivery-1");
    expect(Array.from(deviceDeliveryIdsInHistory(history))).toEqual([
      "device-delivery-1",
    ]);
  });

  it("ignores null messages while reading delivery metadata", () => {
    const history: ChatHistory = {
      status: "idle",
      messages: [
        null,
        {
          id: "user-1",
          role: "user",
          content: "remote prompt",
          metadata: { device_delivery_id: "device-delivery-1" },
        },
      ] as unknown as ChatHistory["messages"],
    };

    expect(openDeviceDeliveryId(history)).toBe("device-delivery-1");
    expect(Array.from(deviceDeliveryIdsInHistory(history))).toEqual([
      "device-delivery-1",
    ]);
  });

  it("does not treat another source device's device delivery as locally open", () => {
    const history: ChatHistory = {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "remote prompt",
          metadata: {
            device_delivery_id: "device-delivery-1",
            source_device_id: "device-a",
            target_device_id: "device-a",
          },
        },
      ],
    };

    expect(openDeviceDeliveryId(history, "device-b")).toBeNull();
    expect(openDeviceDeliveryId(history, "device-a")).toBe("device-delivery-1");
  });

  it("keeps device delivery open until an assistant response arrives", () => {
    const partialHistory: ChatHistory = {
      status: "running",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "remote prompt",
          metadata: { device_delivery_id: "device-delivery-1" },
        },
        {
          id: "tool-1",
          role: "tool",
          content: "tool output before assistant",
        },
      ],
    };
    expect(openDeviceDeliveryId(partialHistory)).toBe("device-delivery-1");
    expect(
      Array.from(completedDeviceDeliveryIdsInHistory(partialHistory)),
    ).toEqual([]);

    const completedHistory: ChatHistory = {
      ...partialHistory,
      status: "idle",
      messages: [
        ...partialHistory.messages,
        {
          id: "assistant-1",
          role: "assistant",
          content: "done",
        },
      ],
    };
    expect(openDeviceDeliveryId(completedHistory)).toBeNull();
    expect(
      Array.from(completedDeviceDeliveryIdsInHistory(completedHistory)),
    ).toEqual(["device-delivery-1"]);
  });

  it("closes device delivery when a terminal marker arrives without assistant text", () => {
    const abortedHistory: ChatHistory = {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "remote prompt",
          metadata: { device_delivery_id: "device-delivery-1" },
        },
        {
          id: "marker-1",
          role: "system",
          type: "turn_aborted",
          content: "<turn_aborted>",
        },
      ],
    };

    expect(openDeviceDeliveryId(abortedHistory)).toBeNull();
    expect(
      Array.from(completedDeviceDeliveryIdsInHistory(abortedHistory)),
    ).toEqual(["device-delivery-1"]);
  });

  it("closes device delivery when a terminal runtime note arrives", () => {
    const failedHistory: ChatHistory = {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "remote prompt",
          metadata: { device_delivery_id: "device-delivery-1" },
        },
        {
          id: "note-1",
          role: "system",
          type: "runtime_note",
          content: "failed",
          runtime_note: {
            runtime_note_id: "note-1",
            kind: "conversation_runtime_terminal",
            created_at: "2026-07-20T00:00:00Z",
            payload: {
              agent_id: "agent-a",
              chat_id: "chat-1",
              turn_id: "turn-1",
              terminal_status: "failed",
              summary: "remote runtime failed",
            },
          },
        },
      ],
    };

    expect(openDeviceDeliveryId(failedHistory)).toBeNull();
    expect(
      Array.from(completedDeviceDeliveryIdsInHistory(failedHistory)),
    ).toEqual(["device-delivery-1"]);
  });

  it("starts passive live activity discovery immediately for an active viewer", async () => {
    vi.useFakeTimers();
    try {
      const startLiveActivityViewer = vi.fn(() => false);
      renderHook(() =>
        useConversationLiveActivityViewerLifecycle(
          baseOptions({ startLiveActivityViewer }),
        ),
      );

      expect(startLiveActivityViewer).toHaveBeenCalledWith({
        passive: true,
      });
      startLiveActivityViewer.mockClear();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(100);
      });
      expect(startLiveActivityViewer).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not attach while hidden and attaches when the viewer becomes visible", () => {
    const startLiveActivityViewer = vi.fn(() => false);
    const { rerender } = renderHook(
      ({ viewerVisible }: { viewerVisible: boolean }) =>
        useConversationLiveActivityViewerLifecycle(
          baseOptions({ startLiveActivityViewer, viewerVisible }),
        ),
      { initialProps: { viewerVisible: false } },
    );

    expect(startLiveActivityViewer).not.toHaveBeenCalled();
    rerender({ viewerVisible: true });
    expect(startLiveActivityViewer).toHaveBeenCalledWith({
      passive: true,
    });
  });

  it("reloads stable history once when a hidden viewer becomes visible", async () => {
    const stableHistory: ChatHistory = {
      status: "idle",
      messages: [
        {
          id: "assistant-stable",
          role: "assistant",
          content: "stable after background sync",
        },
      ],
    };
    const loadStableHistory = vi.fn(async () => stableHistory);
    const applyStableHistory = vi.fn();
    const onRemotePendingStableHistory = vi.fn();
    const { rerender } = renderHook(
      ({ viewerVisible }: { viewerVisible: boolean }) =>
        useConversationLiveActivityViewerLifecycle(
          baseOptions({
            viewerVisible,
            loadStableHistory,
            applyStableHistory,
            onRemotePendingStableHistory,
          }),
        ),
      { initialProps: { viewerVisible: false } },
    );

    expect(loadStableHistory).not.toHaveBeenCalled();
    rerender({ viewerVisible: true });

    await waitFor(() => expect(loadStableHistory).toHaveBeenCalledOnce());
    expect(loadStableHistory).toHaveBeenCalledWith();
    expect(applyStableHistory).toHaveBeenCalledWith(stableHistory);
    expect(onRemotePendingStableHistory).toHaveBeenCalledWith(stableHistory);

    rerender({ viewerVisible: true });
    expect(loadStableHistory).toHaveBeenCalledOnce();
  });

  it("wakes passive live activity discovery immediately when signalled", () => {
    const startLiveActivityViewer = vi.fn(() => false);
    const { rerender } = renderHook(
      ({ wakeSignal }: { wakeSignal: number }) =>
        useConversationLiveActivityViewerLifecycle(
          baseOptions({ startLiveActivityViewer, wakeSignal }),
        ),
      { initialProps: { wakeSignal: 0 } },
    );

    expect(startLiveActivityViewer).toHaveBeenCalledWith({
      passive: true,
    });
    startLiveActivityViewer.mockClear();
    rerender({ wakeSignal: 1 });

    expect(startLiveActivityViewer).toHaveBeenCalledWith({
      passive: true,
    });
  });

  it("runs targeted sync and applies stable history for pending device deliverys", async () => {
    const loadedHistory: ChatHistory = {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "done",
          metadata: { device_delivery_id: "device-delivery-1" },
        },
      ],
    };
    const loadStableHistory = vi.fn(async () => loadedHistory);
    const applyStableHistory = vi.fn();
    const onRemotePendingStableHistory = vi.fn();

    renderHook(() =>
      useConversationLiveActivityViewerLifecycle(
        baseOptions({
          remotePendingSubmissions: [
            {
              device_delivery_id: "device-delivery-1",
              target_device_id: "device-b",
            },
          ],
          loadStableHistory,
          applyStableHistory,
          onRemotePendingStableHistory,
        }),
      ),
    );

    await waitFor(() =>
      expect(applyStableHistory).toHaveBeenCalledWith(loadedHistory),
    );
    expect(onRemotePendingStableHistory).toHaveBeenCalledWith(loadedHistory);
    expect(fetch).toHaveBeenCalledWith(
      "/api/sync/cycle/target",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          Authorization: "Bearer test-token",
        }),
        body: JSON.stringify({ target_device_id: "device-b" }),
      }),
    );
    expect(loadStableHistory).toHaveBeenCalledWith();
  });

  it("starts a live reconnect once for an open remote turn", () => {
    const startLiveActivityViewer = vi.fn(() => true);
    const openHistory: ChatHistory = {
      status: "running",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "remote prompt",
          metadata: { device_delivery_id: "device-delivery-1" },
        },
      ],
    };
    const options = baseOptions({ startLiveActivityViewer });
    const { rerender } = renderHook(
      ({ history }: { history: ChatHistory }) =>
        useConversationLiveActivityViewerLifecycle({ ...options, history }),
      { initialProps: { history: { status: "idle", messages: [] } } },
    );

    startLiveActivityViewer.mockClear();
    rerender({ history: openHistory });
    expect(startLiveActivityViewer).toHaveBeenCalledWith();
    startLiveActivityViewer.mockClear();

    rerender({ history: openHistory });
    expect(startLiveActivityViewer).not.toHaveBeenCalled();

    rerender({ history: { status: "idle", messages: [] } });
    rerender({ history: openHistory });
    expect(startLiveActivityViewer).toHaveBeenCalledWith();
  });

  it("does not read stable history for an open device delivery", async () => {
    const loadStableHistory = vi.fn(async () => ({
      status: "running" as const,
      messages: [],
    }));
    const openHistory: ChatHistory = {
      status: "running",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "remote prompt",
          metadata: { device_delivery_id: "device-delivery-1" },
        },
      ],
    };

    renderHook(() =>
      useConversationLiveActivityViewerLifecycle(
        baseOptions({
          history: openHistory,
          loadStableHistory,
        }),
      ),
    );

    await act(async () => {
      await Promise.resolve();
    });
    expect(loadStableHistory).not.toHaveBeenCalled();
  });

  it("does not poll targeted sync for an idle remote authority", async () => {
    vi.useFakeTimers();
    try {
      const loadStableHistory = vi.fn(async () => ({
        status: "idle",
        messages: [],
      }));
      const applyStableHistory = vi.fn();

      renderHook(() =>
        useConversationLiveActivityViewerLifecycle(
          baseOptions({
            loadStableHistory,
            applyStableHistory,
          }),
        ),
      );

      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });

      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/sync/cycle/target"),
        ),
      ).toBe(false);
      expect(loadStableHistory).not.toHaveBeenCalled();
      expect(applyStableHistory).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not poll stable history for an idle local authority", async () => {
    vi.useFakeTimers();
    try {
      const loadStableHistory = vi.fn(async () => ({
        status: "idle",
        messages: [],
      }));
      renderHook(() =>
        useConversationLiveActivityViewerLifecycle(
          baseOptions({
            loadStableHistory,
          }),
        ),
      );

      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });

      expect(loadStableHistory).not.toHaveBeenCalled();
      expect(
        (fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/sync/cycle/target"),
        ),
      ).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("reports targeted sync failures with response details", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response("offline", {
            status: 503,
          }),
        ),
      ),
    );

    await expect(triggerTargetedSyncCycle("device-b")).rejects.toThrow(
      "Targeted sync failed: 503 offline",
    );
  });
});
