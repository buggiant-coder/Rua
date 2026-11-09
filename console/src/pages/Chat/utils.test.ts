import { describe, expect, it, vi } from "vitest";
import {
  buildModelError,
  extractCopyableText,
  extractSpeakableText,
  extractUserMessageText,
  formatMessagesAsMarkdown,
  groupIntoMessageBlocks,
  groupIntoTurns,
  normalizeContentUrls,
  toDisplayUrl,
  toMarkdownImageDisplayUrl,
  toStoredName,
  type CopyableResponse,
} from "./utils";

const { mockFilePreviewUrl, mockLocalImagePreviewUrl } = vi.hoisted(() => ({
  mockFilePreviewUrl: vi.fn((path: string) => `/api/files/preview${path}`),
  mockLocalImagePreviewUrl: vi.fn(
    (path: string) =>
      `/api/agents/default/files/preview?path=${encodeURIComponent(path)}`,
  ),
}));

vi.mock("../../api/modules/chat", () => ({
  chatApi: {
    filePreviewUrl: mockFilePreviewUrl,
    localImagePreviewUrl: mockLocalImagePreviewUrl,
    mediaUrl: (
      scope: { kind: string; agentId?: string; projectId?: string },
      chatId: string,
      id: string,
    ) =>
      `/api/${
        scope.kind === "project"
          ? "projects/" + scope.projectId
          : "agents/" + scope.agentId
      }/chats/${chatId}/media/${id}`,
  },
}));

describe("Chat text extraction utilities", () => {
  it("extractCopyableText returns assistant text and refusal content only", () => {
    const response: CopyableResponse = {
      output: [
        { role: "user", content: "question" },
        {
          role: "assistant",
          content: [
            { type: "text", text: "answer" },
            { type: "refusal", refusal: "cannot do that" },
          ],
        },
      ],
    };

    expect(extractCopyableText(response)).toBe("answer\n\ncannot do that");
  });

  it("extractCopyableText falls back to JSON when no assistant text exists", () => {
    const response: CopyableResponse = {
      output: [{ role: "user", content: "only user" }],
    };

    expect(extractCopyableText(response)).toBe(JSON.stringify(response));
  });

  it("extractSpeakableText filters thinking and tool blocks from assistant output", () => {
    const response: CopyableResponse = {
      output: [
        { role: "assistant", type: "reasoning", content: "private thought" },
        {
          role: "assistant",
          type: "plugin_call",
          content: [{ type: "text", text: "tool args" }],
        },
        {
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "spoken answer" }],
        },
      ],
    };

    expect(extractSpeakableText(response)).toBe("spoken answer");
  });

  it("extractUserMessageText joins only text parts from multimodal user content", () => {
    expect(
      extractUserMessageText({
        content: [
          { type: "text", text: "hello" },
          { type: "image", image_url: "/image.png" },
          { type: "text", text: "world" },
        ],
      }),
    ).toBe("hello\nworld");
  });
});

