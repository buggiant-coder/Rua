import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConversationAddress } from "@/pages/Chat/conversationAddress";
import {
  ConversationActivityStore,
  type ConversationCompletion,
} from "./conversationActivity";

vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer fixture" }),
}));
vi.mock("@/api/config", () => ({ getApiUrl: (path: string) => `/api${path}` }));
vi.mock("./conversationNavigationEvents", () => ({
  invalidateConversationNavigation: vi.fn(),
}));

const agent: ConversationAddress = {
  scope: "agent",
  agentId: "alice",
  chatId: "same-chat",
};
const project: ConversationAddress = {
  scope: "project",
  projectId: "work",
  chatId: "same-chat",
};
const completion: ConversationCompletion = {
  turn_id: "turn-one",
  completed_at: "2026-10-23T08:00:00.123456Z",
  token: "epoch:turn-one:2026-10-23T08:00:00.123456Z",
};
const nextCompletion = {
  ...completion,
  completed_at: "2026-10-23T08:00:00.123457Z",
  token: "regenerated",
};

beforeEach(() => {
  localStorage.clear();
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("conversation completion read state", () => {
  it("keeps background completion unread until its scoped conversation is opened", () => {
    const store = new ConversationActivityStore(localStorage, () => true);
    store.setViewed(project);
    store.applyActivity({ address: agent, running: true });
    expect(store.status(agent)).toEqual({ running: true, unread: false });
    store.applyActivity({ address: agent, running: false, completion });
    expect(store.status(agent)).toEqual({ running: false, unread: true });
    expect(store.status(project)).toEqual({ running: false, unread: false });
    store.setViewed(agent);
    expect(store.status(agent)).toEqual({ running: false, unread: false });
  });

  it("marks completion read automatically only in a visible, focused viewer", () => {
    let foreground = true;
    const store = new ConversationActivityStore(localStorage, () => foreground);
    store.setViewed(agent);
    store.applyActivity({ address: agent, running: false, completion });
    expect(store.status(agent).unread).toBe(false);
    foreground = false;
    store.applyActivity({
      address: agent,
      running: false,
      completion: nextCompletion,
    });
    store.setViewed(agent);
    store.markViewedRead();
    expect(store.status(agent).unread).toBe(true);
    foreground = true;
    store.markViewedRead();
    expect(store.status(agent).unread).toBe(false);
  });

  it("preserves read and unread completion watermarks across reload without persisting spinners", () => {
    const first = new ConversationActivityStore(localStorage, () => true);
    first.applyActivity({ address: agent, running: false, completion });
    first.applyActivity({ address: project, running: false, completion });
    first.setViewed(project);
    first.applyActivity({ address: project, running: true });
    const reloaded = new ConversationActivityStore(localStorage, () => true);
    expect(reloaded.status(agent)).toEqual({ running: false, unread: true });
    expect(reloaded.status(project)).toEqual({ running: false, unread: false });
    reloaded.applySnapshot([]);
    expect(reloaded.status(agent).unread).toBe(true);
    reloaded.applySnapshot([{ address: project, running: false, completion }]);
    expect(reloaded.status(project).unread).toBe(false);
  });

  it("keys agent and project identities by their fully encoded canonical path", () => {
    const store = new ConversationActivityStore(localStorage, () => false);
    const encoded: ConversationAddress = {
      scope: "agent",
      agentId: "an/agent",
      chatId: "chat ?#",
    };
    store.applyActivity({ address: encoded, running: false, completion });
    expect(localStorage.key(0)).toBe(
      "rua.conversation-activity.v1:/agents/an%2Fagent/chats/chat%20%3F%23",
    );
    store.applyActivity({ address: agent, running: false, completion });
    store.applyActivity({ address: project, running: true });
    expect(store.status(agent)).toEqual({ running: false, unread: true });
    expect(store.status(project)).toEqual({ running: true, unread: false });
  });

  it("deduplicates completion tokens but detects regeneration of the same turn", () => {
    const onBoundary = vi.fn();
    const listener = vi.fn();
    const store = new ConversationActivityStore(
      localStorage,
      () => true,
      onBoundary,
    );
    store.subscribe(listener);
    store.applyActivity({ address: agent, running: false, completion });
    store.setViewed(agent);
    store.setViewed(project);
    const calls = listener.mock.calls.length;
    store.applyActivity({
      address: agent,
      running: false,
      completion: { ...completion },
    });
    expect(store.status(agent).unread).toBe(false);
    expect(listener).toHaveBeenCalledTimes(calls);
    store.applyActivity({
      address: agent,
      running: false,
      completion: nextCompletion,
    });
    expect(store.status(agent).unread).toBe(true);
    store.setViewed(agent);
    store.setViewed(project);
    store.applyActivity({ address: agent, running: false, completion });
    expect(store.status(agent).unread).toBe(false);
    expect(onBoundary).toHaveBeenCalledTimes(2);
  });

  it("replaces running state on snapshot and stops failed activity without inventing a completion", () => {
    const boundary = vi.fn();
    const store = new ConversationActivityStore(
      localStorage,
      () => false,
      boundary,
    );
    store.applySnapshot([
      { address: agent, running: true },
      { address: project, running: true },
    ]);
    expect(boundary).toHaveBeenCalledTimes(1);
    store.applyActivity({ address: agent, running: false });
    expect(store.status(agent)).toEqual({ running: false, unread: false });
    expect(store.status(project).running).toBe(true);
    store.applySnapshot([]);
    expect(store.status(project)).toEqual({ running: false, unread: false });
  });

  it("merges reads across tabs without rolling back a newer completion", () => {
    const first = new ConversationActivityStore(localStorage, () => true);
    first.applyActivity({ address: agent, running: false, completion });
    const key = localStorage.key(0)!;
    const second = new ConversationActivityStore(localStorage, () => true);
    second.setViewed(agent);
    const oldRead = localStorage.getItem(key)!;
    first.mergeStorage(key, oldRead);
    expect(first.status(agent).unread).toBe(false);
    first.applyActivity({
      address: agent,
      running: false,
      completion: nextCompletion,
    });
    first.mergeStorage(key, oldRead);
    expect(first.status(agent).unread).toBe(true);
    expect(JSON.parse(localStorage.getItem(key)!).completion.token).toBe(
      nextCompletion.token,
    );
    second.mergeStorage(key, localStorage.getItem(key));
    expect(second.status(agent).unread).toBe(false);
    first.mergeStorage(key, localStorage.getItem(key));
    expect(first.status(agent).unread).toBe(false);
  });

  it("survives corrupt or denied storage and removes state after deletion", () => {
    localStorage.setItem("rua.conversation-activity.v1:/broken", "{");
    const store = new ConversationActivityStore(localStorage, () => false);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    store.applyActivity({ address: agent, running: false, completion });
    expect(store.status(agent).unread).toBe(true);
    store.clear(agent);
    expect(store.status(agent)).toEqual({ running: false, unread: false });
    expect(() =>
      store.mergeStorage("rua.conversation-activity.v1:/broken", "{"),
    ).not.toThrow();
  });

  it("does not restore a deleted chat's unread marker when its completion is replayed", () => {
    const store = new ConversationActivityStore(localStorage, () => false);
    store.applyActivity({ address: agent, running: false, completion });
    store.clear(agent);
    store.applySnapshot([{ address: agent, running: false, completion }]);
    expect(store.status(agent)).toEqual({ running: false, unread: false });
    const reloaded = new ConversationActivityStore(localStorage, () => false);
    reloaded.applySnapshot([{ address: agent, running: false, completion }]);
    expect(reloaded.status(agent).unread).toBe(false);
  });

  it("returns stable snapshots and notifies only while subscribed", () => {
    const store = new ConversationActivityStore(null, () => false);
    expect(store.status(agent)).toBe(store.status(agent));
    const listener = vi.fn();
    const stop = store.subscribe(listener);
    store.applyActivity({ address: agent, running: true });
    stop();
    store.applyActivity({ address: agent, running: false, completion });
    expect(listener).toHaveBeenCalledTimes(1);
  });
});

function streamFixture() {
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  const cancel = vi.fn();
  const stream = new ReadableStream<Uint8Array>({
    start(value) {
      controller = value;
    },
    cancel,
  });
  return {
    response: { ok: true, body: stream },
    cancel,
    send: (value: string) =>
      controller.enqueue(new TextEncoder().encode(value)),
    close: () => controller.close(),
  };
}

describe("global conversation activity transport", () => {
  let stops: Array<() => void>;
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
    stops = [];
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
  });
  afterEach(() => {
    stops.forEach((stop) => stop());
  });

  it("shares one connection, consumes fragmented frames and cancels it after the last release", async () => {
    const fixture = streamFixture();
    const fetcher = vi.fn().mockResolvedValue(fixture.response);
    vi.stubGlobal("fetch", fetcher);
    const activity = await import("./conversationActivity");
    const first = activity.startConversationActivity();
    const second = activity.startConversationActivity();
    stops.push(first, second);
    await vi.advanceTimersByTimeAsync(0);
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(fetcher).toHaveBeenCalledWith(
      "/api/platform/conversation-activity",
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    const { invalidateConversationNavigation } = await import(
      "./conversationNavigationEvents"
    );
    vi.mocked(invalidateConversationNavigation).mockClear();
    fixture.send('{"type":"snapshot","activities":[]}\n{"type":"act');
    fixture.send(
      `ivity","address":{"scope":{"kind":"agent","agent_id":"alice"},"chat_id":"same-chat"},"running":false,"completion":${JSON.stringify(
        completion,
      )}}\n`,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(invalidateConversationNavigation).toHaveBeenCalledWith(agent);
    expect(
      localStorage.getItem(
        "rua.conversation-activity.v1:/agents/alice/chats/same-chat",
      ),
    ).toContain(completion.token);
    first();
    expect(fixture.cancel).not.toHaveBeenCalled();
    second();
    expect(fetcher.mock.calls[0][1].signal.aborted).toBe(true);
    expect(fixture.cancel).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(20_000);
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("backs off disconnected fetches and cancels pending retry timers", async () => {
    const fetcher = vi.fn().mockRejectedValue(new Error("offline"));
    vi.stubGlobal("fetch", fetcher);
    const { startConversationActivity } = await import(
      "./conversationActivity"
    );
    const stop = startConversationActivity();
    stops.push(stop);
    await vi.advanceTimersByTimeAsync(999);
    expect(fetcher).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(fetcher).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1_999);
    expect(fetcher).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(fetcher).toHaveBeenCalledTimes(3);
    stop();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(fetcher).toHaveBeenCalledTimes(3);
  });

  it("reconnects after an ended stream and catches up when the document becomes visible", async () => {
    const fixtures = [streamFixture(), streamFixture(), streamFixture()];
    const fetcher = vi
      .fn()
      .mockImplementation(() =>
        Promise.resolve(fixtures[fetcher.mock.calls.length - 1].response),
      );
    vi.stubGlobal("fetch", fetcher);
    const { startConversationActivity } = await import(
      "./conversationActivity"
    );
    stops.push(startConversationActivity());
    await vi.advanceTimersByTimeAsync(0);
    fixtures[0].close();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(fetcher).toHaveBeenCalledTimes(2);
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    document.dispatchEvent(new Event("visibilitychange"));
    expect(fetcher).toHaveBeenCalledTimes(2);
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(fixtures[1].cancel).toHaveBeenCalledTimes(1);
    expect(fetcher).toHaveBeenCalledTimes(3);
  });

  it("ignores metadata and malformed frames, and marks the viewed chat read on focus", async () => {
    const fixture = streamFixture();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(fixture.response));
    const { startConversationActivity, setViewedConversation } = await import(
      "./conversationActivity"
    );
    stops.push(startConversationActivity());
    setViewedConversation(agent);
    await vi.advanceTimersByTimeAsync(0);
    fixture.send(
      '{broken}\n{"type":"metadata","updated_at":"now","name":"Renamed"}\n',
    );
    fixture.send(
      `${JSON.stringify({
        type: "activity",
        address: {
          scope: { kind: "agent", agent_id: "alice" },
          chat_id: "same-chat",
        },
        running: false,
        completion,
      })}\n`,
    );
    await vi.advanceTimersByTimeAsync(0);
    const key = "rua.conversation-activity.v1:/agents/alice/chats/same-chat";
    expect(JSON.parse(localStorage.getItem(key)!).readToken).toBeUndefined();
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    window.dispatchEvent(new Event("focus"));
    expect(JSON.parse(localStorage.getItem(key)!).readToken).toBe(
      completion.token,
    );
  });
});

vi.mock("../api/consoleTransport", () => ({ consoleFetch: (path: string, options?: RequestInit) => fetch(`/api${path}`, options) }));
