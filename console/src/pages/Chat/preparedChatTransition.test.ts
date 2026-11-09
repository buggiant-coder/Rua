import { beforeEach, describe, expect, it, vi } from "vitest";
import { draftChatKey } from "./chatNavigation";
import {
  cancelPreparedChatTransition,
  prepareChatTransition,
} from "./preparedChatTransition";

const { getChatMock, primeChatViewCacheMock } = vi.hoisted(() => ({
  getChatMock: vi.fn(),
  primeChatViewCacheMock: vi.fn(),
}));

vi.mock("@/api/modules/chat", () => ({
  chatApi: {
    getChat: getChatMock,
  },
}));

vi.mock("./owned/useConversationViewRuntime", () => ({
  INITIAL_CHAT_TURN_PAGE_SIZE: 20,
  primeChatViewCache: primeChatViewCacheMock,
}));

describe("prepareChatTransition", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    cancelPreparedChatTransition();
  });

  it("treats a local draft as ready without requesting history", async () => {
    localStorage.setItem(draftChatKey("agent-a"), "draft-a");

    await expect(prepareChatTransition("agent-a", "draft-a")).resolves.toBe(
      true,
    );

    expect(getChatMock).not.toHaveBeenCalled();
    expect(primeChatViewCacheMock).not.toHaveBeenCalled();
  });

  it("loads and primes the target conversation before allowing commit", async () => {
    const history = {
      status: "idle",
      messages: [{ id: "message-1", role: "user", content: "hello" }],
    };
    getChatMock.mockResolvedValue(history);

    await expect(prepareChatTransition("agent-a", "chat-2")).resolves.toBe(
      true,
    );

    expect(getChatMock).toHaveBeenCalledWith("chat-2", "agent-a", {
      turnLimit: 20,
    });
    expect(primeChatViewCacheMock).toHaveBeenCalledWith(
      "agent-a",
      "chat-2",
      history,
    );
  });

  it("keeps an empty remote history lazy", async () => {
    const history = { status: "idle", messages: [] };
    getChatMock.mockResolvedValue(history);

    await expect(prepareChatTransition("agent-a", "chat-lazy")).resolves.toBe(
      true,
    );

    expect(localStorage.getItem(draftChatKey("agent-a"))).toBe("chat-lazy");
    expect(primeChatViewCacheMock).toHaveBeenCalledWith(
      "agent-a",
      "chat-lazy",
      history,
    );
  });

  it("drops an older preparation when a newer target wins", async () => {
    let resolveFirst: ((history: unknown) => void) | undefined;
    let resolveSecond: ((history: unknown) => void) | undefined;
    getChatMock
      .mockReturnValueOnce(
        new Promise((resolve) => {
          resolveFirst = resolve;
        }),
      )
      .mockReturnValueOnce(
        new Promise((resolve) => {
          resolveSecond = resolve;
        }),
      );

    const first = prepareChatTransition("agent-a", "chat-1");
    const second = prepareChatTransition("agent-a", "chat-2");

    resolveSecond?.({ status: "idle", messages: [] });
    await expect(second).resolves.toBe(true);

    resolveFirst?.({ status: "idle", messages: [] });
    await expect(first).resolves.toBe(false);

    expect(primeChatViewCacheMock).toHaveBeenCalledTimes(1);
    expect(primeChatViewCacheMock).toHaveBeenCalledWith(
      "agent-a",
      "chat-2",
      expect.anything(),
    );
  });

  it("silences an obsolete request failure after a newer target wins", async () => {
    let rejectFirst: ((error: Error) => void) | undefined;
    getChatMock
      .mockReturnValueOnce(
        new Promise((_, reject) => {
          rejectFirst = reject;
        }),
      )
      .mockResolvedValueOnce({ status: "idle", messages: [] });

    const first = prepareChatTransition("agent-a", "chat-1");
    const second = prepareChatTransition("agent-a", "chat-2");

    await expect(second).resolves.toBe(true);
    rejectFirst?.(new Error("obsolete failure"));

    await expect(first).resolves.toBe(false);
  });

  it("resolves a cancelled preparation without waiting for its request", async () => {
    getChatMock.mockReturnValueOnce(new Promise(() => undefined));

    const preparation = prepareChatTransition("agent-a", "chat-1");
    cancelPreparedChatTransition();

    await expect(preparation).resolves.toBe(false);
  });
});
