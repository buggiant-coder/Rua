import {
  ArrowRightToLine,
  Check,
  ChevronLeft,
  ChevronRight,
  ChevronsDown,
  ChevronsUp,
  Circle,
  Copy,
  FileText,
  GitBranch,
  Lightbulb,
  LoaderCircle,
  MessageCircle,
  MoreVertical,
  RefreshCw,
  ListChecks,
  Sparkles,
  Undo2,
  Volume2,
  Wrench,
} from "lucide-react";
import type {
  ComponentPropsWithoutRef,
  CSSProperties,
  MouseEvent,
  ReactNode,
  RefObject,
  MutableRefObject,
} from "react";
import {
  createContext,
  memo,
  useCallback,
  useContext,
  useDeferredValue,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { Modal, Popover, toast } from "@/ui";
import type { ApprovalSnapshot } from "@/api/modules/console";
import { chatApi, type ChatMediaScope } from "@/api/modules/chat";
import { ChatRenderBlock } from "@/components/ChatRenderBlock";
import { RuaMarkdown } from "@/components/RuaMarkdown";
import ImageViewer from "@/components/ImageViewer";
import { copyText } from "@/utils/clipboard";
import { isMobile } from "@/utils/isMobile";
import { getDownloadFolderHint, saveFile } from "@/utils/saveFile";
import { formatCompact } from "@/utils/formatNumber";
import { useTTS } from "@/hooks/useTTS";
import { toDisplayUrl, toMarkdownImageDisplayUrl } from "../utils";
import {
  approvalSnapshotForAssistant,
  dataAt,
  mergeToolMessagesForApproval,
  toolApprovalDataFromMessage,
  toolCallId,
} from "./approvalViewModel";
import type {
  ConversationAcpHistoryPreview,
  ConversationAssistantResponse,
  ConversationContentPart,
  ConversationOutputMessage,
  ConversationProjection,
  ConversationRuntimeNote,
  ConversationUserMessage,
  TokenUsage,
  TokenUsageSnapshot,
} from "./projection";
import {
  assistantResponseText,
  runtimeNoteField,
  runtimeNoteTerminalStatus,
} from "./projection";
import {
  DEFAULT_CHAT_BLOCK_PREFERENCES,
  type ChatBlockCollapsePreferences,
} from "./blockPreferences";
import { groupConversationTurns } from "./conversationTurns";
import {
  useConversationViewport,
  type ConversationViewportController,
} from "./useConversationViewport";
import { getChatViewCacheByteLimit } from "./useConversationViewRuntime";
import {
  RenderStateRoot,
  RenderStateScope,
} from "@/components/ChatRenderBlock/viewState";
import {
  useRenderState,
  useRenderProtection,
} from "@/components/ChatRenderBlock/useRenderState";
import "./conversation-message-list.css";
type Translate = ReturnType<typeof useTranslation>["t"];

const TOOL_GROUP_TYPES = new Set([
  "reasoning",
  "plugin_call",
  "plugin_call_output",
  "mcp_call",
  "mcp_call_output",
]);

type ConversationMessageListProps = {
  scrollRef?: RefObject<HTMLDivElement | null>;
  viewportRef?: MutableRefObject<ConversationViewportController | null>;
  viewKey?: string;
  onScrollState?: (
    following: boolean,
    scrolling: boolean,
    scrollTop: number,
  ) => void;
  onTop?: () => void;
  initialScroll?: { scrollTop?: number; followLatest?: boolean } | null;
  onLocateTurn?: (turnId: string) => Promise<void>;
  historyGap?: { beforeMessageId: string; count: number } | null;
  onLoadHistoryGap?: () => Promise<void>;
  projection: ConversationProjection;
  agentId?: string;
  mediaScope?: ChatMediaScope;
  approvalSnapshot?: ApprovalSnapshot;
  activeAcpRunner?: string | null;
  onSelectVariant?: (turnId: string, variantId: string) => Promise<void> | void;
  onRegenerate?: (turnId: string, userText: string) => Promise<void> | void;
  onRollback?: (beforeTurnId: string, userText: string) => Promise<void> | void;
  onBranch?: (beforeTurnId: string, userText: string) => Promise<void> | void;
  onStartPrompt?: (text: string) => Promise<void> | void;
  blockPreferences?: ChatBlockCollapsePreferences;
  contextEditorOpen?: boolean;
  excludedMessageIds?: Set<string>;
  draftExcludedMessageIds?: Set<string>;
  onDraftExcludedMessageIdsChange?: (messageIds: Set<string>) => void;
};

type SearchScrollTarget = {
  messageId?: string;
  turnId?: string;
  conversationId?: string;
  ownerKey?: string;
  cardIndex?: unknown;
  query?: unknown;
};

const SEARCH_JUMP_MARK_CLASS = "rua-search-jump-mark";
const SEARCH_JUMP_CARD_CLASS = "rua-chat-message--search-jump";
const VARIANT_TRANSITION_MS = 190;
const TAP_REVEAL_TIMEOUT_MS = 3_000;
const EMPTY_MESSAGE_IDS = new Set<string>();
const SearchRevealContext = createContext<ReadonlySet<string> | null>(null);
const MarkdownMediaContext = createContext<{
  scope?: ChatMediaScope;
  chatId?: string;
}>({});
const MEDIA_ID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

function textFromParts(parts: ConversationContentPart[]): string {
  return parts
    .filter((part) => part.type === "text" && typeof part.text === "string")
    .map((part) => part.text || "")
    .filter(Boolean)
    .join("\n");
}

function partUrl(
  part: ConversationContentPart,
  agentId?: string,
  mediaScope?: ChatMediaScope,
  chatId?: string,
): string | undefined {
  if (part.media_ref) {
    const mediaId = part.media_ref.startsWith("media/")
      ? part.media_ref.slice("media/".length)
      : "";
    if (mediaScope && chatId && MEDIA_ID_RE.test(mediaId)) {
      return chatApi.mediaUrl(mediaScope, chatId, mediaId);
    }
    if (agentId) {
      return chatApi.localImagePreviewUrl(part.media_ref, agentId);
    }
  }
  const url =
    part.image_url ||
    part.video_url ||
    part.audio_url ||
    part.file_url ||
    part.file_id ||
    part.url;
  return typeof url === "string" && url ? url : undefined;
}

function legacyAttachmentPathHint(
  part: ConversationContentPart,
): string | null {
  const url = partUrl(part);
  if (!url) return null;
  if (part.type === "file") {
    const name = part.file_name || part.filename || part.name || "file";
    return `用户上传文件 \`${name}\`，已经下载到 ${url}`;
  }
  if (part.type === "image") {
    const label =
      part.file_name || part.filename || part.alt || part.name || "image";
    return `用户上传图片 \`${label}\`，已经下载到 ${url}`;
  }
  if (part.type === "audio") {
    return `用户上传音频，已经下载到 ${url}`;
  }
  if (part.type === "video") {
    return `用户上传视频，已经下载到 ${url}`;
  }
  return null;
}

function visibleUserContentParts(
  parts: ConversationContentPart[],
): ConversationContentPart[] {
  return parts.filter((part, index) => {
    if (part.type !== "text" || typeof part.text !== "string" || index === 0) {
      return true;
    }
    const expected = legacyAttachmentPathHint(parts[index - 1]);
    return !expected || part.text.trim() !== expected;
  });
}

function clearSearchJumpMarks(root: HTMLElement) {
  root.classList.remove(SEARCH_JUMP_CARD_CLASS);
  root
    .querySelectorAll<HTMLElement>(`.${SEARCH_JUMP_MARK_CLASS}`)
    .forEach((mark) => {
      const parent = mark.parentNode;
      if (!parent) return;
      parent.replaceChild(
        document.createTextNode(mark.textContent || ""),
        mark,
      );
      parent.normalize();
    });
}

function shouldSkipSearchHighlightNode(node: Node) {
  const parent = node.parentElement;
  if (!parent) return true;
  return Boolean(
    parent.closest(
      [
        "script",
        "style",
        "textarea",
        "input",
        "button",
        "svg",
        "pre",
        "code",
        `.${SEARCH_JUMP_MARK_CLASS}`,
      ].join(","),
    ),
  );
}

function applySearchJumpHighlight(target: HTMLElement, query: string) {
  const trimmed = query.trim();
  if (!trimmed) {
    target.classList.add(SEARCH_JUMP_CARD_CLASS);
    return () => target.classList.remove(SEARCH_JUMP_CARD_CLASS);
  }

  clearSearchJumpMarks(target);
  target.classList.add(SEARCH_JUMP_CARD_CLASS);

  const lowerQuery = trimmed.toLocaleLowerCase();
  const walker = document.createTreeWalker(target, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      if (shouldSkipSearchHighlightNode(node)) {
        return NodeFilter.FILTER_REJECT;
      }
      const text = node.nodeValue || "";
      return text.toLocaleLowerCase().includes(lowerQuery)
        ? NodeFilter.FILTER_ACCEPT
        : NodeFilter.FILTER_REJECT;
    },
  });

  const matches: Text[] = [];
  while (matches.length < 100) {
    const node = walker.nextNode();
    if (!node) break;
    matches.push(node as Text);
  }

  for (const node of matches) {
    const text = node.nodeValue || "";
    const lowerText = text.toLocaleLowerCase();
    const fragment = document.createDocumentFragment();
    let cursor = 0;
    let index = lowerText.indexOf(lowerQuery);

    while (index !== -1) {
      if (index > cursor) {
        fragment.append(document.createTextNode(text.slice(cursor, index)));
      }
      const mark = document.createElement("mark");
      mark.className = SEARCH_JUMP_MARK_CLASS;
      mark.textContent = text.slice(index, index + trimmed.length);
      fragment.append(mark);
      cursor = index + trimmed.length;
      index = lowerText.indexOf(lowerQuery, cursor);
    }

    if (cursor < text.length) {
      fragment.append(document.createTextNode(text.slice(cursor)));
    }
    node.replaceWith(fragment);
  }

  return () => clearSearchJumpMarks(target);
}

function IconButton({
  title,
  className = "rua-local-footer-action",
  disabled,
  children,
  onClick,
}: {
  title: string;
  className?: string;
  disabled?: boolean;
  children: ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={className}
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

function CopyButton({
  value,
  className = "rua-local-footer-action",
}: {
  value: string;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  if (!value) return null;
  return (
    <IconButton
      title={copied ? "Copied" : "Copy"}
      className={className}
      onClick={() => {
        void copyText(value).then((ok) => {
          if (ok) {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1200);
          }
        });
      }}
    >
      {copied ? <Check /> : <Copy />}
    </IconButton>
  );
}

function SpeakButton({ value }: { value: string }) {
  const tts = useTTS();
  useRenderProtection(tts.isSpeaking || tts.isLoading);
  if (!value || tts.isDisabled) return null;
  return (
    <IconButton
      title={tts.isSpeaking ? "Stop speaking" : "Speak"}
      disabled={tts.isLoading}
      onClick={() => {
        if (tts.isSpeaking) {
          tts.stop();
        } else {
          tts.speak(value);
        }
      }}
    >
      <Volume2 />
    </IconButton>
  );
}

function MarkdownBlock({
  text,
  cursor = false,
  streaming = cursor,
}: {
  text: string;
  cursor?: boolean;
  streaming?: boolean;
}) {
  const deferredText = useDeferredValue(text);
  const media = useContext(MarkdownMediaContext);
  const resolveImageSrc = useCallback(
    (source: string) =>
      toMarkdownImageDisplayUrl(source, media.scope, media.chatId),
    [media.scope, media.chatId],
  );
  const content = streaming ? deferredText : text;
  if (!content) return null;
  return (
    <div className="rua-local-markdown">
      <RuaMarkdown
        content={content}
        className="rua-chat-markdown"
        collapsibleRenderBlocks
        resolveImageSrc={resolveImageSrc}
      />
      {cursor && <span className="rua-local-cursor" />}
    </div>
  );
}

function prefersReducedMotion(): boolean {
  return Boolean(
    typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches,
  );
}

function AssistantOutputTransition({
  animationKey,
  variantIndex,
  disabled,
  children,
}: {
  animationKey: string;
  variantIndex: number;
  disabled?: boolean;
  children: ReactNode;
}) {
  const previousKeyRef = useRef(animationKey);
  const previousVariantIndexRef = useRef(variantIndex);
  const frameRef = useRef<number | null>(null);
  const timerRef = useRef<number | null>(null);
  const [phase, setPhase] = useState<"idle" | "prepare" | "enter">("idle");
  const [direction, setDirection] = useState<"forward" | "backward">("forward");

  useEffect(
    () => () => {
      if (frameRef.current !== null) {
        window.cancelAnimationFrame(frameRef.current);
      }
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
      }
    },
    [],
  );

  useLayoutEffect(() => {
    const changed = previousKeyRef.current !== animationKey;

    if (frameRef.current !== null) {
      window.cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    }
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }

    if (changed && !disabled && !prefersReducedMotion()) {
      const nextDirection =
        variantIndex >= previousVariantIndexRef.current
          ? "forward"
          : "backward";

      setDirection(nextDirection);
      setPhase("prepare");

      frameRef.current = window.requestAnimationFrame(() => {
        frameRef.current = null;
        setPhase("enter");
      });
      timerRef.current = window.setTimeout(() => {
        timerRef.current = null;
        setPhase("idle");
      }, VARIANT_TRANSITION_MS);
    } else {
      setPhase("idle");
    }

    previousKeyRef.current = animationKey;
    previousVariantIndexRef.current = variantIndex;
  }, [animationKey, disabled, variantIndex]);

  const style = {
    "--rua-variant-enter-x": direction === "forward" ? "6px" : "-6px",
  } as CSSProperties;

  return (
    <div
      className={[
        "rua-assistant-output-transition",
        phase !== "idle" ? `is-${phase}` : "",
        phase !== "idle" ? `is-${direction}` : "",
      ]
        .filter(Boolean)
        .join(" ")}
      style={style}
    >
      <div
        key={animationKey}
        className="rua-assistant-output-transition-body"
      >
        {children}
      </div>
    </div>
  );
}

