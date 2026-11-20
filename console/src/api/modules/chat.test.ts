import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));
vi.mock("../config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
  getApiToken: vi.fn(() => ""),
}));
vi.mock("../authHeaders", () => ({
  buildAuthHeaders: vi.fn(() => ({ "X-Test": "1" })),
  getSelectedAgentId: vi.fn(() => "selected-agent"),
}));

import { getApiToken } from "../config";
import { buildAuthHeaders } from "../authHeaders";
import { request } from "../request";
import { chatApi } from "./chat";

describe("chatApi file helpers", () => {
  it("requests only scoped Chat metadata and forwards cancellation", async () => {
    const controller = new AbortController();
    await chatApi.getMetadata("chat/one", "agent/one", {
      signal: controller.signal,
    });
    expect(request).toHaveBeenCalledWith("/chats/chat%2Fone/metadata", {
      signal: controller.signal,
    });
  });

  it("requests a bounded complete-turn window by stable Turn id", async () => {
    await chatApi.getChat("chat/1", {
      turnLimit: 20,
      aroundTurnId: "turn/1",
    });
    expect(request).toHaveBeenCalledWith(
      "/chats/chat%2F1?turn_limit=20&around_turn_id=turn%2F1",
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("builds Rust file preview URLs and appends the API token when present", () => {
    expect(chatApi.filePreviewUrl("")).toBe("");
    expect(chatApi.filePreviewUrl("https://cdn.example/img.png")).toBe(
      "https://cdn.example/img.png",
    );
    expect(chatApi.filePreviewUrl("/img.png")).toBe(
      "/api/files/preview/img.png",
    );

    vi.mocked(getApiToken).mockReturnValueOnce("tok en+1");
    expect(chatApi.filePreviewUrl("img.png")).toBe(
      "/api/files/preview/img.png?token=tok%20en%2B1",
    );
  });

  it("builds agent-scoped local image preview URLs without changing the source path", () => {
    expect(
      chatApi.localImagePreviewUrl("/sdcard/DCIM/Camera/IMG 20260722_HDR.jpg"),
    ).toBe(
      "/api/agents/selected-agent/files/preview?path=%2Fsdcard%2FDCIM%2FCamera%2FIMG+20260722_HDR.jpg",
    );

    vi.mocked(getApiToken).mockReturnValueOnce("tok en+1");
    expect(chatApi.localImagePreviewUrl("~/Pictures/照片.jpg", "agent/1")).toBe(
      "/api/agents/agent%2F1/files/preview?path=%7E%2FPictures%2F%E7%85%A7%E7%89%87.jpg&token=tok+en%2B1",
    );
  });

  it("stages Chat Media with explicit draft context", async () => {
    const result = {
      media_ref: "media/00000000-0000-4000-8000-000000000001",
      media_id: "00000000-0000-4000-8000-000000000001",
      mime_type: "image/png",
      byte_size: 5,
      authority_epoch: "000001a00343124c00000000f8e40a95f10d7f0e",
      original_filename: "img.png",
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify(result), {
          status: 201,
          headers: { "Content-Type": "application/json" },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify(result), {
          status: 201,
          headers: { "Content-Type": "application/json" },
        }),
      );
    vi.stubGlobal("fetch", fetchMock);

    const file = new File(["image"], "img.png", { type: "image/png" });

    await expect(
      chatApi.uploadFile("chat 1", file, { agentId: "agent/1" }),
    ).resolves.toEqual(result);
    await expect(
      chatApi.uploadFile("chat 1", file, {
        projectId: "project/1",
        agentId: "agent-1",
        model: { provider_id: "provider-1", model: "model-1" },
      }),
    ).resolves.toEqual(result);
    expect(buildAuthHeaders).toHaveBeenCalled();
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "/api/chats/chat%201/media",
      expect.objectContaining({
        method: "POST",
        headers: { "X-Test": "1" },
      }),
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "/api/chats/chat%201/media",
      expect.objectContaining({
        method: "POST",
        headers: { "X-Test": "1" },
      }),
    );
    const projectForm = fetchMock.mock.calls[1][1]?.body as FormData;
    expect(projectForm.get("project_id")).toBe("project/1");
    expect(projectForm.get("agent_id")).toBe("agent-1");
    expect(JSON.parse(String(projectForm.get("model")))).toEqual({
      provider_id: "provider-1",
      model: "model-1",
    });
    expect(chatApi.mediaUrl("chat 1", result.media_id)).toBe(
      "/api/chats/chat%201/media/00000000-0000-4000-8000-000000000001",
    );

    vi.mocked(getApiToken).mockReturnValueOnce("media token");
    expect(chatApi.mediaUrl("chat 1", result.media_id)).toBe(
      "/api/chats/chat%201/media/00000000-0000-4000-8000-000000000001?token=media%20token",
    );
  });

  it("surfaces upload HTTP failures with status, text, and no extra dash for empty bodies", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValueOnce(
          new Response("File too large", {
            status: 413,
            statusText: "Payload Too Large",
          }),
        )
        .mockResolvedValueOnce(
          new Response("", {
            status: 500,
            statusText: "Internal Server Error",
          }),
        ),
    );

    await expect(
      chatApi.uploadFile("chat-1", new File(["big"], "big.bin")),
    ).rejects.toThrow("Upload failed: 413 Payload Too Large - File too large");
    await expect(
      chatApi.uploadFile("chat-1", new File(["bad"], "bad.bin")),
    ).rejects.toThrow("Upload failed: 500 Internal Server Error");
  });
});

