import { isValidElement, memo } from "react";
import type { CSSProperties, ReactNode } from "react";
import type { Components } from "react-markdown";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { ChatRenderBlock } from "@/components/ChatRenderBlock";
import Check from "@/ui/Check";

type MarkdownComponentsOptions = {
  collapsibleRenderBlocks: boolean;
  renderMarkdown: (text: string) => ReactNode;
};

const syntaxTheme: Record<string, CSSProperties> = {
  'code[class*="language-"]': {
    color: "inherit",
    background: "transparent",
    textShadow: "none",
    fontFamily: "var(--font-mono)",
  },
  'pre[class*="language-"]': {
    color: "inherit",
    background: "transparent",
    textShadow: "none",
  },
  comment: { color: "var(--text-3)" },
  punctuation: { color: "var(--text-2)" },
  property: { color: "var(--danger)" },
  tag: { color: "var(--danger)" },
  string: { color: "var(--success)" },
  number: { color: "var(--warning)" },
  boolean: { color: "var(--warning)" },
  keyword: { color: "var(--primary)" },
  function: { color: "var(--info)" },
  operator: { color: "var(--text-2)" },
};

const ALERT_RE = /^\[!(INFO|WARNING|ERROR)\]\s*/i;

const HighlightedCode = memo(function HighlightedCode({
  code,
  language,
  collapsible,
}: {
  code: string;
  language: string;
  collapsible: boolean;
}) {
  return (
    <SyntaxHighlighter
      className="rua-markdown-code-highlight"
      language={language}
      PreTag="div"
      style={syntaxTheme}
      customStyle={{
        margin: 0,
        padding: collapsible ? "calc(var(--space-3) - 2px) var(--space-3)" : 0,
        background: "transparent",
      }}
      codeTagProps={{ "data-language": language }}
    >
      {code}
    </SyntaxHighlighter>
  );
});

function extractText(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(extractText).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) {
    return extractText(node.props.children);
  }
  return "";
}

export function createMarkdownComponents({
  collapsibleRenderBlocks,
  renderMarkdown,
}: MarkdownComponentsOptions): Components {
  return {
    input({ type, checked }) {
      if (type !== "checkbox") {
        return <input type={type} checked={checked} readOnly />;
      }

      return (
        <Check
          checked={checked}
          className="rua-markdown-task-checkbox"
          disabled
          readOnly
        />
      );
    },

    blockquote({ children }) {
      const text = extractText(children).trimStart();
      const match = ALERT_RE.exec(text);
      if (!match) {
        return <blockquote>{children}</blockquote>;
      }

      const tone = match[1].toLowerCase();
      const body = text.slice(match[0].length).trimStart();
      return (
        <div className={`rua-markdown-alert rua-markdown-alert--${tone}`}>
          <div className="rua-markdown-alert-title">
            {match[1].toUpperCase()}
          </div>
          <div className="rua-markdown-alert-body">
            {renderMarkdown(body)}
          </div>
        </div>
      );
    },

    pre({ children, node, ...props }) {
      void node;

      if (!collapsibleRenderBlocks) {
        return <pre {...props}>{children}</pre>;
      }
      return <>{children}</>;
    },

    code({ className, children, node, ...props }) {
      const language = /language-([\w-]+)/.exec(className || "")?.[1];
      const rawCode = String(children);
      const isBlockCode =
        Boolean(language) || rawCode.includes("\n") || rawCode.endsWith("\n");
      if (!language) {
        if (collapsibleRenderBlocks && isBlockCode) {
          return (
            <ChatRenderBlock
              stateKey={"code:" + node?.position?.start.offset}
              className="rua-render-block--code"
              ariaLabel="Code block"
            >
              <pre>
                <code className={className} {...props}>
                  {children}
                </code>
              </pre>
            </ChatRenderBlock>
          );
        }

        return (
          <code className={className} {...props}>
            {children}
          </code>
        );
      }

      const code = rawCode.replace(/\n$/, "");
      const highlighted = (
        <HighlightedCode
          code={code}
          language={language}
          collapsible={collapsibleRenderBlocks}
        />
      );

      if (!collapsibleRenderBlocks) {
        return highlighted;
      }

      return (
        <ChatRenderBlock
          stateKey={"code:" + node?.position?.start.offset}
          className="rua-render-block--code"
          ariaLabel="Code block"
        >
          {highlighted}
        </ChatRenderBlock>
      );
    },
  };
}