function filenameFromContentDisposition(disposition: string): string | null {
  const encoded = disposition.match(/filename\*=UTF-8''([^;]+)/i)?.[1];
  if (encoded) {
    try {
      return decodeURIComponent(encoded);
    } catch {
      return encoded;
    }
  }
  return disposition.match(/filename="?([^";]+)"?/i)?.[1] ?? null;
}

function fallbackDownloadName(label: string): string {
  const clean = label.split(/[\\/]/).pop()?.split("?")[0]?.trim();
  return clean || "attachment.bin";
}

function FilePart({
  part,
  agentId,
  mediaScope,
  chatId,
}: {
  part: ConversationContentPart;
  agentId?: string;
  mediaScope?: ChatMediaScope;
  chatId?: string;
}) {
  const { t } = useTranslation();
  const url = partUrl(part, agentId, mediaScope, chatId);
  const label =
    part.file_name ||
    part.filename ||
    part.name ||
    (typeof url === "string" ? url : "file");
  const href = typeof url === "string" ? toDisplayUrl(url) : undefined;

  const handleClick = async (event: MouseEvent<HTMLAnchorElement>) => {
    if (!href || !isMobile()) return;
    event.preventDefault();
    try {
      const response = await fetch(href);
      if (!response.ok) {
        throw new Error(`${response.status} ${response.statusText}`.trim());
      }
      const contentType =
        response.headers.get("content-type") || "application/octet-stream";
      const disposition = response.headers.get("content-disposition") || "";
      const filename =
        filenameFromContentDisposition(disposition) ||
        fallbackDownloadName(String(label));
      const originalBlob = await response.blob();
      const blob = originalBlob.type
        ? originalBlob
        : new Blob([originalBlob], { type: contentType });
      const result = await saveFile(blob, filename);
      if (result.ok) {
        const folder = result.usedPicker ? "" : getDownloadFolderHint();
        toast.success(
          folder
            ? t("chat.export.successMobile", { folder, filename })
            : t("chat.export.successDownload", { filename }),
        );
      }
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("chat.export.failed"),
      );
    }
  };

  return (
    <a
      className="rua-chat-file-part"
      href={href}
      target="_blank"
      rel="noreferrer"
      onClick={handleClick}
    >
      <FileText size={15} />
      <span>{label}</span>
    </a>
  );
}

function ContentParts({
  parts,
  streaming = false,
}: {
  parts: ConversationContentPart[];
  streaming?: boolean;
}) {
  const text = textFromParts(parts);
  const media = parts.filter((part) => part.type !== "text");

  return (
    <>
      <MarkdownBlock text={text} streaming={streaming} />
      {media.length > 0 && (
        <div className="rua-chat-media-grid">
          {media.map((part, index) => {
            const mediaUrl = partUrl(part);
            if (part.type === "image" && mediaUrl) {
              return (
                <ImageViewer
                  key={index}
                  src={toDisplayUrl(mediaUrl)}
                  alt={
                    part.file_name ||
                    part.filename ||
                    part.alt ||
                    part.name ||
                    ""
                  }
                  imageClassName="rua-chat-image-part"
                />
              );
            }
            if (part.type === "video" && mediaUrl) {
              return (
                <video
                  preload="none"
                  key={index}
                  src={toDisplayUrl(mediaUrl)}
                  poster={
                    part.video_poster ? toDisplayUrl(part.video_poster) : ""
                  }
                  className="rua-chat-video-part"
                  controls
                />
              );
            }
            if (
              part.type === "audio" &&
              (mediaUrl || typeof part.data === "string")
            ) {
              return (
                <audio
                  preload="none"
                  key={index}
                  src={toDisplayUrl(String(mediaUrl || part.data))}
                  controls
                />
              );
            }
            if (part.type === "file") {
              return <FilePart key={index} part={part} />;
            }
            return null;
          })}
        </div>
      )}
    </>
  );
}

function UserContentParts({
  parts,
  agentId,
  mediaScope,
  chatId,
}: {
  parts: ConversationContentPart[];
  agentId?: string;
  mediaScope?: ChatMediaScope;
  chatId: string;
}) {
  const [previewImageIndex, setPreviewImageIndex] = useState<number | null>(
    null,
  );
  let imageIndex = 0;
  const images: ConversationContentPart[] = [];
  const otherMedia = parts.filter(
    (part) => part.type !== "text" && part.type !== "image",
  );
  const text = parts
    .map((part) => {
      if (part.type === "text") return part.text || "";
      if (part.type !== "image") return "";
      imageIndex += 1;
      images.push(part);
      return `[[Image #${imageIndex}]](#rua-image-${imageIndex})`;
    })
    .join("");

  const previewImage =
    previewImageIndex === null ? null : images[previewImageIndex];
  const handleContentClick = (event: MouseEvent<HTMLDivElement>) => {
    const link = (event.target as HTMLElement).closest<HTMLAnchorElement>(
      'a[href^="#rua-image-"]',
    );
    if (!link) return;
    const index = Number(link.hash.replace("#rua-image-", "")) - 1;
    if (!images[index]) return;
    event.preventDefault();
    setPreviewImageIndex(index);
  };

  return (
    <div className="rua-chat-user-content" onClick={handleContentClick}>
      <MarkdownBlock text={text} />
      {images.length > 0 && (
        <div className="rua-chat-user-images">
          {images.map((part, index) => {
            const mediaUrl = partUrl(part, agentId, mediaScope, chatId);
            return mediaUrl ? (
              <div key={index} className="rua-chat-user-image">
                <ImageViewer
                  src={toDisplayUrl(mediaUrl)}
                  alt={
                    part.file_name ||
                    part.filename ||
                    part.alt ||
                    part.name ||
                    ""
                  }
                  triggerClassName="rua-chat-user-image-trigger"
                  imageClassName="rua-chat-image-part rua-chat-image-part--thumbnail"
                />
              </div>
            ) : null;
          })}
        </div>
      )}
      {otherMedia.map((part, index) => {
        const mediaUrl = partUrl(part, agentId, mediaScope, chatId);
        return (
          <div key={index} className="rua-chat-media-grid">
            {part.type === "video" && mediaUrl && (
              <video
                preload="none"
                src={toDisplayUrl(mediaUrl)}
                poster={
                  part.video_poster ? toDisplayUrl(part.video_poster) : ""
                }
                className="rua-chat-video-part"
                controls
              />
            )}
            {part.type === "audio" &&
              (mediaUrl || typeof part.data === "string") && (
                <audio
                  preload="none"
                  src={toDisplayUrl(String(mediaUrl || part.data))}
                  controls
                />
              )}
            {part.type === "file" && (
              <FilePart
                part={part}
                agentId={agentId}
                mediaScope={mediaScope}
                chatId={chatId}
              />
            )}
          </div>
        );
      })}
      {previewImage && partUrl(previewImage, agentId, mediaScope, chatId) && (
        <ImageViewer
          src={toDisplayUrl(
            partUrl(previewImage, agentId, mediaScope, chatId)!,
          )}
          alt={
            previewImage.file_name ||
            previewImage.filename ||
            previewImage.alt ||
            previewImage.name ||
            ""
          }
          open
          hideTrigger
          onOpenChange={(open) => {
            if (!open) setPreviewImageIndex(null);
          }}
        />
      )}
    </div>
  );
}

function hasRenderableContentParts(parts: ConversationContentPart[]): boolean {
  return (
    textFromParts(parts).trim().length > 0 ||
    parts.some((part) => part.type !== "text")
  );
}

type JsonContainerDisplay =
  | "full"
  | "object-fields"
  | "single-array-object-fields";

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function formatObjectFields(value: Record<string, unknown>): string {
  const lines = JSON.stringify(value, null, 2).split("\n");
  if (lines.length <= 2) return "";
  return lines
    .slice(1, -1)
    .map((line) => (line.startsWith("  ") ? line.slice(2) : line))
    .join("\n");
}

function formatJsonValue(
  value: unknown,
  display: JsonContainerDisplay,
): string {
  if (display === "object-fields" && isPlainRecord(value)) {
    return formatObjectFields(value);
  }
  if (
    display === "single-array-object-fields" &&
    Array.isArray(value) &&
    value.length === 1 &&
    isPlainRecord(value[0])
  ) {
    return formatObjectFields(value[0]);
  }
  return JSON.stringify(value, null, 2);
}

function formatBlockValue(
  value: unknown,
  display: JsonContainerDisplay = "full",
): { text: string; language: string } {
  if (value == null || value === "") {
    return { text: "", language: "text" };
  }
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (
      (trimmed.startsWith("{") && trimmed.endsWith("}")) ||
      (trimmed.startsWith("[") && trimmed.endsWith("]"))
    ) {
      try {
        return {
          text: formatJsonValue(JSON.parse(trimmed), display),
          language: "json",
        };
      } catch {
        return { text: value, language: "text" };
      }
    }
    return { text: value, language: "text" };
  }
  return { text: formatJsonValue(value, display), language: "json" };
}

function CodeBlock({
  title,
  value,
  display = "full",
}: {
  title?: string;
  value: unknown;
  display?: JsonContainerDisplay;
}) {
  const formatted = useMemo(
    () => formatBlockValue(value, display),
    [value, display],
  );
  if (!formatted.text) return null;

  return (
    <ChatRenderBlock
      stateKey={"local-code:" + (title ?? "body")}
      className="rua-local-code-block rua-render-block--local-code"
      ariaLabel={title ? `${title} code block` : "Code block"}
      title={title}
      sticky={false}
      actions={
        title ? (
          <CopyButton
            value={formatted.text}
            className="rua-local-copy-button"
          />
        ) : undefined
      }
    >
      <pre data-language={formatted.language}>
        <code>{formatted.text}</code>
      </pre>
    </ChatRenderBlock>
  );
}

type LocalOperateCardProps = Omit<
  ComponentPropsWithoutRef<"details">,
  "children" | "className"
> & {
  children: ReactNode;
  className?: string;
  sticky?: boolean;
  summary: ReactNode;
};

