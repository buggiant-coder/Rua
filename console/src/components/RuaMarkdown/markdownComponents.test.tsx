import ReactMarkdown from "react-markdown";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createMarkdownComponents } from "./markdownComponents";

function MarkdownHarness({
  text,
  collapsibleRenderBlocks = false,
}: {
  text: string;
  collapsibleRenderBlocks?: boolean;
}) {
  const components = createMarkdownComponents({
    collapsibleRenderBlocks,
    renderMarkdown: (body) => (
      <MarkdownHarness
        text={body}
        collapsibleRenderBlocks={collapsibleRenderBlocks}
      />
    ),
  });
  return <ReactMarkdown components={components}>{text}</ReactMarkdown>;
}

describe("createMarkdownComponents", () => {
  it("wraps language fenced code in a Chat Render Block when enabled", () => {
    const { container } = render(
      <MarkdownHarness
        text={"```ts\nconst answer = 42;\n```"}
        collapsibleRenderBlocks
      />,
    );

    expect(
      container.querySelector(".rua-render-block--code"),
    ).toBeInTheDocument();
    expect(container.querySelector("code")).toHaveAttribute(
      "data-language",
      "ts",
    );
  });

  it("keeps inline code inline", () => {
    const { container } = render(
      <MarkdownHarness text={"Use `target` here."} collapsibleRenderBlocks />,
    );

    expect(container.querySelector(".rua-render-block--code")).toBeNull();
    expect(container.querySelector("code")).toHaveTextContent("target");
  });

  it("renders alert blockquotes through the supplied markdown renderer", () => {
    render(
      <MarkdownHarness text={"> [!WARNING]\n> Use `clippy-touched.sh`."} />,
    );

    expect(screen.getByText("WARNING")).toBeInTheDocument();
    expect(screen.getByText(/Use clippy-touched\.sh/)).toBeInTheDocument();
    expect(screen.queryByText("[!WARNING]")).not.toBeInTheDocument();
  });
});
