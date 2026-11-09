import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  RuaMarkdown,
  mermaidRenderConfig,
  sanitizeMermaidSvgForRender,
  toMermaidSource,
} from "./RuaMarkdown";
import { repairMermaidSource } from "./mermaidRepair";

const parsedMarkdown = vi.hoisted(() => vi.fn());
const highlightedCode = vi.hoisted(() => vi.fn());
vi.mock("react-syntax-highlighter", async (importOriginal) => {
  const actual = await importOriginal<
    typeof import("react-syntax-highlighter")
  >();
  return {
    ...actual,
    Prism: (props: React.ComponentProps<typeof actual.Prism>) => {
      highlightedCode(props.children);
      return <actual.Prism {...props} />;
    },
  };
});
vi.mock("react-markdown", async (importOriginal) => {
  const actual = await importOriginal<typeof import("react-markdown")>();
  return {
    ...actual,
    default: (props: React.ComponentProps<typeof actual.default>) => {
      parsedMarkdown(props.children);
      return <actual.default {...props} />;
    },
  };
});

describe("RuaMarkdown", () => {
  it("reuses completed code highlighting while trailing text streams, but updates changed code", () => {
    const code = "```ts\nconst value = 42;\n```";
    const { container, rerender } = render(
      <RuaMarkdown content={code + "\n\nTail"} collapsibleRenderBlocks />,
    );
    highlightedCode.mockClear();
    for (let index = 0; index < 10; index++) {
      rerender(
        <RuaMarkdown
          content={code + `\n\nTail ${index}`}
          collapsibleRenderBlocks
        />,
      );
    }
    expect(highlightedCode).not.toHaveBeenCalled();
    expect(container).toHaveTextContent("Tail 9");
    rerender(
      <RuaMarkdown
        content={"```ts\nconst value = 43;\n```\n\nTail"}
        collapsibleRenderBlocks
      />,
    );
    expect(highlightedCode).toHaveBeenCalledTimes(1);
    expect(container).toHaveTextContent("const value = 43;");
  });
  it("does not reparse a completed Markdown segment when only the trailing segment streams", () => {
    const prefix =
      "## Complete section\n\n**Unchanged** text.\n\n@figure\n/image.png\n@end\n\n";
    const { rerender } = render(<RuaMarkdown content={prefix + "First"} />);
    parsedMarkdown.mockClear();
    rerender(<RuaMarkdown content={prefix + "First second"} />);
    expect(parsedMarkdown.mock.calls.map(([text]) => text)).not.toContain(
      "## Complete section\n\n**Unchanged** text.\n",
    );
    expect(
      parsedMarkdown.mock.calls.every(([text]) => !text.includes("Unchanged")),
    ).toBe(true);
    expect(screen.getByText("First second")).toBeInTheDocument();
  });

  it("renders markdown body text through the real renderer", () => {
    render(<RuaMarkdown content={"# Release Notes\n\n- Fixed login"} />);

    expect(
      screen.getByRole("heading", { level: 1, name: "Release Notes" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Fixed login")).toBeInTheDocument();
  });

  it("keeps GitHub-flavored markdown tables usable", () => {
    render(
      <RuaMarkdown
        content={
          "| Tool | State |\n| --- | --- |\n| Bash | enabled |\n| Read | disabled |"
        }
      />,
    );

    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(
      screen.getByRole("columnheader", { name: "Tool" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "Bash" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "disabled" })).toBeInTheDocument();
  });

  it("renders inline LaTeX formulas with KaTeX", () => {
    const { container } = render(
      <RuaMarkdown content={String.raw`Euler: $e^{i\pi} + 1 = 0$`} />,
    );

    expect(container.querySelector(".katex")).toBeInTheDocument();
    expect(container.querySelector(".katex-display")).not.toBeInTheDocument();
    expect(screen.getByText("Euler:", { exact: false })).toBeInTheDocument();
  });

  it("renders display LaTeX formulas with KaTeX", () => {
    const { container } = render(
      <RuaMarkdown
        content={String.raw`Before

$$
\int_0^1 x^2\,dx = \frac{1}{3}
$$

After`}
      />,
    );

    expect(
      container.querySelector(".katex-display .katex"),
    ).toBeInTheDocument();
    expect(screen.getByText("Before")).toBeInTheDocument();
    expect(screen.getByText("After")).toBeInTheDocument();
  });

  it("recovers display LaTeX when the closing delimiter follows the formula", () => {
    const { container } = render(
      <RuaMarkdown
        content={String.raw`Before

$$
\text{Attention}(Q, K, V) = \text{softmax}\left(\frac{QK^T}{\sqrt{d_k}}\right)V$$

## After

- Markdown remains visible`}
      />,
    );

    expect(
      container.querySelector(".katex-display .katex"),
    ).toBeInTheDocument();
    expect(container.querySelector(".katex-error")).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 2, name: "After" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Markdown remains visible")).toBeInTheDocument();
  });

  it("does not rewrite display math delimiters inside fenced code", () => {
    const { container } = render(
      <RuaMarkdown content={"```md\n$$\nformula$$\n```\n\nAfter"} />,
    );

    expect(container.querySelector(".katex")).not.toBeInTheDocument();
    expect(container.querySelector("code")).toHaveTextContent(
      /\$\$\s+formula\$\$/,
    );
    expect(screen.getByText("After")).toBeInTheDocument();
  });

  it("renders task list checkboxes as themeable read-only markers", () => {
    const { container } = render(
      <RuaMarkdown content={"- [x] shipped\n- [ ] pending"} />,
    );

    const markers = screen.getAllByRole("checkbox");
    expect(markers[0]).toBeChecked();
    expect(markers[0]).toBeDisabled();
    expect(markers[0].closest(".rua-check")).toHaveClass(
      "rua-markdown-task-checkbox",
    );
    expect(markers[1]).not.toBeChecked();
    expect(markers[1]).toBeDisabled();
    expect(container.querySelectorAll(".rua-check-box")).toHaveLength(2);
  });

  it("renders fenced code blocks through the Rust console syntax highlighter boundary", () => {
    const { container } = render(
      <RuaMarkdown content={"```ts\nconst answer = 42;\n```"} />,
    );

    const highlighted = container.querySelector(
      ".rua-markdown-code-highlight",
    );
    const code = highlighted?.querySelector("code");

    expect(highlighted).toBeInTheDocument();
    expect(code).toHaveAttribute("data-language", "ts");
    expect(code).toHaveTextContent("const answer = 42;");
  });

  it("wraps chat fenced code blocks in a collapsible render block when enabled", () => {
    const { container } = render(
      <RuaMarkdown
        content={"```ts\nconst answer = 42;\n```"}
        collapsibleRenderBlocks
      />,
    );

    const block = container.querySelector(".rua-render-block--code");
    const body = block?.querySelector(".rua-render-block-body");

    expect(block).toBeInTheDocument();
    expect(
      container.querySelectorAll(".rua-render-block--code"),
    ).toHaveLength(1);
    expect(body).toBeInTheDocument();
    expect(body).not.toHaveAttribute("hidden");

    fireEvent.click(screen.getByRole("button", { name: "Collapse block" }));

    expect(body).toHaveAttribute("hidden");
    expect(
      screen.getByRole("button", { name: "Expand block" }),
    ).toHaveAttribute("aria-expanded", "false");
  });

  it("does not bubble fenced code collapse clicks to the surrounding message", () => {
    const onMessageClick = vi.fn();
    const { container } = render(
      <div onClick={onMessageClick}>
        <RuaMarkdown
          content={"```ts\nconst answer = 42;\n```"}
          collapsibleRenderBlocks
        />
      </div>,
    );

    const body = container.querySelector(".rua-render-block-body");

    fireEvent.click(screen.getByRole("button", { name: "Collapse block" }));

    expect(body).toHaveAttribute("hidden");
    expect(
      screen.getByRole("button", { name: "Expand block" }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(onMessageClick).not.toHaveBeenCalled();
  });

  it("keeps chat fenced code collapsed across parent rerenders", () => {
    function RerenderHarness() {
      const [tick, setTick] = useState(0);
      return (
        <div data-tick={tick}>
          <button type="button" onClick={() => setTick((value) => value + 1)}>
            rerender
          </button>
          <RuaMarkdown
            content={"```ts\nconst answer = 42;\n```"}
            collapsibleRenderBlocks
          />
        </div>
      );
    }

    const { container } = render(<RerenderHarness />);
    const body = container.querySelector(".rua-render-block-body");

    fireEvent.click(screen.getByRole("button", { name: "Collapse block" }));
    expect(body).toHaveAttribute("hidden");

    fireEvent.click(screen.getByRole("button", { name: "rerender" }));

    expect(body).toHaveAttribute("hidden");
    expect(
      screen.getByRole("button", { name: "Expand block" }),
    ).toHaveAttribute("aria-expanded", "false");
  });

  it("renders Rua markdown alerts", () => {
    render(
      <RuaMarkdown
        content={
          "> [!INFO]\n> Reference values vary.\n\n> [!WARNING]\n> Cleaning `target/` frees disk, but rebuilds are slower.\n\n> [!ERROR]\n> Build failed."
        }
      />,
    );

    expect(screen.getByText("INFO")).toBeInTheDocument();
    expect(screen.getByText("WARNING")).toBeInTheDocument();
    expect(screen.getByText("ERROR")).toBeInTheDocument();
    expect(screen.getByText(/Reference values vary/)).toBeInTheDocument();
    expect(screen.getByText(/Cleaning/)).toBeInTheDocument();
    expect(screen.getByText(/Build failed/)).toBeInTheDocument();
    expect(screen.queryByText("[!INFO]")).not.toBeInTheDocument();
    expect(screen.queryByText("[!WARNING]")).not.toBeInTheDocument();
    expect(screen.queryByText("[!ERROR]")).not.toBeInTheDocument();
  });

  it("renders Markdown Plus figure blocks", () => {
    render(
      <RuaMarkdown
        content={
          '@figure columns=2 caption="Screenshots"\n  - src=./before.png caption="Before"\n  - src=./after.png caption="After"\n@end'
        }
      />,
    );

    expect(screen.getByText("Screenshots")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Before" })).toHaveAttribute(
      "src",
      "./before.png",
    );
    expect(screen.getByRole("img", { name: "After" })).toHaveAttribute(
      "src",
      "./after.png",
    );
  });

  it("renders Markdown Plus chart blocks with direct CSV bodies", () => {
    const { container } = render(
      <RuaMarkdown
        content={
          '@chart type=bar title="Space"\n  directory,gb\n  target,50\n  console,0.44\n@end'
        }
      />,
    );

    expect(screen.getByText("Space")).toBeInTheDocument();
    expect(screen.getByText("target")).toBeInTheDocument();
    expect(screen.getByText("50")).toBeInTheDocument();
    expect(screen.queryByText("@chart bar")).not.toBeInTheDocument();
    expect(screen.queryByText("data csv")).not.toBeInTheDocument();
    expect(container.querySelectorAll(".rua-mdp-bar-fill")).toHaveLength(2);
    expect(
      container.querySelector(".rua-mdp-bar-item--single"),
    ).toBeInTheDocument();
  });

  it("wraps Markdown Plus blocks in a collapsible render block when enabled", () => {
    const { container } = render(
      <RuaMarkdown
        collapsibleRenderBlocks
        content={
          '@chart type=bar title="Space"\n  directory,gb\n  target,50\n@end'
        }
      />,
    );

    expect(
      container.querySelector(".rua-render-block--markdown-plus"),
    ).toBeInTheDocument();
    expect(
      container.querySelector(".rua-render-block-title"),
    ).toHaveTextContent("Space");
    expect(
      container.querySelector(".rua-mdp-caption"),
    ).not.toBeInTheDocument();
  });

  it("renders fenced Mermaid blocks as diagrams instead of code", () => {
    const { container } = render(
      <RuaMarkdown
        collapsibleRenderBlocks
        content={
          '```mermaid\nflowchart TB\n  subgraph Input["输入"]\n    X["输入序列"]\n  end\n  X --> Q["Query"]\n```'
        }
      />,
    );

    expect(
      container.querySelector(".rua-mdp-diagram-loading"),
    ).toBeInTheDocument();
    expect(
      container.querySelector(".rua-render-block--markdown-plus"),
    ).toBeInTheDocument();
    expect(container.querySelector(".rua-render-block--code")).toBeNull();
    expect(container.textContent).not.toContain("flowchart TB");
    expect(container.textContent).not.toContain('subgraph Input["输入"]');
  });

  it("keeps legacy @diagram blocks compatible", () => {
    const { container } = render(
      <RuaMarkdown
        content={
          '@diagram title="Flow"\n  flowchart TB\n    subgraph Runtime["Runtime"]\n      A[User] --> B{Need tool?}\n    end\n    B -- yes --> C[Tool]\n@end'
        }
      />,
    );

    expect(screen.getByText("Flow")).toBeInTheDocument();
    expect(
      container.querySelector(".rua-mdp-diagram-loading"),
    ).toBeInTheDocument();
    expect(container.textContent).not.toContain("flowchart TB");
    expect(container.textContent).not.toContain('subgraph Runtime["Runtime"]');
    expect(screen.queryByText(/Unsupported Mermaid/)).not.toBeInTheDocument();
  });

  it("keeps legacy diagram bodies compatible by adding a flowchart directive", () => {
    expect(toMermaidSource(["A[User] -> B[Agent]"])).toBe(
      "flowchart TD\nA[User] --> B[Agent]",
    );
  });

  it("preserves explicit Mermaid directives", () => {
    expect(toMermaidSource(["sequenceDiagram", "User->>Agent: Ask"])).toBe(
      "sequenceDiagram\nUser->>Agent: Ask",
    );
  });

  it("repairs literal semicolons in Mermaid sequence messages after parse failure", async () => {
    const source = `sequenceDiagram
  participant R as Capability resolver
  participant M as MCP manager
  participant S as stale-codegraph

  R->>M: list_servers()
  M-->>R: Ready
  R->>M: list_tools()
  M->>S: connection/discovery
  S-->>M: executable missing; state becomes Failed
  M-->>R: stale or concurrently obtained tool list`;
    const repaired = repairMermaidSource(source);
    const { default: mermaid } = await import("mermaid");

    await expect(mermaid.parse(source)).rejects.toBeDefined();
    expect(repaired).toContain(
      "S-->>M: executable missing#59; state becomes Failed",
    );
    await expect(mermaid.parse(repaired)).resolves.toBeDefined();
  });

  it("allows Mermaid br line breaks in labels", () => {
    const { container } = render(
      <RuaMarkdown
        content={
          '@diagram title="ACP"\n  flowchart TB\n    SVC["AcpService<br/>会话生命周期管理"]\n    CLIENT["AcpClient<br />JSON-RPC 通信"]\n    SVC --> CLIENT\n@end'
        }
      />,
    );

    expect(screen.queryByText(/Unsupported Mermaid/)).not.toBeInTheDocument();
    expect(
      container.querySelector(".rua-mdp-diagram-loading"),
    ).toBeInTheDocument();
    expect(container.textContent).not.toContain("AcpService<br/>");
    expect(container.textContent).not.toContain("AcpClient<br />");
  });

  it("allows model special tokens in Mermaid sequence messages", () => {
    const { container } = render(
      <RuaMarkdown
        content={
          '@diagram title="Generation"\n  sequenceDiagram\n    participant Dec as Decoder\n    Dec->>Dec: Input "<sos>", masked attention\n    Dec->>Dec: Emit "<eos>"\n@end'
        }
      />,
    );

    expect(screen.queryByText(/Unsupported Mermaid/)).not.toBeInTheDocument();
    expect(
      container.querySelector(".rua-mdp-diagram-loading"),
    ).toBeInTheDocument();
  });

  it("configures Mermaid to emit HTML labels for sanitizer conversion", () => {
    const config = mermaidRenderConfig();

    expect(config.flowchart.htmlLabels).toBe(true);
    expect(config.sequence.actorFontSize).toBe("14px");
    expect(config.sequence.messageFontSize).toBe("14px");
    expect(config.sequence.noteFontSize).toBe("14px");
    expect(config.themeVariables.strokeWidth).toBe("1.35px");
    expect(config.themeVariables.fontSize).toBe("14px");
  });

  it("rejects unsafe Mermaid directives", () => {
    render(
      <RuaMarkdown
        content={
          '```mermaid\nflowchart TB\n  A[User] --> B[UI]\n  click A href "https://example.com"\n```'
        }
      />,
    );

    expect(screen.getByText(/Unsupported Mermaid syntax/)).toBeInTheDocument();
    expect(screen.getByText(/click A href/)).toBeInTheDocument();
  });

  it("rejects Mermaid labels with non-br HTML", () => {
    render(
      <RuaMarkdown
        content={
          '@diagram title="Unsafe HTML"\n  flowchart TB\n    A["<span>unsafe</span>"]\n@end'
        }
      />,
    );

    expect(screen.getByText(/Unsupported Mermaid syntax/)).toBeInTheDocument();
    expect(screen.getByText(/<span>unsafe/)).toBeInTheDocument();
  });

  it("keeps Mermaid foreignObject node labels after SVG sanitization", () => {
    const sanitized = sanitizeMermaidSvgForRender(`
      <svg xmlns="http://www.w3.org/2000/svg">
        <g class="node">
          <g class="label">
            <rect></rect>
            <foreignObject x="-40" y="-12" width="80" height="24">
              <div xmlns="http://www.w3.org/1999/xhtml">
                <span class="nodeLabel"><p>用户请求</p></span>
              </div>
            </foreignObject>
          </g>
        </g>
        <script>alert("xss")</script>
      </svg>
    `);

    expect(sanitized).toContain("用户请求");
    expect(sanitized).toContain("<text");
    expect(sanitized).toContain('x="0"');
    expect(sanitized).toContain('y="0"');
    expect(sanitized).not.toContain("foreignObject");
    expect(sanitized).not.toContain("<script");
  });

  it("preserves Mermaid foreignObject br labels as SVG tspans", () => {
    const sanitized = sanitizeMermaidSvgForRender(`
      <svg xmlns="http://www.w3.org/2000/svg">
        <g class="node">
          <g class="label">
            <foreignObject x="-90" y="-24" width="180" height="48">
              <div xmlns="http://www.w3.org/1999/xhtml">
                <span class="nodeLabel">AcpService<br/>会话生命周期管理</span>
              </div>
            </foreignObject>
          </g>
        </g>
      </svg>
    `);

    expect(sanitized).toContain("AcpService");
    expect(sanitized).toContain("会话生命周期管理");
    expect(sanitized).toContain("<tspan");
    expect(sanitized).toContain('x="0"');
    expect(sanitized).not.toContain("foreignObject");
  });

  it("normalizes Mermaid HTML-style br tags before SVG parsing", () => {
    const sanitized = sanitizeMermaidSvgForRender(`
      <svg xmlns="http://www.w3.org/2000/svg">
        <g class="node">
          <g class="label">
            <foreignObject x="-90" y="-24" width="180" height="48">
              <div xmlns="http://www.w3.org/1999/xhtml">
                <span class="nodeLabel"><p>AcpService<br>会话生命周期管理</p></span>
              </div>
            </foreignObject>
          </g>
        </g>
      </svg>
    `);

    expect(sanitized).toContain("AcpService");
    expect(sanitized).toContain("会话生命周期管理");
    expect(sanitized).toContain("<tspan");
    expect(sanitized).not.toContain("foreignObject");
  });

  it("normalizes flowchart SVG styling to the Rua diagram contract", () => {
    const sanitized = sanitizeMermaidSvgForRender(`
      <svg id="diagram" xmlns="http://www.w3.org/2000/svg">
        <style>#diagram .node rect{stroke-width:1px;}</style>
        <g class="node"><rect x="0" y="0" width="120" height="40"></rect></g>
        <g class="edgePath"><path class="path" d="M 0 0 L 120 40"></path></g>
      </svg>
    `);

    expect(sanitized).toContain('data-rua-mdp-theme="true"');
    expect(sanitized).toContain('data-rua-mdp-style="true"');
    expect(sanitized).toContain('stroke-width="1.35px"');
    expect(sanitized).toContain('rx="2"');
    expect(sanitized).toContain("#diagram .node rect");
    expect(sanitized).toContain("font-size: 14px");
  });

  it("normalizes sequence SVG styling to the Rua diagram contract", () => {
    const sanitized = sanitizeMermaidSvgForRender(`
      <svg id="sequence" xmlns="http://www.w3.org/2000/svg">
        <rect class="actor actor-top" x="0" y="0" width="120" height="40" rx="3" ry="3"></rect>
        <line class="actor-line" x1="60" y1="40" x2="60" y2="200" stroke-width="0.5px"></line>
        <path class="messageLine0" d="M 60 80 L 180 80"></path>
        <rect class="labelBox" x="10" y="100" width="100" height="24"></rect>
        <path class="loopLine" d="M 10 100 L 210 100"></path>
      </svg>
    `);

    expect(sanitized).toContain('class="actor actor-top"');
    expect(sanitized).toContain('rx="2"');
    expect(sanitized).toContain('stroke-width="1.35px"');
    expect(sanitized).toContain('stroke-dasharray="4 4"');
    expect(sanitized).toContain("#sequence rect.actor");
    expect(sanitized).toContain(".messageLine0");
    expect(sanitized).toContain(".loopLine");
  });

  it("keeps unclosed Markdown Plus blocks as readable text", () => {
    render(
      <RuaMarkdown
        content={'@chart type=bar title="Space"\n  directory,gb\n  target,50'}
      />,
    );

    expect(screen.getByText(/@chart type=bar/)).toBeInTheDocument();
    expect(screen.queryByText("@chart bar")).not.toBeInTheDocument();
  });

  it("merges caller className and style with the markdown root", () => {
    const { container } = render(
      <RuaMarkdown
        content="plain"
        className="custom-markdown"
        style={{ maxHeight: 120 }}
      />,
    );

    const root = container.firstElementChild;
    expect(root).toHaveClass("rua-markdown");
    expect(root).toHaveClass("custom-markdown");
    expect(root).toHaveStyle({ maxHeight: "120px" });
  });
});
