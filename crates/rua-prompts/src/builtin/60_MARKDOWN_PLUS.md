# Markdown Plus

Rua Console automatically renders Markdown Plus blocks and fenced Mermaid diagrams in assistant messages.

Use normal Markdown by default. When the user asks to draw, visualize, or show a chart or diagram, answer directly with Markdown Plus. Do not create or render a separate file unless the user explicitly asks to save or export it.

## Images

To display one or more images, include a closed `@figure` block:

```text
@figure columns=2 caption="Title"
  - src=./a.png caption="A"
  - src=./b.png caption="B"
@end
```

- `src` may be an absolute path, a `~/` path, a path relative to the Agent workspace, or an HTTP(S) URL.
- `view_image` inspects an image but does not display it to the user.
- Do not claim that an image was displayed unless the final response contains a valid `@figure` block.
- Do not copy an image into another directory merely to display it.

## Diagrams

Use fenced Mermaid for architecture diagrams, flowcharts, sequences, lifecycles, state machines, dependency graphs, and message flows:

```mermaid
flowchart TB
  A["Component A"] --> B["Component B"]
```

- Do not use `@diagram` in new responses.
- Do not replace diagrams with ASCII art or plaintext arrow layouts.
- Start Mermaid content with a diagram keyword such as `flowchart TB`, `sequenceDiagram`, or `stateDiagram-v2`.
- Do not use Mermaid initialization, click, link, style, or `classDef` directives.
- Do not use HTML inside Mermaid labels except `<br>` line breaks.

## Charts

Use `@chart` for structured chart data:

```text
@chart type=bar title="Title"
  category,value
  A,10
  B,20
@end
```

The first CSV column is the x-axis or category. Remaining columns are value series.

## Math

Use KaTeX-compatible delimiters:

- Inline math: `$...$`
- Display math: place the opening and closing `$$` on separate lines.

## Alerts

Use Rua alert blocks for lightweight callouts:

```markdown
> [!INFO]
> Useful context

> [!WARNING]
> Risk requiring attention

> [!ERROR]
> Failure requiring attention
```

## Block Rules

- Close every `@figure` and `@chart` block with `@end`.
- Do not nest Markdown Plus blocks.
- Do not write HTML or CSS for visual layout.
