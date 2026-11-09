import ReactMarkdown from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { memo, useMemo } from "react";
import { RenderStateScope } from "@/components/ChatRenderBlock/viewState";
import type { CSSProperties } from "react";
import { createMarkdownComponents } from "./markdownComponents";
import { splitMarkdownPlus } from "./markdownPlus";
import { MarkdownPlusBlockView } from "./markdownPlusRender";
export {
  mermaidRenderConfig,
  sanitizeMermaidSvgForRender,
  toMermaidSource,
} from "./markdownPlusRender";
import "katex/dist/katex.min.css";
import "./rua-markdown.css";

type RuaMarkdownProps = {
  content: string;
  className?: string;
  style?: CSSProperties;
  collapsibleRenderBlocks?: boolean;
  resolveImageSrc?: (source: string) => string;
};

const MARKDOWN_FENCE_RE = /^( {0,3})(`{3,}|~{3,})/;

function normalizeDisplayMathDelimiters(text: string): string {
  const normalized: string[] = [];
  let displayMathIndent: string | null = null;
  let inFence: { marker: string; length: number } | null = null;

  for (const line of text.split(/\r?\n/)) {
    const fence = MARKDOWN_FENCE_RE.exec(line);
    if (fence) {
      const marker = fence[2][0];
      const length = fence[2].length;
      if (!inFence) {
        inFence = { marker, length };
      } else if (inFence.marker === marker && length >= inFence.length) {
        inFence = null;
      }
      normalized.push(line);
      continue;
    }

    if (inFence) {
      normalized.push(line);
      continue;
    }

    if (displayMathIndent === null) {
      if (line.trim() === "$$") {
        displayMathIndent = line.slice(0, line.indexOf("$$"));
      }
      normalized.push(line);
      continue;
    }

    if (line.trim() === "$$") {
      displayMathIndent = null;
      normalized.push(line);
      continue;
    }

    const trimmedEnd = line.trimEnd();
    if (trimmedEnd.endsWith("$$")) {
      const formula = trimmedEnd.slice(0, -2).trimEnd();
      if (formula.trim()) {
        normalized.push(formula, `${displayMathIndent}$$`);
        displayMathIndent = null;
        continue;
      }
    }

    normalized.push(line);
  }

  return normalized.join("\n");
}

const MarkdownFragment = memo(function MarkdownFragment({
  text,
  collapsibleRenderBlocks = false,
  resolveImageSrc,
}: {
  text: string;
  collapsibleRenderBlocks?: boolean;
  resolveImageSrc?: (source: string) => string;
}) {
  const markdownComponents = useMemo(
    () =>
      createMarkdownComponents({
        collapsibleRenderBlocks,
        renderMarkdown: (body) => (
          <MarkdownFragment
            text={body}
            collapsibleRenderBlocks={collapsibleRenderBlocks}
            resolveImageSrc={resolveImageSrc}
          />
        ),
      }),
    [collapsibleRenderBlocks, resolveImageSrc],
  );

  if (!text) return null;

  const normalizedText = normalizeDisplayMathDelimiters(text);

  return (
    <ReactMarkdown
      components={{
        ...markdownComponents,
        img: ({ src, alt, title }) => {
          const resolved =
            typeof src === "string" ? resolveImageSrc?.(src) ?? src : "";
          return resolved ? (
            <img src={resolved} alt={alt} title={title} />
          ) : (
            <span role="status">{alt || "Image unavailable"}</span>
          );
        },
      }}
      remarkPlugins={[remarkGfm, remarkMath]}
      rehypePlugins={[
        [rehypeKatex, { strict: "warn", throwOnError: false, trust: false }],
      ]}
    >
      {normalizedText}
    </ReactMarkdown>
  );
});

export const RuaMarkdown = memo(function RuaMarkdown({
  content,
  className,
  style,
  collapsibleRenderBlocks = false,
  resolveImageSrc,
}: RuaMarkdownProps) {
  const segments = splitMarkdownPlus(content);
  return (
    <div
      className={["rua-markdown", className].filter(Boolean).join(" ")}
      style={style}
    >
      {segments.map((segment, index) => (
        <RenderStateScope key={index} name={"markdown-part:" + index}>
          {segment.kind === "markdown" ? (
            <MarkdownFragment
              text={segment.text}
              collapsibleRenderBlocks={collapsibleRenderBlocks}
              resolveImageSrc={resolveImageSrc}
              key={`markdown-${index}`}
            />
          ) : (
            <MarkdownPlusBlockView
              block={segment.block}
              collapsibleRenderBlocks={collapsibleRenderBlocks}
              resolveImageSrc={resolveImageSrc}
              renderMarkdown={(text) => (
                <MarkdownFragment
                  text={text}
                  collapsibleRenderBlocks={collapsibleRenderBlocks}
                  resolveImageSrc={resolveImageSrc}
                />
              )}
              key={`mdp-${index}`}
            />
          )}
        </RenderStateScope>
      ))}
    </div>
  );
});
