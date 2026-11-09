import { __resetConversationViewportForTests } from "../useConversationViewport";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { useLayoutEffect, useState, type ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ConversationMessageListImpl from "../ConversationMessageList";
import {
  projectConversation as projectConversationImpl,
  createConversationProjector,
  type ConversationProjection,
} from "../projection";
import { ModalProvider } from "@/ui";
import * as markdownPlus from "@/components/RuaMarkdown/markdownPlus";
import { canonicalHistoryFixture } from "./canonicalMessageFixtures";
vi.mock("@/api/config", () => ({ getApiUrl: (path: string) => `/api${path}` }));
vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer test" }),
}));

function projectConversation(
  ...args: Parameters<typeof projectConversationImpl>
): ReturnType<typeof projectConversationImpl> {
  const [conversationId, history, options] = args;
  return projectConversationImpl(
    conversationId,
    canonicalHistoryFixture(history),
    options,
  );
}

const ConversationMessageList = ConversationMessageListImpl;
const EXPANDED_BLOCKS = {
  aggregateExpanded: true,
  thinkingExpanded: true,
  toolExpanded: true,
};

function withActiveTurn(
  projection: ConversationProjection,
): ConversationProjection {
  const response = [...projection.items]
    .reverse()
    .find((item) => item.kind === "assistant");
  return {
    ...projection,
    turnActivity: {
      kind: "active",
      turnId: null,
      userMessageId: null,
      responseId: response?.id ?? null,
    },
  };
}

const ttsMock = vi.hoisted(() => ({
  speak: vi.fn(),
  stop: vi.fn(),
  isSpeaking: false,
  isLoading: false,
  isDisabled: false,
  error: null,
}));

const mobileMock = vi.hoisted(() => ({
  enabled: false,
  saveFile: vi.fn(),
}));

vi.mock("@/api/modules/chat", () => ({
  chatApi: {
    filePreviewUrl: (path: string) => `/preview${path}`,
    localImagePreviewUrl: (path: string, agentId = "default") =>
      `/agents/${agentId}/files/preview?path=${encodeURIComponent(path)}`,
    mediaUrl: (
      scope:
        | { kind: "agent"; agentId: string }
        | { kind: "project"; projectId: string },
      chatId: string,
      mediaId: string,
    ) =>
      scope.kind === "project"
        ? `/api/projects/${scope.projectId}/chats/${chatId}/media/${mediaId}?token=test`
        : `/api/agents/${scope.agentId}/chats/${chatId}/media/${mediaId}?token=test`,
  },
}));

vi.mock("@/utils/isMobile", () => ({
  isMobile: () => mobileMock.enabled,
}));

vi.mock("@/utils/saveFile", () => ({
  getDownloadFolderHint: () =>
    mobileMock.enabled ? "/sdcard/Download/rua/" : "",
  saveFile: mobileMock.saveFile,
}));

vi.mock("@/hooks/useTTS", () => ({
  useTTS: () => ttsMock,
}));

