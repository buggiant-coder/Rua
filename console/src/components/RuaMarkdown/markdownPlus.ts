export type MarkdownSegment =
  | { kind: "markdown"; text: string }
  | { kind: "mdp"; block: MarkdownPlusBlock };

export type MarkdownPlusBlock = {
  type: "figure" | "diagram" | "chart";
  params: Record<string, string>;
  body: string[];
  raw: string;
};

const MDP_HEADER_RE = /^@(figure|diagram|chart)(?:\s+(.*))?$/;
const FENCE_RE = /^( {0,3})(`{3,}|~{3,})(.*)$/;

type MarkdownFence = {
  marker: string;
  length: number;
  info: string;
};

function parseMarkdownFence(line: string): MarkdownFence | null {
  const match = FENCE_RE.exec(line);
  if (!match) return null;
  return {
    marker: match[2][0],
    length: match[2].length,
    info: match[3].trim(),
  };
}

function closesMarkdownFence(
  fence: MarkdownFence,
  openFence: MarkdownFence,
): boolean {
  return (
    fence.marker === openFence.marker &&
    fence.length >= openFence.length &&
    fence.info === ""
  );
}

function isMermaidFence(fence: MarkdownFence): boolean {
  return fence.info.split(/\s+/, 1)[0]?.toLowerCase() === "mermaid";
}

function findClosingFence(
  lines: string[],
  start: number,
  openFence: MarkdownFence,
): number {
  for (let i = start; i < lines.length; i += 1) {
    const fence = parseMarkdownFence(lines[i]);
    if (fence && closesMarkdownFence(fence, openFence)) return i;
  }
  return -1;
}

export function splitMarkdownPlus(content: string): MarkdownSegment[] {
  const lines = content.split(/\r?\n/);
  const segments: MarkdownSegment[] = [];
  let markdownStart = 0;
  let inFence: MarkdownFence | null = null;
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];
    const fence = parseMarkdownFence(line);

    if (inFence) {
      if (fence && closesMarkdownFence(fence, inFence)) {
        inFence = null;
      }
      i += 1;
      continue;
    }

    if (fence) {
      if (isMermaidFence(fence)) {
        const end = findClosingFence(lines, i + 1, fence);
        if (end < 0) break;

        const markdown = lines.slice(markdownStart, i).join("\n");
        if (markdown) {
          segments.push({ kind: "markdown", text: markdown });
        }

        segments.push({
          kind: "mdp",
          block: {
            type: "diagram",
            params: {},
            body: stripSharedIndent(lines.slice(i + 1, end)),
            raw: lines.slice(i, end + 1).join("\n"),
          },
        });

        i = end + 1;
        markdownStart = i;
        continue;
      }

      inFence = fence;
      i += 1;
      continue;
    }

    const header = MDP_HEADER_RE.exec(line);
    if (!header) {
      i += 1;
      continue;
    }

    let end = i + 1;
    while (end < lines.length && lines[end] !== "@end") {
      end += 1;
    }

    if (end >= lines.length) {
      break;
    }

    const markdown = lines.slice(markdownStart, i).join("\n");
    if (markdown) {
      segments.push({ kind: "markdown", text: markdown });
    }

    const raw = lines.slice(i, end + 1).join("\n");
    const body = stripSharedIndent(lines.slice(i + 1, end));
    segments.push({
      kind: "mdp",
      block: {
        type: header[1] as MarkdownPlusBlock["type"],
        params: parseMarkdownPlusParams(header[2] || ""),
        body,
        raw,
      },
    });

    i = end + 1;
    markdownStart = i;
  }

  const remaining = lines.slice(markdownStart).join("\n");
  if (remaining) {
    segments.push({ kind: "markdown", text: remaining });
  }

  return segments.length > 0 ? segments : [{ kind: "markdown", text: content }];
}

function stripSharedIndent(lines: string[]): string[] {
  const indents = lines
    .filter((line) => line.trim().length > 0)
    .map((line) => line.match(/^[ \t]*/)?.[0].length ?? 0);
  const minIndent = indents.length ? Math.min(...indents) : 0;
  return minIndent > 0 ? lines.map((line) => line.slice(minIndent)) : lines;
}

export function parseMarkdownPlusParams(input: string): Record<string, string> {
  const params: Record<string, string> = {};
  let i = 0;

  while (i < input.length) {
    while (i < input.length && /\s/.test(input[i])) i += 1;
    const keyStart = i;
    while (i < input.length && /[A-Za-z0-9_-]/.test(input[i])) i += 1;
    const key = input.slice(keyStart, i);
    if (!key) break;
    if (input[i] !== "=") {
      while (i < input.length && !/\s/.test(input[i])) i += 1;
      continue;
    }
    i += 1;

    let value = "";
    if (input[i] === '"') {
      i += 1;
      while (i < input.length) {
        if (input[i] === "\\" && i + 1 < input.length) {
          value += input[i + 1];
          i += 2;
          continue;
        }
        if (input[i] === '"') {
          i += 1;
          break;
        }
        value += input[i];
        i += 1;
      }
    } else {
      const valueStart = i;
      while (i < input.length && !/\s/.test(input[i])) i += 1;
      value = input.slice(valueStart, i);
    }
    params[key] = value;
  }

  return params;
}
