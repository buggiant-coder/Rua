import { describe, expect, it } from "vitest";
import { splitMarkdownPlus } from "./markdownPlus";

describe("splitMarkdownPlus", () => {
  it("splits closed Markdown Plus blocks into renderable segments", () => {
    expect(
      splitMarkdownPlus(
        'Intro\n@chart type=bar title="Space"\n  directory,gb\n  target,50\n@end\nOutro',
      ),
    ).toEqual([
      { kind: "markdown", text: "Intro" },
      {
        kind: "mdp",
        block: {
          type: "chart",
          params: { type: "bar", title: "Space" },
          body: ["directory,gb", "target,50"],
          raw: '@chart type=bar title="Space"\n  directory,gb\n  target,50\n@end',
        },
      },
      { kind: "markdown", text: "Outro" },
    ]);
  });

  it("leaves Markdown Plus-looking text inside fenced code alone", () => {
    const content = "```md\n@chart type=bar\n@end\n```";

    expect(splitMarkdownPlus(content)).toEqual([
      { kind: "markdown", text: content },
    ]);
  });

  it("converts closed Mermaid fences into diagram blocks", () => {
    expect(
      splitMarkdownPlus(
        'Intro\n```mermaid\n  flowchart TB\n    A["Input"] --> B["Output"]\n```\nOutro',
      ),
    ).toEqual([
      { kind: "markdown", text: "Intro" },
      {
        kind: "mdp",
        block: {
          type: "diagram",
          params: {},
          body: ["flowchart TB", '  A["Input"] --> B["Output"]'],
          raw: '```mermaid\n  flowchart TB\n    A["Input"] --> B["Output"]\n```',
        },
      },
      { kind: "markdown", text: "Outro" },
    ]);
  });

  it("keeps an unclosed Mermaid fence as markdown while streaming", () => {
    const content = "Before\n```mermaid\nflowchart TB\n  A --> B";

    expect(splitMarkdownPlus(content)).toEqual([
      { kind: "markdown", text: content },
    ]);
  });

  it("keeps an unclosed trailing block as markdown for streaming safety", () => {
    const content = 'Before\n@diagram title="Flow"\n  A --> B';

    expect(splitMarkdownPlus(content)).toEqual([
      { kind: "markdown", text: content },
    ]);
  });
});