describe("ConversationMessageList", () => {
  it("skips list work when an unrelated parent render keeps the same projection", () => {
    const projected = projectConversation("unchanged-list", {
      messages: [
        { id: "user", role: "user", content: "Question" },
        { id: "answer", role: "assistant", content: "Answer" },
      ],
    });
    const readItems = vi.fn(() => projected.items);
    const projection = {
      ...projected,
      get items() {
        return readItems();
      },
    };
    const { rerender } = render(
      <ConversationMessageList projection={projection} />,
    );
    readItems.mockClear();
    rerender(<ConversationMessageList projection={projection} />);
    expect(readItems).not.toHaveBeenCalled();
    expect(screen.getByText("Answer")).toBeInTheDocument();
  });

  it("does not remeasure all user messages on height-only viewport events when ResizeObserver is available", () => {
    const originalObserver = globalThis.ResizeObserver;
    class Observer {
      observe() {}
      disconnect() {}
    }
    vi.stubGlobal("ResizeObserver", Observer);
    const readHeight = vi
      .spyOn(HTMLElement.prototype, "scrollHeight", "get")
      .mockReturnValue(40);
    const { unmount } = render(
      <ConversationMessageList
        projection={projectConversation("resize-list", {
          messages: Array.from({ length: 20 }, (_, index) => ({
            id: `user-${index}`,
            role: "user",
            content: `Question ${index}`,
          })),
        })}
      />,
    );
    try {
      readHeight.mockClear();
      fireEvent(window, new Event("resize"));
      expect(readHeight).not.toHaveBeenCalled();
    } finally {
      unmount();
      readHeight.mockRestore();
      vi.stubGlobal("ResizeObserver", originalObserver);
    }
  });

  it("renders the browser tool attachment image with its canonical call identity", () => {
    const image = "data:image/png;base64,iVBORw0KGgo=";
    const projection = projectConversation("browser-chat", {
      status: "idle",
      messages: [
        {
          id: "msg-tc-browser-image",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "tool",
              data: {
                call_id: "browser-image",
                name: "mcp__rua_browser__agent_browser_screenshot",
                arguments: {},
              },
            },
          ],
        },
        {
          id: "msg-tr-browser-image",
          role: "tool",
          type: "plugin_call_output",
          content: [
            {
              type: "tool_result",
              data: {
                call_id: "browser-image",
                output: "Current Chat browser screenshot",
              },
            },
          ],
        },
        {
          id: "msg-tr-browser-image-attachment",
          role: "assistant",
          type: "message",
          content: [{ type: "image", image_url: image }],
        },
      ],
    });
    const { container } = render(
      <ConversationMessageList
        projection={projection}
        blockPreferences={EXPANDED_BLOCKS}
      />,
    );
    expect(container.querySelector(".rua-chat-image-part")).toHaveAttribute(
      "src",
      image,
    );
    expect(
      container.querySelector('[data-rua-tool-call-id="browser-image"]'),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Current Chat browser screenshot"),
    ).toBeInTheDocument();
  });
  it("does not parse or mount folded tool groups until opened, including later stream updates", () => {
    const parseMarkdown = vi.spyOn(markdownPlus, "splitMarkdownPlus");
    const stringify = vi.spyOn(JSON, "stringify");
    const result = { text: "Complete tool output" };
    const projectionWithThinking = (text: string) =>
      projectConversation("chat-lazy", {
        status: "idle",
        messages: [
          {
            id: "thinking",
            role: "assistant",
            type: "reasoning",
            content: text,
          },
          {
            id: "tool-call",
            role: "assistant",
            type: "plugin_call",
            content: [
              {
                type: "tool",
                data: {
                  call_id: "call-1",
                  name: "Read",
                  arguments: { path: "notes.txt" },
                },
              },
            ],
          },
          {
            id: "tool-result",
            role: "tool",
            type: "plugin_call_output",
            content: [
              {
                type: "tool_result",
                data: { call_id: "call-1", output: result },
              },
            ],
          },
        ],
      });
    const blockPreferences = {
      aggregateExpanded: false,
      thinkingExpanded: true,
      toolExpanded: true,
    };
    try {
      const { container, rerender } = render(
        <ConversationMessageList
          projection={projectionWithThinking("Long reasoning")}
          blockPreferences={blockPreferences}
        />,
      );
      const group = screen.getByRole("button", {
        name: /1 thinking, 1 tool call/,
      });
      expect(parseMarkdown).not.toHaveBeenCalled();
      expect(stringify.mock.calls.some(([value]) => value === result)).toBe(
        false,
      );
      expect(container.querySelector(".rua-tool-group-content")).toBeNull();

      fireEvent.click(group);
      expect(screen.getByText("Long reasoning")).toBeInTheDocument();
      expect(screen.getByLabelText("Output code block")).toHaveTextContent(
        "Complete tool output",
      );
      expect(stringify.mock.calls.some(([value]) => value === result)).toBe(
        true,
      );

      fireEvent.click(group);
      parseMarkdown.mockClear();
      stringify.mockClear();
      rerender(
        <ConversationMessageList
          projection={projectionWithThinking(
            "Long reasoning with the final detail",
          )}
          blockPreferences={blockPreferences}
        />,
      );
      expect(parseMarkdown).not.toHaveBeenCalled();
      expect(stringify.mock.calls.some(([value]) => value === result)).toBe(
        false,
      );
      expect(
        screen.queryByLabelText("Output code block"),
      ).not.toBeInTheDocument();

      fireEvent.click(group);
      expect(
        screen.getByText("Long reasoning with the final detail"),
      ).toBeInTheDocument();
      expect(screen.getByLabelText("Output code block")).toHaveTextContent(
        "Complete tool output",
      );
    } finally {
      parseMarkdown.mockRestore();
      stringify.mockRestore();
    }
  });

  it("mounts individual thinking and tool details only while open", async () => {
    const parseMarkdown = vi.spyOn(markdownPlus, "splitMarkdownPlus");
    const stringify = vi.spyOn(JSON, "stringify");
    const result = { text: "Lazy tool detail" };
    const projectionWithThinking = (text: string) =>
      projectConversation("chat-lazy-details", {
        status: "idle",
        messages: [
          {
            id: "thinking",
            role: "assistant",
            type: "reasoning",
            content: text,
          },
          {
            id: "tool",
            role: "assistant",
            type: "plugin_call",
            content: [
              {
                type: "tool",
                data: {
                  call_id: "call-1",
                  name: "Read",
                  arguments: { path: "notes.txt" },
                },
              },
            ],
          },
          {
            id: "result",
            role: "tool",
            type: "plugin_call_output",
            content: [
              {
                type: "tool_result",
                data: { call_id: "call-1", output: result },
              },
            ],
          },
        ],
      });
    const blockPreferences = {
      aggregateExpanded: true,
      thinkingExpanded: false,
      toolExpanded: false,
    };
    try {
      const { rerender } = render(
        <ConversationMessageList
          projection={projectionWithThinking("Private reasoning")}
          blockPreferences={blockPreferences}
        />,
      );
      expect(parseMarkdown).not.toHaveBeenCalled();
      expect(stringify.mock.calls.some(([value]) => value === result)).toBe(
        false,
      );
      expect(
        screen.queryByLabelText("Output code block"),
      ).not.toBeInTheDocument();

      fireEvent.click(screen.getByText("Thinking"));
      fireEvent.click(screen.getByText("Read"));
      await waitFor(() => {
        expect(screen.getByText("Private reasoning")).toBeInTheDocument();
        expect(screen.getByLabelText("Output code block")).toHaveTextContent(
          "Lazy tool detail",
        );
      });

      fireEvent.click(screen.getByText("Thinking"));
      fireEvent.click(screen.getByText("Read"));
      await waitFor(() => {
        expect(screen.queryByText("Private reasoning")).not.toBeInTheDocument();
        expect(
          screen.queryByLabelText("Output code block"),
        ).not.toBeInTheDocument();
      });
      parseMarkdown.mockClear();
      stringify.mockClear();
      rerender(
        <ConversationMessageList
          projection={projectionWithThinking("Updated reasoning")}
          blockPreferences={blockPreferences}
        />,
      );
      expect(parseMarkdown).not.toHaveBeenCalled();
      expect(stringify.mock.calls.some(([value]) => value === result)).toBe(
        false,
      );
      fireEvent.click(screen.getByText("Thinking"));
      await waitFor(() =>
        expect(screen.getByText("Updated reasoning")).toBeInTheDocument(),
      );
    } finally {
      parseMarkdown.mockRestore();
      stringify.mockRestore();
    }
  });

  it("does not reparse unchanged Markdown when selecting messages or streaming the next reply", () => {
    const parseMarkdown = vi.spyOn(markdownPlus, "splitMarkdownPlus");
    const liveProjection = (text: string) =>
      withActiveTurn(
        projectConversation("chat-rendering", {
          status: "streaming",
          messages: [
            { role: "user", content: "First question" },
            {
              role: "assistant",
              content: "## Settled answer\n\nSaved history.",
            },
            { role: "user", content: "Next question" },
            { role: "assistant", content: text },
          ],
        }),
      );

    try {
      const { rerender } = render(
        <ConversationMessageList projection={liveProjection("Starting")} />,
      );
      expect(parseMarkdown).toHaveBeenCalled();
      parseMarkdown.mockClear();

      fireEvent.click(screen.getByText("First question"));
      expect(parseMarkdown).not.toHaveBeenCalled();

      for (const text of ["Starting the next", "Starting the next reply"]) {
        rerender(<ConversationMessageList projection={liveProjection(text)} />);
        expect(parseMarkdown.mock.calls.map(([content]) => content)).toEqual([
          text,
        ]);
        expect(screen.getByText(text)).toBeInTheDocument();
        expect(screen.getByText("Saved history.")).toBeInTheDocument();
        parseMarkdown.mockClear();
      }
    } finally {
      parseMarkdown.mockRestore();
    }
  });

  it("does not remeasure settled cards while the next reply streams", () => {
    const project = createConversationProjector();
    const settled = canonicalHistoryFixture({
      status: "idle",
      messages: [
        { role: "user", content: "Earlier question" },
        { role: "assistant", content: "Settled response" },
        { role: "user", content: "Continue" },
      ],
    });
    const liveProjection = (text: string) =>
      withActiveTurn(
        project("chat-stable-cards", {
          ...settled,
          messages: [
            ...settled.messages,
            { id: "active-answer", role: "assistant", content: text },
          ],
        }),
      );
    const { container, rerender } = render(
      <ConversationMessageList projection={liveProjection("First chunk")} />,
    );
    const oldBubble = container.querySelector<HTMLElement>(
      ".rua-chat-bubble--assistant",
    )!;
    const measure = vi.spyOn(oldBubble, "getBoundingClientRect");
    try {
      rerender(
        <ConversationMessageList
          projection={liveProjection("First chunk and more")}
        />,
      );
      expect(screen.getByText("First chunk and more")).toBeInTheDocument();
      expect(measure).not.toHaveBeenCalled();
      fireEvent.click(screen.getByText("Continue"));
      expect(measure).not.toHaveBeenCalled();
      expect(screen.getByText("Settled response")).toBeInTheDocument();
    } finally {
      measure.mockRestore();
    }
  });

  it("defers live Markdown behind urgent input and renders final text without deferring", () => {
    const parseMarkdown = vi.spyOn(markdownPlus, "splitMarkdownPlus");
    const commits: string[][] = [];
    function StreamingHarness() {
      const [text, setText] = useState("Initial reply");
      const [generating, setGenerating] = useState(true);
      useLayoutEffect(() => {
        commits.push(parseMarkdown.mock.calls.map(([content]) => content));
      }, [text, generating]);
      return (
        <>
          <input
            aria-label="Urgent input"
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
          <button
            onClick={() => {
              setText("Final complete reply");
              setGenerating(false);
            }}
          >
            Finish reply
          </button>
          <ConversationMessageList
            projection={{
              conversationId: "chat-deferred",
              turnActivity: generating
                ? {
                    kind: "active",
                    turnId: "turn-1",
                    userMessageId: null,
                    responseId: "assistant-1",
                  }
                : { kind: "idle" },
              items: [
                {
                  kind: "assistant",
                  id: "assistant-1",
                  role: "assistant",
                  status: generating ? "generating" : "completed",
                  variants: [],
                  output: [
                    {
                      id: "text-1",
                      role: "assistant",
                      type: "message",
                      status: generating ? "in_progress" : "completed",
                      content: [{ type: "text", text }],
                    },
                  ],
                },
              ],
            }}
          />
        </>
      );
    }
    try {
      render(<StreamingHarness />);
      commits.length = 0;
      parseMarkdown.mockClear();
      fireEvent.change(screen.getByLabelText("Urgent input"), {
        target: { value: "Updated live reply" },
      });
      expect(screen.getByLabelText("Urgent input")).toHaveValue(
        "Updated live reply",
      );
      expect(commits[0]).not.toContain("Updated live reply");
      expect(screen.getByText("Updated live reply")).toBeInTheDocument();

      commits.length = 0;
      parseMarkdown.mockClear();
      fireEvent.click(screen.getByText("Finish reply"));
      expect(commits[0]).toContain("Final complete reply");
      expect(screen.getByText("Final complete reply")).toBeInTheDocument();
    } finally {
      parseMarkdown.mockRestore();
    }
  });

  it("selects thinking, tool pair, and assistant text independently", () => {
    const onChange = vi.fn();
    const projection: ConversationProjection = {
      conversationId: "chat-edit-context",
      turnActivity: { kind: "idle" },
      items: [
        {
          kind: "user",
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "继续审查 ADR" }],
        },
        {
          kind: "assistant",
          id: "assistant-response",
          role: "assistant",
          status: "completed",
          output: [
            {
              id: "assistant-thinking",
              role: "assistant",
              type: "reasoning",
              content: [{ type: "text", text: "错误思考" }],
            },
            {
              id: "assistant-text",
              role: "assistant",
              content: [{ type: "text", text: "错误任务" }],
            },
            {
              id: "assistant-tool",
              role: "assistant",
              type: "plugin_call",
              content: [
                {
                  type: "tool",
                  data: {
                    id: "call-1",
                    call_id: "call-1",
                    tool_call_id: "call-1",
                    name: "CodeGraph",
                  },
                },
              ],
            },
            {
              id: "assistant-tool-result",
              role: "tool",
              type: "plugin_call_output",
              content: [
                {
                  type: "tool_result",
                  data: {
                    id: "call-1",
                    call_id: "call-1",
                    tool_call_id: "call-1",
                  },
                },
              ],
            },
          ],
          variants: [],
        },
      ],
    };

    render(
      <ConversationMessageList
        projection={projection}
        contextEditorOpen
        draftExcludedMessageIds={new Set()}
        onDraftExcludedMessageIdsChange={onChange}
      />,
    );

    const checkboxes = screen.getAllByRole("checkbox");
    expect(checkboxes).toHaveLength(3);

    fireEvent.click(checkboxes[0]);
    expect(Array.from(onChange.mock.calls[0][0])).toEqual([
      "assistant-thinking",
    ]);

    fireEvent.click(checkboxes[1]);
    expect(Array.from(onChange.mock.calls[1][0]).sort()).toEqual([
      "assistant-text",
    ]);

    fireEvent.click(checkboxes[2]);
    expect(Array.from(onChange.mock.calls[2][0]).sort()).toEqual([
      "assistant-tool",
      "assistant-tool-result",
    ]);
  });

  beforeEach(() => {
    __resetConversationViewportForTests();
    vi.clearAllMocks();
    vi.unstubAllGlobals();
    sessionStorage.clear();
    HTMLElement.prototype.scrollIntoView = vi.fn();
    mobileMock.enabled = false;
    mobileMock.saveFile.mockResolvedValue({ ok: true, usedPicker: false });
    ttsMock.isSpeaking = false;
    ttsMock.isLoading = false;
    ttsMock.isDisabled = false;
  });

  it("renders owned user and assistant messages with stable DOM hooks", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
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
        {
          id: "user-2",
          role: "user",
          content: [{ type: "text", text: "Follow-up question" }],
        },
        {
          id: "assistant-2",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Short answer" }],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(screen.getByText("Question")).toBeInTheDocument();
    expect(screen.getByText("Answer")).toBeInTheDocument();
    expect(
      container.querySelector('[data-rua-chat-conversation-id="chat-1"]'),
    ).toBeTruthy();
    expect(
      container.querySelector('[data-rua-chat-message-id="user-1"]'),
    ).toBeTruthy();
    expect(
      container.querySelector('[data-rua-chat-role="assistant"]'),
    ).toBeTruthy();
    const turns = container.querySelectorAll("[data-rua-chat-turn]");
    expect(turns).toHaveLength(2);
    expect(container.querySelector(".rua-chat-turn-sticky")).toBeNull();
    expect(
      turns[1].querySelector('[data-rua-chat-message-id="user-2"]'),
    ).toBeTruthy();
  });

  it("does not offer collapse for user input within two lines", () => {
    const scrollHeight = vi
      .spyOn(HTMLElement.prototype, "scrollHeight", "get")
      .mockReturnValue(40);
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "Short user input" }],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );
    const bubble = container.querySelector(
      '[data-rua-chat-message-id="user-1"] .rua-chat-bubble--user',
    );

    expect(
      within(bubble as HTMLElement).queryByRole("button", {
        name: "Collapse user input",
      }),
    ).not.toBeInTheDocument();
    scrollHeight.mockRestore();
  });

  it("collapses and expands complete user input taller than two lines", () => {
    const scrollHeight = vi
      .spyOn(HTMLElement.prototype, "scrollHeight", "get")
      .mockImplementation(function (this: HTMLElement) {
        return this.classList.contains("rua-chat-user-content-frame")
          ? 120
          : 0;
      });
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [
            { type: "text", text: "A long user input" },
            {
              type: "image",
              image_url: "/uploads/picture.png",
              file_name: "picture.png",
            },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );
    const bubble = container.querySelector(
      '[data-rua-chat-message-id="user-1"] .rua-chat-bubble--user',
    );
    const contentFrame = bubble?.querySelector(
      ".rua-chat-user-content-frame",
    );
    const collapse = within(bubble as HTMLElement).getByRole("button", {
      name: "Collapse user input",
    });

    expect(contentFrame).not.toHaveClass("is-collapsed");
    expect(collapse).toHaveClass("rua-chat-user-content-toggle");
    expect(contentFrame?.querySelector("img")).toBeInTheDocument();
    expect(collapse).toHaveAttribute("aria-expanded", "true");

    fireEvent.click(collapse);

    expect(contentFrame).toHaveClass("is-collapsed");
    const expand = within(bubble as HTMLElement).getByRole("button", {
      name: "Expand user input",
    });
    expect(expand).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(expand);

    expect(contentFrame).not.toHaveClass("is-collapsed");
    scrollHeight.mockRestore();
  });

  it("keeps user messages in the normal scroll flow", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "Long question" }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Long answer" }],
        },
        {
          id: "user-2",
          role: "user",
          content: [{ type: "text", text: "Next question" }],
        },
        {
          id: "assistant-2",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Next answer" }],
        },
      ],
    });
    const { container } = render(
      <ConversationMessageList projection={projection} agentId="agent-one" />,
    );
    const list = container.querySelector(
      ".rua-chat-message-list",
    ) as HTMLElement;
    const turns = Array.from(
      list.querySelectorAll<HTMLElement>(".rua-chat-turn"),
    );

    expect(turns).toHaveLength(2);
    for (const turn of turns) {
      expect(turn).not.toHaveAttribute("data-rua-chat-turn-stuck");
      expect(
        turn.style.getPropertyValue("--rua-chat-user-sticky-translate"),
      ).toBe("");
      expect(
        turn.style.getPropertyValue("--chat-render-block-sticky-top"),
      ).toBe("");
    }
  });

  it("keeps image markers in continuous user text and thumbnails below it", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [
            { type: "text", text: "Before " },
            {
              type: "image",
              image_url: "/uploads/picture.png",
              file_name: "picture.png",
            },
            { type: "text", text: " after" },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );
    const content = container.querySelector(".rua-chat-user-content");
    const link = screen.getByRole("link", { name: "[Image #1]" });

    expect(content?.querySelectorAll(".rua-local-markdown")).toHaveLength(1);
    expect(content?.textContent).toContain("Before [Image #1] after");
    expect(link).toHaveAttribute("href", "#rua-image-1");
    expect(
      content?.querySelector(".rua-chat-user-images img"),
    ).toHaveAttribute("src", "/preview/uploads/picture.png");

    fireEvent.click(link);
    expect(screen.getByRole("dialog", { name: "picture.png" })).toBeVisible();
  });

  it("resolves managed media references through the current agent workspace", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "image", media_ref: "media/uuid-image.png" }],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} agentId="agent-one" />,
    );

    expect(
      container.querySelector(".rua-chat-user-images img"),
    ).toHaveAttribute(
      "src",
      "/preview/agents/agent-one/files/preview?path=media%2Fuuid-image.png",
    );
  });

  it.each([
    {
      scope: { kind: "project" as const, projectId: "project-1" },
      ownerPath: "projects/project-1",
    },
    {
      scope: { kind: "agent" as const, agentId: "agent-1" },
      ownerPath: "agents/agent-1",
    },
  ])(
    "reads submitted user images through $ownerPath after reopening history",
    async ({ scope, ownerPath }) => {
      const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue({
        ok: true,
        blob: async () => new Blob(["image"], { type: "image/png" }),
      } as Response);
      const createObjectURL = vi
        .spyOn(URL, "createObjectURL")
        .mockReturnValue("blob:submitted-media");
      const revokeObjectURL = vi.spyOn(URL, "revokeObjectURL");
      const projection = projectConversation("chat-project-1", {
        status: "idle",
        messages: [
          {
            id: "user-1",
            role: "user",
            content: [
              {
                type: "image",
                media_ref: "media/00000000-0000-4000-8000-000000000001",
                file_name: "submitted.png",
              },
            ],
          },
        ],
      });

      const view = (
        <ConversationMessageList projection={projection} mediaScope={scope} />
      );
      const { container, unmount } = render(view);

      await waitFor(() =>
        expect(
          container.querySelector(".rua-chat-user-images img"),
        ).toHaveAttribute("src", "blob:submitted-media"),
      );
      expect(fetchMock).toHaveBeenCalledWith(
        new URL(
          `/api/${ownerPath}/chats/chat-project-1/media/00000000-0000-4000-8000-000000000001`,
          window.location.href,
        ).href,
        expect.objectContaining({
          headers: expect.objectContaining({
            Authorization: expect.any(String),
          }),
        }),
      );
      fireEvent.click(
        screen.getByRole("button", { name: "Open submitted.png" }),
      );
      expect(
        within(screen.getByRole("dialog")).getByAltText("submitted.png"),
      ).toHaveAttribute("src", "blob:submitted-media");
      expect(fetchMock).toHaveBeenCalledTimes(1);
      fireEvent.click(screen.getByRole("button", { name: "Close image" }));

      fireEvent.click(screen.getByRole("link", { name: "[Image #1]" }));
      await waitFor(() =>
        expect(
          within(screen.getByRole("dialog")).getByAltText("submitted.png"),
        ).toHaveAttribute("src", "blob:submitted-media"),
      );
      expect(fetchMock).toHaveBeenCalledTimes(2);
      fireEvent.click(screen.getByRole("button", { name: "Close image" }));
      unmount();
      expect(revokeObjectURL).toHaveBeenCalledWith("blob:submitted-media");

      const reopened = render(view);
      await waitFor(() =>
        expect(
          reopened.container.querySelector(".rua-chat-user-images img"),
        ).toHaveAttribute("src", "blob:submitted-media"),
      );
      expect(fetchMock).toHaveBeenCalledTimes(3);
      reopened.unmount();
      fetchMock.mockRestore();
      createObjectURL.mockRestore();
      revokeObjectURL.mockRestore();
    },
  );

  it("hides legacy model-only image path hints and keeps a compact preview", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-image",
          role: "user",
          content: [
            {
              type: "image",
              url: "/tmp/photo.png",
              alt: "photo.png",
            },
            {
              type: "text",
              text: "用户上传图片 `photo.png`，已经下载到 /tmp/photo.png",
            },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(screen.queryByText(/已经下载到/)).not.toBeInTheDocument();
    expect(screen.getByAltText("photo.png")).toHaveAttribute(
      "src",
      "/preview/tmp/photo.png",
    );
    fireEvent.click(screen.getByRole("button", { name: "Open photo.png" }));
    expect(
      screen.getByRole("dialog", { name: "photo.png" }),
    ).toBeInTheDocument();
    expect(container.querySelector(".rua-chat-bubble--user")).toHaveClass(
      "has-media",
      "is-media-only",
    );
  });

  it("keeps ordered image links in the text and thumbnails below it", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-images",
          role: "user",
          content: [
            { type: "text", text: "Compare these:\n" },
            {
              type: "image",
              url: "/tmp/one.png",
              alt: "one.png",
            },
            { type: "text", text: "\nwith this:\n" },
            {
              type: "image",
              url: "/tmp/two.png",
              alt: "two.png",
            },
          ],
        },
      ],
    });

    render(<ConversationMessageList projection={projection} />);

    const firstMarker = screen.getByRole("link", { name: "[Image #1]" });
    const firstImage = screen.getByRole("img", { name: "one.png" });
    const secondMarker = screen.getByRole("link", { name: "[Image #2]" });
    const secondImage = screen.getByRole("img", { name: "two.png" });
    const markdown = firstMarker.closest(".rua-local-markdown");

    expect(markdown?.textContent).toContain(
      "Compare these:\n[Image #1]\nwith this:\n[Image #2]",
    );
    expect(
      firstMarker.compareDocumentPosition(secondMarker) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      secondMarker.compareDocumentPosition(firstImage) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(firstImage).toHaveClass("rua-chat-image-part--thumbnail");
    expect(secondImage).toHaveClass("rua-chat-image-part--thumbnail");
  });

  it("renders both Markdown and figure managed images through the Project Chat", () => {
    const id = "00000000-0000-4000-8000-000000000001";
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [
            {
              type: "text",
              text: `![Markdown preview](media/${id})\n\n@figure\n- src=media/${id} caption=Figure\n@end`,
            },
          ],
        },
      ],
    });
    render(
      <ConversationMessageList
        projection={projection}
        mediaScope={{ kind: "project", projectId: "project-1" }}
      />,
    );
    for (const name of ["Markdown preview", "Figure"]) {
      expect(screen.getByRole("img", { name })).toHaveAttribute(
        "src",
        `/api/projects/project-1/chats/chat-1/media/${id}?token=test`,
      );
    }
  });

  it("does not read assistant figure paths from the viewing Device", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [
            {
              type: "text",
              text:
                '@figure caption="Latest photo"\n' +
                '  - src=/sdcard/DCIM/Camera/IMG_20260722_215408393_HDR.jpg caption="July 22 photo"\n' +
                "@end",
            },
          ],
        },
      ],
    });

    render(<ConversationMessageList projection={projection} />);

    expect(
      screen.queryByRole("img", { name: "July 22 photo" }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("Image unavailable")).toBeInTheDocument();
  });

  it("keeps network figure sources direct while resolving local sources independently", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [
            {
              type: "text",
              text:
                '@figure columns=2 caption="Mixed photos"\n' +
                '  - src=https://cdn.example/photo.jpg caption="Network photo"\n' +
                '  - src=file:///sdcard/DCIM/Camera/local.jpg caption="Local photo"\n' +
                "@end",
            },
          ],
        },
      ],
    });

    render(<ConversationMessageList projection={projection} />);

    expect(screen.getByRole("img", { name: "Network photo" })).toHaveAttribute(
      "src",
      "https://cdn.example/photo.jpg",
    );
    expect(
      screen.queryByRole("img", { name: "Local photo" }),
    ).not.toBeInTheDocument();
  });

  it("keeps the figure caption and shows an unavailable state when an image fails", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [
            {
              type: "text",
              text:
                '@figure columns=2 caption="Phone album"\n' +
                '  - src=https://cdn.example/available.jpg caption="Available photo"\n' +
                '  - src=https://cdn.example/missing.jpg caption="Missing photo"\n' +
                "@end",
            },
          ],
        },
      ],
    });

    render(<ConversationMessageList projection={projection} />);
    fireEvent.error(screen.getByRole("img", { name: "Missing photo" }));

    expect(screen.getByRole("status")).toHaveTextContent("Image unavailable");
    expect(screen.getByText("Missing photo")).toBeInTheDocument();
    expect(
      screen.getByRole("img", { name: "Available photo" }),
    ).toHaveAttribute("src", "https://cdn.example/available.jpg");
  });

  it("does not refresh settled usage controls when streaming text leaves usage unchanged", () => {
    const project = createConversationProjector();
    const stored = canonicalHistoryFixture({
      messages: [
        { id: "first-user", role: "user", content: "First" },
        {
          id: "first-answer",
          role: "assistant",
          content: "First answer",
          metadata: {
            rua_token_usage: {
              usage: {
                prompt_tokens: 100,
                completion_tokens: 10,
                total_tokens: 110,
              },
              last_call_usage: {
                prompt_tokens: 100,
                completion_tokens: 10,
                total_tokens: 110,
              },
              max_input_length: 1000,
            },
          },
        },
        { id: "live-user", role: "user", content: "Next" },
      ],
    });
    const projected = (text: string) =>
      withActiveTurn(
        project("usage-chat", {
          ...stored,
          messages: [
            ...stored.messages,
            { id: "live-answer", role: "assistant", content: text },
          ],
        }),
      );
    const first = projected("Starting");
    const settled = first.items.find((item) => item.kind === "assistant");
    if (settled?.kind !== "assistant" || !settled.output[0].token_usage)
      throw new Error("Expected stored usage");
    const context = vi.spyOn(
      settled.output[0].token_usage,
      "last_call_usage",
      "get",
    );
    const { rerender } = render(<ConversationMessageList projection={first} />);
    try {
      context.mockClear();
      rerender(
        <ConversationMessageList
          projection={projected("Starting the next response")}
        />,
      );
      expect(context.mock.calls.length).toBe(0);
      expect(
        screen.getByText("Starting the next response"),
      ).toBeInTheDocument();
    } finally {
      context.mockRestore();
    }
  });

  it("renders token usage from assistant message metadata", async () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
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
          metadata: {
            rua_token_usage: {
              usage: {
                prompt_tokens: 10500,
                completion_tokens: 770,
                total_tokens: 11270,
                cache_read_tokens: 4096,
              },
              last_call_usage: {
                prompt_tokens: 962,
                completion_tokens: 70,
                total_tokens: 1032,
                cache_read_tokens: 512,
              },
              max_input_length: 131100,
            },
          },
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(container.querySelector(".rua-turn-usage-ring")).toBeTruthy();
    expect(container.querySelector(".rua-turn-usage-ring svg")).toBeNull();

    fireEvent.mouseEnter(
      screen
        .getByRole("button", { name: "Token usage" })
        .closest(".rua-ui-popover-trigger") as HTMLElement,
    );

    const usageTable = await screen.findByRole("table");
    expect(within(usageTable).getByText("Total")).toBeInTheDocument();
    expect(within(usageTable).getByText("in")).toBeInTheDocument();
    expect(within(usageTable).getByText("out")).toBeInTheDocument();
    expect(within(usageTable).getByText("cached")).toBeInTheDocument();
    expect(within(usageTable).getByText("This turn")).toBeInTheDocument();
    expect(within(usageTable).getAllByText("11.3K")).toHaveLength(2);
    expect(within(usageTable).getAllByText("10.5K")).toHaveLength(2);
    expect(within(usageTable).getAllByText("770")).toHaveLength(2);
    expect(within(usageTable).getAllByText("4.1K")).toHaveLength(2);
    expect(
      within(usageTable).getByText("Conversation total"),
    ).toBeInTheDocument();

    expect(
      screen.getByText(/Context usage at this turn 0\.7%/),
    ).toBeInTheDocument();
    expect(screen.getByText("962/131.1K · cached 512")).toBeInTheDocument();
  });

  it("renders persisted token usage from nested backend metadata", async () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
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
          metadata: {
            original_id: "assistant-1",
            metadata: {
              rua_token_usage: {
                usage: {
                  prompt_tokens: 10500,
                  completion_tokens: 770,
                  total_tokens: 11270,
                  cache_read_tokens: 4096,
                },
                last_call_usage: {
                  prompt_tokens: 962,
                  completion_tokens: 70,
                  total_tokens: 1032,
                  cache_read_tokens: 512,
                },
                max_input_length: 131100,
              },
            },
          },
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(container.querySelector(".rua-turn-usage-ring")).toBeTruthy();

    fireEvent.mouseEnter(
      screen
        .getByRole("button", { name: "Token usage" })
        .closest(".rua-ui-popover-trigger") as HTMLElement,
    );

    const usageTable = await screen.findByRole("table");
    expect(within(usageTable).getByText("This turn")).toBeInTheDocument();
    expect(within(usageTable).getAllByText("11.3K")).toHaveLength(2);
    expect(within(usageTable).getAllByText("4.1K")).toHaveLength(2);
    expect(
      screen.getByText(/Context usage at this turn 0\.7%/),
    ).toBeInTheDocument();
  });

  it("shows the context snapshot captured for each assistant response", async () => {
    const tokenUsage = (promptTokens: number) => ({
      usage: {
        prompt_tokens: promptTokens,
        completion_tokens: 100,
        total_tokens: promptTokens + 100,
      },
      last_call_usage: {
        prompt_tokens: promptTokens,
        completion_tokens: 100,
        total_tokens: promptTokens + 100,
      },
      max_input_length: 10000,
    });
    const projection = projectConversation("chat-1", {
      status: "idle",
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
          metadata: { rua_token_usage: tokenUsage(2000) },
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
          metadata: { rua_token_usage: tokenUsage(7000) },
        },
      ],
    });

    render(<ConversationMessageList projection={projection} />);

    const triggers = screen.getAllByRole("button", { name: "Token usage" });
    const firstTrigger = triggers[0].closest(
      ".rua-ui-popover-trigger",
    ) as HTMLElement;
    const secondTrigger = triggers[1].closest(
      ".rua-ui-popover-trigger",
    ) as HTMLElement;

    fireEvent.mouseEnter(firstTrigger);
    expect(
      await screen.findByText(/Context usage at this turn 20%/),
    ).toBeInTheDocument();

    fireEvent.mouseLeave(firstTrigger);
    await waitFor(() => {
      expect(
        screen.queryByText(/Context usage at this turn 20%/),
      ).not.toBeInTheDocument();
    });

    fireEvent.mouseEnter(secondTrigger);
    expect(
      await screen.findByText(/Context usage at this turn 70%/),
    ).toBeInTheDocument();
  });

  it("keeps hook order stable when an empty chat receives its first message", () => {
    const emptyProjection = projectConversation("chat-1", {
      status: "idle",
      messages: [],
    });
    const firstMessageProjection = projectConversation("chat-1", {
      status: "running",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "First prompt" }],
        },
      ],
    });

    const { rerender } = render(
      <ConversationMessageList projection={emptyProjection} />,
    );

    expect(
      screen.getByText("Hello, how can I help you today?"),
    ).toBeInTheDocument();
    expect(() =>
      rerender(<ConversationMessageList projection={firstMessageProjection} />),
    ).not.toThrow();
    expect(screen.getByText("First prompt")).toBeInTheDocument();
  });

  it("does not animate when a completed live response is replaced by synced history", () => {
    const liveProjection = projectConversation("chat-1", {
      status: "running",
      messages: [
        {
          id: "user-live",
          role: "user",
          content: [{ type: "text", text: "Question" }],
        },
        {
          id: "assistant-live",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Answer" }],
        },
      ],
    });
    const syncedProjection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-live",
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "Question" }],
        },
        {
          id: "assistant-stored",
          role: "assistant",
          type: "message",
          turn_id: "turn-1",
          content: [{ type: "text", text: "Answer" }],
        },
      ],
    });

    const { container, rerender } = render(
      <ConversationMessageList projection={withActiveTurn(liveProjection)} />,
    );

    rerender(<ConversationMessageList projection={syncedProjection} />);

    const transition = container.querySelector(
      ".rua-assistant-output-transition",
    );
    expect(transition).not.toHaveClass("is-prepare");
    expect(transition).not.toHaveClass("is-enter");
  });

  it("does not badge ordinary messages from the current active ACP runner", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-plain",
          role: "user",
          content: [{ type: "text", text: "Plain prompt" }],
        },
        {
          id: "assistant-plain",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Plain answer" }],
        },
      ],
    });
    const props = {
      projection: withActiveTurn(projection),
      activeAcpRunner: "claude",
    } satisfies ComponentProps<typeof ConversationMessageList>;

    const { container } = render(<ConversationMessageList {...props} />);

    expect(screen.getByText("Plain prompt")).toBeInTheDocument();
    expect(screen.getByText("Plain answer")).toBeInTheDocument();
    expect(container.querySelector(".rua-chat-origin-badge--acp")).toBeNull();
  });

  it("badges only messages with persisted ACP origin metadata", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-acp",
          role: "user",
          content: [{ type: "text", text: "ACP prompt" }],
          metadata: { origin: { kind: "acp", runner: "claude" } },
        },
        {
          id: "assistant-acp",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "ACP answer" }],
          metadata: { origin: { kind: "acp", runner: "claude" } },
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(screen.getByText("ACP prompt")).toBeInTheDocument();
    expect(screen.getByText("ACP answer")).toBeInTheDocument();
    const badges = container.querySelectorAll(".rua-chat-origin-badge--acp");
    expect(badges).toHaveLength(2);
    expect(
      Array.from(badges).every((badge) => badge.querySelector("svg") === null),
    ).toBe(true);
  });

  it("renders ACP history preview as a full-width resume history block", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "acp-history-preview-message-1",
          role: "ui",
          type: "acp_history_preview",
          turn_id: "acp-history-preview-1",
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

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(screen.getByText("previous ACP transcript")).toBeInTheDocument();
    expect(
      screen.getByText("History loaded from ACP resume"),
    ).toBeInTheDocument();
    expect(screen.getByText("ACP · codex")).toBeInTheDocument();
    const message = container.querySelector(
      '[data-rua-chat-message-id="acp-history-preview-message-1"]',
    );
    expect(message?.classList.contains("rua-chat-message--assistant")).toBe(
      true,
    );
    const bubble = message?.querySelector(
      ".rua-chat-bubble--acp-history-preview",
    );
    expect(bubble?.classList.contains("rua-chat-bubble--assistant")).toBe(
      true,
    );
    const preview = container.querySelector(
      ".rua-chat-acp-history-preview-scroll",
    );
    expect(preview).toBeTruthy();
    expect(preview?.tagName).toBe("BLOCKQUOTE");
    expect(container.querySelector('[data-rua-chat-role="ui"]')).toBeTruthy();
  });

  it("saves file parts through the mobile bridge instead of opening them in WebView", async () => {
    mobileMock.enabled = true;
    const fetchMock = vi.fn().mockResolvedValue(
      new Response("hello", {
        status: 200,
        headers: {
          "content-disposition": 'attachment; filename="report.txt"',
          "content-type": "text/plain",
        },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [
            {
              type: "file",
              file_url: "/tmp/report.txt",
              file_name: "report.txt",
            },
          ],
        },
      ],
    });

    render(<ConversationMessageList projection={projection} />);
    fireEvent.click(screen.getByRole("link", { name: /report\.txt/ }));

    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith("/preview/tmp/report.txt"),
    );
    expect(mobileMock.saveFile).toHaveBeenCalledWith(
      expect.any(Blob),
      "report.txt",
    );
  });

  it("flashes matching text after search-result navigation", async () => {
    sessionStorage.setItem(
      "rua_search_scroll_target",
      JSON.stringify({ cardIndex: 1, query: "matched" }),
    );
    const projection = projectConversation("chat-1", {
      status: "idle",
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
          content: [{ type: "text", text: "A matched answer" }],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    await waitFor(() => {
      expect(
        container.querySelector(".rua-search-jump-mark"),
      ).toHaveTextContent("matched");
    });
    expect(
      container.querySelector(".rua-chat-message--search-jump"),
    ).toBeTruthy();
    expect(sessionStorage.getItem("rua_search_scroll_target")).toBeNull();
  });

  it("opens lazy reasoning when navigating to a matching search result", async () => {
    const projection = projectConversation("chat-search-lazy", {
      messages: [
        { id: "user-search", role: "user", content: "Question" },
        {
          id: "reasoning-search",
          role: "assistant",
          type: "reasoning",
          content: "The hidden needle explains the result",
        },
        { id: "answer-search", role: "assistant", content: "Final answer" },
      ],
    });
    const { rerender } = render(
      <ConversationMessageList projection={projection} />,
    );
    expect(
      screen.queryByText("The hidden needle explains the result"),
    ).not.toBeInTheDocument();
    sessionStorage.setItem(
      "rua_search_scroll_target",
      JSON.stringify({ cardIndex: 1, query: "hidden needle" }),
    );
    fireEvent(window, new Event("rua-search-scroll"));
    await waitFor(() =>
      expect(
        document.querySelector("mark.rua-search-jump-mark"),
      ).toHaveTextContent("hidden needle"),
    );
    expect(screen.getByText("Thinking").closest("details")).toHaveAttribute(
      "open",
    );
    expect(screen.getByRole("button", { name: /1 thinking/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    rerender(
      <ConversationMessageList
        projection={projectConversation("another-chat", {
          messages: [
            { id: "user-search", role: "user", content: "Question" },
            {
              id: "reasoning-search",
              role: "assistant",
              type: "reasoning",
              content: "A different hidden thought",
            },
            {
              id: "answer-search",
              role: "assistant",
              content: "Another answer",
            },
          ],
        })}
      />,
    );
    expect(
      screen.queryByText("A different hidden thought"),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /1 thinking/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(
      document.querySelector(".rua-search-jump-mark"),
    ).not.toBeInTheDocument();
  });

  it("opens lazy runtime notes when navigating to a matching search result", async () => {
    const projection = projectConversation("chat-search-note", {
      messages: [
        { id: "user-search", role: "user", content: "Question" },
        {
          id: "note-search",
          role: "system",
          type: "runtime_note",
          content: [],
          runtime_note: {
            runtime_note_id: "runtime-note-search",
            kind: "conversation_runtime_terminal",
            created_at: "2026-09-19T00:00:00Z",
            payload: { summary: "The hidden note explains the result" },
          },
        },
      ],
    });
    render(<ConversationMessageList projection={projection} />);
    expect(
      screen.queryByText("The hidden note explains the result"),
    ).not.toBeInTheDocument();
    sessionStorage.setItem(
      "rua_search_scroll_target",
      JSON.stringify({ cardIndex: 1, query: "hidden note" }),
    );
    fireEvent(window, new Event("rua-search-scroll"));
    await waitFor(() =>
      expect(
        document.querySelector("mark.rua-search-jump-mark"),
      ).toHaveTextContent("hidden note"),
    );
    expect(screen.getByText("Conversation").closest("details")).toHaveAttribute(
      "open",
    );
  });

  it("renders an owned empty state", () => {
    const projection = projectConversation("chat-empty", {
      status: "idle",
      messages: [],
    });
    const onStartPrompt = vi.fn();

    const { container } = render(
      <ConversationMessageList
        projection={projection}
        onStartPrompt={onStartPrompt}
      />,
    );

    expect(
      screen.getByText("Hello, how can I help you today?"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "I am a helpful assistant that can help you with your questions.",
      ),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByText("Let's start a new journey!"));
    expect(onStartPrompt).toHaveBeenCalledWith("Let's start a new journey!");
    expect(container.querySelector("[data-rua-chat-empty]")).toBeTruthy();
  });

  it("shows a fallback for completed assistant responses with no output", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        { role: "assistant", type: "message", content: [] },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(
      screen.getByText("The assistant returned no output."),
    ).toBeInTheDocument();
    expect(container.querySelector(".rua-chat-empty-assistant")).toBeTruthy();
    expect(screen.queryByText("Generating")).not.toBeInTheDocument();
  });

  it("does not render reasoning-only completed output as a thinking card", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Tell another joke" },
        {
          id: "reasoning-only",
          role: "assistant",
          type: "reasoning",
          content: [
            { type: "text", text: "Visible answer trapped in thinking" },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(
      screen.getByText("Visible answer trapped in thinking"),
    ).toBeInTheDocument();
    expect(screen.queryByText("Thinking")).not.toBeInTheDocument();
    expect(container.querySelector(".rua-local-reasoning")).toBeNull();
  });

  it("does not render whitespace-only reasoning between tool calls", () => {
    const projection = projectConversation("chat-1", {
      status: "running",
      messages: [
        { role: "user", content: "Run commands" },
        {
          id: "tool-1",
          role: "assistant",
          type: "plugin_call",
          content: [{ type: "tool", data: { name: "Bash" } }],
        },
        {
          id: "blank-thinking",
          role: "assistant",
          type: "reasoning",
          status: "in_progress",
          content: [{ type: "text", text: "\n" }],
        },
        {
          id: "tool-2",
          role: "assistant",
          type: "plugin_call",
          content: [{ type: "tool", data: { name: "Bash" } }],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={withActiveTurn(projection)}
      />,
    );

    expect(screen.getAllByText("Bash")).toHaveLength(2);
    expect(screen.queryByText("Thinking")).not.toBeInTheDocument();
    expect(container.querySelector(".rua-local-reasoning")).toBeNull();
  });

  it("renders runtime notes and variant count without vendor cards", async () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Selected" }],
          turn_id: "turn-1",
          selected_variant_id: "variant-2",
          assistant_response_variants: [
            {
              variant_id: "variant-1",
              output: [{ id: "a1", role: "assistant", content: "First" }],
            },
            {
              variant_id: "variant-2",
              output: [{ id: "a2", role: "assistant", content: "Selected" }],
            },
          ],
        },
        {
          id: "note-1",
          role: "system",
          type: "runtime_note",
          content: [],
          turn_id: "turn-1",
          runtime_note: {
            runtime_note_id: "note-1",
            kind: "task_attempt_terminal",
            created_at: "2026-08-03T00:00:00Z",
            payload: {
              task_id: "task-1",
              runner_name: "Bash",
              terminal_status: "completed",
              output: "finished",
            },
          },
        },
      ],
    } as ReturnType<typeof projectConversation>);

    render(<ConversationMessageList projection={projection} />);

    fireEvent.click(screen.getByText("Task · Bash"));
    await screen.findByText("finished");

    expect(screen.getByText("2/2")).toBeInTheDocument();
    expect(screen.getByText("Task · Bash")).toBeInTheDocument();
    expect(screen.getByText("completed")).toBeInTheDocument();
    expect(screen.getByText("finished")).toBeInTheDocument();
  });

  it("renders task watch output records inside the runtime note", async () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "runtime-watch-1",
          role: "system",
          type: "runtime_note",
          content: [],
          turn_id: "turn-1",
          runtime_note: {
            runtime_note_id: "task-watch:task-1:attempt:1:batch:1",
            kind: "task_watch_output",
            created_at: "2026-08-03T00:00:00Z",
            payload: {
              task_id: "task-1",
              attempt_no: 1,
              records: [
                { source: "stdout", text: "Compiling rua\nFinished tests" },
                { source: "stderr", text: "warning: example" },
              ],
            },
          },
        },
      ],
    } as ReturnType<typeof projectConversation>);

    render(<ConversationMessageList projection={projection} />);

    fireEvent.click(screen.getByText("Runtime note"));
    await screen.findByLabelText("Task output");

    const output = screen.getByLabelText("Task output");
    expect(within(output).getByText("stdout")).toBeInTheDocument();
    expect(within(output).getByText("stderr")).toBeInTheDocument();
    expect(output.querySelector("pre")?.textContent).toBe(
      "Compiling rua\nFinished tests",
    );
    expect(within(output).getByText("warning: example")).toBeInTheDocument();
  });

  it("renders a terminal Shell Task with the original input and final output", async () => {
    const { container } = render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "user",
              id: "user-1",
              role: "user",
              content: [{ type: "text", text: "Run tests" }],
            },
            {
              kind: "assistant",
              id: "assistant-1",
              role: "assistant",
              status: "completed",
              output: [
                {
                  id: "shell-call",
                  role: "assistant",
                  type: "plugin_call",
                  call_id: "call-1",
                  tool_call_id: "call-1",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-1",
                        name: "bash",
                        arguments: {
                          command: "cargo test -p rua-server",
                        },
                      },
                    },
                  ],
                },
                {
                  id: "shell-result",
                  role: "tool",
                  type: "plugin_call_output",
                  call_id: "call-1",
                  tool_call_id: "call-1",
                  content: [
                    {
                      type: "tool_result",
                      data: {
                        id: "call-1",
                        name: "bash",
                        output:
                          "Command is still running. task_id: task-shell-1",
                      },
                    },
                  ],
                },
                {
                  id: "assistant-between-task-and-tool",
                  role: "assistant",
                  type: "message",
                  content: [
                    {
                      type: "text",
                      text: "The background task is still running.",
                    },
                  ],
                },
                {
                  id: "task-note",
                  role: "system",
                  type: "runtime_note",
                  content: [],
                  runtime_note: {
                    runtime_note_id: "task-note",
                    kind: "task_attempt_terminal",
                    created_at: "2026-08-13T00:00:00Z",
                    payload: {
                      task_id: "task-shell-1",
                      attempt_no: 1,
                      terminal_status: "completed",
                      runner_name: "bash",
                      tool_call_id: "call-1",
                      output: "1 test passed",
                    },
                  },
                },
              ],
              variants: [],
            },
          ],
        }}
        blockPreferences={{
          aggregateExpanded: false,
          thinkingExpanded: false,
          toolExpanded: true,
        }}
      />,
    );

    expect(screen.queryByText("Task · bash")).not.toBeInTheDocument();
    expect(screen.queryByText("1 test passed")).not.toBeInTheDocument();
    expect(
      screen.getByText("The background task is still running."),
    ).toBeVisible();
    const toolGroup = screen.getByRole("button", { name: /1 tool call/ });
    expect(toolGroup).not.toHaveAccessibleName(/2 tool calls/);
    fireEvent.click(toolGroup);

    fireEvent.click(screen.getByText("Task · bash"));
    await screen.findByText("1 test passed");

    expect(
      screen.getByText("Command is still running. task_id: task-shell-1"),
    ).toBeInTheDocument();
    const taskCard = container.querySelector(
      '[data-rua-task-id="task-shell-1"]',
    );
    expect(taskCard).toHaveClass(
      "rua-local-task",
      "rua-local-operate-card--sticky",
    );
    expect(taskCard).toHaveTextContent("Task · bash");
    expect(taskCard).toHaveTextContent("cargo test -p rua-server");
    expect(taskCard).toHaveTextContent("1 test passed");
    expect(
      taskCard?.querySelectorAll(".rua-render-block--non-sticky"),
    ).toHaveLength(2);
  });

  it("renders turn-aborted markers as an interrupted status", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "执行一个 sleep 命令" },
        {
          id: "assistant-tool",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: { call_id: "call-1", name: "bash" },
            },
          ],
        },
        {
          id: "turn-aborted",
          role: "system",
          type: "message",
          content:
            "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>",
          metadata: {
            metadata: {
              model_visible_marker: "turn_aborted",
            },
          },
        },
      ],
    });

    render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={projection}
      />,
    );

    expect(screen.getByText("bash")).toBeInTheDocument();
    expect(screen.getByText("用户已中断本轮")).toBeInTheDocument();
    expect(screen.queryByText(/turn_aborted/)).not.toBeInTheDocument();
    expect(
      screen.queryByText(/interrupted the previous turn/),
    ).not.toBeInTheDocument();
  });

  it("pages assistant variants and exposes owned response actions", async () => {
    const onSelectVariant = vi.fn();
    const onRegenerate = vi.fn();
    const onRollback = vi.fn();
    const onBranch = vi.fn();
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: [{ type: "text", text: "Question" }] },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Selected" }],
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
        {
          role: "user",
          turn_id: "turn-2",
          content: [{ type: "text", text: "Follow-up" }],
        },
      ],
    });

    render(
      <ModalProvider>
        <ConversationMessageList
          projection={projection}
          onSelectVariant={onSelectVariant}
          onRegenerate={onRegenerate}
          onRollback={onRollback}
          onBranch={onBranch}
        />
      </ModalProvider>,
    );

    fireEvent.click(screen.getByLabelText("Next variant"));
    expect(onSelectVariant).toHaveBeenCalledWith("turn-1", "variant-2");

    fireEvent.click(screen.getByLabelText("Regenerate"));
    expect(onRegenerate).toHaveBeenCalledWith("turn-1", "Question");

    expect(screen.queryByLabelText("More response actions")).toBeNull();
    const followUp = screen.getByText("Follow-up").closest("article");
    fireEvent.click(
      within(followUp as HTMLElement).getByLabelText("More message actions"),
    );
    fireEvent.click(screen.getByRole("menuitem", { name: "Rollback" }));
    expect(onRollback).not.toHaveBeenCalled();
    const rollbackDialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(rollbackDialog).getByRole("button", { name: "Rollback" }),
    );
    await waitFor(() =>
      expect(onRollback).toHaveBeenCalledWith("turn-2", "Follow-up"),
    );

    await waitFor(() =>
      expect(
        screen.queryByRole("menuitem", { name: "Rollback" }),
      ).not.toBeInTheDocument(),
    );
    fireEvent.click(
      within(followUp as HTMLElement).getByLabelText("More message actions"),
    );
    fireEvent.click(screen.getByRole("menuitem", { name: "Branch" }));
    expect(onBranch).not.toHaveBeenCalled();
    const branchDialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(branchDialog).getByRole("button", { name: "Branch" }),
    );
    await waitFor(() =>
      expect(onBranch).toHaveBeenCalledWith("turn-2", "Follow-up"),
    );
  });

  it("offers rollback and branch from the first user message", async () => {
    const onRollback = vi.fn();
    const onBranch = vi.fn();
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "First question" }],
        },
        {
          role: "assistant",
          turn_id: "turn-1",
          selected_variant_id: "variant-1",
          content: "First answer",
        },
      ],
    });

    render(
      <ModalProvider>
        <ConversationMessageList
          projection={projection}
          onRollback={onRollback}
          onBranch={onBranch}
        />
      </ModalProvider>,
    );

    fireEvent.click(screen.getByLabelText("More message actions"));
    const menu = screen.getByRole("menu");
    expect(menu.closest(".rua-chat-message")).toBeNull();
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("menu")).not.toBeInTheDocument(),
    );
    fireEvent.click(screen.getByLabelText("More message actions"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Rollback" }));
    const rollbackDialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(rollbackDialog).getByRole("button", { name: "Rollback" }),
    );
    await waitFor(() =>
      expect(onRollback).toHaveBeenCalledWith("turn-1", "First question"),
    );

    fireEvent.click(screen.getByLabelText("More message actions"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Branch" }));
    const branchDialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(branchDialog).getByRole("button", { name: "Branch" }),
    );
    await waitFor(() =>
      expect(onBranch).toHaveBeenCalledWith("turn-1", "First question"),
    );
  });

  it("disables response-mutating controls while ACP passthrough is active", () => {
    const onSelectVariant = vi.fn();
    const onRegenerate = vi.fn();
    const onRollback = vi.fn();
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: [{ type: "text", text: "Question" }] },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Selected" }],
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
        {
          role: "user",
          turn_id: "turn-2",
          content: [{ type: "text", text: "Follow-up" }],
        },
      ],
    });

    render(
      <ConversationMessageList
        projection={projection}
        activeAcpRunner="codex"
        onSelectVariant={onSelectVariant}
        onRegenerate={onRegenerate}
        onRollback={onRollback}
      />,
    );

    expect(screen.getByLabelText("Regenerate")).toBeDisabled();
    expect(
      screen
        .getAllByLabelText("More message actions")
        .every((button) => button.hasAttribute("disabled")),
    ).toBe(true);
    expect(screen.getByLabelText("Next variant")).toBeDisabled();

    fireEvent.click(screen.getByLabelText("Regenerate"));
    fireEvent.click(screen.getAllByLabelText("More message actions")[0]);
    fireEvent.click(screen.getByLabelText("Next variant"));

    expect(onRegenerate).not.toHaveBeenCalled();
    expect(onRollback).not.toHaveBeenCalled();
    expect(onSelectVariant).not.toHaveBeenCalled();
  });

  it("allows rollback menu on inactive ACP segment tail while keeping ACP-only controls disabled", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          role: "user",
          content: [{ type: "text", text: "Question" }],
          metadata: {
            origin: {
              kind: "acp",
              runner: "codex",
            },
          },
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Selected" }],
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
        {
          role: "user",
          turn_id: "turn-2",
          content: [{ type: "text", text: "Continue" }],
        },
      ],
    });

    render(<ConversationMessageList projection={projection} />);

    expect(screen.getByLabelText("Regenerate")).toBeDisabled();
    expect(screen.getAllByLabelText("More message actions")).toHaveLength(2);
    expect(
      screen
        .getAllByLabelText("More message actions")
        .every((button) => !button.hasAttribute("disabled")),
    ).toBe(true);
    expect(screen.getByLabelText("Next variant")).toBeDisabled();
  });

  it("disables rollback menu inside an inactive ACP segment but enables it at the tail", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          role: "user",
          content: [{ type: "text", text: "C" }],
          metadata: { origin: { kind: "acp", runner: "codex" } },
        },
        {
          id: "assistant-c",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "C ACP" }],
          turn_id: "turn-c",
          selected_variant_id: "assistant-c",
          metadata: { origin: { kind: "acp", runner: "codex" } },
        },
        {
          role: "user",
          content: [{ type: "text", text: "D" }],
          metadata: { origin: { kind: "acp", runner: "codex" } },
        },
        {
          id: "assistant-d",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "D ACP" }],
          turn_id: "turn-d",
          selected_variant_id: "assistant-d",
          metadata: { origin: { kind: "acp", runner: "codex" } },
        },
        { role: "user", content: [{ type: "text", text: "F" }] },
        {
          id: "assistant-f",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "F normal" }],
          turn_id: "turn-f",
          selected_variant_id: "assistant-f",
        },
      ],
    });

    render(<ConversationMessageList projection={projection} />);

    const moreButtons = screen.getAllByLabelText("More message actions");
    expect(moreButtons).toHaveLength(3);
    expect(moreButtons[0]).not.toBeDisabled();
    expect(moreButtons[1]).toBeDisabled();
    expect(moreButtons[2]).not.toBeDisabled();
  });

  it("exposes owned response actions when turn metadata exists without variants", () => {
    const onRegenerate = vi.fn();
    const onRollback = vi.fn();
    const onBranch = vi.fn();
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: [{ type: "text", text: "Question" }] },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Answer" }],
          turn_id: "turn-1",
          selected_variant_id: "variant-1",
        },
        {
          role: "user",
          turn_id: "turn-2",
          content: [{ type: "text", text: "Follow-up" }],
        },
      ],
    });

    render(
      <ModalProvider>
        <ConversationMessageList
          projection={projection}
          onRegenerate={onRegenerate}
          onRollback={onRollback}
          onBranch={onBranch}
        />
      </ModalProvider>,
    );

    fireEvent.click(screen.getByLabelText("Regenerate"));
    expect(onRegenerate).toHaveBeenCalledWith("turn-1", "Question");
    expect(screen.getAllByLabelText("More message actions")).toHaveLength(2);
    expect(screen.queryByText("1/1")).not.toBeInTheDocument();
  });

  it("keeps the response actions menu open across the portal gap until dismissed", async () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          role: "assistant",
          content: "Answer",
          turn_id: "turn-1",
          selected_variant_id: "variant-1",
          assistant_response_variants: [
            {
              variant_id: "variant-1",
              output: [{ role: "assistant", content: "Answer" }],
            },
          ],
        },
        { role: "user", turn_id: "turn-2", content: "Follow-up" },
      ],
    });

    render(
      <ModalProvider>
        <ConversationMessageList projection={projection} />
      </ModalProvider>,
    );

    const followUp = screen.getByText("Follow-up").closest("article");
    fireEvent.click(
      within(followUp as HTMLElement).getByLabelText("More message actions"),
    );
    expect(
      screen.getByRole("menuitem", { name: "Rollback" }),
    ).toBeInTheDocument();

    const menu = followUp?.querySelector(".rua-ui-popover-trigger");
    fireEvent.mouseLeave(menu!);

    expect(
      screen.getByRole("menuitem", { name: "Rollback" }),
    ).toBeInTheDocument();

    fireEvent.pointerDown(document.body);
    await waitFor(() =>
      expect(
        screen.queryByRole("menuitem", { name: "Rollback" }),
      ).not.toBeInTheDocument(),
    );
  });

  it("renders assistant footer outside the scrollable bubble so hover actions are not clipped", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        { role: "assistant", content: "Answer" },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    const assistant = container.querySelector(
      '.rua-chat-message--assistant[data-rua-chat-role="assistant"]',
    );
    const bubble = assistant?.querySelector(".rua-chat-bubble--assistant");
    const footerRow = assistant?.querySelector(
      ".rua-chat-response-footer-row",
    );

    expect(assistant).toBeTruthy();
    expect(bubble).toBeTruthy();
    expect(footerRow).toBeTruthy();
    expect(bubble?.contains(footerRow)).toBe(false);
  });

  it("keeps assistant actions on the left and variant pager on the right", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          role: "assistant",
          content: "Selected",
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
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    const footerRow = container.querySelector(
      ".rua-chat-response-footer-row",
    );
    expect(footerRow?.children[0]).toHaveClass("rua-local-bubble-footer");
    expect(footerRow?.children[1]).toHaveClass("rua-response-variant-pager");
  });

  it("hides the variant pager while the assistant is generating", () => {
    const projection = projectConversation("chat-1", {
      status: "running",
      messages: [
        { role: "user", content: "Question" },
        {
          role: "assistant",
          content: "Streaming selected",
          status: "generating",
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
                { id: "a2", role: "assistant", content: "Streaming selected" },
              ],
            },
            {
              variant_id: "variant-3",
              output: [{ id: "a3", role: "assistant", content: "Third" }],
            },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={withActiveTurn(projection)} />,
    );

    const footerRow = container.querySelector(
      ".rua-chat-response-footer-row",
    );
    expect(footerRow).toBeNull();
    expect(container.querySelector(".rua-response-variant-pager")).toBeNull();
  });

  it("speaks assistant text through the owned response actions", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        { role: "assistant", content: "Answer" },
      ],
    });

    render(<ConversationMessageList projection={projection} />);

    fireEvent.click(screen.getByLabelText("Speak"));
    expect(ttsMock.speak).toHaveBeenCalledWith("Answer");
  });

  it("pairs delayed tool outputs by call id instead of rendering a tail tool group", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-tool-message-id",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                name: "Bash",
                arguments: '{"command":"echo hi"}',
              },
            },
          ],
        },
        {
          id: "assistant-answer",
          role: "assistant",
          type: "message",
          content: "Answer after tool",
        },
        {
          id: "system-output-message-id",
          role: "system",
          type: "plugin_call_output",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                name: "Bash",
                output: "hi",
              },
            },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList
        blockPreferences={{ ...EXPANDED_BLOCKS, aggregateExpanded: false }}
        projection={projection}
      />,
    );

    expect(container.querySelectorAll(".rua-tool-group")).toHaveLength(1);
    expect(screen.getByText("Answer after tool")).toBeInTheDocument();
    fireEvent.click(screen.getByText("1 tool call"));
    const inputBlock = screen.getByLabelText("Input code block");
    const outputBlock = screen.getByLabelText("Output code block");
    expect(within(inputBlock).getByText("Input")).toBeInTheDocument();
    expect(within(outputBlock).getByText("Output")).toBeInTheDocument();
    expect(screen.getByText("hi")).toBeInTheDocument();
  });

  it("pairs a tool output that arrives before its call by tool call id", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "tool-output-first",
          role: "tool",
          type: "plugin_call_output",
          call_id: "call-1",
          content: [
            {
              type: "tool_result",
              data: {
                call_id: "call-1",
                name: "Bash",
                output: "correct output",
              },
            },
          ],
        },
        {
          id: "tool-call-second",
          role: "assistant",
          type: "plugin_call",
          call_id: "call-1",
          content: [
            {
              type: "tool",
              data: {
                call_id: "call-1",
                name: "Bash",
                arguments: '{"command":"echo correct"}',
              },
            },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={projection}
      />,
    );

    expect(
      container.querySelectorAll('[data-rua-tool-call-id="call-1"]'),
    ).toHaveLength(1);
    expect(screen.getByLabelText("Input code block")).toHaveTextContent(
      '"command": "echo correct"',
    );
    expect(screen.getByLabelText("Output code block")).toHaveTextContent(
      "correct output",
    );
  });

  it("renders only the latest valid update_plan checklist in a turn", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Implement the harness change" },
        {
          id: "plan-call-1",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "tool",
              data: {
                id: "plan-call-1",
                name: "update_plan",
                arguments: JSON.stringify({
                  explanation: "Starting investigation.",
                  plan: [
                    { step: "Old investigation step", status: "in_progress" },
                    { step: "Old verification step", status: "pending" },
                  ],
                }),
              },
            },
          ],
        },
        {
          id: "plan-result-1",
          role: "system",
          type: "plugin_call_output",
          content: [
            {
              type: "tool_result",
              data: {
                id: "plan-call-1",
                name: "update_plan",
                output: "Plan updated",
              },
            },
          ],
        },
        {
          id: "plan-call-2",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "tool",
              data: {
                id: "plan-call-2",
                name: "update_plan",
                arguments: {
                  explanation: "Implementation is complete.",
                  plan: [
                    { step: "Implement the change", status: "completed" },
                    { step: "Run focused tests", status: "in_progress" },
                  ],
                },
              },
            },
          ],
        },
        {
          id: "plan-result-2",
          role: "system",
          type: "plugin_call_output",
          content: [
            {
              type: "tool_result",
              data: {
                id: "plan-call-2",
                name: "update_plan",
                output: "Plan updated",
              },
            },
          ],
        },
        {
          id: "assistant-answer",
          role: "assistant",
          type: "message",
          content: "Tests are running.",
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(
      container.querySelectorAll("[data-rua-work-checklist]"),
    ).toHaveLength(1);
    expect(screen.getByText("Implementation is complete.")).toBeInTheDocument();
    expect(screen.getByText("Implement the change")).toBeInTheDocument();
    expect(screen.getByText("Run focused tests")).toBeInTheDocument();
    expect(
      container.querySelector(".lucide-arrow-right-to-line"),
    ).toBeInTheDocument();
    expect(
      container.querySelector(".lucide-loader-circle"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("Starting investigation."),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("Old investigation step"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("update_plan")).not.toBeInTheDocument();
    expect(screen.getByText("Tests are running.")).toBeInTheDocument();
  });

  it("uses the paired tool output name when the original call name is missing", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-tool-message-id",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                arguments: '{"path":"README.md"}',
              },
            },
          ],
        },
        {
          id: "system-output-message-id",
          role: "system",
          type: "plugin_call_output",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                name: "Read File",
                output: "file contents",
              },
            },
          ],
        },
      ],
    });

    render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={projection}
      />,
    );

    expect(screen.getByText("Read File")).toBeInTheDocument();
    expect(screen.queryByText("Call tool")).not.toBeInTheDocument();
  });

  it("does not treat assistant created_at as a lifecycle finish time", () => {
    const withLifecycle = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: "Answer",
          created_at: "2026-07-03T03:50:13Z",
          started_at: "2026-07-03T03:50:01Z",
          finished_at: "2026-07-03T03:50:13Z",
        },
      ],
    });
    const { container, rerender } = render(
      <ConversationMessageList projection={withLifecycle} />,
    );

    expect(container.querySelectorAll(".rua-chat-time")).toHaveLength(2);

    const onlyCreatedAt = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: "Answer",
          created_at: "2026-07-03T03:50:13Z",
        },
      ],
    });
    rerender(<ConversationMessageList projection={onlyCreatedAt} />);

    expect(container.querySelectorAll(".rua-chat-time")).toHaveLength(0);
  });

  it("keeps lifecycle timestamps visible for a selected regenerated variant", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-2",
          role: "assistant",
          type: "message",
          content: "Regenerated",
          turn_id: "turn-1",
          selected_variant_id: "variant-2",
          started_at: "2026-07-03T03:50:21Z",
          finished_at: "2026-07-03T03:50:34Z",
          assistant_response_variants: [
            {
              variant_id: "variant-1",
              output: [
                {
                  id: "assistant-1",
                  role: "assistant",
                  type: "message",
                  content: "Original",
                },
              ],
            },
            {
              variant_id: "variant-2",
              output: [
                {
                  id: "assistant-2",
                  role: "assistant",
                  type: "message",
                  content: "Regenerated",
                },
              ],
            },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(container.querySelectorAll(".rua-chat-time")).toHaveLength(2);
  });

  it("clears tap-revealed floating controls after three seconds", () => {
    vi.useFakeTimers();
    try {
      const projection = projectConversation("chat-1", {
        status: "idle",
        messages: [
          { id: "user-1", role: "user", content: "Question" },
          { id: "assistant-1", role: "assistant", content: "Answer" },
        ],
      });

      const { container } = render(
        <ConversationMessageList projection={projection} />,
      );
      const userMessage = container.querySelector(
        '.rua-chat-message[data-rua-chat-message-id="user-1"]',
      );

      expect(userMessage).toBeTruthy();
      fireEvent.click(userMessage!);
      expect(userMessage).toHaveAttribute("data-rua-tap-reveal", "true");

      act(() => {
        vi.advanceTimersByTime(3_000);
      });

      expect(userMessage).not.toHaveAttribute("data-rua-tap-reveal");
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the tool group, open details, and code block state when later steps arrive", async () => {
    const firstCall = {
      id: "call-message-1",
      role: "assistant",
      type: "plugin_call",
      content: [
        {
          type: "tool",
          data: {
            call_id: "call-1",
            name: "Read",
            arguments: { path: "notes.txt" },
          },
        },
      ],
    };
    const nextCall = {
      id: "call-message-2",
      role: "assistant",
      type: "plugin_call",
      content: [
        {
          type: "tool",
          data: {
            call_id: "call-2",
            name: "Write",
            arguments: { path: "copy.txt" },
          },
        },
      ],
    };
    const initial = projectConversation("chat-stable-group", {
      status: "idle",
      messages: [firstCall],
    });
    const { container, rerender } = render(
      <ConversationMessageList projection={withActiveTurn(initial)} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /1 tool call/ }));
    fireEvent.click(screen.getByText("Read"));
    const input = await screen.findByLabelText("Input code block");
    fireEvent.click(
      within(input).getByRole("button", { name: "Collapse block" }),
    );
    const group = container.querySelector(".rua-tool-group");
    const details = screen.getByText("Read").closest("details");

    const appended = projectConversation("chat-stable-group", {
      status: "idle",
      messages: [firstCall, nextCall],
    });
    rerender(<ConversationMessageList projection={withActiveTurn(appended)} />);
    expect(container.querySelector(".rua-tool-group")).toBe(group);
    expect(group).toHaveClass("pinned-open");
    expect(screen.getByText("Read").closest("details")).toBe(details);
    expect(details).toHaveAttribute("open");
    expect(screen.getByLabelText("Input code block")).toBe(input);
    expect(input).toHaveAttribute("data-collapsed", "true");
    expect(screen.getByText("Write")).toBeInTheDocument();

    rerender(<ConversationMessageList projection={appended} />);
    expect(container.querySelector(".rua-tool-group")).toBe(group);
    expect(group).toHaveClass("pinned-open");
    expect(details).toHaveAttribute("open");
    expect(input).toHaveAttribute("data-collapsed", "true");
  });

  it("does not reformat unchanged expanded tool results during later updates", () => {
    const output = { text: "Retained full output" };
    const projection = (text: string) =>
      projectConversation("chat-format", {
        messages: [
          {
            id: "call",
            role: "assistant",
            type: "plugin_call",
            content: [
              {
                type: "tool",
                data: {
                  call_id: "call",
                  name: "Read",
                  arguments: { path: "file" },
                },
              },
            ],
          },
          {
            id: "result",
            role: "tool",
            type: "plugin_call_output",
            content: [
              { type: "tool_result", data: { call_id: "call", output } },
            ],
          },
          { id: "answer", role: "assistant", content: text },
        ],
      });
    const { rerender } = render(
      <ConversationMessageList
        projection={projection("Start")}
        blockPreferences={EXPANDED_BLOCKS}
      />,
    );
    const stringify = vi.spyOn(JSON, "stringify");
    try {
      rerender(
        <ConversationMessageList
          projection={projection("Continue")}
          blockPreferences={EXPANDED_BLOCKS}
        />,
      );
      expect(screen.getByLabelText("Output code block")).toHaveTextContent(
        "Retained full output",
      );
      expect(stringify.mock.calls.some(([value]) => value === output)).toBe(
        false,
      );
    } finally {
      stringify.mockRestore();
    }
  });

  it("keeps a manually opened tool group open across rerenders", () => {
    const history = {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-tool-message-id",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                name: "Bash",
                arguments: '{"command":"echo hi"}',
              },
            },
          ],
        },
        {
          id: "assistant-answer",
          role: "assistant",
          type: "message",
          content: "Answer after tool",
        },
      ],
    };
    const projection = projectConversation("chat-1", history);

    const { container, rerender } = render(
      <ConversationMessageList
        blockPreferences={{ ...EXPANDED_BLOCKS, aggregateExpanded: false }}
        projection={projection}
      />,
    );

    fireEvent.click(screen.getByText("1 tool call"));
    expect(
      container.querySelector(".rua-tool-group.pinned-open"),
    ).toBeTruthy();

    rerender(
      <ConversationMessageList
        blockPreferences={{ ...EXPANDED_BLOCKS, aggregateExpanded: false }}
        projection={projectConversation("chat-1", history)}
      />,
    );

    expect(
      container.querySelector(".rua-tool-group.pinned-open"),
    ).toBeTruthy();
    expect(screen.getByLabelText("Input code block")).toBeInTheDocument();
  });

  it("omits outer JSON containers from tool input and single-object output display", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-tool-message-id",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                name: "bash",
                arguments: {
                  command: "find ~/MyProjects/rua-rust -type f",
                },
              },
            },
          ],
        },
        {
          id: "system-output-message-id",
          role: "system",
          type: "plugin_call_output",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                name: "bash",
                output: [{ text: "/tmp/a.rs", type: "text" }],
              },
            },
          ],
        },
      ],
    });

    const { container } = render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={projection}
      />,
    );

    const codeBlocks = Array.from(
      container.querySelectorAll(".rua-local-code-block code"),
    ).map((node) => node.textContent);

    expect(codeBlocks).toEqual([
      '"command": "find ~/MyProjects/rua-rust -type f"',
      '"text": "/tmp/a.rs",\n"type": "text"',
    ]);
    expect(codeBlocks.join("\n")).not.toContain("{");
    expect(codeBlocks.join("\n")).not.toContain("}");
    expect(codeBlocks.join("\n")).not.toContain("[");
    expect(codeBlocks.join("\n")).not.toContain("]");
  });

  it("does not expose manual background transfer on generating tool calls", () => {
    render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: {
            kind: "active",
            turnId: null,
            userMessageId: null,
            responseId: "assistant-1",
          },
          items: [
            {
              kind: "user",
              id: "user-1",
              role: "user",
              content: [{ type: "text", text: "Run slow tool" }],
            },
            {
              kind: "assistant",
              id: "assistant-1",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "tool-call-1",
                  role: "assistant",
                  type: "plugin_call",
                  content: [{ type: "tool", data: { name: "Bash" } }],
                },
              ],
              variants: [],
            },
          ],
        }}
        blockPreferences={{
          aggregateExpanded: true,
          thinkingExpanded: false,
          toolExpanded: true,
        }}
      />,
    );

    expect(
      screen.queryByRole("button", { name: "chat.backgroundTool" }),
    ).not.toBeInTheDocument();
  });

  it("groups thinking and tool blocks while the assistant is generating", () => {
    const { container } = render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: {
            kind: "active",
            turnId: null,
            userMessageId: null,
            responseId: "assistant-1",
          },
          items: [
            {
              kind: "assistant",
              id: "assistant-1",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "thinking-1",
                  role: "assistant",
                  type: "reasoning",
                  status: "in_progress",
                  content: [{ type: "text", text: "thinking..." }],
                },
                {
                  id: "tool-call-1",
                  role: "assistant",
                  type: "plugin_call",
                  status: "in_progress",
                  content: [{ type: "tool", data: { name: "Bash" } }],
                },
                {
                  id: "assistant-text-1",
                  role: "assistant",
                  type: "message",
                  content: [{ type: "text", text: "visible answer" }],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    const group = container.querySelector(".rua-tool-group");
    expect(group).toBeTruthy();
    expect(group).toHaveClass("collapsed");
    expect(group).not.toHaveClass("is-generating");
    expect(group?.querySelector(".rua-tool-group-spinner")).toBeNull();
    expect(
      screen.getByRole("button", { name: /1 thinking/ }),
    ).toHaveTextContent("1 thinking, 1 tool call");
    expect(screen.getByText("visible answer")).toBeInTheDocument();
    expect(screen.queryByText("thinking...")).not.toBeInTheDocument();
  });

  it("only shows the group spinner on the trailing tool group while generating", () => {
    const { container } = render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: {
            kind: "active",
            turnId: null,
            userMessageId: null,
            responseId: "assistant-1",
          },
          items: [
            {
              kind: "assistant",
              id: "assistant-1",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "thinking-1",
                  role: "assistant",
                  type: "reasoning",
                  status: "completed",
                  content: [{ type: "text", text: "first thinking" }],
                },
                {
                  id: "assistant-text-1",
                  role: "assistant",
                  type: "message",
                  content: [{ type: "text", text: "visible answer" }],
                },
                {
                  id: "thinking-2",
                  role: "assistant",
                  type: "reasoning",
                  status: "in_progress",
                  content: [{ type: "text", text: "second thinking" }],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    const groups = Array.from(container.querySelectorAll(".rua-tool-group"));
    expect(groups).toHaveLength(2);
    expect(groups[0]).not.toHaveClass("is-generating");
    expect(groups[0].querySelector(".rua-tool-group-spinner")).toBeNull();
    expect(groups[1]).toHaveClass("is-generating");
    expect(groups[1].querySelector(".rua-tool-group-spinner")).toBeTruthy();
  });

  it("renders approval requests on the matching tool card", () => {
    render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-approval",
              role: "assistant",
              status: "idle",
              output: [
                {
                  id: "call-1",
                  role: "assistant",
                  type: "plugin_call",
                  status: "approval_pending",
                  call_id: "call-1",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-1",
                        name: "Bash",
                        arguments: { cmd: "rm -rf tmp" },
                      },
                    },
                  ],
                },
                {
                  id: "approval-1",
                  role: "assistant",
                  type: "tool_approval_request",
                  status: "in_progress",
                  content: [
                    {
                      type: "tool_approval_request",
                      data: {
                        approval_request_id: "approval-1",
                        call_id: "call-1",
                        name: "Bash",
                        arguments: { cmd: "rm -rf tmp" },
                        prompt: "Bash requires confirmation",
                        status: "waiting",
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    expect(screen.getByText("Pending")).toBeInTheDocument();
    expect(screen.getByText("Bash")).toBeInTheDocument();
    expect(screen.getByText("Bash requires confirmation")).toBeInTheDocument();
    expect(screen.queryByText("1 tool call")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Run once" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Reject" }),
    ).not.toBeInTheDocument();
  });

  it("does not render a separate approval card when the authority snapshot already attaches it to the tool", () => {
    const { container } = render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-approval",
              role: "assistant",
              status: "idle",
              output: [
                {
                  id: "call-1",
                  role: "assistant",
                  type: "plugin_call",
                  status: "in_progress",
                  call_id: "call-1",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-1",
                        name: "Bash",
                        arguments: {
                          command: "du -sh /Users/example/projects/rua",
                        },
                      },
                    },
                  ],
                },
                {
                  id: "approval-1",
                  role: "assistant",
                  type: "tool_approval_request",
                  status: "in_progress",
                  content: [
                    {
                      type: "tool_approval_request",
                      data: {
                        approval_request_id: "approval-1",
                        call_id: "call-1",
                        name: "Bash",
                        prompt: "Tool 'Bash' requires approval (Balanced)",
                        status: "waiting",
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
        approvalSnapshot={{
          pending: [
            {
              id: "approval-1",
              agent_id: "agent-a",
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
                display: "Bash du -sh /Users/example/projects/rua",
                args: {
                  command: "du -sh /Users/example/projects/rua",
                },
              },
              created_at: "2026-06-21T00:00:00Z",
              reuse_options: [],
            },
          ],
          resolved: [],
        }}
      />,
    );

    expect(
      container.querySelectorAll('[data-rua-tool-call-id="call-1"]'),
    ).toHaveLength(1);
    expect(
      container.querySelectorAll('[data-rua-approval-id="approval-1"]'),
    ).toHaveLength(1);
  });

  it("describes approval-mode reasons without exposing raw backend prompts", () => {
    render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-approval-reason",
              role: "assistant",
              status: "idle",
              output: [
                {
                  id: "call-1",
                  role: "assistant",
                  type: "plugin_call",
                  status: "approval_pending",
                  call_id: "call-1",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-1",
                        name: "Bash",
                        arguments: { command: "git status --short" },
                        approval: {
                          approval_request_id: "approval-balanced",
                          call_id: "call-1",
                          name: "Bash",
                          status: "pending",
                          trigger: {
                            source: "approval_mode",
                            message: "Tool 'Bash' requires approval (Balanced)",
                          },
                        },
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    expect(
      screen.getByText(
        "Balanced mode asks before shell, write/edit, MCP/plugin, and web tool calls.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("Tool 'Bash' requires approval (Balanced)"),
    ).not.toBeInTheDocument();
  });

  it("does not render approval events as standalone tool cards", () => {
    render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-orphan-approval",
              role: "assistant",
              status: "idle",
              output: [
                {
                  id: "approval-2",
                  role: "assistant",
                  type: "tool_approval_request",
                  status: "in_progress",
                  content: [
                    {
                      type: "tool_approval_request",
                      data: {
                        approval_request_id: "approval-2",
                        call_id: "call-2",
                        name: "Bash",
                        arguments: { command: "rm -rf tmp" },
                        prompt: "Rule matched destructive command",
                        status: "waiting",
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    expect(screen.queryByText("Bash")).not.toBeInTheDocument();
    expect(screen.queryByText("Pending")).not.toBeInTheDocument();
    expect(
      screen.queryByText("Rule matched destructive command"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Tool")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Run once" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Reject" }),
    ).not.toBeInTheDocument();
  });

  it("uses block preferences instead of forcing approval tool groups open", () => {
    const { container } = render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-multiple-approvals",
              role: "assistant",
              status: "completed",
              output: [
                {
                  id: "call-approved",
                  role: "assistant",
                  type: "plugin_call",
                  status: "approval_pending",
                  call_id: "call-approved",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-approved",
                        name: "Bash",
                        arguments: { cmd: "ls" },
                        approval: {
                          approval_request_id: "approval-approved",
                          call_id: "call-approved",
                          name: "Bash",
                          status: "approved",
                          prompt: "Already approved",
                        },
                      },
                    },
                  ],
                },
                {
                  id: "call-waiting",
                  role: "assistant",
                  type: "plugin_call",
                  status: "approval_pending",
                  call_id: "call-waiting",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-waiting",
                        name: "Bash",
                        arguments: { cmd: "rm -rf tmp" },
                        approval: {
                          approval_request_id: "approval-waiting",
                          call_id: "call-waiting",
                          name: "Bash",
                          status: "waiting",
                          prompt: "Rule matched destructive command",
                        },
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
        blockPreferences={{
          aggregateExpanded: true,
          thinkingExpanded: false,
          toolExpanded: true,
        }}
      />,
    );

    expect(screen.getByText(/2 tool calls/)).toHaveTextContent("1 pending");
    expect(screen.getByText(/2 tool calls/)).toHaveTextContent("1 approved");
    expect(
      container.querySelector(".rua-tool-group.pinned-open"),
    ).not.toBeNull();
    expect(
      screen.getByText("Rule matched destructive command"),
    ).toBeInTheDocument();
    expect(screen.getAllByText("Bash")).toHaveLength(2);
    expect(screen.getByText("Approved")).toBeInTheDocument();
    expect(screen.getByText("Pending")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Run once" }),
    ).not.toBeInTheDocument();
  });

  it("attaches approval authority snapshots only to matching tool calls", () => {
    const { container } = render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-snapshot-approval",
              role: "assistant",
              status: "completed",
              output: [
                {
                  id: "call-pending",
                  role: "assistant",
                  type: "plugin_call",
                  status: "approval_pending",
                  call_id: "call-pending",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-pending",
                        name: "Bash",
                        arguments: { command: "git status --short" },
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
        approvalSnapshot={{
          pending: [
            {
              id: "approval-pending",
              agent_id: "agent-a",
              chat_id: "chat-1",
              turn_id: "turn-1",
              tool_call_id: "call-pending",
              tool_name: "Bash",
              status: "pending",
              trigger: {
                source: "approval_mode",
                message: "Balanced requires approval before running Bash",
              },
              tool_call: {
                display: "Run git status --short",
                args: { command: "git status --short" },
              },
              created_at: "2026-06-21T00:00:00Z",
              reuse_options: [],
            },
          ],
          resolved: [
            {
              id: "approval-approved",
              agent_id: "agent-a",
              chat_id: "chat-1",
              turn_id: "turn-1",
              tool_call_id: "call-approved",
              tool_name: "Bash",
              status: "approved",
              tool_call: {
                display: "Run git status",
                args: { command: "git status" },
              },
              created_at: "2026-06-21T00:00:00Z",
              reuse_options: [],
            },
            {
              id: "approval-denied",
              agent_id: "agent-a",
              chat_id: "chat-1",
              turn_id: "turn-1",
              tool_call_id: "call-denied",
              tool_name: "Read",
              status: "denied",
              tool_call: {
                display: "Read ~/.ssh/id_rsa",
                args: { path: "~/.ssh/id_rsa" },
              },
              created_at: "2026-06-21T00:00:00Z",
              reuse_options: [],
            },
          ],
        }}
      />,
    );

    expect(
      screen.getByText("Balanced requires approval before running Bash"),
    ).toBeInTheDocument();
    expect(screen.getByText("Approval Mode")).toBeInTheDocument();
    expect(screen.queryByText("Run git status")).not.toBeInTheDocument();
    expect(screen.queryByText("Read ~/.ssh/id_rsa")).not.toBeInTheDocument();
    expect(screen.queryByText("Denied")).not.toBeInTheDocument();
    expect(screen.getByText("Pending")).toBeInTheDocument();
    expect(
      container.querySelector('[data-rua-tool-call-id="call-pending"]'),
    ).toBeTruthy();
    expect(
      container.querySelector('[data-rua-tool-call-id="call-approved"]'),
    ).toBeNull();
    expect(
      container.querySelector('[data-rua-tool-call-id="call-denied"]'),
    ).toBeNull();
    expect(screen.queryByLabelText("Approval queue")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Run once" }),
    ).not.toBeInTheDocument();
  });

  it("does not synthesize snapshot-only approval cards in the message list", () => {
    const { container } = render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "user",
              id: "user-1",
              role: "user",
              content: [{ type: "text", text: "First prompt" }],
            },
            {
              kind: "assistant",
              id: "assistant-1",
              role: "assistant",
              status: "completed",
              output: [
                {
                  id: "answer-1",
                  role: "assistant",
                  type: "message",
                  content: [{ type: "text", text: "First answer" }],
                },
              ],
              variants: [],
            },
            {
              kind: "user",
              id: "user-2",
              role: "user",
              content: [{ type: "text", text: "Second prompt" }],
            },
            {
              kind: "assistant",
              id: "assistant-2",
              role: "assistant",
              status: "completed",
              output: [
                {
                  id: "answer-2",
                  role: "assistant",
                  type: "message",
                  content: [{ type: "text", text: "Second answer" }],
                },
              ],
              variants: [],
            },
          ],
        }}
        approvalSnapshot={{
          pending: [
            {
              id: "approval-orphan",
              agent_id: "agent-a",
              chat_id: "chat-1",
              turn_id: "turn-2",
              tool_call_id: "call-orphan",
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
        }}
      />,
    );

    expect(
      container.querySelectorAll('[data-rua-approval-id="approval-orphan"]'),
    ).toHaveLength(0);
  });

  it("renders ask_user requests as answer cards instead of approval cards", () => {
    const onStartPrompt = vi.fn();

    render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-ask-user",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "call-ask-1",
                  role: "assistant",
                  type: "plugin_call",
                  status: "completed",
                  call_id: "call-ask-1",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-ask-1",
                        name: "ask_user",
                        arguments: {
                          questions: [
                            {
                              question: "Which option?",
                              options: ["A", "B"],
                              type: "single_select",
                            },
                          ],
                        },
                      },
                    },
                  ],
                },
                {
                  id: "ask-1",
                  role: "assistant",
                  type: "ask_user_request",
                  status: "in_progress",
                  content: [
                    {
                      type: "ask_user_request",
                      data: {
                        request_id: "ask-1",
                        questions: [
                          {
                            question: "Which option?",
                            options: ["A", "B"],
                            multi_select: false,
                          },
                        ],
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
        onStartPrompt={onStartPrompt}
      />,
    );

    expect(screen.getByText("需要你回答")).toBeInTheDocument();
    expect(
      screen.getByText("如果没有合适的选项，可以直接输入文字回复"),
    ).toBeInTheDocument();
    expect(screen.getByText("Which option?")).toBeInTheDocument();
    expect(screen.queryByText("等待审批")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Run once" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Reject" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("1 tool call")).not.toBeInTheDocument();

    const optionA = screen.getByRole("button", { name: "A" });
    fireEvent.click(optionA);
    expect(optionA).toHaveAttribute("aria-pressed", "true");
    expect(onStartPrompt).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "提交回答" }));
    expect(onStartPrompt).toHaveBeenCalledWith("A");
  });

  it("lets ask_user collect multi-select and multi-question answers before submitting", () => {
    const onStartPrompt = vi.fn();

    render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-ask-user-multi",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "ask-1",
                  role: "assistant",
                  type: "ask_user_request",
                  status: "in_progress",
                  content: [
                    {
                      type: "ask_user_request",
                      data: {
                        request_id: "ask-1",
                        questions: [
                          {
                            question: "Pick colors",
                            options: ["Red", "Blue"],
                            multi_select: true,
                          },
                          {
                            question: "Pick speed",
                            options: ["Fast", "Slow"],
                            multi_select: false,
                          },
                        ],
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
        onStartPrompt={onStartPrompt}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Red" }));
    fireEvent.click(screen.getByRole("button", { name: "Blue" }));
    expect(screen.getByRole("button", { name: "Red" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "Blue" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(onStartPrompt).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "提交回答" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Fast" }));
    fireEvent.click(screen.getByRole("button", { name: "提交回答" }));
    expect(onStartPrompt).toHaveBeenCalledWith(
      "1. Pick colors  Red、Blue\n\n2. Pick speed  Fast",
    );
  });

  it("renders text ask_user questions with an inline input", () => {
    const onStartPrompt = vi.fn();

    render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-ask-user-text",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "ask-1",
                  role: "assistant",
                  type: "ask_user_request",
                  status: "in_progress",
                  content: [
                    {
                      type: "ask_user_request",
                      data: {
                        request_id: "ask-1",
                        questions: [
                          {
                            question: "Describe your mood",
                            options: [],
                            multi_select: false,
                          },
                        ],
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
        onStartPrompt={onStartPrompt}
      />,
    );

    const textarea = screen.getByPlaceholderText("输入你的回答");
    expect(screen.getByRole("button", { name: "提交回答" })).toBeDisabled();
    fireEvent.change(textarea, { target: { value: "还不错" } });
    fireEvent.click(screen.getByRole("button", { name: "提交回答" }));
    expect(onStartPrompt).toHaveBeenCalledWith("还不错");
  });

  it("keeps completed ask_user interactions visible instead of hiding them in a tool group", () => {
    render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-ask-user-complete",
              role: "assistant",
              status: "completed",
              output: [
                {
                  id: "call-ask",
                  role: "assistant",
                  type: "plugin_call",
                  status: "completed",
                  call_id: "call-ask",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-ask",
                        name: "ask_user",
                        arguments: JSON.stringify({
                          questions: [
                            {
                              question: "Pick a mode",
                              options: ["运动", "阅读"],
                              multi_select: false,
                            },
                          ],
                        }),
                      },
                    },
                  ],
                },
                {
                  id: "call-ask-output",
                  role: "tool",
                  type: "plugin_call_output",
                  status: "completed",
                  call_id: "call-ask",
                  content: [
                    {
                      type: "tool_result",
                      data: {
                        id: "call-ask",
                        name: "ask_user",
                        output: "Q1: Pick a mode\n(single-select)",
                        status: "completed",
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    expect(screen.getByText("Pick a mode")).toBeInTheDocument();
    expect(screen.getByText("运动")).toBeInTheDocument();
    expect(screen.queryByText("1 tool call")).not.toBeInTheDocument();
    expect(screen.queryByText("Input")).not.toBeInTheDocument();
    expect(screen.queryByText("Output")).not.toBeInTheDocument();
  });

  it("renders canonical ask_user history as an answer card", () => {
    const onStartPrompt = vi.fn();

    render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-user-ask",
              role: "assistant",
              status: "completed",
              output: [
                {
                  id: "call-user-ask",
                  role: "assistant",
                  type: "plugin_call",
                  status: "completed",
                  call_id: "call-user-ask",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-user-ask",
                        name: "ask_user",
                        arguments: {
                          questions: [
                            {
                              question: "Pick a mode",
                              options: ["运动", "阅读"],
                              type: "single_select",
                            },
                          ],
                        },
                      },
                    },
                  ],
                },
                {
                  id: "call-user-ask-output",
                  role: "tool",
                  type: "plugin_call_output",
                  status: "completed",
                  call_id: "call-user-ask",
                  content: [
                    {
                      type: "tool_result",
                      data: {
                        id: "call-user-ask",
                        name: "ask_user",
                        output: "Q1: Pick a mode\n(single-select)",
                        status: "completed",
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
        onStartPrompt={onStartPrompt}
      />,
    );

    expect(screen.getByText("Pick a mode")).toBeInTheDocument();
    expect(screen.getByText("运动")).toBeInTheDocument();
    expect(screen.queryByText("1 tool call")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "运动" }));
    fireEvent.click(screen.getByRole("button", { name: "提交回答" }));
    expect(onStartPrompt).toHaveBeenCalledWith("运动");
  });

  it("shows the ask_user answer on the question card and hides the duplicate user bubble", () => {
    render(
      <ConversationMessageList
        projection={{
          conversationId: "chat-1",
          turnActivity: { kind: "idle" },
          items: [
            {
              kind: "assistant",
              id: "assistant-ask-user-complete",
              role: "assistant",
              status: "completed",
              output: [
                {
                  id: "call-ask",
                  role: "assistant",
                  type: "plugin_call",
                  status: "completed",
                  call_id: "call-ask",
                  content: [
                    {
                      type: "tool",
                      data: {
                        id: "call-ask",
                        name: "ask_user",
                        arguments: JSON.stringify({
                          question: "Describe your mood",
                        }),
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
            {
              kind: "user",
              id: "user-answer",
              role: "user",
              content: [{ type: "text", text: "还不错" }],
            },
          ],
        }}
      />,
    );

    expect(screen.getByText("Describe your mood")).toBeInTheDocument();
    expect(screen.getByText("你的回答")).toBeInTheDocument();
    expect(screen.getByText("还不错")).toBeInTheDocument();
    expect(
      screen.queryByRole("article", { name: /还不错/ }),
    ).not.toBeInTheDocument();
    expect(
      document.querySelector('[data-rua-chat-role="user"]'),
    ).not.toBeInTheDocument();
  });

  it("labels unnamed generating tool calls with a neutral fallback", () => {
    render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={{
          conversationId: "chat-1",
          turnActivity: {
            kind: "active",
            turnId: null,
            userMessageId: null,
            responseId: "assistant-1",
          },
          items: [
            {
              kind: "assistant",
              id: "assistant-1",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "tool-call-1",
                  role: "assistant",
                  type: "plugin_call",
                  content: [{ type: "tool", data: {} }],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    expect(screen.getByText("Call tool")).toBeInTheDocument();
    expect(screen.queryByText("Tool call")).not.toBeInTheDocument();
  });

  it("updates expanded tool input while arguments are streaming", async () => {
    const projectionWithArguments = (argumentsValue: string) => ({
      conversationId: "chat-1",
      turnActivity: {
        kind: "active" as const,
        turnId: null,
        userMessageId: null,
        responseId: "assistant-1",
      },
      items: [
        {
          kind: "assistant" as const,
          id: "assistant-1",
          role: "assistant" as const,
          status: "generating" as const,
          output: [
            {
              id: "msg-tc-call-write",
              role: "assistant" as const,
              type: "plugin_call",
              status: "in_progress",
              content: [
                {
                  type: "data",
                  data: {
                    call_id: "call-write",
                    name: "Write",
                    arguments: argumentsValue,
                  },
                },
              ],
            },
          ],
          variants: [],
        },
      ],
    });
    const blockPreferences = {
      aggregateExpanded: true,
      thinkingExpanded: false,
      toolExpanded: false,
    };

    const { rerender } = render(
      <ConversationMessageList
        projection={projectionWithArguments(
          '{"path":"notes.txt","content":"first',
        )}
        blockPreferences={blockPreferences}
      />,
    );

    expect(screen.getByText("Write")).toBeInTheDocument();
    const toolCard = screen.getByText("Write").closest("details");
    expect(toolCard).not.toHaveAttribute("open");
    fireEvent.click(screen.getByText("Write"));
    expect(toolCard).toHaveAttribute("open");
    const inputBlock = await screen.findByLabelText("Input code block");
    expect(within(inputBlock).getByText("Input")).toBeInTheDocument();
    expect(inputBlock).toHaveTextContent(
      '{"path":"notes.txt","content":"first',
    );
    fireEvent.click(
      within(inputBlock).getByRole("button", { name: "Collapse block" }),
    );
    expect(inputBlock).toHaveAttribute("data-collapsed", "true");
    fireEvent.click(
      within(inputBlock).getByRole("button", { name: "Expand block" }),
    );

    rerender(
      <ConversationMessageList
        projection={projectionWithArguments(
          '{"path":"notes.txt","content":"first second"}',
        )}
        blockPreferences={blockPreferences}
      />,
    );

    expect(screen.getByLabelText("Input code block")).toHaveTextContent(
      '"content": "first second"',
    );
    expect(toolCard).toHaveAttribute("open");
  });

  it("does not show a tool spinner after output arrives while the assistant is still generating", () => {
    const { container } = render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={{
          conversationId: "chat-1",
          turnActivity: {
            kind: "active",
            turnId: null,
            userMessageId: null,
            responseId: "assistant-1",
          },
          items: [
            {
              kind: "assistant",
              id: "assistant-1",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "tool-call-1",
                  role: "assistant",
                  type: "plugin_call",
                  content: [{ type: "tool", data: { name: "Bash" } }],
                },
                {
                  id: "tool-result-1",
                  role: "tool",
                  type: "plugin_call_output",
                  content: [
                    {
                      type: "data",
                      data: {
                        call_id: "tool-call-1",
                        output: "tool finished",
                      },
                    },
                  ],
                },
                {
                  id: "assistant-text-1",
                  role: "assistant",
                  type: "message",
                  content: [{ type: "text", text: "Still writing answer" }],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    expect(screen.getByText("tool finished")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "chat.backgroundTool" }),
    ).not.toBeInTheDocument();
    expect(container.querySelector(".rua-local-tool-spinner")).toBeNull();
    expect(
      container.querySelector(".rua-chat-inline-spinner"),
    ).not.toBeNull();
  });

  it("shows one independent running indicator while active Turn content recovers", () => {
    const activeTurn = {
      turn_id: "recovering-turn",
      user_message_id: "recovering-user",
      phase: "streaming" as const,
      authority_device_id: "desktop",
      authority_epoch: "epoch-recovering",
      driver: "rua" as const,
      started_at: "2026-08-30T00:00:00Z",
      queued_count: 0,
    };
    const project = (messages: Parameters<typeof projectConversationImpl>[1]) =>
      projectConversationImpl("recovering-chat", {
        ...messages,
        messages: messages?.messages ?? [],
        active_turn: activeTurn,
      });
    const { container, rerender } = render(
      <ConversationMessageList projection={project(null)} />,
    );
    const expectRunning = () => {
      const indicators = container.querySelectorAll(
        "[data-rua-chat-running]",
      );
      expect(indicators).toHaveLength(1);
      expect(indicators[0].textContent).toBe("");
      expect(
        indicators[0].closest("[data-rua-chat-role='assistant']"),
      ).toBeNull();
      expect(
        container.querySelectorAll(".rua-chat-inline-spinner"),
      ).toHaveLength(1);
    };
    expectRunning();
    expect(container.querySelector("[data-rua-chat-empty]")).toBeNull();

    const user = {
      id: "recovering-user",
      turn_id: "recovering-turn",
      role: "user",
      content: "Continue",
    };
    rerender(
      <ConversationMessageList projection={project({ messages: [user] })} />,
    );
    expectRunning();
    expect(
      container.querySelector("[data-rua-chat-role='assistant']"),
    ).toBeNull();

    const recovered = {
      messages: [
        user,
        {
          id: "recovering-answer",
          turn_id: "recovering-turn",
          role: "assistant",
          content: "Working on it",
        },
      ],
    };
    rerender(<ConversationMessageList projection={project(recovered)} />);
    expectRunning();
    expect(screen.getByText("Working on it")).toBeInTheDocument();

    rerender(
      <ConversationMessageList
        projection={projectConversationImpl("recovering-chat", {
          ...recovered,
          active_turn: null,
        })}
      />,
    );
    expect(container.querySelector("[data-rua-chat-running]")).toBeNull();
    expect(container.querySelector(".rua-chat-inline-spinner")).toBeNull();
    expect(screen.getByText("Working on it")).toBeInTheDocument();
  });

  it("keeps the running indicator after remount without a Live overlay or matching response", () => {
    const history = {
      active_turn: {
        turn_id: "turn-current",
        authority_epoch: "epoch-current",
        phase: "streaming" as const,
        authority_device_id: "desktop",
        driver: "rua" as const,
        started_at: "2026-08-30T00:00:00Z",
      },
      messages: [
        { id: "user", role: "user", content: "Continue" },
        { id: "answer", role: "assistant", content: "Partial response" },
      ],
    };
    const mounted = render(
      <ConversationMessageList
        projection={projectConversationImpl("chat", history, {
          liveActive: true,
          hasLiveProjection: true,
        })}
      />,
    );
    expect(
      mounted.container.querySelectorAll("[data-rua-chat-running]"),
    ).toHaveLength(1);
    mounted.unmount();
    const restored = projectConversationImpl("chat", history);
    expect(restored.turnActivity).toMatchObject({
      kind: "active",
      responseId: null,
    });
    const { container } = render(
      <ConversationMessageList projection={restored} />,
    );
    expect(
      container.querySelectorAll("[data-rua-chat-running]"),
    ).toHaveLength(1);
    expect(screen.getByText("Partial response")).toBeInTheDocument();
  });

  it("keeps the active Shell turn spinner visible before messages carry the Turn id", () => {
    const projection = projectConversation(
      "chat-1",
      {
        status: "running",
        active_turn: {
          turn_id: "turn-shell",
          phase: "streaming",
          authority_device_id: "desktop",
          driver: "rua",
          started_at: "2026-08-30T00:00:00Z",
          queued_count: 0,
        },
        messages: [
          { id: "user-shell", role: "user", content: "Start servers" },
          {
            id: "shell-call",
            role: "assistant",
            type: "plugin_call",
            content: [
              {
                type: "data",
                data: { call_id: "call-shell", name: "bash" },
              },
            ],
          },
        ],
      },
      { liveActive: true, hasLiveProjection: true },
    );
    const { container } = render(
      <ConversationMessageList projection={projection} />,
    );

    expect(projection.turnActivity).toMatchObject({
      kind: "active",
      responseId: "chat-1:assistant:user-shell",
    });
    expect(
      container.querySelector(".rua-chat-inline-spinner"),
    ).not.toBeNull();
    expect(
      container.querySelector(".rua-chat-response-footer-row"),
    ).toBeNull();
  });

  it("keeps generating tool cards visible for unregistered tool names", () => {
    const { container } = render(
      <ConversationMessageList
        blockPreferences={EXPANDED_BLOCKS}
        projection={{
          conversationId: "chat-1",
          turnActivity: {
            kind: "active",
            turnId: null,
            userMessageId: null,
            responseId: "assistant-1",
          },
          items: [
            {
              kind: "assistant",
              id: "assistant-1",
              role: "assistant",
              status: "generating",
              output: [
                {
                  id: "tool-call-unknown",
                  role: "assistant",
                  type: "plugin_call",
                  content: [
                    {
                      type: "tool",
                      data: {
                        tool_call_id: "call-unknown",
                        name: "future_custom_tool",
                      },
                    },
                  ],
                },
              ],
              variants: [],
            },
          ],
        }}
      />,
    );

    expect(screen.getByText("future_custom_tool")).toBeInTheDocument();
    expect(container.querySelector(".rua-local-tool-spinner")).not.toBeNull();
  });
});
