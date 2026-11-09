import { chatApi } from "../../api/modules/chat";
export type CopyableContent = {
  type?: string;
  text?: string;
  refusal?: string;
};

export type CopyableMessage = {
  role?: string;
  type?: string;
  content?: string | CopyableContent[];
};

export type CopyableResponse = {
  output?: CopyableMessage[];
};

type TextContentPart = {
  type?: string;
  text?: string;
  image_url?: string;
  file_url?: string;
  data?: string;
  video_url?: string;
};

type MessageWithContent = {
  content?: string | TextContentPart[];
};

export function extractCopyableText(response: CopyableResponse): string {
  const collectText = (assistantOnly: boolean) => {
    const chunks = (response.output || []).flatMap((item: CopyableMessage) => {
      if (assistantOnly && item.role !== "assistant") return [];

      if (typeof item.content === "string") {
        return [item.content];
      }

      if (!Array.isArray(item.content)) {
        return [];
      }

      return item.content.flatMap((content: CopyableContent) => {
        if (content.type === "text" && typeof content.text === "string") {
          return [content.text];
        }

        if (content.type === "refusal" && typeof content.refusal === "string") {
          return [content.refusal];
        }

        return [];
      });
    });

    return chunks.filter(Boolean).join("\n\n").trim();
  };

  return collectText(true) || JSON.stringify(response);
}

export function extractSpeakableText(response: CopyableResponse): string {
  const chunks = (response.output || []).flatMap((item: CopyableMessage) => {
    if (item.role !== "assistant") return [];
    if (item.type && item.type !== "message") return [];

    if (typeof item.content === "string") {
      return [item.content];
    }

    if (!Array.isArray(item.content)) {
      return [];
    }

    return item.content.flatMap((content: CopyableContent) => {
      if (content.type === "text" && typeof content.text === "string") {
        return [content.text];
      }
      return [];
    });
  });

  return chunks.filter(Boolean).join("\n\n").trim();
}

export function extractUserMessageText(m: MessageWithContent): string {
  if (typeof m.content === "string") return m.content;
  if (!Array.isArray(m.content)) return "";
  return m.content
    .filter((p) => p.type === "text")
    .map((p) => p.text || "")
    .join("\n");
}

export { copyText } from "../../utils/clipboard";

export function buildModelError(): Response {
  return new Response(
    JSON.stringify({
      error: "Model not configured",
      message: "Please configure a model first",
    }),
    { status: 400, headers: { "Content-Type": "application/json" } },
  );
}

function decodeUriPathSegments(path: string): string {
  return path
    .split("/")
    .map((segment) => {
      if (!segment) return segment;
      try {
        return decodeURIComponent(segment);
      } catch {
        return segment;
      }
    })
    .join("/");
}

export function toStoredName(v: string): string {
  const marker = "/files/preview/";
  const idx = v.indexOf(marker);
  if (idx !== -1) {
    let rest = v.slice(idx + marker.length);
    const q = rest.indexOf("?");
    if (q !== -1) rest = rest.slice(0, q);
    const h = rest.indexOf("#");
    if (h !== -1) rest = rest.slice(0, h);
    if (rest) {
      const decoded = decodeUriPathSegments(rest);

      const isWindowsAbsolute = /^[a-zA-Z]:[\\/]/.test(decoded);
      if (isWindowsAbsolute) return decoded;
      return decoded.startsWith("/") ? decoded : `/${decoded}`;
    }
  }
  return v;
}

export function normalizeContentUrls<T extends TextContentPart>(part: T): T {
  const p = { ...part };
  if (p.type === "image" && typeof p.image_url === "string")
    p.image_url = toStoredName(p.image_url);
  if (p.type === "file" && typeof p.file_url === "string")
    p.file_url = toStoredName(p.file_url);
  if (p.type === "audio" && typeof p.data === "string")
    p.data = toStoredName(p.data);
  if (p.type === "video" && typeof p.video_url === "string")
    p.video_url = toStoredName(p.video_url);
  return p;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function toDisplayUrl(url: string | undefined): string {
  if (!url) return "";
  if (/^data:image\/(png|jpeg|webp|gif);base64,/i.test(url)) return url;
  if (url.startsWith("http://") || url.startsWith("https://")) return url;
  if (url.startsWith("/api/")) return url;
  if (url.startsWith("file://")) url = url.replace("file://", "");
  return chatApi.filePreviewUrl(url.startsWith("/") ? url : `/${url}`);
}

export function toMarkdownImageDisplayUrl(
  source: string,
  scope?: import("@/api/modules/chat").ChatMediaScope,
  chatId?: string,
): string {
  if (/^(https?:|data:|blob:)/i.test(source)) return source;
  const mediaId = source.startsWith("media/") ? source.slice(6) : "";
  if (
    scope &&
    chatId &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
      mediaId,
    )
  ) {
    return chatApi.mediaUrl(scope, chatId, mediaId);
  }
  return "";
}

