# ADR-0048: Markdown Plus editable visual documents

> **Status:** Accepted

## Context

Agents need to produce diagrams, charts, and image collections that remain easy
for humans and models to inspect and revise. Plain Markdown has weak structured
visual semantics. Generated binaries are hard to diff, while HTML, arbitrary
SVG, and CSS create a large execution and authoring surface with fragile nested
structure.

The durable representation should remain text, degrade intelligibly in ordinary
Markdown tools, and support incremental streaming. Rendering must never require
executing author-controlled HTML, JavaScript, Mermaid initialization directives,
or click handlers.

## Decision

### D1 - Markdown Plus extends Markdown as text

Markdown Plus is an editable textual document contract. Ordinary Markdown
remains valid input. Assistant messages remain plain text; Markdown Plus is not
a Conversation message type, binary artifact, or separate persistence model.

Files may use `.mdp`, while the Console also recognizes supported visual blocks
inside rendered Markdown messages.

### D2 - Visual blocks are flat and explicitly closed

The native v0 block form begins with `@figure` or `@chart`, accepts
`key=value` parameters, and ends with a generic `@end`. Blocks do not nest.
Shared body indentation is presentation-neutral and may be stripped by the
parser. Historical `@diagram ... @end` input remains parser compatibility only;
new diagrams use a standard fenced Mermaid block.

The small component vocabulary is semantic:

- `figure` describes one or more ordered image resources and captions;
- `chart` contains simple structured chart data.

The typed parser representation may still contain `diagram` because both
fenced Mermaid and historical `@diagram` input normalize to that renderer path.

It is not a page-layout or application UI language.

### D3 - Parsing produces a small intermediate representation

One parser splits ordinary Markdown from closed visual blocks and produces a
typed block representation before rendering. Parameter parsing, validation,
formatting, and renderer selection consume that representation rather than
rescanning raw text independently.

Standard fenced Mermaid input and historical `@diagram` input are normalized
into the diagram representation so the renderer has one diagram path.

### D4 - Streaming is fail-visible and non-destructive

During streaming, a block is recognized only after its closing `@end` or
closing Mermaid fence arrives. An incomplete trailing block remains visible as
ordinary source text. Invalid or unsupported content produces inline fallback
or an error surface; it must not blank the surrounding message.

The parser preserves unrelated Markdown and code fences exactly enough that an
`@figure`-looking line inside a normal code block is not treated as a visual.

### D5 - Rendering is capability-limited

Renderers own responsive layout, accessibility, theme integration, and safe
defaults. The target contract limits authors to semantic content and a bounded
parameter set, not arbitrary DOM, CSS, script, event handlers, coordinates, or
component trees.

Mermaid rendering disables author initialization, links, click handlers,
embedded HTML, and unsupported directives. Resource loading follows the
existing image and URL trust boundaries.

### D6 - Source remains the editing authority

Rendered visuals are projections of the text. Edits change the Markdown Plus
source and are reparsed; the visual renderer does not become a second document
store. Stable formatting should preserve local edits and readable diffs.

### D7 - Unsupported clients receive readable source

A client without Markdown Plus support may show the raw block as text. The
format does not require a binary sidecar merely to preserve meaning. Exact
values and explanatory prose should remain in Markdown when they matter beyond
the visualization.

### D8 - The core language remains intentionally small

Additional layout components, arbitrary nested containers, visual block ids,
cross-references, coordinates, and width systems require separate evidence and
decisions. Familiar Markdown alerts remain Markdown rendering extensions, not
new Markdown Plus components.

## Current implementation gap

The parser currently accepts any syntactically valid parameter key and stores
it in a generic string map. Renderer-side `columns` parsing accepts any positive
integer and imposes no upper bound. Sanitization, Mermaid isolation, and
streaming fallback are implemented, but the intended per-block parameter
allow-list and numeric bounds are not. The format must not be described as a
fully bounded parameter schema until those checks exist.

## Hard invariants

- Markdown Plus source is plain text and ordinary Markdown is valid input.
- Assistant persistence does not add a Markdown Plus message type.
- Native `@figure` and `@chart` blocks are flat and closed with `@end`;
  `@diagram` is accepted only for historical compatibility.
- Unclosed streaming blocks remain visible as source text.
- Code-fenced content is not reparsed as Markdown Plus.
- Parsing is shared before component rendering.
- Author-controlled HTML, CSS, JavaScript, and Mermaid click/init behavior are
  not executed.
- Rendered output is a projection, never the editing authority.
- Unsupported clients can preserve and display the original source.
- The component set does not become a general UI DSL.

## Authority, persistence, and failure boundaries

The textual document or Conversation Message owns durable content. The
Markdown Plus parser owns block recognition and parameter parsing. Component
renderers own safe visual projection. Existing media/resource services own
resource access and authorization.

A parse or renderer failure affects only the visual projection and falls back
to visible source or an inline error. It does not mutate persisted text,
discard surrounding Markdown, or execute a substitute format. Missing images
and invalid diagrams are resource/render failures, not Conversation failures.

## Consequences

- Visual documents remain diffable, synchronizable, and Agent-editable.
- Streaming output never disappears because a block is incomplete.
- Renderers can improve without changing durable Conversation schemas.
- Security review is bounded to a small parser and renderer capability set.
- Cross-client fidelity depends on shared syntax and validation tests.

## Rejected alternatives

- HTML or a restricted HTML subset as the authoring language.
- Arbitrary SVG, Canvas, or JavaScript supplied by the author.
- Nested fenced containers or component-specific closing tags.
- A broad set of layout components such as grid, card, tabs, and modal.
- Manual coordinates, spans, widths, or named layout templates.
- Persisting rendered images as the only source.
- Treating incomplete blocks as empty output.

## Implementation and verification

- `console/src/components/RuaMarkdown/markdownPlus.ts`
- `console/src/components/RuaMarkdown/markdownPlusRender.tsx`
- `console/src/components/RuaMarkdown/RuaMarkdown.tsx`
- `console/src/components/RuaMarkdown/mermaidRepair.ts`
- `console/src/components/RuaMarkdown/markdownPlus.test.ts`
- `console/src/components/RuaMarkdown/RuaMarkdown.test.tsx`
- `crates/rua-prompts/src/builtin/60_MARKDOWN_PLUS.md`

Verification covers flat block parsing, quoted parameters, code-fence
isolation, Mermaid compatibility, incomplete streaming input, invalid fallback,
unsafe directive rejection, resource errors, and ordinary Markdown
preservation. It does not cover rejection of unknown parameter keys or an upper
bound for `figure.columns`.

## Related decisions

- ADR-0020 keeps Conversation Message Log content as durable text.
- ADR-0044 keeps Live rendering separate from Conversation persistence.