function LocalOperateCard({
  children,
  className,
  sticky = true,
  summary,
  open,
  onToggle,
  ...detailsProps
}: LocalOperateCardProps) {
  return (
    <details
      {...detailsProps}
      open={open}
      onToggle={(event) => {
        if (event.target === event.currentTarget) onToggle?.(event);
      }}
      className={[
        "rua-local-operate-card",
        sticky ? "rua-local-operate-card--sticky" : "",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <summary>{summary}</summary>
      {open && children}
    </details>
  );
}

const UserBubble = memo(function UserBubble({
  item,
  agentId,
  mediaScope,
  chatId,
  cardIndex,
  branchActionsDisabled,
  onRollback,
  onBranch,
  tapRevealed,
  onTapReveal,
}: {
  item: ConversationUserMessage;
  agentId?: string;
  mediaScope?: ChatMediaScope;
  chatId: string;
  cardIndex: number;
  branchActionsDisabled?: boolean;
  onRollback?: (beforeTurnId: string, userText: string) => Promise<void> | void;
  onBranch?: (beforeTurnId: string, userText: string) => Promise<void> | void;
  tapRevealed?: boolean;
  onTapReveal?: (messageId: string) => void;
}) {
  const { t } = useTranslation();
  const [contentCollapsed, setContentCollapsed] = useRenderState(
    "user-collapsed",
    false,
  );
  const [contentCollapsible, setContentCollapsible] = useState(false);
  const contentFrameRef = useRef<HTMLDivElement>(null);
  const visibleContent = visibleUserContentParts(item.content);
  const text = textFromParts(visibleContent);
  const hasMedia = visibleContent.some((part) => part.type !== "text");
  const mediaOnly = hasMedia && !text.trim();
  const userTime = fmtTime(item.created_at);
  const acpRunner = item.acp_origin_runner;
  const hasAcpBadge = acpRunner !== undefined;

  useLayoutEffect(() => {
    const contentFrame = contentFrameRef.current;
    if (!contentFrame) return;

    const updateCollapsible = () => {
      const fontSize = Number.parseFloat(
        window.getComputedStyle(contentFrame).fontSize,
      );
      const collapsedHeight =
        2 * 1.7 * (Number.isFinite(fontSize) ? fontSize : 16);
      const nextCollapsible = contentFrame.scrollHeight > collapsedHeight + 1;
      setContentCollapsible(nextCollapsible);
      if (!nextCollapsible) setContentCollapsed(false);
    };

    updateCollapsible();
    const resizeObserver =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(updateCollapsible);
    resizeObserver?.observe(contentFrame);
    if (!resizeObserver) window.addEventListener("resize", updateCollapsible);

    return () => {
      resizeObserver?.disconnect();
      if (!resizeObserver)
        window.removeEventListener("resize", updateCollapsible);
    };
  }, [item.content, setContentCollapsed]);

  return (
    <article
      className="rua-chat-message rua-chat-message--user"
      data-rua-chat-message-id={item.id}
      data-rua-chat-role="user"
      data-rua-chat-card-index={cardIndex}
      data-rua-tap-reveal={tapRevealed ? "true" : undefined}
      onClick={() => onTapReveal?.(item.id)}
    >
      <div
        className="rua-chat-message-actions"
        onClick={(event) => event.stopPropagation()}
      >
        <CopyButton value={text} />
        {item.turn_id && (
          <BranchActionsMenu
            beforeTurnId={item.turn_id}
            userText={text}
            disabled={branchActionsDisabled}
            onRollback={onRollback}
            onBranch={onBranch}
          />
        )}
      </div>
      {hasAcpBadge && (
        <span className="rua-chat-origin-badge rua-chat-origin-badge--acp">
          ACP{acpRunner ? ` · ${acpRunner}` : ""}
        </span>
      )}
      {userTime && (
        <span className="rua-chat-time-cluster rua-chat-time-cluster--top-right">
          <span className="rua-chat-time">{userTime}</span>
        </span>
      )}
      <div
        className={[
          "rua-chat-bubble",
          "rua-chat-bubble--user",
          hasMedia ? "has-media" : "",
          mediaOnly ? "is-media-only" : "",
        ]
          .filter(Boolean)
          .join(" ")}
      >
        <div
          ref={contentFrameRef}
          className={`rua-chat-user-content-frame${
            contentCollapsed ? " is-collapsed" : ""
          }`}
        >
          <UserContentParts
            parts={visibleContent}
            agentId={agentId}
            mediaScope={mediaScope}
            chatId={chatId}
          />
        </div>
        {contentCollapsible && (
          <button
            type="button"
            className="rua-chat-user-content-toggle"
            aria-label={
              contentCollapsed
                ? t("chat.expandUserInput", {
                    defaultValue: "Expand user input",
                  })
                : t("chat.collapseUserInput", {
                    defaultValue: "Collapse user input",
                  })
            }
            aria-expanded={!contentCollapsed}
            onClick={(event) => {
              event.stopPropagation();
              setContentCollapsed((collapsed) => !collapsed);
            }}
          >
            {contentCollapsed ? (
              <ChevronsDown aria-hidden="true" size={17} />
            ) : (
              <ChevronsUp aria-hidden="true" size={17} />
            )}
          </button>
        )}
        {item.remote_delivery_state && (
          <div className="rua-chat-remote-pending">
            <LoaderCircle className="rua-chat-inline-spinner" size={13} />
            <span>
              {item.remote_delivery_state === "sending"
                ? t("chat.deviceDelivery.delivering", {
                    defaultValue: "Delivering to remote device...",
                  })
                : t("chat.deviceDelivery.pending", {
                    defaultValue: "Delivered, waiting for result",
                  })}
            </span>
          </div>
        )}
      </div>
    </article>
  );
});

function AcpHistoryPreviewCard({
  item,
  cardIndex,
}: {
  item: ConversationAcpHistoryPreview;
  cardIndex: number;
}) {
  const { t } = useTranslation();
  const text = textFromParts(item.content);
  const runner = item.acp_origin_runner;
  return (
    <article
      className="rua-chat-message rua-chat-message--assistant rua-chat-message--acp-history-preview"
      data-rua-chat-message-id={item.id}
      data-rua-chat-role="ui"
      data-rua-chat-turn-id={item.turn_id || undefined}
      data-rua-chat-card-index={cardIndex}
    >
      <span className="rua-chat-origin-badge rua-chat-origin-badge--acp">
        ACP{runner ? ` · ${runner}` : ""}
      </span>
      <div className="rua-chat-bubble rua-chat-bubble--assistant rua-chat-bubble--acp-history-preview">
        <div className="rua-chat-acp-history-preview-header">
          <FileText size={14} aria-hidden="true" />
          <span>
            {t("chat.acpHistoryPreview.title", {
              defaultValue: "History loaded from ACP resume",
            })}
          </span>
        </div>
        <blockquote className="rua-chat-acp-history-preview-scroll">
          <MarkdownBlock text={text} />
        </blockquote>
      </div>
    </article>
  );
}

function RuntimeNoteBlock({
  note,
  messageId,
  inputValue,
}: {
  note: ConversationRuntimeNote;
  messageId: string;
  inputValue?: unknown;
}) {
  const searchReveal = useContext(SearchRevealContext);
  const [open, setOpen] = useRenderState("note:" + messageId, false);
  useEffect(() => {
    if (searchReveal?.has(messageId)) setOpen(true);
  }, [searchReveal, messageId, setOpen]);
  const runner =
    runtimeNoteField(note, "runner_name") ??
    runtimeNoteField(note, "tool_name");
  const isTaskTerminal = note.kind === "task_attempt_terminal";
  const title = isTaskTerminal
    ? `Task${runner ? ` · ${String(runner)}` : ""}`
    : runner ??
      (note.kind === "memory_signal"
        ? "Memory signal"
        : note.kind === "conversation_runtime_terminal"
        ? "Conversation"
        : "Runtime note");
  const status =
    runtimeNoteTerminalStatus(note) ??
    runtimeNoteField(note, "confidence") ??
    note.kind;
  const summary =
    runtimeNoteField(note, "output") ??
    runtimeNoteField(note, "summary") ??
    runtimeNoteField(note, "evidence");
  const identity =
    runtimeNoteField(note, "task_id") ?? runtimeNoteField(note, "turn_id");
  const watchRecords = taskWatchOutputRecords(note);
  return (
    <LocalOperateCard
      className={`rua-local-runtime-note${
        isTaskTerminal ? " rua-local-task" : ""
      }`}
      sticky={isTaskTerminal}
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
      data-rua-task-id={
        isTaskTerminal && identity ? String(identity) : undefined
      }
      summary={
        <>
          <span className="rua-local-operate-card-icon">
            {isTaskTerminal ? <ListChecks size={15} /> : <Wrench size={15} />}
          </span>
          <span className="rua-local-runtime-note-title">
            {String(title)}
          </span>
          <span className="rua-local-runtime-note-status">
            {String(status)}
          </span>
        </>
      }
    >
      <div className="rua-local-runtime-note-body">
        {isTaskTerminal && inputValue != null && (
          <CodeBlock title="Input" value={inputValue} display="object-fields" />
        )}
        {isTaskTerminal && summary != null ? (
          <CodeBlock title="Output" value={String(summary)} />
        ) : summary != null ? (
          <div className="rua-local-runtime-note-summary">
            {String(summary)}
          </div>
        ) : null}
        {watchRecords.length > 0 && (
          <div
            className="rua-local-runtime-note-output"
            aria-label="Task output"
          >
            {watchRecords.map((record, index) => (
              <div
                className="rua-local-runtime-note-output-record"
                key={`${record.source}-${index}`}
              >
                <span className="rua-local-runtime-note-output-source">
                  {record.source}
                </span>
                <pre>{record.text}</pre>
              </div>
            ))}
          </div>
        )}
        <div className="rua-local-runtime-note-meta">
          {identity != null && <span>{String(identity)}</span>}
          <span>{note.kind}</span>
        </div>
      </div>
    </LocalOperateCard>
  );
}

function taskWatchOutputRecords(
  note: ConversationRuntimeNote,
): Array<{ source: string; text: string }> {
  if (note.kind !== "task_watch_output") return [];
  const records = runtimeNoteField(note, "records");
  if (!Array.isArray(records)) return [];

  return records.flatMap((record) => {
    if (!record || typeof record !== "object" || Array.isArray(record))
      return [];
    const source = "source" in record ? record.source : undefined;
    const text = "text" in record ? record.text : undefined;
    if (typeof text !== "string") return [];
    return [{ source: typeof source === "string" ? source : "output", text }];
  });
}

function InterruptedTurnNotice({
  kind = "aborted",
}: {
  kind?: "aborted" | "interjected";
}) {
  const { t } = useTranslation();
  return (
    <div className="rua-chat-interrupted" role="status">
      {kind === "interjected"
        ? t("chat.turnInterjected", {
            defaultValue: "用户已插入新消息，本轮已结束",
          })
        : t("chat.turnInterrupted", {
            defaultValue: "用户已中断本轮",
          })}
    </div>
  );
}

function toolApprovalData(
  msg: ConversationOutputMessage,
): Record<string, unknown> {
  return toolApprovalDataFromMessage(msg);
}

function approvalStatusLabel(t: Translate, status: string): string {
  if (status === "pending" || status === "waiting")
    return t("chat.approval.status.pending", { defaultValue: "Pending" });
  if (status === "approved")
    return t("chat.approval.status.approved", { defaultValue: "Approved" });
  if (status === "rejected")
    return t("chat.approval.status.rejected", { defaultValue: "Rejected" });
  if (status === "denied")
    return t("chat.approval.status.denied", { defaultValue: "Denied" });
  if (status === "expired")
    return t("chat.approval.status.expired", { defaultValue: "Expired" });
  if (status === "consumed")
    return t("chat.approval.status.consumed", { defaultValue: "Consumed" });
  if (status === "superseded")
    return t("chat.approval.status.superseded", { defaultValue: "Superseded" });
  return status;
}

function approvalPrompt(data: Record<string, unknown>): string {
  const trigger = data.trigger;
  if (trigger && typeof trigger === "object") {
    const message = (trigger as Record<string, unknown>).message;
    if (typeof message === "string" && message.trim()) {
      if (message.includes("(Balanced)")) {
        return "Balanced mode asks before shell, write/edit, MCP/plugin, and web tool calls.";
      }
      if (message.includes("(Ask First)")) {
        return "Ask First mode requests authorization before every tool call.";
      }
      return message.trim();
    }
  }
  const prompt = data.reason ?? data.prompt;
  return typeof prompt === "string" ? prompt.trim() : "";
}

function approvalTriggerSource(data: Record<string, unknown>): string {
  const trigger = data.trigger;
  if (!trigger || typeof trigger !== "object") return "";
  const source = (trigger as Record<string, unknown>).source;
  return typeof source === "string" ? source : "";
}

function approvalTriggerSourceLabel(t: Translate, source: string): string {
  if (source === "approval_mode")
    return t("chat.approval.source.approvalMode", {
      defaultValue: "Approval Mode",
    });
  if (source === "tool_guard")
    return t("chat.approval.source.safetyRule", {
      defaultValue: "Safety Rule",
    });
  if (source === "file_guard")
    return t("chat.approval.source.fileGuard", { defaultValue: "File Guard" });
  return source;
}

function countLabel(
  t: Translate,
  key: string,
  count: number,
  defaultValue: string,
): string {
  return t(key, { count, defaultValue }).replace("{{count}}", String(count));
}

function approvalStatus(msg: ConversationOutputMessage): string {
  const approval = toolApprovalData(msg);
  return typeof approval.status === "string" ? approval.status : "";
}

function hasWaitingApproval(msg: ConversationOutputMessage): boolean {
  const status = approvalStatus(msg);
  return status === "waiting" || status === "pending";
}

function hasRejectedApproval(msg: ConversationOutputMessage): boolean {
  return approvalStatus(msg) === "rejected";
}

function hasToolOutput(msg: ConversationOutputMessage): boolean {
  return dataAt(msg, 1) !== undefined || String(msg.type).endsWith("_output");
}

function isRunningTool(msg: ConversationOutputMessage): boolean {
  if (msg.type !== "plugin_call" && msg.type !== "mcp_call") return false;
  const status = outputStatus(msg);
  return (
    !hasToolOutput(msg) && (status === "in_progress" || status === "created")
  );
}

function dataForPartType(
  msg: ConversationOutputMessage,
  type: string,
): Record<string, unknown> {
  for (const part of msg.content) {
    if (part.type === type) {
      const data = part.data;
      if (data && typeof data === "object") {
        return data as Record<string, unknown>;
      }
      return part as Record<string, unknown>;
    }
  }
  const first = dataAt(msg, 0);
  return first && typeof first === "object"
    ? (first as Record<string, unknown>)
    : {};
}

type AskUserQuestion = {
  question: string;
  options: string[];
  multiSelect: boolean;
};

function normalizeAskUserQuestions(rawQuestions: unknown): AskUserQuestion[] {
  const rawList = Array.isArray(rawQuestions) ? rawQuestions : [];
  return rawList
    .map((raw): AskUserQuestion | null => {
      if (!raw || typeof raw !== "object") return null;
      const record = raw as Record<string, unknown>;
      const question =
        typeof record.question === "string" ? record.question : "";
      if (!question) return null;
      const options = Array.isArray(record.options)
        ? record.options.filter(
            (option): option is string => typeof option === "string",
          )
        : [];
      return {
        question,
        options,
        multiSelect:
          record.multi_select === true ||
          record.multiSelect === true ||
          record.type === "multi_select",
      };
    })
    .filter((question): question is AskUserQuestion => question !== null);
}

function askUserQuestions(msg: ConversationOutputMessage): AskUserQuestion[] {
  const data = dataForPartType(msg, "ask_user_request");
  const questions = normalizeAskUserQuestions(data.questions);
  if (questions.length > 0) return questions;
  const prompt = typeof data.prompt === "string" ? data.prompt : "";
  return prompt ? [{ question: prompt, options: [], multiSelect: false }] : [];
}

function askUserStatus(msg: ConversationOutputMessage): string {
  const data = dataForPartType(msg, "ask_user_request");
  return typeof data.status === "string" ? data.status : "waiting";
}

function selectedAnswerText(
  questions: AskUserQuestion[],
  selections: Record<number, string[]>,
  textAnswers: Record<number, string>,
): string {
  const answers = questions.map((question, index) => ({
    question: question.question,
    value:
      question.options.length > 0
        ? (selections[index] || []).join("、").trim()
        : (textAnswers[index] || "").trim(),
  }));
  if (answers.length === 1) {
    return answers[0].value;
  }
  return answers
    .map((answer, index) => {
      return `${index + 1}. ${answer.question}  ${answer.value}`;
    })
    .join("\n\n")
    .trim();
}

function AskUserRequestBlock({
  msg,
  answerText,
  onAnswerUser,
}: {
  msg: ConversationOutputMessage;
  answerText?: string;
  onAnswerUser?: (answer: string) => Promise<void> | void;
}) {
  const questions = askUserQuestions(msg);
  const status = askUserStatus(msg);
  const isAnswered = Boolean(answerText?.trim()) || status === "answered";
  const isWaiting = status === "waiting" && !isAnswered;
  const [selections, setSelections] = useRenderState<Record<number, string[]>>(
    "ask-options:" + msg.id,
    {},
  );
  const [textAnswers, setTextAnswers] = useRenderState<Record<number, string>>(
    "ask-text:" + msg.id,
    {},
  );
  useRenderProtection(isWaiting);
  if (questions.length === 0) return null;
  const canSubmit =
    isWaiting &&
    questions.every((question, index) =>
      question.options.length > 0
        ? (selections[index] || []).length > 0
        : Boolean((textAnswers[index] || "").trim()),
    );
  const submitAnswer = () => {
    if (!canSubmit) return;
    const answer = selectedAnswerText(questions, selections, textAnswers);
    if (answer) void onAnswerUser?.(answer);
  };
  const toggleOption = (
    question: AskUserQuestion,
    questionIndex: number,
    option: string,
  ) => {
    if (!isWaiting) return;
    setSelections((current) => {
      const selected = current[questionIndex] || [];
      if (!question.multiSelect) {
        return { ...current, [questionIndex]: [option] };
      }
      const next = selected.includes(option)
        ? selected.filter((item) => item !== option)
        : [...selected, option];
      return { ...current, [questionIndex]: next };
    });
  };

  return (
    <div className="rua-local-operate-card rua-local-ask-user-card">
      <div className="rua-local-ask-user-header">
        <span className="rua-local-operate-card-icon">
          <MessageCircle size={15} />
        </span>
        <span className="rua-local-ask-user-title">需要你回答</span>
        <span className="rua-local-ask-user-hint">
          如果没有合适的选项，可以直接输入文字回复
        </span>
      </div>
      <div className="rua-local-ask-user-body">
        {questions.map((question, index) => (
          <div className="rua-local-ask-user-question" key={index}>
            <div className="rua-local-ask-user-question-text">
              {question.question}
            </div>
            {question.options.length === 0 && isWaiting && (
              <textarea
                className="rua-local-ask-user-textarea"
                value={textAnswers[index] || ""}
                rows={3}
                placeholder="输入你的回答"
                disabled={!onAnswerUser}
                onChange={(event) => {
                  const value = event.target.value;
                  setTextAnswers((current) => ({
                    ...current,
                    [index]: value,
                  }));
                }}
              />
            )}
            {question.options.length > 0 && (
              <div className="rua-local-ask-user-options">
                {question.options.map((option) => {
                  const selected = (selections[index] || []).includes(option);
                  return (
                    <button
                      type="button"
                      className={`rua-local-ask-user-option${
                        selected ? " is-selected" : ""
                      }`}
                      key={option}
                      aria-pressed={selected}
                      disabled={!isWaiting || !onAnswerUser}
                      onClick={() => toggleOption(question, index, option)}
                    >
                      {option}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        ))}
        {answerText?.trim() && (
          <div className="rua-local-ask-user-answer">
            <span className="rua-local-ask-user-answer-label">你的回答</span>
            <span className="rua-local-ask-user-answer-text">
              {answerText.trim()}
            </span>
          </div>
        )}
        {isWaiting && (
          <div className="rua-local-ask-user-actions">
            <button
              type="button"
              className="rua-local-ask-user-submit"
              disabled={!canSubmit || !onAnswerUser}
              onClick={submitAnswer}
            >
              <Check size={14} />
              <span>提交回答</span>
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

function outputStatus(msg: ConversationOutputMessage): string {
  if (typeof msg.status === "string") return msg.status;
  for (const part of msg.content) {
    if (typeof part.status === "string") return part.status;
    const data = part.data;
    if (data && typeof data === "object") {
      const status = (data as Record<string, unknown>).status;
      if (typeof status === "string") return status;
    }
  }
  return "";
}

function getToolTitle(msg: ConversationOutputMessage): string {
  const records = msg.content
    .map((_, index) => dataAt(msg, index))
    .filter(
      (value): value is Record<string, unknown> =>
        Boolean(value) && typeof value === "object",
    );
  const rawName = records
    .map((record) =>
      typeof record.name === "string" && record.name
        ? record.name
        : typeof record.tool_name === "string" && record.tool_name
        ? record.tool_name
        : "",
    )
    .find(Boolean);
  const name = rawName ?? "";
  const serverLabel =
    records
      .map((record) =>
        typeof record.server_label === "string" && record.server_label
          ? record.server_label
          : "",
      )
      .find(Boolean) || "";
  const parts = [serverLabel, name].filter(Boolean);

  return parts.length ? parts.join(" / ") : "Call tool";
}

function toolInputValue(msg: ConversationOutputMessage): unknown {
  const firstData = dataAt(msg, 0);
  const firstRecord =
    firstData && typeof firstData === "object"
      ? (firstData as Record<string, unknown>)
      : {};
  return (
    firstRecord.arguments ??
    firstRecord.input ??
    (msg.type?.endsWith("_output") ? null : firstData)
  );
}

function LocalReasoning({
  msg,
  defaultOpen,
}: {
  msg: ConversationOutputMessage;
  defaultOpen: boolean;
}) {
  const searchReveal = useContext(SearchRevealContext);
  const [open, setOpen] = useRenderState(
    "reasoning:" + msg.id,
    defaultOpen,
    defaultOpen,
  );
  useEffect(() => {
    if (searchReveal?.has(msg.id)) setOpen(true);
  }, [searchReveal, msg.id, setOpen]);

  if (outputStatus(msg) === "created") return null;
  const text = textFromParts(msg.content);
  if (!text.trim()) return null;

  return (
    <details
      className="rua-local-operate-card rua-local-reasoning"
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary>
        <span className="rua-local-operate-card-icon">
          <Lightbulb size={15} />
        </span>
        <span>Thinking</span>
      </summary>
      {open && (
        <MarkdownBlock
          text={text}
          cursor={outputStatus(msg) === "in_progress"}
        />
      )}
    </details>
  );
}

function hasRenderableAssistantMessage(
  msg: ConversationOutputMessage,
): boolean {
  if (msg.type === "turn_aborted" || msg.type === "turn_interjected") {
    return false;
  }
  if (msg.runtime_note) return true;
  if (
    msg.type === "plugin_call" ||
    msg.type === "plugin_call_output" ||
    msg.type === "mcp_call" ||
    msg.type === "mcp_call_output" ||
    msg.type === "ask_user_request"
  ) {
    return true;
  }
  return hasRenderableContentParts(msg.content);
}

function LocalTool({
  msg,
  generating,
  defaultOpen,
}: {
  msg: ConversationOutputMessage;
  generating?: boolean;
  defaultOpen: boolean;
}) {
  const { t } = useTranslation();
  const normalized = msg;
  const firstData = dataAt(normalized, 0);
  const secondData = dataAt(normalized, 1);
  const secondRecord =
    secondData && typeof secondData === "object"
      ? (secondData as Record<string, unknown>)
      : {};
  const inputValue = toolInputValue(normalized);
  const hasPairedOutput = secondData !== undefined;
  const outputValue =
    secondRecord.output ??
    secondRecord.content ??
    (normalized.type?.endsWith("_output") ? firstData : secondData);
  const status = outputStatus(normalized);
  const approval = toolApprovalData(normalized);
  const approvalStatus =
    typeof approval.status === "string" ? approval.status : "";
  const approvalRequestId =
    typeof approval.approval_request_id === "string"
      ? approval.approval_request_id
      : typeof approval.id === "string"
      ? approval.id
      : "";
  const approvalReason = approvalPrompt(approval);
  const approvalTrigger = approvalTriggerSource(approval);
  const approvalWaiting =
    approvalStatus === "waiting" || approvalStatus === "pending";
  const inferredRunning =
    generating &&
    !hasPairedOutput &&
    !approvalWaiting &&
    (normalized.type === "plugin_call" || normalized.type === "mcp_call") &&
    !!toolCallId(normalized);
  const isRunning =
    (!hasPairedOutput && status === "in_progress" && !approvalWaiting) ||
    inferredRunning;
  const searchReveal = useContext(SearchRevealContext);
  const callId = toolCallId(normalized);
  const [open, setOpen] = useRenderState(
    "tool:" + normalized.id,
    defaultOpen,
    defaultOpen,
  );
  useEffect(() => {
    if (searchReveal?.has(callId || normalized.id)) setOpen(true);
  }, [searchReveal, callId, normalized.id, setOpen]);

  return (
    <LocalOperateCard
      className="rua-local-tool"
      data-rua-tool-call-id={toolCallId(normalized) || undefined}
      data-rua-approval-id={approvalRequestId || undefined}
      tabIndex={-1}
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
      summary={
        <>
          <span className="rua-local-operate-card-icon">
            {isRunning ? (
              <LoaderCircle className="rua-local-tool-spinner" size={15} />
            ) : (
              <Wrench size={15} />
            )}
          </span>
          <span>{getToolTitle(normalized)}</span>
          {approvalStatus && (
            <span className="rua-local-approval-status">
              {approvalStatusLabel(t, approvalStatus)}
            </span>
          )}
        </>
      }
    >
      <div className="rua-local-tool-body">
        {approvalReason && (
          <div className="rua-local-approval-reason">
            {approvalTrigger && (
              <span className="rua-local-approval-source">
                {approvalTriggerSourceLabel(t, approvalTrigger)}
              </span>
            )}
            <span>{approvalReason}</span>
          </div>
        )}
        <CodeBlock
          title={
            approvalWaiting
              ? t("chat.approval.toolInput", { defaultValue: "Tool input" })
              : "Input"
          }
          value={inputValue}
          display="object-fields"
        />
        <CodeBlock
          title="Output"
          value={outputValue}
          display="single-array-object-fields"
        />
      </div>
    </LocalOperateCard>
  );
}

function OutputBlock({
  msg,
  generating,
  blockPreferences,
  askUserAnswerText,
  onAnswerUser,
  allMessages,
}: {
  msg: ConversationOutputMessage;
  generating?: boolean;
  blockPreferences: ChatBlockCollapsePreferences;
  askUserAnswerText?: string;
  onAnswerUser?: (answer: string) => Promise<void> | void;
  allMessages?: ConversationOutputMessage[];
}) {
  if (msg.runtime_note) {
    const originToolCallId = runtimeNoteField(msg.runtime_note, "tool_call_id");
    const originTool =
      typeof originToolCallId === "string"
        ? allMessages?.find(
            (candidate) => toolCallId(candidate) === originToolCallId,
          )
        : undefined;
    return (
      <RuntimeNoteBlock
        note={msg.runtime_note}
        messageId={msg.id}
        inputValue={originTool ? toolInputValue(originTool) : undefined}
      />
    );
  }
  if (msg.type === "ask_user_request") {
    return (
      <AskUserRequestBlock
        msg={msg}
        answerText={askUserAnswerText}
        onAnswerUser={onAnswerUser}
      />
    );
  }
  if (msg.type === "tool_approval_request") {
    return null;
  }
  if (msg.type === "reasoning") {
    return (
      <LocalReasoning
        msg={msg}
        defaultOpen={blockPreferences.thinkingExpanded}
      />
    );
  }
  if (
    msg.type === "plugin_call" ||
    msg.type === "plugin_call_output" ||
    msg.type === "mcp_call" ||
    msg.type === "mcp_call_output"
  ) {
    return (
      <LocalTool
        msg={msg}
        generating={generating}
        defaultOpen={blockPreferences.toolExpanded}
      />
    );
  }
  return (
    <ContentParts
      parts={msg.content}
      streaming={Boolean(generating && outputStatus(msg) !== "completed")}
    />
  );
}

function parseToolArguments(value: unknown): Record<string, unknown> {
  if (value && typeof value === "object") {
    return value as Record<string, unknown>;
  }
  if (typeof value !== "string" || !value.trim()) return {};
  try {
    const parsed = JSON.parse(value) as unknown;
    return parsed && typeof parsed === "object"
      ? (parsed as Record<string, unknown>)
      : {};
  } catch {
    return {};
  }
}

type WorkChecklistStep = {
  step: string;
  status: "pending" | "in_progress" | "completed";
};

type WorkChecklist = {
  explanation?: string;
  plan: WorkChecklistStep[];
};

function workChecklistFromToolCall(
  msg: ConversationOutputMessage,
): WorkChecklist | null {
  if (msg.type !== "plugin_call" && msg.type !== "mcp_call") return null;
  const records = msg.content
    .map((_, index) => dataAt(msg, index))
    .filter(
      (value): value is Record<string, unknown> =>
        Boolean(value) && typeof value === "object",
    );
  const call = records.find((record) => record.name === "update_plan");
  if (!call) return null;
  const args = parseToolArguments(call.arguments);
  if (!Array.isArray(args.plan)) return null;

  let inProgressCount = 0;
  const plan: WorkChecklistStep[] = [];
  for (const value of args.plan) {
    if (!value || typeof value !== "object") return null;
    const record = value as Record<string, unknown>;
    const step = typeof record.step === "string" ? record.step.trim() : "";
    const status = record.status;
    if (
      !step ||
      (status !== "pending" &&
        status !== "in_progress" &&
        status !== "completed")
    ) {
      return null;
    }
    if (status === "in_progress") inProgressCount += 1;
    plan.push({ step, status });
  }
  if (inProgressCount > 1) return null;

  const explanation =
    typeof args.explanation === "string" && args.explanation.trim()
      ? args.explanation.trim()
      : undefined;
  return { explanation, plan };
}

function WorkChecklistBlock({ checklist }: { checklist: WorkChecklist }) {
  const { t } = useTranslation();
  return (
    <section
      className="rua-work-checklist"
      data-rua-work-checklist="true"
      aria-label={t("chat.workChecklist.title", { defaultValue: "Plan" })}
    >
      <div className="rua-work-checklist-header">
        <ListChecks size={16} />
        <span>{t("chat.workChecklist.title", { defaultValue: "Plan" })}</span>
      </div>
      {checklist.explanation && (
        <div className="rua-work-checklist-explanation">
          {checklist.explanation}
        </div>
      )}
      <ol className="rua-work-checklist-steps">
        {checklist.plan.map((item, index) => (
          <li
            key={`${index}:${item.step}`}
            className={`rua-work-checklist-step is-${item.status}`}
            data-status={item.status}
          >
            <span className="rua-work-checklist-step-icon" aria-hidden="true">
              {item.status === "completed" ? (
                <Check size={14} />
              ) : item.status === "in_progress" ? (
                <ArrowRightToLine size={14} />
              ) : (
                <Circle size={13} />
              )}
            </span>
            <span>{item.step}</span>
          </li>
        ))}
      </ol>
    </section>
  );
}

function askUserRequestFromToolCall(
  msg: ConversationOutputMessage,
): ConversationOutputMessage | null {
  const data = dataAt(msg, 0);
  if (!data || typeof data !== "object") return null;
  const record = data as Record<string, unknown>;
  const name = typeof record.name === "string" ? record.name : "";
  if (name !== "ask_user") return null;
  const args = parseToolArguments(record.arguments);
  const questions =
    normalizeAskUserQuestions(args.questions).length > 0
      ? args.questions
      : typeof args.question === "string"
      ? [
          {
            question: args.question,
            options: Array.isArray(args.options) ? args.options : [],
            multi_select: args.mode === "multi_select",
          },
        ]
      : [];
  if (normalizeAskUserQuestions(questions).length === 0) return null;
  return {
    id: `${msg.id}-ask-user-request`,
    role: "assistant",
    type: "ask_user_request",
    status: "completed",
    content: [
      {
        type: "ask_user_request",
        data: {
          id: `${msg.id}-ask-user-request`,
          request_id: `${msg.id}-ask-user-request`,
          questions,
          status: "waiting",
        },
      },
    ],
  } as ConversationOutputMessage;
}

function messageHasAskUserRequest(msg: ConversationOutputMessage): boolean {
  return (
    msg.type === "ask_user_request" || askUserRequestFromToolCall(msg) !== null
  );
}

const assistantAskUserCache = new WeakMap<
  ConversationAssistantResponse,
  boolean
>();
function assistantHasAskUser(item: ConversationAssistantResponse): boolean {
  const cached = assistantAskUserCache.get(item);
  if (cached !== undefined) return cached;
  const value = mergeToolMessagesForApproval(
    item.output,
    undefined,
    askUserRequestFromToolCall,
  ).some(messageHasAskUserRequest);
  assistantAskUserCache.set(item, value);
  return value;
}

type Segment =
  | {
      kind: "tool-group";
      items: ConversationOutputMessage[];
      messageIds: string[];
    }
  | {
      kind: "work-checklist";
      checklist: WorkChecklist;
      item: ConversationOutputMessage;
      messageIds: string[];
    }
  | { kind: "single"; item: ConversationOutputMessage };

function taskRuntimeNoteToolCallId(
  message: ConversationOutputMessage,
): string | null {
  if (!message.runtime_note || !message.runtime_note.kind.startsWith("task_")) {
    return null;
  }
  const callId = runtimeNoteField(message.runtime_note, "tool_call_id");
  return typeof callId === "string" && callId.trim() ? callId : null;
}

function groupedToolCallId(message: ConversationOutputMessage): string | null {
  return toolCallId(message) || taskRuntimeNoteToolCallId(message);
}

function groupMessages(messages: ConversationOutputMessage[]): Segment[] {
  const segments: Segment[] = [];
  let currentGroup: ConversationOutputMessage[] = [];
  const attachedTaskNoteIds = new Set<string>();
  const taskNotesByToolCallId = new Map<string, ConversationOutputMessage[]>();
  const visibleToolCallIds = new Set(
    messages
      .filter((message) => TOOL_GROUP_TYPES.has(String(message.type)))
      .map(toolCallId)
      .filter((callId): callId is string => Boolean(callId)),
  );
  for (const message of messages) {
    const callId = taskRuntimeNoteToolCallId(message);
    if (!callId || !visibleToolCallIds.has(callId)) continue;
    const notes = taskNotesByToolCallId.get(callId) ?? [];
    notes.push(message);
    taskNotesByToolCallId.set(callId, notes);
    attachedTaskNoteIds.add(message.id);
  }
  const checklists = messages.map(workChecklistFromToolCall);
  let latestChecklistIndex = -1;
  for (let index = 0; index < checklists.length; index += 1) {
    if (checklists[index]) latestChecklistIndex = index;
  }

  const flushGroup = () => {
    if (
      currentGroup.length === 1 &&
      Object.keys(toolApprovalData(currentGroup[0])).length > 0
    ) {
      segments.push({ kind: "single", item: currentGroup[0] });
      currentGroup = [];
    } else if (currentGroup.length > 0) {
      const callIds = new Set(
        currentGroup
          .map(toolCallId)
          .filter((callId): callId is string => Boolean(callId)),
      );
      for (const callId of callIds) {
        currentGroup.push(...(taskNotesByToolCallId.get(callId) ?? []));
      }
      segments.push({
        kind: "tool-group",
        items: currentGroup,
        messageIds: currentGroup.map((item) => item.id),
      });
      currentGroup = [];
    }
  };

  for (let index = 0; index < messages.length; index += 1) {
    const msg = messages[index];
    if (attachedTaskNoteIds.has(msg.id)) continue;
    const checklist = checklists[index];
    if (checklist) {
      flushGroup();
      if (index === latestChecklistIndex) {
        segments.push({
          kind: "work-checklist",
          checklist,
          item: msg,
          messageIds: [msg.id],
        });
      }
      continue;
    }
    if (TOOL_GROUP_TYPES.has(String(msg.type))) {
      currentGroup.push(msg);
    } else {
      flushGroup();
      segments.push({ kind: "single", item: msg });
    }
  }
  flushGroup();
  return segments;
}

function groupMessagesForContextEditing(
  messages: ConversationOutputMessage[],
  originalMessages: ConversationOutputMessage[],
): Segment[] {
  const segments: Segment[] = [];
  const consumed = new Set<string>();
  const originalIdsByCallId = new Map<string, string[]>();
  for (const original of originalMessages) {
    const callId = groupedToolCallId(original);
    if (!callId) continue;
    const ids = originalIdsByCallId.get(callId) ?? [];
    ids.push(original.id);
    originalIdsByCallId.set(callId, ids);
  }
  for (const item of messages) {
    if (consumed.has(item.id)) continue;
    const checklist = workChecklistFromToolCall(item);
    if (checklist) {
      const callId = toolCallId(item);
      segments.push({
        kind: "work-checklist",
        checklist,
        item,
        messageIds: callId
          ? originalIdsByCallId.get(callId) ?? [item.id]
          : [item.id],
      });
      consumed.add(item.id);
      continue;
    }
    const callId = groupedToolCallId(item);
    if (callId) {
      const paired = messages.filter(
        (candidate) =>
          !consumed.has(candidate.id) &&
          groupedToolCallId(candidate) === callId,
      );
      for (const candidate of paired) consumed.add(candidate.id);
      segments.push({
        kind: "tool-group",
        items: paired,
        messageIds:
          originalIdsByCallId.get(callId) ?? paired.map((item) => item.id),
      });
      continue;
    }
    consumed.add(item.id);
    segments.push({ kind: "single", item });
  }
  return segments;
}

function segmentMessageIds(segment: Segment): string[] {
  return segment.kind === "single" ? [segment.item.id] : segment.messageIds;
}

function CollapsibleToolGroup({
  items,
  generating,
  blockPreferences,
  askUserAnswerText,
  onAnswerUser,
}: {
  items: ConversationOutputMessage[];
  generating?: boolean;
  blockPreferences: ChatBlockCollapsePreferences;
  askUserAnswerText?: string;
  onAnswerUser?: (answer: string) => Promise<void> | void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useRenderState(
    "tool-group:" + items[0]?.id,
    blockPreferences.aggregateExpanded,
    blockPreferences.aggregateExpanded,
  );
  const searchReveal = useContext(SearchRevealContext);
  const revealGroup = Boolean(
    searchReveal &&
      items.some(
        (item) =>
          searchReveal.has(groupedToolCallId(item) || item.id) ||
          searchReveal.has(item.id),
      ),
  );
  useEffect(() => {
    if (revealGroup) setOpen(true);
  }, [searchReveal, revealGroup, setOpen]);
  const thinkingCount = items.filter(
    (item) => item.type === "reasoning",
  ).length;
  const toolCount = items.filter(
    (item) => item.type !== "reasoning" && !item.runtime_note,
  ).length;
  const waitingApprovalCount = items.filter(hasWaitingApproval).length;
  const approvedApprovalCount = items.filter(
    (item) => approvalStatus(item) === "approved",
  ).length;
  const rejectedApprovalCount = items.filter(hasRejectedApproval).length;
  const runningToolCount = items.filter(isRunningTool).length;
  const summaryParts: string[] = [];
  if (thinkingCount)
    summaryParts.push(
      countLabel(
        t,
        "chat.toolGroup.thinkingCount",
        thinkingCount,
        "{{count}} thinking",
      ),
    );
  if (toolCount) {
    summaryParts.push(
      countLabel(
        t,
        "chat.toolGroup.toolCallCount",
        toolCount,
        toolCount === 1 ? "{{count}} tool call" : "{{count}} tool calls",
      ),
    );
  }
  if (runningToolCount)
    summaryParts.push(
      countLabel(
        t,
        "chat.toolGroup.runningCount",
        runningToolCount,
        "{{count}} running",
      ),
    );
  if (waitingApprovalCount)
    summaryParts.push(
      countLabel(
        t,
        "chat.toolGroup.pendingApprovalCount",
        waitingApprovalCount,
        "{{count}} pending",
      ),
    );
  if (approvedApprovalCount)
    summaryParts.push(
      countLabel(
        t,
        "chat.toolGroup.approvedApprovalCount",
        approvedApprovalCount,
        "{{count}} approved",
      ),
    );
  if (rejectedApprovalCount)
    summaryParts.push(
      countLabel(
        t,
        "chat.toolGroup.rejectedApprovalCount",
        rejectedApprovalCount,
        "{{count}} rejected",
      ),
    );
  const expanded = open;
  const showSpinner = Boolean(generating);

  return (
    <div
      className={[
        "rua-tool-group",
        expanded ? "pinned-open" : "collapsed",
        showSpinner ? "is-generating" : "",
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <button
        type="button"
        className="rua-tool-group-summary"
        aria-expanded={expanded}
        onClick={() => {
          setOpen((prev) => !prev);
        }}
      >
        <span className="rua-tool-group-icon">{expanded ? "▾" : "▸"}</span>
        {showSpinner && (
          <LoaderCircle className="rua-tool-group-spinner" size={13} />
        )}
        <span>{summaryParts.join(", ")}</span>
      </button>
      {expanded && (
        <div className="rua-tool-group-content">
          {items.map((item) => (
            <RenderStateScope key={item.id} name={"tool-output:" + item.id}>
              <OutputBlock
                msg={item}
                generating={generating}
                blockPreferences={blockPreferences}
                askUserAnswerText={askUserAnswerText}
                onAnswerUser={onAnswerUser}
                allMessages={items}
              />
            </RenderStateScope>
          ))}
        </div>
      )}
    </div>
  );
}

function selectedVariantIndex(item: ConversationAssistantResponse): number {
  const selectedIndex = Math.max(
    0,
    item.variants.findIndex(
      (variant) => variant.variant_id === item.selected_variant_id,
    ),
  );
  return selectedIndex;
}

function selectedVariantId(item: ConversationAssistantResponse): string | null {
  return (
    item.selected_variant_id ||
    item.variants[selectedVariantIndex(item)]?.variant_id ||
    item.variants[0]?.variant_id ||
    null
  );
}

function tx(
  t: ReturnType<typeof useTranslation>["t"],
  key: string,
  fallback: string,
): string {
  return t(key, { defaultValue: fallback });
}

function VariantPager({
  item,
  disabled,
  onSelectVariant,
}: {
  item: ConversationAssistantResponse;
  disabled?: boolean;
  onSelectVariant?: (turnId: string, variantId: string) => Promise<void> | void;
}) {
  const { t } = useTranslation();
  if (item.variants.length <= 1 || !item.turn_id) return null;
  const selectedIndex = selectedVariantIndex(item);
  const selectAt = (nextIndex: number) => {
    const next = item.variants[nextIndex];
    if (!next) return;
    void onSelectVariant?.(item.turn_id!, next.variant_id);
  };

  return (
    <div className="rua-response-variant-pager" data-rua-chat-variant-pager>
      <IconButton
        title={tx(t, "chat.previousVariant", "Previous variant")}
        className="rua-response-variant-button"
        disabled={disabled || selectedIndex <= 0}
        onClick={() => selectAt(selectedIndex - 1)}
      >
        <ChevronLeft size={14} />
      </IconButton>
      <span>
        {selectedIndex + 1}/{item.variants.length}
      </span>
      <IconButton
        title={tx(t, "chat.nextVariant", "Next variant")}
        className="rua-response-variant-button"
        disabled={disabled || selectedIndex >= item.variants.length - 1}
        onClick={() => selectAt(selectedIndex + 1)}
      >
        <ChevronRight size={14} />
      </IconButton>
    </div>
  );
}

function BranchActionsMenu({
  beforeTurnId,
  userText,
  disabled,
  onRollback,
  onBranch,
}: {
  beforeTurnId: string;
  userText: string;
  disabled?: boolean;
  onRollback?: (beforeTurnId: string, userText: string) => Promise<void> | void;
  onBranch?: (beforeTurnId: string, userText: string) => Promise<void> | void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<"rollback" | "branch" | null>(null);
  useRenderProtection(open || busy !== null);

  const runAction = async (action: "rollback" | "branch") => {
    setOpen(false);
    const confirmed = await Modal.confirm({
      title:
        action === "rollback"
          ? tx(t, "chat.rollbackConfirmTitle", "Rollback conversation?")
          : tx(t, "chat.branchConfirmTitle", "Create branch?"),
      content:
        action === "rollback"
          ? tx(
              t,
              "chat.rollbackConfirmContent",
              "This will remove this message and everything after it.",
            )
          : tx(
              t,
              "chat.branchConfirmContent",
              "This will create a new conversation from before this message.",
            ),
      okText:
        action === "rollback"
          ? tx(t, "chat.rollback", "Rollback")
          : tx(t, "chat.branch", "Branch"),
      cancelText: tx(t, "common.cancel", "Cancel"),
      danger: action === "rollback",
    });
    if (!confirmed) return;
    setBusy(action);
    try {
      const result =
        action === "rollback"
          ? onRollback?.(beforeTurnId, userText)
          : onBranch?.(beforeTurnId, userText);
      void Promise.resolve(result)
        .catch((err) => {
          toast.error(err instanceof Error ? err.message : String(err));
        })
        .finally(() => setBusy(null));
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
      setBusy(null);
    }
  };

  return (
    <Popover
      open={open && !disabled}
      onOpenChange={(nextOpen) => setOpen(nextOpen && !disabled)}
      trigger="click"
      placement="top-start"
      overlayClassName="rua-local-branch-menu-popover"
      content={
        <div role="menu">
          <button
            type="button"
            role="menuitem"
            disabled={disabled || busy !== null}
            className="rua-local-branch-menu-item"
            onClick={() => void runAction("rollback")}
          >
            <Undo2 size={14} />
            <span>
              {busy === "rollback"
                ? tx(t, "chat.rollingBack", "Rolling back...")
                : tx(t, "chat.rollback", "Rollback")}
            </span>
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={disabled || busy !== null}
            className="rua-local-branch-menu-item"
            onClick={() => void runAction("branch")}
          >
            <GitBranch size={14} />
            <span>
              {busy === "branch"
                ? tx(t, "chat.branching", "Branching...")
                : tx(t, "chat.branch", "Branch")}
            </span>
          </button>
        </div>
      }
    >
      <button
        type="button"
        className="rua-local-footer-action"
        title={tx(t, "chat.moreMessageActions", "More message actions")}
        aria-label={tx(t, "chat.moreMessageActions", "More message actions")}
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open && !disabled}
      >
        <MoreVertical />
      </button>
    </Popover>
  );
}

function usageNumber(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

const assistantUsageSnapshots = new WeakMap<
  ConversationAssistantResponse,
  TokenUsageSnapshot | null
>();

function assistantTokenUsage(
  item: ConversationAssistantResponse,
): TokenUsageSnapshot | null {
  const cached = assistantUsageSnapshots.get(item);
  if (cached !== undefined) return cached;
  for (let index = item.output.length - 1; index >= 0; index -= 1) {
    const snapshot = item.output[index]?.token_usage;
    if (snapshot) {
      assistantUsageSnapshots.set(item, snapshot);
      return snapshot;
    }
  }
  assistantUsageSnapshots.set(item, null);
  return null;
}

type CurrentContextUsage = {
  usage: TokenUsage;
  maxInputLength: number | null;
};

type ConversationUsageAggregate = {
  cumulative: TokenUsage | null;
};

const ConversationUsageContext = createContext<ConversationUsageAggregate>({
  cumulative: null,
});

function computeConversationUsage(
  items: readonly { kind: string }[],
): ConversationUsageAggregate {
  let prompt = 0;
  let completion = 0;
  let total = 0;
  let cacheRead = 0;
  let cacheWrite = 0;
  let seen = false;
  for (const raw of items) {
    if (raw.kind !== "assistant") continue;
    const snapshot = assistantTokenUsage(raw as ConversationAssistantResponse);
    if (!snapshot) continue;
    if (snapshot.usage) {
      seen = true;
      prompt += usageNumber(snapshot.usage.prompt_tokens);
      completion += usageNumber(snapshot.usage.completion_tokens);
      total +=
        usageNumber(snapshot.usage.total_tokens) ||
        usageNumber(snapshot.usage.prompt_tokens) +
          usageNumber(snapshot.usage.completion_tokens);
      cacheRead += usageNumber(snapshot.usage.cache_read_tokens);
      cacheWrite += usageNumber(snapshot.usage.cache_write_tokens);
    }
  }
  return {
    cumulative: seen
      ? {
          prompt_tokens: prompt,
          completion_tokens: completion,
          total_tokens: total,
          cache_read_tokens: cacheRead,
          cache_write_tokens: cacheWrite,
        }
      : null,
  };
}

function contextRatio(context: CurrentContextUsage | null): number {
  if (!context?.maxInputLength) return 0;
  return Math.max(
    0,
    Math.min(
      (usageNumber(context.usage.prompt_tokens) / context.maxInputLength) * 100,
      100,
    ),
  );
}

function usageRingColor(ratio: number): string {
  if (ratio >= 95) return "#cf1322";
  if (ratio >= 85) return "#f5222d";
  if (ratio >= 75) return "#fa8c16";
  if (ratio >= 50) return "#d48806";
  return "#389e0d";
}

function usageTotal(usage: TokenUsage | null | undefined): number {
  if (!usage) return 0;
  return (
    usageNumber(usage.total_tokens) ||
    usageNumber(usage.prompt_tokens) + usageNumber(usage.completion_tokens)
  );
}

function TokenUsageAction({
  usage,
  bubbleRef,
}: {
  usage: TokenUsageSnapshot | null;
  bubbleRef: RefObject<HTMLDivElement | null>;
}) {
  const { t } = useTranslation();
  const conversation = useContext(ConversationUsageContext);

  const turn = usage?.usage ?? null;
  const cumulative = conversation.cumulative;

  const context = usage?.last_call_usage
    ? {
        usage: usage.last_call_usage,
        maxInputLength: usage.max_input_length,
      }
    : null;

  if (!turn && !cumulative && !context) return null;

  const totalLabel = tx(t, "chat.turnUsage.total", "Total");
  const inLabel = tx(t, "chat.turnUsage.input", "in");
  const outLabel = tx(t, "chat.turnUsage.output", "out");
  const cachedLabel = tx(t, "chat.turnUsage.cached", "cached");
  const usageRow = (label: string, value: TokenUsage) => {
    const cacheRead = usageNumber(value.cache_read_tokens);
    return (
      <tr>
        <th scope="row">{label}</th>
        <td>{formatCompact(usageTotal(value))}</td>
        <td>{formatCompact(usageNumber(value.prompt_tokens))}</td>
        <td>{formatCompact(usageNumber(value.completion_tokens))}</td>
        <td>{cacheRead > 0 ? formatCompact(cacheRead) : "—"}</td>
      </tr>
    );
  };

  const ratio = contextRatio(context);
  const ringStyle = {
    "--rua-turn-usage-ratio": `${ratio}`,
    "--rua-turn-usage-color": usageRingColor(ratio),
  } as CSSProperties;

  return (
    <Popover
      content={
        <div className="rua-turn-usage-content">
          {(turn || cumulative) && (
            <div className="rua-turn-usage-section">
              <div className="rua-turn-usage-group-title">
                {tx(t, "chat.turnUsage.consumption", "Token usage")}
              </div>
              <table className="rua-turn-usage-table">
                <thead>
                  <tr>
                    <th
                      scope="col"
                      aria-label={tx(t, "chat.turnUsage.period", "Period")}
                    />
                    <th scope="col">{totalLabel}</th>
                    <th scope="col">{inLabel}</th>
                    <th scope="col">{outLabel}</th>
                    <th scope="col">{cachedLabel}</th>
                  </tr>
                </thead>
                <tbody>
                  {turn &&
                    usageRow(
                      tx(
                        t,
                        turn.estimated
                          ? "chat.turnUsage.turnEstimated"
                          : "chat.turnUsage.turn",
                        turn.estimated ? "This turn (estimated)" : "This turn",
                      ),
                      turn,
                    )}
                  {cumulative &&
                    usageRow(
                      tx(t, "chat.turnUsage.cumulative", "Conversation total"),
                      cumulative,
                    )}
                </tbody>
              </table>
            </div>
          )}
          {context && (
            <div className="rua-turn-usage-section">
              <div className="rua-turn-usage-group-title">
                {tx(t, "chat.turnUsage.context", "Context usage at this turn")}
                {context.maxInputLength
                  ? ` ${
                      ratio > 0 && ratio < 1
                        ? ratio.toFixed(1)
                        : Math.round(ratio)
                    }%`
                  : ""}
              </div>
              <div className="rua-turn-usage-usageline">
                {formatCompact(usageNumber(context.usage.prompt_tokens))}
                {context.maxInputLength
                  ? `/${formatCompact(context.maxInputLength)}`
                  : ""}
                {usageNumber(context.usage.cache_read_tokens) > 0
                  ? ` · ${cachedLabel} ${formatCompact(
                      usageNumber(context.usage.cache_read_tokens),
                    )}`
                  : ""}
              </div>
              <div className="rua-turn-usage-bar" aria-hidden>
                <span
                  style={{
                    width: `${ratio}%`,
                    background: usageRingColor(ratio),
                  }}
                />
              </div>
            </div>
          )}
        </div>
      }
      placement="top-start"
      trigger={["hover"]}
      positionReferenceRef={bubbleRef}
      positionReferencePoint="bottom-start"
      offsetDistance={0}
      overlayClassName="rua-turn-usage-popover"
    >
      <button
        type="button"
        className="rua-local-footer-action rua-turn-usage-trigger"
        aria-label={tx(t, "chat.turnUsage.ariaLabel", "Token usage")}
      >
        <span className="rua-turn-usage-ring" style={ringStyle} />
      </button>
    </Popover>
  );
}

function AssistantFooter({
  item,
  bubbleRef,
  previousUserText,
  isLast,
  disabled,
  onRegenerate,
}: {
  item: ConversationAssistantResponse;
  bubbleRef: RefObject<HTMLDivElement | null>;
  previousUserText: string;
  isLast: boolean;
  disabled?: boolean;
  onRegenerate?: (turnId: string, userText: string) => Promise<void> | void;
}) {
  const { t } = useTranslation();
  const text = assistantResponseText(item);
  const usage = assistantTokenUsage(item);
  return (
    <div className="rua-local-bubble-footer">
      <div className="rua-local-footer-actions">
        <TokenUsageAction usage={usage} bubbleRef={bubbleRef} />
        <CopyButton value={text} />
        <SpeakButton value={text} />
        {isLast && item.turn_id && (
          <IconButton
            title={tx(t, "chat.regenerate", "Regenerate")}
            disabled={disabled || !previousUserText}
            onClick={() => void onRegenerate?.(item.turn_id!, previousUserText)}
          >
            <RefreshCw />
          </IconButton>
        )}
      </div>
    </div>
  );
}

type AcpOrigin = { kind: "acp"; runner?: string };

function readAcpOrigin(
  msg: ConversationOutputMessage | undefined,
): AcpOrigin | null {
  if (!msg) return null;
  const meta = (msg as { metadata?: unknown }).metadata;
  if (!meta || typeof meta !== "object") return null;
  const origin = (meta as Record<string, unknown>).origin;
  if (!origin || typeof origin !== "object") return null;
  const kind = (origin as Record<string, unknown>).kind;
  if (kind !== "acp") return null;
  const runner = (origin as Record<string, unknown>).runner;
  return {
    kind: "acp",
    runner: typeof runner === "string" ? runner : undefined,
  };
}

function assistantAcpOrigin(
  item: ConversationAssistantResponse,
): AcpOrigin | null {
  return (
    readAcpOrigin(item.output[0]) ??
    (item.acp_origin_runner !== undefined
      ? {
          kind: "acp" as const,
          runner: item.acp_origin_runner || undefined,
        }
      : null)
  );
}

function readAssistantTimestamps(msg: unknown): {
  started?: string;
  finished?: string;
} {
  if (!msg || typeof msg !== "object") return {};
  const m = msg as {
    started_at?: unknown;
    finished_at?: unknown;
  };
  const started =
    typeof m.started_at === "string" && m.started_at.trim()
      ? m.started_at
      : undefined;
  const finished =
    typeof m.finished_at === "string" && m.finished_at.trim()
      ? m.finished_at
      : undefined;
  return { started, finished };
}

function fmtTime(raw: string | undefined): string {
  if (!raw || !raw.trim()) return "";

  let d = new Date(raw);
  if (isNaN(d.getTime())) {
    d = new Date(raw.replace(" ", "T"));
  }
  if (isNaN(d.getTime())) return "";
  return d.toLocaleTimeString(undefined, { hour12: false });
}

const AssistantCard = memo(function AssistantCard({
  item,
  approvalSnapshot,
  previousUserText,
  isLast,
  activeTurnResponse,
  onSelectVariant,
  onRegenerate,
  onAnswerUser,
  askUserAnswerText,
  blockPreferences,
  cardIndex,
  emptyOutputText,
  tapRevealed,
  onTapReveal,
  activeAcpRunner,
  contextEditorOpen,
  excludedMessageIds,
  draftExcludedMessageIds,
  onDraftExcludedMessageIdsChange,
}: {
  item: ConversationAssistantResponse;
  approvalSnapshot?: ApprovalSnapshot;
  previousUserText: string;
  isLast: boolean;
  cardIndex: number;
  emptyOutputText: string;
  activeTurnResponse: boolean;
  onSelectVariant?: (turnId: string, variantId: string) => Promise<void> | void;
  onRegenerate?: (turnId: string, userText: string) => Promise<void> | void;
  onAnswerUser?: (answer: string) => Promise<void> | void;
  askUserAnswerText?: string;
  blockPreferences: ChatBlockCollapsePreferences;
  tapRevealed?: boolean;
  onTapReveal?: (messageId: string) => void;
  activeAcpRunner?: string | null;
  contextEditorOpen: boolean;
  excludedMessageIds: Set<string>;
  draftExcludedMessageIds: Set<string>;
  onDraftExcludedMessageIdsChange?: (messageIds: Set<string>) => void;
}) {
  const { t } = useTranslation();
  const scopedApprovalSnapshot = useMemo(
    () => approvalSnapshotForAssistant(item, approvalSnapshot, isLast),
    [item, approvalSnapshot, isLast],
  );
  const messages = useMemo(
    () =>
      mergeToolMessagesForApproval(
        item.output,
        scopedApprovalSnapshot,
        askUserRequestFromToolCall,
      ).filter(hasRenderableAssistantMessage),
    [item.output, scopedApprovalSnapshot],
  );
  const segments = useMemo(
    () =>
      contextEditorOpen
        ? groupMessagesForContextEditing(messages, item.output)
        : groupMessages(messages),
    [contextEditorOpen, item.output, messages],
  );
  const isGenerating = activeTurnResponse;
  const isInterrupted = item.status === "interrupted";
  const interruptedKind = item.output.some(
    (msg) => msg.type === "turn_interjected",
  )
    ? "interjected"
    : "aborted";
  const hasRenderableOutput = messages.some(hasRenderableAssistantMessage);
  const variantIndex = selectedVariantIndex(item);
  const selectedVariant = selectedVariantId(item);
  const outputAnimationKey =
    item.variants.length > 1 && selectedVariant
      ? `variant:${selectedVariant}`
      : "single";
  const bubbleRef = useRef<HTMLDivElement>(null);
  const previousBubbleKeyRef = useRef(outputAnimationKey);
  const previousBubbleHeightRef = useRef<number | null>(null);
  const bubbleFrameRef = useRef<number | null>(null);
  const bubbleTimerRef = useRef<number | null>(null);
  const [bubbleHeight, setBubbleHeight] = useState<number | null>(null);

  useEffect(
    () => () => {
      if (bubbleFrameRef.current !== null) {
        window.cancelAnimationFrame(bubbleFrameRef.current);
      }
      if (bubbleTimerRef.current !== null) {
        window.clearTimeout(bubbleTimerRef.current);
      }
    },
    [],
  );

  useLayoutEffect(() => {
    const bubble = bubbleRef.current;
    const changed = previousBubbleKeyRef.current !== outputAnimationKey;
    if (!bubble) return;

    if (bubbleFrameRef.current !== null) {
      window.cancelAnimationFrame(bubbleFrameRef.current);
      bubbleFrameRef.current = null;
    }
    if (bubbleTimerRef.current !== null) {
      window.clearTimeout(bubbleTimerRef.current);
      bubbleTimerRef.current = null;
    }

    if (changed && !isGenerating && !prefersReducedMotion()) {
      const fromHeight =
        previousBubbleHeightRef.current ??
        bubble.getBoundingClientRect().height;
      const toHeight = bubble.getBoundingClientRect().height;

      setBubbleHeight(fromHeight);

      bubbleFrameRef.current = window.requestAnimationFrame(() => {
        bubbleFrameRef.current = null;
        setBubbleHeight(toHeight);
      });
      bubbleTimerRef.current = window.setTimeout(() => {
        bubbleTimerRef.current = null;
        setBubbleHeight(null);
      }, VARIANT_TRANSITION_MS);
    } else {
      setBubbleHeight(null);
    }

    previousBubbleKeyRef.current = outputAnimationKey;
  }, [isGenerating, outputAnimationKey]);

  useLayoutEffect(() => {
    if (bubbleHeight !== null) return;
    previousBubbleHeightRef.current =
      bubbleRef.current?.getBoundingClientRect().height ?? null;
  });

  const bubbleStyle =
    bubbleHeight === null
      ? undefined
      : ({ height: bubbleHeight } as CSSProperties);

  const firstOutputMsg = item.output[0];
  const acpOrigin = assistantAcpOrigin(item);
  const acpModeActive = Boolean(activeAcpRunner);
  const regenerateDisabled = Boolean(acpOrigin) || acpModeActive;
  const variantActionsDisabled = Boolean(acpOrigin) || acpModeActive;
  const outputTimestamps = readAssistantTimestamps(firstOutputMsg);
  const responseTimestamps = readAssistantTimestamps(item);
  const started = outputTimestamps.started ?? responseTimestamps.started;
  const finished = outputTimestamps.finished ?? responseTimestamps.finished;
  const startedTime = fmtTime(started);
  const finishedTime = fmtTime(finished);

  // Active Turn activity is rendered once by the list, independently of content.
  if (isGenerating && !hasRenderableOutput) return null;

  return (
    <article
      className="rua-chat-message rua-chat-message--assistant"
      data-rua-chat-message-id={item.id}
      data-rua-chat-role="assistant"
      data-rua-chat-turn-id={item.turn_id || undefined}
      data-rua-chat-card-index={cardIndex}
      data-rua-tap-reveal={tapRevealed ? "true" : undefined}
      onClick={() => onTapReveal?.(item.id)}
    >
      {acpOrigin && (
        <span className="rua-chat-origin-badge rua-chat-origin-badge--acp">
          ACP{acpOrigin.runner ? ` · ${acpOrigin.runner}` : ""}
        </span>
      )}
      {(startedTime || finishedTime) && (
        <span className="rua-chat-time-cluster rua-chat-time-cluster--top-right">
          {startedTime && (
            <span className="rua-chat-time">开始 {startedTime}</span>
          )}
          {finishedTime && (
            <span className="rua-chat-time">结束 {finishedTime}</span>
          )}
        </span>
      )}
      <div
        ref={bubbleRef}
        className={[
          "rua-chat-bubble rua-chat-bubble--assistant",
          bubbleHeight !== null ? "is-size-transitioning" : "",
        ]
          .filter(Boolean)
          .join(" ")}
        style={bubbleStyle}
      >
        <AssistantOutputTransition
          animationKey={outputAnimationKey}
          variantIndex={variantIndex}
          disabled={isGenerating}
        >
          {!isGenerating && !hasRenderableOutput ? (
            isInterrupted ? (
              <InterruptedTurnNotice kind={interruptedKind} />
            ) : (
              <div className="rua-chat-empty-assistant">
                {emptyOutputText}
              </div>
            )
          ) : (
            <>
              {segments.map((segment, index) => {
                const messageIds = segmentMessageIds(segment);
                const isExcluded = messageIds.every((id) =>
                  excludedMessageIds.has(id),
                );
                const isDraftExcluded = messageIds.every((id) =>
                  draftExcludedMessageIds.has(id),
                );
                const segmentType =
                  segment.kind === "tool-group"
                    ? "tool"
                    : segment.item.type === "reasoning"
                    ? "thinking"
                    : "text";
                const content =
                  segment.kind === "tool-group" ? (
                    <CollapsibleToolGroup
                      items={segment.items}
                      generating={isGenerating && index === segments.length - 1}
                      blockPreferences={blockPreferences}
                      askUserAnswerText={askUserAnswerText}
                      onAnswerUser={onAnswerUser}
                    />
                  ) : segment.kind === "work-checklist" ? (
                    <WorkChecklistBlock checklist={segment.checklist} />
                  ) : (
                    <OutputBlock
                      msg={segment.item}
                      generating={isGenerating}
                      blockPreferences={blockPreferences}
                      askUserAnswerText={askUserAnswerText}
                      onAnswerUser={onAnswerUser}
                      allMessages={messages}
                    />
                  );
                return (
                  <RenderStateScope
                    key={`${outputAnimationKey}:${segment.kind}:${messageIds[0]}`}
                    name={`${outputAnimationKey}:${segment.kind}:${messageIds[0]}`}
                  >
                    <div
                      className={`rua-context-editable-segment${
                        isExcluded ? " is-excluded" : ""
                      }${contextEditorOpen ? " is-editing" : ""}`}
                      data-rua-context-segment={segmentType}
                    >
                      {contextEditorOpen && (
                        <label className="rua-context-segment-checkbox">
                          <input
                            type="checkbox"
                            checked={isDraftExcluded}
                            onChange={(event) => {
                              const next = new Set(draftExcludedMessageIds);
                              for (const id of messageIds) {
                                if (event.target.checked) next.add(id);
                                else next.delete(id);
                              }
                              onDraftExcludedMessageIdsChange?.(next);
                            }}
                            aria-label={t("chat.contextEditor.excludeMessage", {
                              defaultValue: `从模型上下文排除这条${
                                segmentType === "thinking"
                                  ? "思考"
                                  : segmentType === "tool"
                                  ? "工具调用"
                                  : "回复"
                              }`,
                            })}
                          />
                        </label>
                      )}
                      <div className="rua-context-segment-content">
                        {content}
                      </div>
                    </div>
                  </RenderStateScope>
                );
              })}
              {!isGenerating && isInterrupted && (
                <InterruptedTurnNotice kind={interruptedKind} />
              )}
            </>
          )}
        </AssistantOutputTransition>
      </div>
      {!isGenerating && (
        <div
          className="rua-chat-response-footer-row"
          onClick={(event) => event.stopPropagation()}
        >
          <AssistantFooter
            item={item}
            bubbleRef={bubbleRef}
            previousUserText={previousUserText}
            isLast={isLast}
            disabled={isGenerating || regenerateDisabled}
            onRegenerate={onRegenerate}
          />
          {!isGenerating && (
            <VariantPager
              item={item}
              disabled={variantActionsDisabled}
              onSelectVariant={onSelectVariant}
            />
          )}
        </div>
      )}
    </article>
  );
});

export default memo(function ConversationMessageList({
  scrollRef,
  viewportRef,
  viewKey,
  onScrollState,
  onTop,
  initialScroll,
  onLocateTurn,
  historyGap,
  onLoadHistoryGap,
  projection,
  agentId,
  mediaScope,
  approvalSnapshot,
  activeAcpRunner,
  onSelectVariant,
  onRegenerate,
  onRollback,
  onBranch,
  onStartPrompt,
  blockPreferences = DEFAULT_CHAT_BLOCK_PREFERENCES,
  contextEditorOpen = false,
  excludedMessageIds = EMPTY_MESSAGE_IDS,
  draftExcludedMessageIds = EMPTY_MESSAGE_IDS,
  onDraftExcludedMessageIdsChange,
}: ConversationMessageListProps) {
  const { t } = useTranslation();
  const locatingRef = useRef<string | null>(null);
  const pendingSearchRef = useRef<SearchScrollTarget | null>(null);
  const retrySearchRef = useRef<(() => void) | null>(null);
  const [gapLoading, setGapLoading] = useState(false);
  const turns = useMemo(
    () => groupConversationTurns(projection.items, assistantHasAskUser),
    [projection.items],
  );
  const nextActiveKeys = useMemo(() => {
    const lastUserIndex = projection.items.reduce(
      (last, item, index) => (item.kind === "user" ? index : last),
      -1,
    );
    const approvalTurns = new Set(
      approvalSnapshot?.pending.map((request) => request.turn_id),
    );
    return new Set(
      turns
        .filter(
          (turn) =>
            turn.items.some(
              ({ item, index }) =>
                item.kind === "assistant" &&
                (item.status === "generating" ||
                  (index > lastUserIndex && assistantHasAskUser(item))),
            ) || turn.turnIds.some((id) => approvalTurns.has(id)),
        )
        .map((turn) => turn.key),
    );
  }, [turns, approvalSnapshot?.pending, projection.items]);
  const activeKeysRef = useRef(nextActiveKeys);
  if (
    activeKeysRef.current.size !== nextActiveKeys.size ||
    [...nextActiveKeys].some((key) => !activeKeysRef.current.has(key))
  ) {
    activeKeysRef.current = nextActiveKeys;
  }
  const activeKeys = activeKeysRef.current;
  const turnsRef = useRef(turns);
  turnsRef.current = turns;
  const viewport = useConversationViewport({
    turns,
    viewKey: viewKey ?? projection.conversationId,
    scrollRef,
    controllerRef: viewportRef,
    activeKeys,
    mobile: getChatViewCacheByteLimit() <= 128 * 1024 * 1024,
    onScrollState,
    onTop,
    initialScroll,
  });
  const { listRef, reveal: revealTurn } = viewport;
  const searchHighlightCleanupRef = useRef<(() => void) | null>(null);
  const projectionRef = useRef(projection);
  projectionRef.current = projection;
  const [searchTarget, setSearchTarget] = useState<{
    conversationId: string;
    itemId: string;
    turnKey: string;
    query: string;
    reveal: ReadonlySet<string>;
  } | null>(null);
  const {
    prompt_tokens,
    completion_tokens,
    total_tokens,
    cache_read_tokens,
    cache_write_tokens,
  } = useMemo(
    () => computeConversationUsage(projection.items).cumulative ?? {},
    [projection.items],
  );
  // Text-only stream updates must not notify every historical usage control.
  const conversationUsage = useMemo<ConversationUsageAggregate>(
    () => ({
      cumulative:
        total_tokens === undefined
          ? null
          : {
              prompt_tokens,
              completion_tokens,
              total_tokens,
              cache_read_tokens,
              cache_write_tokens,
            },
    }),
    [
      prompt_tokens,
      completion_tokens,
      total_tokens,
      cache_read_tokens,
      cache_write_tokens,
    ],
  );
  const turnActive = projection.turnActivity.kind === "active";

  useEffect(() => {
    setSearchTarget(null);
  }, [projection.conversationId]);

  useEffect(() => {
    const scrollToSearchTarget = (event?: Event) => {
      if (event?.type === "rua-search-scroll") locatingRef.current = null;
      let parsed = pendingSearchRef.current;
      if (!parsed || event?.type === "rua-search-scroll") {
        const raw = sessionStorage.getItem("rua_search_scroll_target");
        if (!raw) return;
        try {
          parsed = JSON.parse(raw) as SearchScrollTarget;
          if (!parsed || typeof parsed !== "object")
            throw new Error("Invalid search target");
        } catch {
          sessionStorage.removeItem("rua_search_scroll_target");
          return;
        }
        pendingSearchRef.current = parsed;
      }
      if (
        parsed.conversationId &&
        parsed.conversationId !== projectionRef.current.conversationId
      )
        return;
      if (
        parsed.ownerKey &&
        viewKey !== parsed.ownerKey + ":" + projectionRef.current.conversationId
      )
        return;
      const items = projectionRef.current.items;
      const matchedIndex = parsed.messageId
        ? items.findIndex(
            (item) =>
              item.id === parsed.messageId ||
              (item.kind === "assistant" &&
                item.output.some((message) => message.id === parsed.messageId)),
          )
        : -1;
      const cardIndex =
        matchedIndex >= 0
          ? matchedIndex
          : parsed.turnId
          ? items.findIndex(
              (item) =>
                item.turn_id === parsed.turnId && item.kind === "assistant",
            )
          : typeof parsed.cardIndex === "number"
          ? parsed.cardIndex
          : -1;
      if (cardIndex < 0 || cardIndex >= items.length) {
        if (
          parsed.turnId &&
          onLocateTurn &&
          locatingRef.current !== parsed.turnId
        ) {
          locatingRef.current = parsed.turnId;
          void onLocateTurn(parsed.turnId);
        }
        return;
      }
      const turn = turnsRef.current.find((turn) =>
        turn.items.some((entry) => entry.index === cardIndex),
      );
      if (turn) revealTurn(turn.key, items[cardIndex].id);
      const query = typeof parsed.query === "string" ? parsed.query.trim() : "";
      const reveal = new Set<string>();
      const item = projectionRef.current.items[cardIndex];
      if (query && item?.kind === "assistant") {
        const lowerQuery = query.toLocaleLowerCase();
        // Inspect payloads only on an explicit search jump, never per token.
        for (const message of item.output) {
          const matches =
            message.content.some((part) =>
              (typeof part.text === "string" ? part.text : JSON.stringify(part))
                .toLocaleLowerCase()
                .includes(lowerQuery),
            ) ||
            (message.runtime_note &&
              JSON.stringify(message.runtime_note)
                .toLocaleLowerCase()
                .includes(lowerQuery));
          if (matches) {
            reveal.add(message.id);
            reveal.add(toolCallId(message) || message.id);
          }
        }
      }
      if (!turn) return;
      setSearchTarget({
        conversationId: projectionRef.current.conversationId,
        itemId: items[cardIndex].id,
        turnKey: turn.key,
        query,
        reveal,
      });
      pendingSearchRef.current = null;
      sessionStorage.removeItem("rua_search_scroll_target");
    };
    retrySearchRef.current = scrollToSearchTarget;

    const timer = window.setTimeout(scrollToSearchTarget, 0);
    window.addEventListener("rua-search-scroll", scrollToSearchTarget);
    return () => {
      window.clearTimeout(timer);
      retrySearchRef.current = null;
      window.removeEventListener("rua-search-scroll", scrollToSearchTarget);
      searchHighlightCleanupRef.current?.();
      searchHighlightCleanupRef.current = null;
    };
  }, [projection.conversationId, listRef, revealTurn, viewKey, onLocateTurn]);
  useEffect(() => {
    if (pendingSearchRef.current) retrySearchRef.current?.();
  }, [projection.items]);

  useEffect(() => {
    if (
      !searchTarget ||
      searchTarget.conversationId !== projection.conversationId
    )
      return;
    let clearTimer: number | undefined;
    // Let the lazy matching group and its details mount before locating text.
    const frame = window.requestAnimationFrame(() => {
      const target = [
        ...(listRef.current?.querySelectorAll<HTMLElement>(
          "[data-rua-chat-message-id]",
        ) ?? []),
      ].find(
        (element) => element.dataset.ruaChatMessageId === searchTarget.itemId,
      );
      if (!target) return;
      searchHighlightCleanupRef.current?.();
      if (!scrollRef) target.scrollIntoView?.({ block: "center" });
      else revealTurn(searchTarget.turnKey, searchTarget.itemId);
      searchHighlightCleanupRef.current = applySearchJumpHighlight(
        target,
        searchTarget.query,
      );
      clearTimer = window.setTimeout(() => {
        searchHighlightCleanupRef.current?.();
        searchHighlightCleanupRef.current = null;
      }, 1800);
    });
    return () => {
      window.cancelAnimationFrame(frame);
      window.clearTimeout(clearTimer);
      searchHighlightCleanupRef.current?.();
      searchHighlightCleanupRef.current = null;
    };
  }, [searchTarget, projection.conversationId, listRef, scrollRef, revealTurn]);

  const [tappedMessageId, setTappedMessageId] = useState<string | null>(null);
  const tapRevealTimerRef = useRef<number | null>(null);
  const revealTap = useCallback((id: string) => {
    setTappedMessageId((prev) => (prev === id ? null : id));
  }, []);
  useEffect(() => {
    if (tapRevealTimerRef.current !== null) {
      window.clearTimeout(tapRevealTimerRef.current);
      tapRevealTimerRef.current = null;
    }
    if (!tappedMessageId) return;
    tapRevealTimerRef.current = window.setTimeout(() => {
      setTappedMessageId(null);
      tapRevealTimerRef.current = null;
    }, TAP_REVEAL_TIMEOUT_MS);
    return () => {
      if (tapRevealTimerRef.current !== null) {
        window.clearTimeout(tapRevealTimerRef.current);
        tapRevealTimerRef.current = null;
      }
    };
  }, [tappedMessageId]);
  useEffect(() => {
    if (!tappedMessageId) return;
    const onDocClick = (e: Event) => {
      const target = e.target as HTMLElement | null;
      if (!target) return;

      if (target.closest(".rua-chat-message")) return;
      setTappedMessageId(null);
    };
    document.addEventListener("click", onDocClick);
    return () => document.removeEventListener("click", onDocClick);
  }, [tappedMessageId]);

  const {
    previousUserTextByIndex,
    previousAssistantByIndex,
    nextAssistantByIndex,
  } = useMemo(() => {
    const previousUserTextByIndex: string[] = [];
    const previousAssistantByIndex: Array<
      ConversationAssistantResponse | undefined
    > = [];
    const nextAssistantByIndex: Array<
      ConversationAssistantResponse | undefined
    > = [];
    let previousUserText = "";
    let previousAssistant: ConversationAssistantResponse | undefined;
    let nextAssistant: ConversationAssistantResponse | undefined;
    for (const [index, item] of projection.items.entries()) {
      previousUserTextByIndex[index] = previousUserText;
      previousAssistantByIndex[index] = previousAssistant;
      if (item.kind === "user") previousUserText = textFromParts(item.content);
      if (item.kind === "assistant") previousAssistant = item;
    }
    for (let index = projection.items.length - 1; index >= 0; index -= 1) {
      nextAssistantByIndex[index] = nextAssistant;
      const item = projection.items[index];
      if (item.kind === "assistant") nextAssistant = item;
    }

    return {
      previousUserTextByIndex,
      previousAssistantByIndex,
      nextAssistantByIndex,
    };
  }, [projection.items]);

  if (projection.items.length === 0 && !turnActive) {
    const prompts = [
      tx(t, "chat.prompt1", "Let's start a new journey!"),
      tx(t, "chat.prompt2", "Can you tell me what skills you have?"),
    ];
    return (
      <div className="rua-chat-empty" data-rua-chat-empty="true">
        <div className="rua-chat-empty-welcome">
          <h2>{tx(t, "chat.greeting", "Hello, how can I help you today?")}</h2>
          <p>
            {tx(
              t,
              "chat.description",
              "I am a helpful assistant that can help you with your questions.",
            )}
          </p>
          <div className="rua-chat-empty-prompts">
            {prompts.map((prompt) => (
              <button
                key={prompt}
                type="button"
                className="rua-chat-empty-prompt"
                disabled={turnActive || !onStartPrompt}
                onClick={() => void onStartPrompt?.(prompt)}
              >
                <Sparkles size={14} aria-hidden="true" />
                <span>{prompt}</span>
                <ChevronRight size={14} aria-hidden="true" />
              </button>
            ))}
          </div>
        </div>
      </div>
    );
  }

  return (
    <RenderStateRoot store={viewport.viewState}>
      <MarkdownMediaContext.Provider
        value={{ scope: mediaScope, chatId: projection.conversationId }}
      >
        <ConversationUsageContext.Provider value={conversationUsage}>
          <div
            ref={listRef}
            className="rua-chat-message-list"
            data-rua-chat-conversation-id={projection.conversationId}
          >
            {(() => {
              const output: ReactNode[] = [];
              const measurements = viewport.virtualizer.measurementsCache;
              let gapStart: number | null = null;
              const flushGap = (end: number) => {
                if (gapStart === null) return;
                const first = measurements[gapStart];
                const last = measurements[end - 1];
                const height = first && last ? last.end - first.start : 0;
                output.push(
                  <div
                    key={"gap:" + turns[gapStart].key}
                    className="rua-chat-turn-gap"
                    data-rua-turn-gap=""
                    aria-hidden="true"
                    style={{ height }}
                  />,
                );
                gapStart = null;
              };
              let previousIndex = -1;
              for (const turnIndex of [...viewport.mounted].sort(
                (a, b) => a - b,
              )) {
                const turn = turns[turnIndex];
                if (!turn) continue;
                if (turnIndex > previousIndex + 1) gapStart = previousIndex + 1;
                flushGap(turnIndex);
                previousIndex = turnIndex;
                output.push(
                  <RenderStateScope
                    key={turn.key}
                    name={"turn:" + turn.key}
                    turnKey={turn.key}
                  >
                    <section
                      ref={
                        viewport.enabled
                          ? viewport.virtualizer.measureElement
                          : undefined
                      }
                      data-index={turnIndex}
                      data-rua-turn-key={turn.key}
                      className="rua-chat-turn"
                      data-rua-chat-turn={turnIndex}
                      style={
                        turnIndex > 0
                          ? { paddingTop: "calc(24px + var(--space-3))" }
                          : undefined
                      }
                    >
                      {historyGap &&
                        turn.items.some(
                          (entry) =>
                            entry.item.id === historyGap.beforeMessageId,
                        ) && (
                          <button
                            type="button"
                            className="rua-history-range-gap"
                            disabled={gapLoading}
                            onClick={() => {
                              if (!onLoadHistoryGap) return;
                              viewportRef?.current?.beforePrepend();
                              setGapLoading(true);
                              void onLoadHistoryGap().finally(() =>
                                setGapLoading(false),
                              );
                            }}
                          >
                            {gapLoading
                              ? t("common.loading", {
                                  defaultValue: "Loading...",
                                })
                              : countLabel(
                                  t,
                                  "chat.historyGap",
                                  historyGap.count,
                                  "Load {{count}} intervening turns",
                                )}
                          </button>
                        )}
                      {turn.items.map(({ item, index }) => {
                        if (item.kind === "user") {
                          const previous = projection.items[index - 1];
                          if (
                            previous?.kind === "assistant" &&
                            assistantHasAskUser(previous)
                          ) {
                            return null;
                          }
                          return (
                            <RenderStateScope
                              key={item.id}
                              name={"message:" + item.id}
                            >
                              <UserBubble
                                item={item}
                                agentId={agentId}
                                mediaScope={mediaScope}
                                chatId={projection.conversationId}
                                cardIndex={index}
                                branchActionsDisabled={
                                  turnActive ||
                                  Boolean(activeAcpRunner) ||
                                  Boolean(
                                    previousAssistantByIndex[index] &&
                                      assistantAcpOrigin(
                                        previousAssistantByIndex[index]!,
                                      ) &&
                                      nextAssistantByIndex[index] &&
                                      assistantAcpOrigin(
                                        nextAssistantByIndex[index]!,
                                      ),
                                  )
                                }
                                onRollback={onRollback}
                                onBranch={onBranch}
                                tapRevealed={tappedMessageId === item.id}
                                onTapReveal={revealTap}
                              />
                            </RenderStateScope>
                          );
                        }
                        if (item.kind === "acp_history_preview") {
                          return (
                            <AcpHistoryPreviewCard
                              key={item.id}
                              item={item}
                              cardIndex={index}
                            />
                          );
                        }
                        const nextUser =
                          projection.items[index + 1]?.kind === "user"
                            ? (projection.items[
                                index + 1
                              ] as ConversationUserMessage)
                            : null;
                        const askUserAnswerText =
                          nextUser && assistantHasAskUser(item)
                            ? textFromParts(nextUser.content)
                            : undefined;
                        const isLastAssistant = !nextAssistantByIndex[index];
                        return (
                          <RenderStateScope
                            key={item.id}
                            name={"message:" + item.id}
                          >
                            <SearchRevealContext.Provider
                              value={
                                searchTarget?.conversationId ===
                                  projection.conversationId &&
                                searchTarget.itemId === item.id
                                  ? searchTarget.reveal
                                  : null
                              }
                            >
                              <AssistantCard
                                item={item}
                                approvalSnapshot={approvalSnapshot}
                                previousUserText={
                                  previousUserTextByIndex[index]
                                }
                                isLast={isLastAssistant}
                                emptyOutputText={t(
                                  "chat.emptyAssistantResponse",
                                  {
                                    defaultValue:
                                      "The assistant returned no output.",
                                  },
                                )}
                                activeTurnResponse={
                                  projection.turnActivity.kind === "active" &&
                                  projection.turnActivity.responseId === item.id
                                }
                                cardIndex={index}
                                onSelectVariant={onSelectVariant}
                                onRegenerate={
                                  isLastAssistant ? onRegenerate : undefined
                                }
                                onAnswerUser={onStartPrompt}
                                askUserAnswerText={askUserAnswerText}
                                blockPreferences={blockPreferences}
                                tapRevealed={tappedMessageId === item.id}
                                onTapReveal={revealTap}
                                activeAcpRunner={activeAcpRunner}
                                contextEditorOpen={
                                  contextEditorOpen &&
                                  item.status !== "generating"
                                }
                                excludedMessageIds={excludedMessageIds}
                                draftExcludedMessageIds={
                                  draftExcludedMessageIds
                                }
                                onDraftExcludedMessageIdsChange={
                                  onDraftExcludedMessageIdsChange
                                }
                              />
                            </SearchRevealContext.Provider>
                          </RenderStateScope>
                        );
                      })}
                    </section>
                  </RenderStateScope>,
                );
              }
              if (previousIndex < turns.length - 1)
                gapStart = previousIndex + 1;
              flushGap(turns.length);
              return output;
            })()}
            {turnActive && (
              <div
                className="rua-chat-generating"
                data-rua-chat-running="true"
                role="status"
                aria-label={tx(t, "chat.statusRunning", "Running")}
              >
                <LoaderCircle
                  className="rua-chat-inline-spinner"
                  size={15}
                  aria-hidden="true"
                />
              </div>
            )}
          </div>
        </ConversationUsageContext.Provider>
      </MarkdownMediaContext.Provider>
    </RenderStateRoot>
  );
});