describe("Chat URL utilities", () => {
  it("toStoredName preserves Rust stored paths after /files/preview", () => {
    expect(
      toStoredName("http://localhost:8765/files/preview/uploads/a%20b.png?t=1"),
    ).toBe("/uploads/a b.png");
  });

  it("toStoredName keeps Windows absolute preview paths usable", () => {
    expect(toStoredName("http://host/files/preview/C%3A/tmp/file.txt")).toBe(
      "C:/tmp/file.txt",
    );
  });

  it("normalizeContentUrls supports image, file, audio, and Rust video parts", () => {
    expect(
      normalizeContentUrls({
        type: "image",
        image_url: "http://host/files/preview/img.png",
      }),
    ).toMatchObject({ image_url: "/img.png" });
    expect(
      normalizeContentUrls({
        type: "file",
        file_url: "http://host/files/preview/doc.pdf",
      }),
    ).toMatchObject({ file_url: "/doc.pdf" });
    expect(
      normalizeContentUrls({
        type: "audio",
        data: "http://host/files/preview/audio.mp3",
      }),
    ).toMatchObject({ data: "/audio.mp3" });
    expect(
      normalizeContentUrls({
        type: "video",
        video_url: "http://host/files/preview/movie.mp4",
      }),
    ).toMatchObject({ video_url: "/movie.mp4" });
  });

  it("toDisplayUrl leaves remote URLs alone and delegates stored paths", () => {
    expect(toDisplayUrl("https://cdn.example/a.png")).toBe(
      "https://cdn.example/a.png",
    );
    expect(toDisplayUrl("file:///uploads/a.png")).toBe(
      "/api/files/preview/uploads/a.png",
    );
    expect(mockFilePreviewUrl).toHaveBeenCalledWith("/uploads/a.png");
  });

  it("toDisplayUrl preserves canonical API media URLs", () => {
    expect(
      toDisplayUrl(
        "/api/projects/project-1/chats/chat-1/media/00000000-0000-4000-8000-000000000001?token=secret",
      ),
    ).toBe(
      "/api/projects/project-1/chats/chat-1/media/00000000-0000-4000-8000-000000000001?token=secret",
    );
  });

  it("toMarkdownImageDisplayUrl keeps browser sources direct and rejects local paths", () => {
    expect(toMarkdownImageDisplayUrl("https://cdn.example/a.png")).toBe(
      "https://cdn.example/a.png",
    );
    expect(toMarkdownImageDisplayUrl("data:image/png;base64,AA==")).toBe(
      "data:image/png;base64,AA==",
    );
    expect(toMarkdownImageDisplayUrl("blob:https://console.example/id")).toBe(
      "blob:https://console.example/id",
    );
    expect(toMarkdownImageDisplayUrl("~/Pictures/照片.jpg")).toBe("");
    expect(toMarkdownImageDisplayUrl("/tmp/preview.png")).toBe("");
    const id = "00000000-0000-4000-8000-000000000001";
    expect(toMarkdownImageDisplayUrl(`media/${id}`)).toBe("");
    expect(
      toMarkdownImageDisplayUrl(
        `media/${id}`,
        { kind: "project", projectId: "project-1" },
        "chat-1",
      ),
    ).toBe(`/api/projects/project-1/chats/chat-1/media/${id}`);
    expect(
      toMarkdownImageDisplayUrl(
        `media/${id}`,
        { kind: "agent", agentId: "agent-1" },
        "chat-1",
      ),
    ).toBe(`/api/agents/agent-1/chats/chat-1/media/${id}`);
  });
});

