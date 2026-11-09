import {
  act,
  createEvent,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import OwnedConversationComposerImpl from "../OwnedConversationComposer";
import * as drafts from "../../drafts";

const uploadFileMock = vi.hoisted(() => vi.fn());
vi.mock("@/api/config", () => ({ getApiUrl: (path: string) => `/api${path}` }));
const requestBrowserNotificationPermissionMock = vi.hoisted(() => vi.fn());
const composerRenderMock = vi.hoisted(() => vi.fn());

const mediaUrlMock = vi.hoisted(() =>
  vi.fn((_scope, _chatId, mediaId: string) => `/media/${mediaId}`),
);

vi.mock("@/api/modules/chat", () => ({
  chatApi: {
    uploadFile: uploadFileMock,
    mediaUrl: mediaUrlMock,
    filePreviewUrl: (path: string) => `/preview${path}`,
  },
}));

vi.mock("@/userNotifications/browserPermission", () => ({
  requestBrowserNotificationPermissionFromUserGesture:
    requestBrowserNotificationPermissionMock,
}));

function OwnedConversationComposer({
  turnActive = false,
  ...props
}: Omit<
  React.ComponentProps<typeof OwnedConversationComposerImpl>,
  "chatId" | "mediaScope" | "turnActive"
> & {
  turnActive?: boolean;
}) {
  return (
    <OwnedConversationComposerImpl
      chatId="chat-1"
      mediaScope={{ kind: "agent", agentId: "default" }}
      turnActive={turnActive}
      {...props}
    />
  );
}

function mediaUpload(filename: string, mediaId: string) {
  return {
    media_ref: `media/${mediaId}`,
    media_id: mediaId,
    mime_type: "image/png",
    byte_size: 5,
    authority_epoch: "000001a00343124c00000000f8e40a95f10d7f0e",
    original_filename: filename,
  };
}

vi.mock("@/hooks/useSlashCommands", () => ({
  useSlashCommands: () => (
    composerRenderMock(),
    [
      {
        command: "/hello",
        value: "hello",
        description: "Say hello",
      },
      {
        command: "/plan",
        value: "plan",
        description: "Make a plan",
      },
    ]
  ),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

describe("OwnedConversationComposer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => "attachment-uuid" },
      configurable: true,
    });
    Object.defineProperty(URL, "createObjectURL", {
      value: vi.fn(() => "blob:local-preview"),
      configurable: true,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      value: vi.fn(),
      configurable: true,
    });
  });

  it("skips unrelated parent updates but reacts to active-turn changes", () => {
    const props = {
      chatId: "chat-1",
      mediaScope: { kind: "agent" as const, agentId: "default" },
      turnActive: true,
      onSubmit: vi.fn(),
      onStop: vi.fn(),
    };
    const { rerender } = render(<OwnedConversationComposerImpl {...props} />);
    composerRenderMock.mockClear();
    for (let index = 0; index < 20; index += 1) {
      rerender(<OwnedConversationComposerImpl {...props} />);
    }
    expect(composerRenderMock).not.toHaveBeenCalled();
    rerender(<OwnedConversationComposerImpl {...props} turnActive={false} />);
    expect(composerRenderMock).toHaveBeenCalled();
    expect(screen.queryByLabelText("Stop generating")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Send message")).toBeInTheDocument();
  });

  it("loads a draft, submits trimmed text, and clears after acceptance", async () => {
    const onSubmit = vi.fn();
    drafts.save("chat-1", "saved draft");

    render(
      <OwnedConversationComposer
        draftKey="chat-1"
        onSubmit={onSubmit}
        onStop={vi.fn()}
      />,
    );

    const input = screen.getByPlaceholderText("Message");
    expect(input).toHaveValue("saved draft");

    fireEvent.change(input, { target: { value: "  hello rust  " } });
    expect(drafts.load("chat-1")).toBe("  hello rust  ");

    fireEvent.keyDown(input, { key: "Enter" });

    expect(onSubmit).toHaveBeenCalledWith("hello rust", []);
    expect(requestBrowserNotificationPermissionMock).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(input).toHaveValue(""));
    expect(drafts.load("chat-1")).toBe("");
  });

  it("does not reread stored drafts while typing or receiving parent updates", () => {
    const { rerender } = render(
      <OwnedConversationComposer
        draftKey="typing-chat"
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    const input = screen.getByPlaceholderText("Message");
    const storedReads = vi.spyOn(Storage.prototype, "getItem");
    try {
      fireEvent.compositionStart(input);
      for (const text of ["n", "ni", "你"]) {
        fireEvent.change(input, { target: { value: text } });
      }
      fireEvent.compositionEnd(input);
      rerender(
        <OwnedConversationComposer
          draftKey="typing-chat"
          turnActive
          onSubmit={vi.fn()}
          onStop={vi.fn()}
        />,
      );
      expect(input).toHaveValue("你");
      expect(
        storedReads.mock.calls.filter(
          ([key]) => key === drafts.DRAFT_KEY("typing-chat"),
        ),
      ).toHaveLength(0);
    } finally {
      storedReads.mockRestore();
    }
  });

  it("coalesces draft storage writes while keeping typed text immediately recoverable", () => {
    vi.useFakeTimers();
    const { unmount } = render(
      <OwnedConversationComposer
        draftKey="typing-chat"
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    const storedWrites = vi.spyOn(Storage.prototype, "setItem");
    const input = screen.getByPlaceholderText("Message");
    try {
      for (const text of ["n", "ni", "你好"]) {
        fireEvent.change(input, { target: { value: text } });
      }
      expect(drafts.load("typing-chat")).toBe("你好");
      expect(
        storedWrites.mock.calls.filter(
          ([key]) => key === drafts.DRAFT_KEY("typing-chat"),
        ),
      ).toHaveLength(0);
      act(() => {
        vi.advanceTimersByTime(300);
      });
      expect(
        storedWrites.mock.calls.filter(
          ([key]) => key === drafts.DRAFT_KEY("typing-chat"),
        ),
      ).toEqual([[drafts.DRAFT_KEY("typing-chat"), "你好"]]);
      fireEvent.change(input, { target: { value: "你好世界" } });
      fireEvent.blur(input);
      expect(sessionStorage.getItem(drafts.DRAFT_KEY("typing-chat"))).toBe(
        "你好世界",
      );
    } finally {
      unmount();
      storedWrites.mockRestore();
      vi.useRealTimers();
    }
  });

  it("flushes the right draft across route changes, backgrounding and unmount", () => {
    vi.useFakeTimers();
    const { rerender, unmount } = render(
      <OwnedConversationComposer
        draftKey="first-chat"
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    try {
      fireEvent.change(screen.getByPlaceholderText("Message"), {
        target: { value: "first draft" },
      });
      rerender(
        <OwnedConversationComposer
          draftKey="second-chat"
          onSubmit={vi.fn()}
          onStop={vi.fn()}
        />,
      );
      expect(sessionStorage.getItem(drafts.DRAFT_KEY("first-chat"))).toBe(
        "first draft",
      );
      expect(screen.getByPlaceholderText("Message")).toHaveValue("");
      fireEvent.change(screen.getByPlaceholderText("Message"), {
        target: { value: "second draft" },
      });
      act(() => window.dispatchEvent(new Event("rua:background")));
      expect(sessionStorage.getItem(drafts.DRAFT_KEY("second-chat"))).toBe(
        "second draft",
      );
      fireEvent.change(screen.getByPlaceholderText("Message"), {
        target: { value: "latest draft" },
      });
      unmount();
      expect(sessionStorage.getItem(drafts.DRAFT_KEY("second-chat"))).toBe(
        "latest draft",
      );
      act(() => vi.runOnlyPendingTimers());
      expect(sessionStorage.getItem(drafts.DRAFT_KEY("first-chat"))).toBe(
        "first draft",
      );
    } finally {
      unmount();
      vi.useRealTimers();
    }
  });

  it("keeps the native IME text visible while an attachment marker is present", () => {
    drafts.save("with-marker", "[Image #1] ", [
      {
        id: "image",
        name: "image.png",
        type: "image",
        url: "/image.png",
        marker: "[Image #1]",
      },
    ]);
    const { container } = render(
      <OwnedConversationComposer
        draftKey="with-marker"
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    const input = screen.getByPlaceholderText("Message");
    expect(input).toHaveClass("has-marker-layer");
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "[Image #1] ni" } });
    expect(input).not.toHaveClass("has-marker-layer");
    expect(
      container.querySelector(".rua-owned-composer-marker-layer"),
    ).toBeNull();
    fireEvent.change(input, { target: { value: "[Image #1] 你" } });
    fireEvent.compositionEnd(input);
    expect(input).toHaveClass("has-marker-layer");
    expect(
      screen.getByRole("button", { name: "Open [Image #1]" }),
    ).toBeInTheDocument();
  });

  it("uses native textarea text without a duplicate marker layer for plain input", () => {
    const { container } = render(
      <OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />,
    );
    const input = screen.getByPlaceholderText("Message");
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "ni" } });
    expect(input).not.toHaveClass("has-marker-layer");
    expect(
      container.querySelector(".rua-owned-composer-marker-layer"),
    ).toBeNull();
    fireEvent.change(input, { target: { value: "你好" } });
    fireEvent.compositionEnd(input);
    expect(input).toHaveValue("你好");
  });

  it.each([
    { isComposing: true, keyCode: 13 },
    { isComposing: false, keyCode: 229 },
  ])("does not submit an IME confirmation Enter (%j)", async (nativeEvent) => {
    const onSubmit = vi.fn();
    render(<OwnedConversationComposer onSubmit={onSubmit} onStop={vi.fn()} />);
    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "english" } });

    const confirmation = createEvent.keyDown(input, {
      key: "Enter",
      ...nativeEvent,
    });
    fireEvent(input, confirmation);

    expect(onSubmit).not.toHaveBeenCalled();
    expect(confirmation.defaultPrevented).toBe(false);
    expect(input).toHaveValue("english");
    fireEvent.keyDown(input, { key: "Enter", keyCode: 13 });
    expect(onSubmit).toHaveBeenCalledWith("english", []);
    await waitFor(() => expect(input).toHaveValue(""));
  });

  it("keeps the composition-ending Enter out of submit and slash completion", () => {
    vi.useFakeTimers();
    try {
      const onSubmit = vi.fn();
      render(
        <OwnedConversationComposer onSubmit={onSubmit} onStop={vi.fn()} />,
      );
      const input = screen.getByPlaceholderText("Message");
      fireEvent.compositionStart(input);
      fireEvent.change(input, { target: { value: "/hel" } });
      fireEvent.keyDown(input, { key: "Enter" });
      expect(input).toHaveValue("/hel");
      fireEvent.compositionEnd(input);
      act(() => vi.runOnlyPendingTimers());
      fireEvent.keyDown(input, { key: "Enter", keyCode: 229 });
      expect(input).toHaveValue("/hel");
      expect(onSubmit).not.toHaveBeenCalled();
      fireEvent.keyDown(input, { key: "Enter", keyCode: 13 });
      expect(input).toHaveValue("/hello ");
    } finally {
      vi.useRealTimers();
    }
  });

  it("restores uploaded image attachments after the composer remounts", async () => {
    uploadFileMock.mockResolvedValue(mediaUpload("photo.png", "stored-photo"));
    const first = render(
      <OwnedConversationComposer
        draftKey="chat-with-image"
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    const firstInput = screen.getByPlaceholderText("Message");

    fireEvent.paste(firstInput, {
      clipboardData: {
        files: [new File(["image"], "photo.png", { type: "image/png" })],
      },
    });
    await screen.findByAltText("photo.png");
    expect(drafts.loadDraft("chat-with-image").attachments).toHaveLength(1);
    first.unmount();

    const onSubmit = vi.fn();
    render(
      <OwnedConversationComposer
        draftKey="chat-with-image"
        onSubmit={onSubmit}
        onStop={vi.fn()}
      />,
    );
    const restoredInput = screen.getByPlaceholderText("Message");
    expect(restoredInput).toHaveValue("[Image #1]");
    expect(screen.getByAltText("photo.png")).toHaveAttribute(
      "src",
      "/media/stored-photo",
    );

    fireEvent.keyDown(restoredInput, { key: "Enter" });
    await waitFor(() =>
      expect(onSubmit).toHaveBeenCalledWith("[Image #1]", [
        expect.objectContaining({
          name: "photo.png",
          url: "media/stored-photo",
          marker: "[Image #1]",
          storedName: "stored-photo",
        }),
      ]),
    );
  });

  it("moves submitted text out of the editor while retaining a recoverable draft until acceptance", async () => {
    let resolveSubmit: ((accepted: boolean) => void) | undefined;
    const onSubmit = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          resolveSubmit = resolve;
        }),
    );
    drafts.save("chat-1", "saved draft");

    render(
      <OwnedConversationComposer
        draftKey="chat-1"
        onSubmit={onSubmit}
        onStop={vi.fn()}
      />,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "  choose a model first  " } });
    fireEvent.click(screen.getByLabelText("Send message"));

    expect(onSubmit).toHaveBeenCalledWith("choose a model first", []);
    expect(input).toHaveValue("");
    expect(drafts.load("chat-1")).toBe("  choose a model first  ");

    resolveSubmit?.(false);
    await waitFor(() => expect(input).toHaveValue("  choose a model first  "));
    expect(drafts.load("chat-1")).toBe("  choose a model first  ");
  });

  it("keeps the textarea editable while only Submit is disabled", () => {
    const onSubmit = vi.fn();
    render(
      <OwnedConversationComposer
        draftKey="chat-1"
        submitDisabled
        submitDisabledReason="Resource Device offline"
        onSubmit={onSubmit}
        onStop={vi.fn()}
      />,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "offline draft" } });

    expect(input).toBeEnabled();
    expect(input).toHaveValue("offline draft");
    expect(drafts.load("chat-1")).toBe("offline draft");
    expect(screen.getByLabelText("Send message")).toBeDisabled();
    expect(screen.getByLabelText("Send message")).toHaveAttribute(
      "title",
      "Resource Device offline",
    );
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("switches draft keys without rendering an empty intermediate value", () => {
    drafts.save("chat-1", "first draft");
    drafts.save("chat-2", "second draft");

    const { rerender } = render(
      <OwnedConversationComposer
        draftKey="chat-1"
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );

    const input = screen.getByPlaceholderText("Message");
    expect(input).toHaveValue("first draft");

    rerender(
      <OwnedConversationComposer
        draftKey="chat-2"
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );

    expect(input).toHaveValue("second draft");
  });

  it("restores a recalled queued draft with attachments", async () => {
    const onSubmit = vi.fn();
    const attachment = {
      id: "attachment-1",
      name: "notes.txt",
      type: "file" as const,
      url: "/uploads/notes.txt",
    };

    const { rerender } = render(
      <OwnedConversationComposer
        draftKey="chat-1"
        onSubmit={onSubmit}
        onStop={vi.fn()}
      />,
    );

    const input = screen.getByPlaceholderText("Message");
    expect(input).toHaveValue("");

    rerender(
      <OwnedConversationComposer
        draftKey="chat-1"
        recalledDraft={{
          draftKey: "chat-1",
          text: "queued draft",
          attachments: [attachment],
        }}
        onSubmit={onSubmit}
        onStop={vi.fn()}
      />,
    );

    await waitFor(() => expect(input).toHaveValue("queued draft"));
    expect(screen.getByText("notes.txt")).toBeInTheDocument();
    expect(drafts.load("chat-1")).toBe("queued draft");

    fireEvent.click(screen.getByLabelText("Send message"));

    expect(onSubmit).toHaveBeenCalledWith("queued draft", [attachment]);
    await waitFor(() => expect(input).toHaveValue(""));
    expect(screen.queryByText("notes.txt")).not.toBeInTheDocument();
    expect(drafts.load("chat-1")).toBe("");
  });

  it("offers slash commands and inserts the first match with Tab", () => {
    const onSubmit = vi.fn();

    const { container } = render(
      <OwnedConversationComposer onSubmit={onSubmit} onStop={vi.fn()} />,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "/he" } });

    expect(screen.getByText("/hello")).toBeInTheDocument();
    expect(screen.queryByText("/plan")).not.toBeInTheDocument();

    fireEvent.keyDown(input, { key: "Tab" });

    expect(input).toHaveValue("/hello ");
    expect(onSubmit).not.toHaveBeenCalled();
    expect(container.querySelector("[data-rua-chat-slash]")).toBeNull();
  });

  it("navigates slash commands and completes the active match with Enter", () => {
    const onSubmit = vi.fn();

    const { container } = render(
      <OwnedConversationComposer onSubmit={onSubmit} onStop={vi.fn()} />,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "/" } });
    const helloOption = screen.getByRole("button", { name: /\/hello/i });
    const planOption = screen.getByRole("button", { name: /\/plan/i });

    expect(helloOption).toHaveAttribute("aria-selected", "true");
    expect(planOption).toHaveAttribute("aria-selected", "false");

    fireEvent.keyDown(input, { key: "ArrowDown" });

    expect(helloOption).toHaveAttribute("aria-selected", "false");
    expect(planOption).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(input, { key: "Enter" });

    expect(input).toHaveValue("/plan ");
    expect(onSubmit).not.toHaveBeenCalled();
    expect(container.querySelector("[data-rua-chat-slash]")).toBeNull();
  });

  it("closes slash suggestions after submitting with Enter", async () => {
    const onSubmit = vi.fn();

    const { container } = render(
      <OwnedConversationComposer onSubmit={onSubmit} onStop={vi.fn()} />,
    );

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "/hello" } });
    expect(container.querySelector("[data-rua-chat-slash]")).not.toBeNull();

    fireEvent.keyDown(input, { key: "Escape" });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onSubmit).toHaveBeenCalledWith("/hello", []);
    await waitFor(() => expect(input).toHaveValue(""));
    expect(container.querySelector("[data-rua-chat-slash]")).toBeNull();
  });

  it("uploads pasted files, allows removal, and submits attachments", async () => {
    const onSubmit = vi.fn();
    let resolveUpload:
      | ((value: ReturnType<typeof mediaUpload>) => void)
      | undefined;
    uploadFileMock.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveUpload = resolve;
      }),
    );

    render(<OwnedConversationComposer onSubmit={onSubmit} onStop={vi.fn()} />);

    const input = screen.getByPlaceholderText("Message");
    const file = new File(["image"], "local.png", { type: "image/png" });

    fireEvent.paste(input, {
      clipboardData: {
        files: [file],
      },
    });

    expect(screen.getByAltText("local.png")).toHaveAttribute(
      "src",
      "blob:local-preview",
    );
    expect(input).toHaveValue("[Image #1]");
    expect(uploadFileMock).toHaveBeenCalledWith(
      { kind: "agent", agentId: "default" },
      "chat-1",
      file,
    );

    await act(async () => {
      resolveUpload?.(mediaUpload("photo.png", "photo-1"));
    });
    await screen.findByAltText("photo.png");
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:local-preview");

    fireEvent.click(screen.getByLabelText("Remove photo.png"));
    await waitFor(() =>
      expect(screen.queryByAltText("photo.png")).not.toBeInTheDocument(),
    );

    uploadFileMock.mockResolvedValueOnce(mediaUpload("photo.png", "photo-2"));
    fireEvent.paste(input, {
      clipboardData: {
        files: [file],
      },
    });
    await screen.findByAltText("photo.png");
    expect(input).toHaveValue("[Image #1]");

    fireEvent.click(screen.getByLabelText("Send message"));

    expect(onSubmit).toHaveBeenCalledWith("[Image #1]", [
      {
        id: "attachment-uuid",
        name: "photo.png",
        type: "image",
        url: "media/photo-2",
        marker: "[Image #1]",
        storedName: "photo-2",
        mimeType: "image/png",
        size: 5,
        authorityEpoch: "000001a00343124c00000000f8e40a95f10d7f0e",
      },
    ]);
  });

  it("stages attachments without creating Chat and preserves input on upload failure", async () => {
    uploadFileMock.mockRejectedValueOnce(new Error("upload unavailable"));
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);
    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "keep my text" } });
    fireEvent.paste(input, {
      clipboardData: {
        files: [new File(["data"], "photo.png", { type: "image/png" })],
      },
    });
    await waitFor(() => expect(uploadFileMock).toHaveBeenCalledTimes(1));
    expect((input as HTMLTextAreaElement).value).toContain("keep my text");
  });

  it("does not restore an unconfirmed submission as editable draft after remount", () => {
    drafts.save("agent-a:chat-1", "already submitted");
    drafts.saveSubmitted("agent-a:chat-1", {
      submissionId: "original",
      text: "already submitted",
      attachments: [],
      request: { submission_id: "original" },
      targetDeviceId: null,
    });
    render(
      <OwnedConversationComposer
        draftKey="agent-a:chat-1"
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    expect(
      (screen.getByPlaceholderText("Message") as HTMLTextAreaElement).value,
    ).toBe("");
    expect(drafts.loadSubmitted("agent-a:chat-1")?.submissionId).toBe(
      "original",
    );
  });

  it("converts a long plain-text paste into a text attachment", async () => {
    const pastedText = "a".repeat(50_001);
    uploadFileMock.mockImplementationOnce(
      async (_scope, _chatId, file: File) => ({
        ...mediaUpload(file.name, "pasted-text"),
        mime_type: file.type,
        byte_size: file.size,
      }),
    );
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);
    const input = screen.getByPlaceholderText("Message");
    const pasteEvent = createEvent.paste(input, {
      clipboardData: {
        files: [],
        getData: () => pastedText,
      },
    });

    fireEvent(input, pasteEvent);

    expect(pasteEvent.defaultPrevented).toBe(true);
    expect(input).toHaveValue("[File #1]");
    await waitFor(() => expect(uploadFileMock).toHaveBeenCalledOnce());
    const uploadedFile = uploadFileMock.mock.calls[0][2] as File;
    expect(uploadedFile.name).toMatch(
      /^pasted-text-\d{4}-\d{2}-\d{2}T.*Z\.txt$/,
    );
    expect(uploadedFile.type).toBe("text/plain;charset=utf-8");
    expect(uploadedFile.size).toBe(pastedText.length);
    await screen.findByText(uploadedFile.name);
    expect(input).toHaveValue("[File #1]");
  });

  it("inserts a converted text attachment at the paste position", async () => {
    const onSubmit = vi.fn();
    const pastedText = "位置".repeat(25_001);
    uploadFileMock.mockResolvedValueOnce(
      mediaUpload("pasted-text.txt", "pasted-text"),
    );
    render(<OwnedConversationComposer onSubmit={onSubmit} onStop={vi.fn()} />);
    const input = screen.getByPlaceholderText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "before after" } });
    input.setSelectionRange(7, 7);

    fireEvent.paste(input, {
      clipboardData: {
        files: [],
        getData: () => pastedText,
      },
    });

    await waitFor(() => expect(input).toHaveValue("before [File #1] after"));
    await screen.findByText("pasted-text.txt");
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(onSubmit).toHaveBeenCalledWith("before [File #1] after", [
        expect.objectContaining({
          name: "pasted-text.txt",
          marker: "[File #1]",
          type: "file",
        }),
      ]),
    );
  });

  it("keeps shorter plain-text pastes in the textarea path", () => {
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);
    const input = screen.getByPlaceholderText("Message");
    const pasteEvent = createEvent.paste(input, {
      clipboardData: {
        files: [],
        getData: () => "short paste",
      },
    });

    fireEvent(input, pasteEvent);

    expect(pasteEvent.defaultPrevented).toBe(false);
    expect(uploadFileMock).not.toHaveBeenCalled();
  });

  it("restores a long paste at its original selection when upload fails", async () => {
    const pastedText = "b".repeat(50_001);
    uploadFileMock.mockRejectedValueOnce(new Error("upload failed"));
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);
    const input = screen.getByPlaceholderText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "before after" } });
    input.setSelectionRange(7, 7);

    fireEvent.paste(input, {
      clipboardData: {
        files: [],
        getData: () => pastedText,
      },
    });

    await waitFor(() => expect(input).toHaveValue(`before ${pastedText}after`));
  });

  it("does not impose a fixed textarea character limit", () => {
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);

    expect(screen.getByPlaceholderText("Message")).not.toHaveAttribute(
      "maxlength",
    );
  });

  it("re-uploads pending attachments when the Authority epoch changes", async () => {
    uploadFileMock
      .mockResolvedValueOnce(mediaUpload("photo.png", "old-media"))
      .mockResolvedValueOnce({
        ...mediaUpload("photo.png", "new-media"),
        authority_epoch: "epoch-new",
      });
    const onSubmit = vi.fn().mockResolvedValue(true);
    const { rerender, unmount } = render(
      <OwnedConversationComposer
        draftKey="epoch-draft"
        onSubmit={onSubmit}
        currentAuthorityEpoch="000001a00343124c00000000f8e40a95f10d7f0e"
      />,
    );
    const input = screen.getByPlaceholderText("Message");
    fireEvent.paste(input, {
      clipboardData: {
        files: [new File(["image"], "photo.png", { type: "image/png" })],
      },
    });
    await screen.findByAltText("photo.png");

    rerender(
      <OwnedConversationComposer
        draftKey="epoch-draft"
        onSubmit={onSubmit}
        currentAuthorityEpoch="epoch-new"
      />,
    );
    fireEvent.click(screen.getByLabelText("Send message"));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(uploadFileMock).toHaveBeenCalledTimes(2);
    expect(onSubmit.mock.calls[0][1][0]).toMatchObject({
      storedName: "new-media",
      authorityEpoch: "epoch-new",
      url: "media/new-media",
    });
    unmount();
  });

  it("inserts one ordered placeholder for each pasted image", async () => {
    Object.defineProperty(globalThis, "crypto", {
      value: {
        randomUUID: vi
          .fn()
          .mockReturnValueOnce("image-1")
          .mockReturnValueOnce("image-2"),
      },
      configurable: true,
    });
    uploadFileMock
      .mockResolvedValueOnce(mediaUpload("one.png", "one"))
      .mockResolvedValueOnce(mediaUpload("two.png", "two"));

    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, { target: { value: "Compare:" } });
    fireEvent.paste(input, {
      clipboardData: {
        files: [
          new File(["one"], "one.png", { type: "image/png" }),
          new File(["two"], "two.png", { type: "image/png" }),
        ],
      },
    });

    await screen.findByAltText("two.png");
    expect(input).toHaveValue("Compare: [Image #1] [Image #2]");

    fireEvent.click(screen.getByLabelText("Remove one.png"));
    expect(input).toHaveValue("Compare: [Image #1]");
    expect(screen.getByAltText("two.png")).toBeInTheDocument();
  });

  it("keeps image upload available regardless of model capability", async () => {
    uploadFileMock.mockResolvedValue(mediaUpload("accepted.png", "accepted"));
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);

    const input = screen.getByPlaceholderText("Message");
    fireEvent.paste(input, {
      clipboardData: {
        files: [new File(["image"], "accepted.png", { type: "image/png" })],
      },
    });

    await screen.findByAltText("accepted.png");
    expect(uploadFileMock).toHaveBeenCalledOnce();
    expect(input).toHaveValue("[Image #1]");
  });

  it("opens pasted image previews in the shared full-screen viewer", async () => {
    uploadFileMock.mockResolvedValue(mediaUpload("photo.png", "preview"));
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);

    const input = screen.getByPlaceholderText("Message");
    fireEvent.paste(input, {
      clipboardData: {
        files: [new File(["image"], "photo.png", { type: "image/png" })],
      },
    });

    await screen.findByAltText("photo.png");
    fireEvent.click(screen.getByRole("button", { name: "Open photo.png" }));
    expect(
      screen.getByRole("dialog", { name: "photo.png" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(screen.getByText("125%")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close image" }));
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "photo.png" }),
      ).not.toBeInTheDocument(),
    );
  });

  it("opens the matching image preview when its placeholder is clicked", async () => {
    uploadFileMock.mockResolvedValue(mediaUpload("photo.png", "placeholder"));
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);

    const input = screen.getByPlaceholderText("Message");
    fireEvent.paste(input, {
      clipboardData: {
        files: [new File(["image"], "photo.png", { type: "image/png" })],
      },
    });
    await screen.findByAltText("photo.png");

    fireEvent.click(screen.getByRole("button", { name: "Open [Image #1]" }));

    expect(screen.getByRole("dialog", { name: "photo.png" })).toBeVisible();
  });

  it.each([
    ["Backspace", "after"],
    ["Delete", "before"],
  ])("deletes an image placeholder atomically with %s", async (key, caret) => {
    uploadFileMock.mockResolvedValue(mediaUpload("photo.png", "delete"));
    render(<OwnedConversationComposer onSubmit={vi.fn()} onStop={vi.fn()} />);

    const input = screen.getByPlaceholderText("Message") as HTMLTextAreaElement;
    fireEvent.paste(input, {
      clipboardData: {
        files: [new File(["image"], "photo.png", { type: "image/png" })],
      },
    });
    await screen.findByAltText("photo.png");
    const position = caret === "after" ? input.value.length : 0;
    input.setSelectionRange(position, position);

    fireEvent.keyDown(input, { key });

    expect(input).toHaveValue("");
    expect(screen.queryByAltText("photo.png")).not.toBeInTheDocument();
  });

  it("switches the single streaming action between stop and send", async () => {
    const onSubmit = vi.fn();
    const onStop = vi.fn();

    render(
      <OwnedConversationComposer
        turnActive
        onSubmit={onSubmit}
        onStop={onStop}
      />,
    );

    expect(screen.getByLabelText("Stop generating")).toBeInTheDocument();
    expect(screen.queryByLabelText("Send message")).not.toBeInTheDocument();

    const input = screen.getByPlaceholderText("Message");
    fireEvent.change(input, {
      target: { value: "queue this" },
    });

    expect(screen.queryByLabelText("Stop generating")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Send message")).toBeInTheDocument();

    fireEvent.click(screen.getByLabelText("Send message"));

    expect(onSubmit).toHaveBeenCalledWith("queue this", []);
    await waitFor(() => expect(input).toHaveValue(""));
    expect(screen.getByLabelText("Stop generating")).toBeInTheDocument();
    expect(screen.queryByLabelText("Send message")).not.toBeInTheDocument();

    fireEvent.click(screen.getByLabelText("Stop generating"));
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("places the device chip before the send action", () => {
    const { container } = render(
      <OwnedConversationComposer
        deviceChip={<span data-testid="device-chip">Local</span>}
        onSubmit={vi.fn()}
        onStop={vi.fn()}
      />,
    );

    const actions = container.querySelector(".rua-owned-composer-actions");
    expect(actions?.children[0]).toBe(screen.getByTestId("device-chip"));
    expect(actions?.children[1]).toBe(screen.getByLabelText("Send message"));
  });
});