describe("chatApi Rust independent Chat routes", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("lists chats with query params, auth headers, and total header", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      new Response(JSON.stringify([{ id: "chat-1", name: "Chat" }]), {
        status: 200,
        headers: {
          "Content-Type": "application/json",
          "X-Total-Count": "12",
        },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      chatApi.listChats({
        user_id: "u/1",
        channel: "dingtalk",
        tag: "red",
        offset: 10,
        limit: 20,
      }),
    ).resolves.toEqual({
      data: [{ id: "chat-1", name: "Chat" }],
      total: 12,
    });

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/chats?user_id=u%2F1&channel=dingtalk&tag=red&offset=10&limit=20",
      { headers: { "X-Test": "1" }, cache: "no-store" },
    );
  });

  it("lists collaboration chats with relationship filters", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      new Response("[]", {
        status: 200,
        headers: { "X-Total-Count": "0" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    await chatApi.listChats({
      view: "collaboration",
      search: "review",
      source_chat_id: "source/1",
      executor: "mock_agent",
      status: "running",
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/chats?view=collaboration&search=review&source_chat_id=source%2F1&executor=mock_agent&status=running",
      { headers: { "X-Test": "1" }, cache: "no-store" },
    );
  });

  it("uses the Chat ID for chat CRUD, options, pin order, batch delete, stop, and search", async () => {
    vi.mocked(request).mockResolvedValue({});

    await chatApi.createChat(
      { name: "New" } as Parameters<typeof chatApi.createChat>[0],
      "agent-1",
    );
    await chatApi.getChat("chat/1");
    await chatApi.getChat("chat/1", {
      turnLimit: 20,
      beforeTurnOrdinal: 7,
    });
    await chatApi.updateChat("chat/1", { name: "Renamed" }, "agent-1");
    await chatApi.getRunningConfig("chat/1", "agent-1");
    await chatApi.updateRunningConfig("chat/1", { max_iters: 12 }, "agent-1");
    await chatApi.deleteChat("chat/1", "agent-1");
    await chatApi.updatePinOrder([{ id: "chat/1", pin_order: 1 }], "agent-1");
    await chatApi.batchDeleteChats(["chat/1"], "agent-1");
    await chatApi.stopChat("chat/1", {
      turn_id: "turn-1",
      authority_epoch: "epoch-1",
    });
    await chatApi.searchChats("hello world", {
      agent_id: "agent-1",
      without_project: true,
      archived: true,
      offset: 20,
    });

    expect(request).toHaveBeenNthCalledWith(1, "/chats", {
      method: "POST",
      body: JSON.stringify({ name: "New", agent_id: "agent-1" }),
    });
    expect(request).toHaveBeenNthCalledWith(2, "/chats/chat%2F1");
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/chats/chat%2F1?turn_limit=20&before_turn_ordinal=7",
    );
    expect(request).toHaveBeenNthCalledWith(4, "/chats/chat%2F1", {
      method: "PUT",
      body: JSON.stringify({ name: "Renamed" }),
    });
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/chats/chat%2F1/running-config",
    );
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/chats/chat%2F1/running-config",
      {
        method: "PUT",
        body: JSON.stringify({ max_iters: 12 }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(7, "/chats/chat%2F1", {
      method: "DELETE",
    });
    expect(request).toHaveBeenNthCalledWith(8, "/chats/pin-order", {
      method: "PUT",
      body: JSON.stringify([{ id: "chat/1", pin_order: 1 }]),
    });
    expect(request).toHaveBeenNthCalledWith(9, "/chats/batch-delete", {
      method: "POST",
      body: JSON.stringify(["chat/1"]),
    });
    expect(request).toHaveBeenNthCalledWith(
      10,
      "/chats/chat%2F1/console/stop",
      {
        method: "POST",
        body: JSON.stringify({ turn_id: "turn-1", authority_epoch: "epoch-1" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      11,
      "/chats/search?agent_id=agent-1&without_project=true&archived=true&offset=20&q=hello+world",
    );
  });
});
