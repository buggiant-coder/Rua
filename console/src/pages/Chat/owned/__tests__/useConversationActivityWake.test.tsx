import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useConversationActivityWake } from "../useConversationActivityWake";

vi.mock("@/api/config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));

vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer test-token" }),
}));

function controlledJsonLinesResponse() {
  const encoder = new TextEncoder();
  let controller: ReadableStreamDefaultController<Uint8Array> | null = null;
  const body = new ReadableStream<Uint8Array>({
    start(nextController) {
      controller = nextController;
    },
    cancel() {
      controller = null;
    },
  });
  return {
    response: new Response(body, {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }),
    send(payload: unknown) {
      controller?.enqueue(encoder.encode(`${JSON.stringify(payload)}\n`));
    },
    close() {
      controller?.close();
    },
  };
}

describe("useConversationActivityWake", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
  });

  it("reconnects after EOF and signals recovery without waiting for another wake", async () => {
    vi.useFakeTimers();
    const first = controlledJsonLinesResponse();
    const second = controlledJsonLinesResponse();
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(first.response)
      .mockResolvedValueOnce(second.response);
    vi.stubGlobal("fetch", fetchMock);
    const { result, unmount } = renderHook(() =>
      useConversationActivityWake(
        { scope: "agent", agentId: "agent-1" },
        "chat-1",
      ),
    );
    try {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
        first.close();
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });
      expect(fetchMock).toHaveBeenCalledTimes(2);
      expect(result.current).toMatchObject({
        type: "activity_reconnected",
        sequence: 1,
        chat_id: "chat-1",
      });
      await act(async () => {
        second.send({
          type: "chat_live_wake",
          agent_id: "agent-1",
          chat_id: "chat-1",
        });
      });
      expect(result.current).toMatchObject({
        type: "chat_live_wake",
        sequence: 2,
        recoverySequence: 1,
      });
      unmount();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30000);
      });
      expect(fetchMock).toHaveBeenCalledTimes(2);
    } finally {
      unmount();
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it("cancels retries and rejects late responses when switching the complete Chat Address", async () => {
    vi.useFakeTimers();
    const old = controlledJsonLinesResponse();
    const current = controlledJsonLinesResponse();
    let resolveOld!: (response: Response) => void;
    const fetchMock = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<Response>((resolve) => {
            resolveOld = resolve;
          }),
      )
      .mockResolvedValueOnce(current.response);
    vi.stubGlobal("fetch", fetchMock);
    const { result, rerender, unmount } = renderHook(
      ({ agentId }) =>
        useConversationActivityWake({ scope: "agent", agentId }, "same-chat"),
      { initialProps: { agentId: "old-agent" } },
    );
    try {
      rerender({ agentId: "new-agent" });
      await act(async () => {
        old.send({
          type: "chat_live_wake",
          agent_id: "old-agent",
          chat_id: "same-chat",
        });
        resolveOld(old.response);
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(fetchMock.mock.calls[0][1].signal.aborted).toBe(true);
      expect(result.current).toBeNull();
      await act(async () => {
        current.send({
          type: "chat_live_wake",
          agent_id: "new-agent",
          chat_id: "same-chat",
        });
      });
      expect(result.current?.address.scope).toEqual({
        kind: "agent",
        agent_id: "new-agent",
      });
      await act(async () => {
        current.close();
        await vi.advanceTimersByTimeAsync(0);
      });
      unmount();
      await vi.advanceTimersByTimeAsync(30000);
      expect(fetchMock).toHaveBeenCalledTimes(2);
    } finally {
      unmount();
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it("increments only for the complete current Project Chat Address", async () => {
    const activity = controlledJsonLinesResponse();
    const fetchMock = vi.fn(async () => activity.response);
    vi.stubGlobal("fetch", fetchMock);
    const { result, unmount } = renderHook(() =>
      useConversationActivityWake(
        { scope: "project", projectId: "project-1" },
        "chat-1",
      ),
    );

    try {
      await waitFor(() =>
        expect(fetchMock).toHaveBeenCalledWith(
          "/api/projects/project-1/chats/chat-1/console/activity",
          expect.objectContaining({
            method: "GET",
            headers: { Authorization: "Bearer test-token" },
          }),
        ),
      );

      await act(async () => {
        activity.send({
          type: "chat_live_wake",
          address: {
            scope: { kind: "agent", agent_id: "project-1" },
            chat_id: "chat-1",
          },
        });
        activity.send({
          type: "chat_live_wake",
          address: {
            scope: { kind: "project", project_id: "project-1" },
            chat_id: "chat-2",
          },
        });
        await Promise.resolve();
      });
      expect(result.current).toBeNull();

      await act(async () => {
        activity.send({
          type: "chat_live_wake",
          version: 2,
          address: {
            scope: { kind: "project", project_id: "project-1" },
            chat_id: "chat-1",
          },
          stream_id: "delivery-1",
          phase: "started",
          authority_device_id: "device-resource",
          authority_epoch: "0000000000000001000000000000000000000000",
        });
      });
      await waitFor(() =>
        expect(result.current).toEqual({
          sequence: 1,
          type: "chat_live_wake",
          version: 2,
          address: {
            scope: { kind: "project", project_id: "project-1" },
            chat_id: "chat-1",
          },
          chat_id: "chat-1",
          stream_id: "delivery-1",
          phase: "started",
          authority_device_id: "device-resource",
          authority_epoch: "0000000000000001000000000000000000000000",
        }),
      );
    } finally {
      unmount();
      activity.close();
      vi.unstubAllGlobals();
    }
  });

  it("detaches while hidden and reconnects the same Project Chat when visible", async () => {
    const first = controlledJsonLinesResponse();
    const second = controlledJsonLinesResponse();
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(first.response)
      .mockResolvedValueOnce(second.response);
    vi.stubGlobal("fetch", fetchMock);
    const { result, unmount } = renderHook(() =>
      useConversationActivityWake(
        { scope: "project", projectId: "project-1" },
        "chat-1",
      ),
    );

    try {
      await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
      const firstSignal = fetchMock.mock.calls[0]?.[1]?.signal as
        | AbortSignal
        | undefined;
      expect(firstSignal?.aborted).toBe(false);

      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: true,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      expect(firstSignal?.aborted).toBe(true);

      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: false,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
      expect(fetchMock.mock.calls[1]?.[0]).toBe(
        "/api/projects/project-1/chats/chat-1/console/activity",
      );
      await waitFor(() =>
        expect(result.current).toMatchObject({
          type: "activity_reconnected",
          recoverySequence: 1,
        }),
      );
    } finally {
      unmount();
      first.close();
      second.close();
      vi.unstubAllGlobals();
    }
  });
});