export interface ConversationTurn {
  userText: string;
  assistantText: string;
}

export interface MessageBlock {
  role: "user" | "assistant";
  text: string;
}

function extractPlainText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return String(content || "");
  return (content as Array<{ type?: string; text?: string }>)
    .filter((c) => c.type === "text" && c.text)
    .map((c) => c.text!)
    .join("\n");
}

function objectRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object"
    ? (value as Record<string, unknown>)
    : null;
}

function isHiddenModelMarkerMessage(message: {
  content: unknown;
  metadata?: unknown;
}): boolean {
  const metadata = objectRecord(message.metadata);
  const nestedMetadata = objectRecord(metadata?.metadata);
  const marker =
    typeof metadata?.model_visible_marker === "string"
      ? metadata.model_visible_marker
      : typeof nestedMetadata?.model_visible_marker === "string"
      ? nestedMetadata.model_visible_marker
      : null;
  return (
    marker === "memory_signal" ||
    extractPlainText(message.content).includes("<memory_signal>")
  );
}

function extractAnyContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return String(content || "");
  const parts: string[] = [];
  for (const c of content as Array<Record<string, unknown>>) {
    if (c.type === "text" && typeof c.text === "string") {
      parts.push(c.text);
      continue;
    }

    if (isRecord(c.data)) {
      const d = c.data;
      const name = d.name || "";
      const args = d.arguments;
      const output = d.output;
      const lineParts: string[] = [];
      if (name) lineParts.push(`[${name}]`);
      if (args != null) {
        const argsStr =
          typeof args === "string" ? args : JSON.stringify(args, null, 2);
        lineParts.push(`Input: ${argsStr}`);
      }
      if (output != null) {
        const outStr = formatToolResultForExport(output);
        lineParts.push(`Output: ${outStr}`);
      }
      if (lineParts.length) parts.push(lineParts.join("\n"));
      continue;
    }

    const raw = JSON.stringify(c);
    if (raw && raw !== "{}") parts.push(raw);
  }
  return parts.join("\n");
}

type MemorySearchResultItem = {
  id?: string;
  content?: string;
  source?: string;
  path?: string;
  snippet?: string;
  score?: number;
  start_line?: number;
  end_line?: number;
  metadata?: Record<string, unknown>;
};