describe("Chat export utilities", () => {
  it("buildModelError keeps the frontend-compatible 400 JSON shape", async () => {
    const response = buildModelError();

    expect(response.status).toBe(400);
    expect(response.headers.get("Content-Type")).toBe("application/json");
    await expect(response.json()).resolves.toEqual({
      error: "Model not configured",
      message: "Please configure a model first",
    });
  });

  it("groupIntoTurns keeps one user message with consecutive assistant text", () => {
    expect(
      groupIntoTurns([
        { role: "system", content: "ignored" },
        { role: "user", content: "question" },
        { role: "assistant", type: "reasoning", content: "hidden" },
        { role: "assistant", type: "message", content: "answer" },
        { role: "user", content: [{ type: "text", text: "next" }] },
      ]),
    ).toEqual([
      { userText: "question", assistantText: "answer" },
      { userText: "next", assistantText: "" },
    ]);
  });

  it("groupIntoTurns filters hidden memory signal markers", () => {
    expect(
      groupIntoTurns([
        { role: "user", content: "记住一下：以后回答直接给工程结论。" },
        {
          role: "user",
          content:
            "<memory_signal>\nsignal_kind: explicit_memory\n</memory_signal>",
          metadata: {
            model_visible_marker: "memory_signal",
            hidden_from_ui: true,
          },
        },
        { role: "assistant", type: "message", content: "已处理。" },
      ]),
    ).toEqual([
      {
        userText: "记住一下：以后回答直接给工程结论。",
        assistantText: "已处理。",
      },
    ]);
  });

  it("groupIntoMessageBlocks summarizes omitted thinking and tool blocks", () => {
    expect(
      groupIntoMessageBlocks([
        { role: "user", content: "run it" },
        { role: "assistant", type: "reasoning", content: "planning" },
        {
          role: "assistant",
          type: "mcp_call",
          content: [{ data: { name: "Read", arguments: { path: "a" } } }],
        },
        { role: "assistant", type: "message", content: "done" },
      ]),
    ).toEqual([
      { role: "user", text: "run it" },
      { role: "assistant", text: "*[1 thinking, 1 tool call]*\n\ndone" },
    ]);
  });

  it("groupIntoMessageBlocks filters hidden memory signal markers", () => {
    expect(
      groupIntoMessageBlocks([
        { role: "user", content: "记住一下：以后回答直接给工程结论。" },
        {
          role: "user",
          content:
            "<memory_signal>\nsignal_kind: explicit_memory\n</memory_signal>",
          metadata: { model_visible_marker: "memory_signal" },
        },
        { role: "assistant", type: "message", content: "已处理。" },
      ]),
    ).toEqual([
      { role: "user", text: "记住一下：以后回答直接给工程结论。" },
      { role: "assistant", text: "已处理。" },
    ]);
  });

  it("groupIntoMessageBlocks unwraps MCP text-block tool output for export", () => {
    const output = JSON.stringify([
      {
        type: "text",
        text: "memory/2026-05-18.md L1-77\\n# Memory\\nImportant note",
      },
    ]);

    expect(
      groupIntoMessageBlocks(
        [
          { role: "user", content: "search memory" },
          {
            role: "assistant",
            type: "plugin_call",
            content: [
              {
                data: {
                  name: "memory_search",
                  arguments: { query: "Important" },
                  output,
                },
              },
            ],
          },
        ],
        true,
        true,
      ),
    ).toEqual([
      { role: "user", text: "search memory" },
      {
        role: "assistant",
        text: '```\n[memory_search]\nInput: {\n  "query": "Important"\n}\nOutput: memory/2026-05-18.md L1-77\n# Memory\nImportant note\n```',
      },
    ]);
  });

  it("groupIntoMessageBlocks formats memory_search result items for export", () => {
    const memoryResults = [
      {
        content: "actual memory summary",
        score: 0.85,
        source: "structured",
        metadata: {
          id: "mem_abc",
          kind: "preference",
          scope: "user",
          subject: "response_style",
        },
      },
    ];
    const output = JSON.stringify([
      {
        type: "text",
        text: JSON.stringify(memoryResults),
      },
    ]);

    const [, assistantBlock] = groupIntoMessageBlocks(
      [
        { role: "user", content: "search memory" },
        {
          role: "assistant",
          type: "mcp_call_output",
          content: [
            {
              data: {
                name: "memory_search",
                output,
              },
            },
          ],
        },
      ],
      true,
      true,
    );

    expect(assistantBlock.text).toContain("### 1. mem_abc");
    expect(assistantBlock.text).toContain("- **Source**: structured");
    expect(assistantBlock.text).toContain("- **Kind**: preference");
    expect(assistantBlock.text).toContain("- **Scope**: user");
    expect(assistantBlock.text).toContain("- **Subject**: response_style");
    expect(assistantBlock.text).toContain("- **Score**: 0.85");
    expect(assistantBlock.text).toContain("actual memory summary");
    expect(assistantBlock.text).not.toContain('"type": "text"');
  });

  it("formatMessagesAsMarkdown exports selected message blocks in order", () => {
    const markdown = formatMessagesAsMarkdown(
      [
        { role: "assistant", text: "first" },
        { role: "user", text: "second" },
      ],
      new Set([1]),
      "Session A",
    );

    expect(markdown).toContain("# Session A");
    expect(markdown).toContain("## User");
    expect(markdown).toContain("second");
    expect(markdown).not.toContain("first");
  });
});