function tryParseJson(text: string): unknown | null {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function normalizeToolText(text: string): string {
  return text.replace(/\\n/g, "\n");
}

function isMemorySearchResultItem(
  item: unknown,
): item is MemorySearchResultItem {
  if (!item || typeof item !== "object") return false;
  const candidate = item as Record<string, unknown>;
  const metadata = candidate.metadata as Record<string, unknown> | undefined;
  return (
    ("content" in candidate ||
      "id" in candidate ||
      "path" in candidate ||
      typeof metadata?.id === "string") &&
    ("score" in candidate ||
      "snippet" in candidate ||
      "content" in candidate ||
      "metadata" in candidate ||
      "start_line" in candidate)
  );
}

function extractMemorySearchItems(
  value: unknown,
  depth = 0,
): MemorySearchResultItem[] | null {
  if (depth > 5) return null;

  if (Array.isArray(value)) {
    if (value.every(isMemorySearchResultItem)) return value;
    for (const item of value) {
      const extracted = extractMemorySearchItems(item, depth + 1);
      if (extracted) return extracted;
    }
    return null;
  }

  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (typeof record.text === "string") {
    const parsedText = tryParseJson(record.text);
    if (parsedText !== null) {
      return extractMemorySearchItems(parsedText, depth + 1);
    }
  }
  if ("output" in record) {
    return extractMemorySearchItems(record.output, depth + 1);
  }
  return isMemorySearchResultItem(record) ? [record] : null;
}

function parseMalformedMemorySearchText(
  text: string,
): MemorySearchResultItem[] | null {
  const normalizedText = normalizeToolText(text).trim();
  const singleMatch = normalizedText.match(
    /^\[\s*\{\s*"path"\s*:\s*"([^"]+)"\s*,\s*"start_line"\s*:\s*(\d+)\s*,\s*"end_line"\s*:\s*(\d+)\s*,\s*"score"\s*:\s*([\d.]+)\s*,\s*"snippet"\s*:\s*"([\s\S]*)$/,
  );
  if (!singleMatch) return null;

  const [, path, startLine, endLine, score, rawSnippet] = singleMatch;
  return [
    {
      path,
      start_line: Number(startLine),
      end_line: Number(endLine),
      score: Number(score),
      snippet: rawSnippet.replace(/"\s*}\s*]\s*$/, "").trim(),
    },
  ];
}

function formatMemorySearchItems(items: MemorySearchResultItem[]): string {
  return items
    .map((item, index) => {
      const metadata = item.metadata ?? {};
      const id = item.id ?? (metadata.id as string | undefined);
      const title = id || item.path || "unknown";
      const score = item.score != null ? item.score.toFixed(2) : "-";
      const text = (item.content || item.snippet || "").trim();
      const structuredLines = [
        item.source ? `- **Source**: ${item.source}` : "",
        metadata.kind ? `- **Kind**: ${String(metadata.kind)}` : "",
        metadata.scope ? `- **Scope**: ${String(metadata.scope)}` : "",
        metadata.subject ? `- **Subject**: ${String(metadata.subject)}` : "",
      ].filter(Boolean);
      const legacyLines =
        item.start_line != null && item.end_line != null
          ? [`- **Lines**: L${item.start_line}-${item.end_line}`]
          : [];

      return [
        `### ${index + 1}. ${title}`,
        ...structuredLines,
        ...legacyLines,
        `- **Score**: ${score}`,
        text ? `\n${text}` : "",
      ]
        .filter(Boolean)
        .join("\n");
    })
    .join("\n\n---\n\n");
}

function extractTextBlockText(value: unknown): string | null {
  if (Array.isArray(value)) {
    const textParts = value
      .map(extractTextBlockText)
      .filter(
        (chunk): chunk is string =>
          typeof chunk === "string" && chunk.length > 0,
      );
    return textParts.length > 0 ? textParts.join("\n") : null;
  }

  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (typeof record.text === "string") {
    return normalizeToolText(record.text);
  }
  if ("output" in record) {
    return extractTextBlockText(record.output);
  }
  return null;
}

function formatToolResultForExport(result: unknown): string {
  if (typeof result !== "string") {
    return Array.isArray(result) || (result && typeof result === "object")
      ? JSON.stringify(result, null, 2)
      : String(result ?? "");
  }

  const parsed = tryParseJson(result.trim());
  if (parsed === null) return result;

  const memoryItems = extractMemorySearchItems(parsed);
  if (memoryItems && memoryItems.length > 0) {
    return formatMemorySearchItems(memoryItems);
  }

  const textBlockText = extractTextBlockText(parsed);
  if (!textBlockText) return JSON.stringify(parsed, null, 2);

  const malformedMemoryItems = parseMalformedMemorySearchText(textBlockText);
  if (malformedMemoryItems && malformedMemoryItems.length > 0) {
    return formatMemorySearchItems(malformedMemoryItems);
  }

  return textBlockText;
}

export function groupIntoTurns(
  messages: Array<{
    role: string;
    type?: string;
    content: unknown;
    metadata?: unknown;
  }>,
): ConversationTurn[] {
  messages = messages.filter((message) => !isHiddenModelMarkerMessage(message));
  const turns: ConversationTurn[] = [];
  let i = 0;

  if (i < messages.length && messages[i].role !== "user") {
    const parts: string[] = [];
    while (i < messages.length && messages[i].role !== "user") {
      const m = messages[i];

      if (m.role === "assistant" && (!m.type || m.type === "message")) {
        const t = extractPlainText(m.content);
        if (t) parts.push(t);
      }
      i++;
    }
    if (parts.length) {
      turns.push({ userText: "", assistantText: parts.join("\n\n") });
    }
  }

  while (i < messages.length) {
    const userMsg = messages[i];
    const userText =
      userMsg.role === "user" ? extractPlainText(userMsg.content) : "";
    i++;

    const assistantParts: string[] = [];
    while (i < messages.length && messages[i].role !== "user") {
      const m = messages[i];

      if (m.role === "assistant" && (!m.type || m.type === "message")) {
        const t = extractPlainText(m.content);
        if (t) assistantParts.push(t);
      }
      i++;
    }

    turns.push({
      userText,
      assistantText: assistantParts.join("\n\n"),
    });
  }

  return turns;
}

const THINKING_TYPES = new Set(["reasoning"]);

const TOOL_TYPES = new Set([
  "plugin_call",
  "plugin_call_output",
  "mcp_call",
  "mcp_call_output",
  "tool_approval_request",
]);

export function groupIntoMessageBlocks(
  messages: Array<{
    role: string;
    type?: string;
    content: unknown;
    metadata?: unknown;
  }>,
  includeThinking = false,
  includeTools = false,
): MessageBlock[] {
  messages = messages.filter((message) => !isHiddenModelMarkerMessage(message));
  const blocks: MessageBlock[] = [];
  let i = 0;

  while (i < messages.length && messages[i].role !== "user") {
    i++;
  }

  while (i < messages.length) {
    if (messages[i].role === "user") {
      const parts: string[] = [];
      while (i < messages.length && messages[i].role === "user") {
        const t = extractPlainText(messages[i].content);
        if (t) parts.push(t);
        i++;
      }
      if (parts.length > 0) {
        blocks.push({ role: "user", text: parts.join("\n\n") });
      }
      continue;
    }

    const parts: string[] = [];
    let thinkingCount = 0;
    let toolCount = 0;

    while (i < messages.length && messages[i].role !== "user") {
      const m = messages[i];
      const mType = m.type || "";
      const mRole = m.role;

      if (mRole === "assistant") {
        if (!mType || mType === "message") {
          const t = extractPlainText(m.content);
          if (t) parts.push(t);
        } else if (THINKING_TYPES.has(mType)) {
          if (includeThinking) {
            const t = extractAnyContent(m.content);
            if (t) parts.push(`> **Thinking:**\n> ${t.replace(/\n/g, "\n> ")}`);
          } else {
            thinkingCount++;
          }
        } else if (TOOL_TYPES.has(mType)) {
          if (includeTools) {
            const t = extractAnyContent(m.content);
            if (t) parts.push(`\`\`\`\n${t}\n\`\`\``);
          } else {
            toolCount++;
          }
        }
      }

      if ((mRole === "system" || mRole === "tool") && includeTools) {
        const t = extractAnyContent(m.content);
        if (t) parts.push(`\`\`\`\n${t}\n\`\`\``);
      } else if ((mRole === "system" || mRole === "tool") && !includeTools) {
        toolCount++;
      }

      i++;
    }

    if (thinkingCount > 0 || toolCount > 0) {
      const summaryParts: string[] = [];
      if (thinkingCount > 0) summaryParts.push(`${thinkingCount} thinking`);
      if (toolCount > 0)
        summaryParts.push(`${toolCount} tool call${toolCount > 1 ? "s" : ""}`);
      parts.unshift(`*[${summaryParts.join(", ")}]*`);
    }

    if (parts.length > 0) {
      blocks.push({ role: "assistant", text: parts.join("\n\n") });
    }
  }

  return blocks;
}

export function formatMessagesAsMarkdown(
  blocks: MessageBlock[],
  selectedIndices: Set<number>,
  sessionName?: string,
): string {
  const lines: string[] = [];
  const title = sessionName || "Chat Export";
  lines.push(`# ${title}`);
  lines.push(`*Exported on ${new Date().toLocaleString()}*`);
  lines.push("");

  let first = true;
  for (const idx of Array.from(selectedIndices).sort((a, b) => a - b)) {
    const block = blocks[idx];
    if (!block || !block.text) continue;

    if (!first) {
      lines.push("---");
      lines.push("");
    }
    first = false;

    const heading = block.role === "user" ? "User" : "Assistant";
    lines.push(`## ${heading}`);
    lines.push("");
    lines.push(block.text);
    lines.push("");
  }

  return lines.join("\n");
}

export function formatConversationAsMarkdown(
  turns: ConversationTurn[],
  selectedIndices: Set<number>,
  sessionName?: string,
): string {
  const lines: string[] = [];
  const title = sessionName || "Chat Export";
  lines.push(`# ${title}`);
  lines.push(`*Exported on ${new Date().toLocaleString()}*`);
  lines.push("");

  let first = true;
  for (const idx of Array.from(selectedIndices).sort((a, b) => a - b)) {
    const turn = turns[idx];
    if (!turn) continue;

    if (!first) {
      lines.push("---");
      lines.push("");
    }
    first = false;

    if (turn.userText) {
      lines.push("## User");
      lines.push("");
      lines.push(turn.userText);
      lines.push("");
    }

    if (turn.assistantText) {
      lines.push("## Assistant");
      lines.push("");
      lines.push(turn.assistantText);
      lines.push("");
    }
  }

  return lines.join("\n");
}
