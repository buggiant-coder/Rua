import DOMPurify from "dompurify";
import { ImageOff } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ChatRenderBlock } from "@/components/ChatRenderBlock";
import { useTheme } from "@/contexts/ThemeContext";
import {
  parseMarkdownPlusParams,
  type MarkdownPlusBlock,
} from "./markdownPlus";
import { repairMermaidSource } from "./mermaidRepair";

type MarkdownRenderer = (text: string) => ReactNode;

type FigureItem = {
  src: string;
  caption?: string;
};

type ChartData = {
  headers: string[];
  rows: string[][];
};

const MERMAID_DIRECTIVE_RE =
  /^(flowchart|graph|sequenceDiagram|classDiagram|stateDiagram(?:-v2)?|erDiagram|journey|gantt|pie|quadrantChart|requirementDiagram|gitGraph|mindmap|timeline|zenuml|sankey-beta|xychart-beta|block-beta|packet-beta|architecture-beta)\b/i;
const UNSAFE_MERMAID_DIRECTIVE_RE =
  /^\s*(%%\{|click\b|href\b|link\b|style\b|classDef\b|linkStyle\b)/im;
const MERMAID_HTML_TAG_RE = /<\/?([a-z][\w:-]*)(?:\s[^>]*)?>/gim;
const MERMAID_LITERAL_TOKEN_RE = /^<\/?(?:sos|eos|bos|pad|unk|mask)>$/i;
const MDP_DIAGRAM_STROKE_WIDTH = 1.35;
const MDP_DIAGRAM_FONT_SIZE = "14px";
let mermaidRenderCounter = 0;

function parsePositiveInt(value: string | undefined, fallback: number): number {
  if (!value) return fallback;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function parseFigureItems(lines: string[]): FigureItem[] {
  return lines
    .map((line) => line.trim())
    .filter((line) => line.startsWith("- "))
    .map((line) => parseMarkdownPlusParams(line.slice(2)))
    .filter(
      (item): item is FigureItem => typeof item.src === "string" && !!item.src,
    );
}

function parseCsv(lines: string[]): ChartData | null {
  const rows = lines
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => line.split(",").map((cell) => cell.trim()));
  if (rows.length < 2 || rows[0].length < 2) return null;
  return { headers: rows[0], rows: rows.slice(1) };
}

function MarkdownPlusFigureItem({
  item,
  resolveImageSrc,
}: {
  item: FigureItem;
  resolveImageSrc?: (source: string) => string;
}) {
  const [failedSrc, setFailedSrc] = useState<string | null>(null);
  const { t } = useTranslation();
  const src = resolveImageSrc?.(item.src) ?? item.src;

  return (
    <div className="rua-mdp-figure-item">
      {!src || failedSrc === src ? (
        <div className="rua-mdp-figure-unavailable" role="status">
          <ImageOff size={20} aria-hidden="true" />
          <span>
            {t("chat.imageUnavailable", { defaultValue: "Image unavailable" })}
          </span>
        </div>
      ) : (
        <img
          src={src}
          alt={item.caption || item.src}
          onError={() => setFailedSrc(src)}
        />
      )}
      {item.caption && (
        <div className="rua-mdp-figure-caption">{item.caption}</div>
      )}
    </div>
  );
}

function MarkdownPlusFigure({
  block,
  renderMarkdown,
  resolveImageSrc,
  showCaption = true,
}: {
  block: MarkdownPlusBlock;
  renderMarkdown: MarkdownRenderer;
  resolveImageSrc?: (source: string) => string;
  showCaption?: boolean;
}) {
  const items = parseFigureItems(block.body);
  const columns = parsePositiveInt(block.params.columns, 1);
  const caption = block.params.caption;

  if (items.length === 0) {
    return <>{renderMarkdown(block.raw)}</>;
  }

  return (
    <figure className="rua-mdp-block rua-mdp-figure">
      <div
        className="rua-mdp-figure-grid"
        style={{ "--mdp-figure-columns": columns } as CSSProperties}
      >
        {items.map((item, index) => (
          <MarkdownPlusFigureItem
            item={item}
            resolveImageSrc={resolveImageSrc}
            key={`${item.src}-${index}`}
          />
        ))}
      </div>
      {showCaption && caption && (
        <figcaption className="rua-mdp-caption">{caption}</figcaption>
      )}
    </figure>
  );
}

function MarkdownPlusDiagram({
  block,
  showCaption = true,
}: {
  block: MarkdownPlusBlock;
  showCaption?: boolean;
}) {
  const title = block.params.title;
  const lines = trimOuterBlankLines(block.body);
  const fallback = lines.join("\n");
  const validationError = diagramValidationError(fallback);
  const source = validationError ? "" : toMermaidSource(lines);

  return (
    <section
      className="rua-mdp-block rua-mdp-diagram"
      aria-label={title || "Mermaid diagram"}
    >
      {validationError ? (
        <DiagramFallback error={validationError} fallback={fallback} />
      ) : (
        <MermaidDiagram source={source} fallback={fallback} />
      )}
      {showCaption && title && <div className="rua-mdp-caption">{title}</div>}
    </section>
  );
}

function diagramValidationError(body: string): string {
  if (UNSAFE_MERMAID_DIRECTIVE_RE.test(body) || hasUnsafeMermaidHtml(body)) {
    return "Unsupported Mermaid syntax. Mermaid diagrams in Rua cannot use init directives, click/href/link/style directives, class definitions, or HTML other than <br> line breaks.";
  }
  return "";
}

function hasUnsafeMermaidHtml(body: string): boolean {
  for (const match of body.matchAll(MERMAID_HTML_TAG_RE)) {
    if (
      match[1].toLowerCase() !== "br" &&
      !MERMAID_LITERAL_TOKEN_RE.test(match[0])
    ) {
      return true;
    }
  }
  return false;
}

function trimOuterBlankLines(lines: string[]): string[] {
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].trim() === "") start += 1;
  while (end > start && lines[end - 1].trim() === "") end -= 1;
  return lines.slice(start, end);
}

function normalizeLegacyMermaidBody(body: string): string {
  return body.replace(/\s->\s/g, " --> ");
}

export function toMermaidSource(lines: string[]): string {
  const body = lines.join("\n");
  const firstLine = lines.find((line) => line.trim())?.trim() || "";
  if (MERMAID_DIRECTIVE_RE.test(firstLine)) {
    return body;
  }
  return `flowchart TD\n${normalizeLegacyMermaidBody(body)}`;
}

function cssCustomPropertyValue(name: string, fallback: string): string {
  if (typeof window === "undefined") return fallback;
  const style = window.getComputedStyle(document.documentElement);
  return resolveCssCustomPropertyValue(
    style.getPropertyValue(name).trim(),
    style,
    fallback,
  );
}

function resolveCssCustomPropertyValue(
  value: string,
  style: CSSStyleDeclaration,
  fallback: string,
  depth = 0,
): string {
  if (!value || depth > 8) return fallback;
  const match = /^var\(\s*(--[\w-]+)(?:\s*,\s*([^)]+))?\s*\)$/.exec(value);
  if (!match) return value;
  return resolveCssCustomPropertyValue(
    style.getPropertyValue(match[1]).trim() || match[2]?.trim() || fallback,
    style,
    fallback,
    depth + 1,
  );
}

function cssColorTokenValue(name: string, fallback: string): string {
  const value = cssCustomPropertyValue(name, fallback);
  if (isSafeCssColor(value)) return value;
  return cssColorToHex(value) || fallback;
}

function isSafeCssColor(value: string): boolean {
  return (
    /^#[0-9a-f]{3,8}$/i.test(value) ||
    /^rgba?\(\s*[\d.]+(?:\s*,\s*[\d.]+){2}(?:\s*,\s*[\d.]+)?\s*\)$/i.test(
      value,
    ) ||
    /^hsla?\(\s*[\d.]+(?:deg)?(?:\s*,\s*[\d.]+%){2}(?:\s*,\s*[\d.]+)?\s*\)$/i.test(
      value,
    )
  );
}

function cssPixelTokenNumber(
  name: string,
  fallback: number,
  max: number,
): number {
  const value = cssCustomPropertyValue(name, `${fallback}px`);
  if (value === "0") return 0;
  const parsed = /^(-?\d+(?:\.\d+)?)px$/.exec(value.trim());
  if (!parsed) return fallback;
  return Math.max(0, Math.min(max, Number.parseFloat(parsed[1])));
}

function cssColorToHex(value: string): string {
  if (!value) return "";
  if (/^#[0-9a-f]{3,8}$/i.test(value)) return value;

  const rgb = /^rgba?\(([^)]+)\)$/i.exec(value);
  if (!rgb) return "";
  const parts = rgb[1].split(",").map((part) => part.trim());
  const channels = parts.slice(0, 3).map((part) => Number.parseFloat(part));
  if (channels.some((channel) => !Number.isFinite(channel))) return "";
  return `#${channels
    .map((channel) =>
      Math.max(0, Math.min(255, Math.round(channel)))
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`;
}

function mermaidThemeVariables(): Record<string, string | number> {
  const tokens = mermaidStyleTokens();
  return {
    fontFamily: tokens.fontFamily,
    fontSize: MDP_DIAGRAM_FONT_SIZE,
    strokeWidth: `${MDP_DIAGRAM_STROKE_WIDTH}px`,
    radius: tokens.radius,
    primaryColor: tokens.surface,
    primaryTextColor: tokens.text,
    primaryBorderColor: tokens.border,
    lineColor: tokens.line,
    edgeLabelBackground: tokens.surface,
    background: tokens.surface,
    mainBkg: tokens.surface,
    nodeBorder: tokens.border,
    clusterBkg: tokens.surface,
    clusterBorder: tokens.border,
    noteBkgColor: tokens.accentBg,
    noteBorderColor: tokens.accent,
    noteTextColor: tokens.text,
    actorBkg: tokens.surface,
    actorBorder: tokens.border,
    actorTextColor: tokens.text,
    actorLineColor: tokens.line,
    signalColor: tokens.line,
    signalTextColor: tokens.text,
    labelBoxBkgColor: tokens.accentBg,
    labelBoxBorderColor: tokens.accent,
    labelTextColor: tokens.text,
    loopTextColor: tokens.text,
    activationBorderColor: tokens.accent,
    activationBkgColor: tokens.accentBg,
    sequenceNumberColor: tokens.mutedText,
  };
}

type MermaidStyleTokens = {
  fontFamily: string;
  radius: number;
  surface: string;
  text: string;
  mutedText: string;
  line: string;
  border: string;
  accent: string;
  accentBg: string;
};

function mermaidStyleTokens(): MermaidStyleTokens {
  return {
    fontFamily: cssFontTokenValue("--font-sans", "system-ui, sans-serif"),
    radius: cssPixelTokenNumber("--chat-structure-radius", 2, 10),
    surface: cssColorTokenValue("--surface", "#ffffff"),
    text: cssColorTokenValue("--chat-strong-fg", "#0c0f12"),
    mutedText: cssColorTokenValue("--chat-muted-fg", "#595f66"),
    line: cssColorTokenValue("--chat-muted-fg", "#595f66"),
    border: cssColorTokenValue("--chat-structure-border", "#dee2e6"),
    accent: cssColorTokenValue("--primary-fill", "#228be6"),
    accentBg: cssColorTokenValue("--primary-bg", "rgba(34, 139, 230, 0.08)"),
  };
}

function cssFontTokenValue(name: string, fallback: string): string {
  if (typeof window === "undefined") return fallback;
  return (
    window
      .getComputedStyle(document.documentElement)
      .getPropertyValue(name)
      .trim() || fallback
  );
}

function normalizeMermaidLabelText(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}

function appendForeignObjectText(lines: string[], value: string) {
  if (lines.length === 0) lines.push("");
  lines[lines.length - 1] += value;
}

function foreignObjectLabelLines(foreignObject: Element): string[] {
  const lines = [""];

  const visit = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      appendForeignObjectText(lines, node.textContent || "");
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;

    const element = node as Element;
    if (element.localName.toLowerCase() === "br") {
      lines.push("");
      return;
    }
    for (const child of Array.from(element.childNodes)) {
      visit(child);
    }
  };

  for (const child of Array.from(foreignObject.childNodes)) {
    visit(child);
  }

  return lines.map(normalizeMermaidLabelText).filter(Boolean);
}

function normalizeMermaidSvgForRender(svg: string): string {
  if (
    typeof DOMParser === "undefined" ||
    typeof XMLSerializer === "undefined"
  ) {
    return svg;
  }

  const xmlSafeSvg = svg.replace(/<br\s*>/gi, "<br/>");
  const doc = new DOMParser().parseFromString(xmlSafeSvg, "image/svg+xml");
  if (doc.querySelector("parsererror")) return svg;

  replaceMermaidForeignObjectLabels(doc);
  normalizeMermaidSvgStyle(doc);

  return new XMLSerializer().serializeToString(doc.documentElement);
}

function replaceMermaidForeignObjectLabels(doc: Document) {
  for (const foreignObject of Array.from(
    doc.getElementsByTagName("foreignObject"),
  )) {
    const lines = foreignObjectLabelLines(foreignObject);
    if (lines.length === 0) {
      foreignObject.remove();
      continue;
    }

    const objectX = Number.parseFloat(foreignObject.getAttribute("x") || "0");
    const objectY = Number.parseFloat(foreignObject.getAttribute("y") || "0");
    const width = Number.parseFloat(foreignObject.getAttribute("width") || "0");
    const height = Number.parseFloat(
      foreignObject.getAttribute("height") || "0",
    );
    const x =
      (Number.isFinite(objectX) ? objectX : 0) +
      (Number.isFinite(width) && width > 0 ? width / 2 : 0);
    const centerY =
      (Number.isFinite(objectY) ? objectY : 0) +
      (Number.isFinite(height) && height > 0 ? height / 2 : 0);
    const lineHeight = 16;
    const text = doc.createElementNS("http://www.w3.org/2000/svg", "text");
    text.setAttribute("class", "nodeLabel");
    text.setAttribute("x", String(x));
    text.setAttribute("y", String(centerY));
    text.setAttribute("text-anchor", "middle");
    lines.forEach((line, index) => {
      const tspan = doc.createElementNS("http://www.w3.org/2000/svg", "tspan");
      tspan.setAttribute("x", String(x));
      tspan.setAttribute(
        "y",
        String(centerY + (index - (lines.length - 1) / 2) * lineHeight),
      );
      tspan.setAttribute("dy", "0.35em");
      tspan.textContent = line;
      text.append(tspan);
    });
    foreignObject.replaceWith(text);
  }
}

function normalizeMermaidSvgStyle(doc: Document) {
  const tokens = mermaidStyleTokens();
  const svg = doc.documentElement;
  svg.setAttribute("font-family", tokens.fontFamily);
  svg.setAttribute("data-rua-mdp-theme", "true");

  for (const shape of Array.from(
    doc.querySelectorAll(
      ".node rect, .node polygon, .node circle, .node ellipse, .cluster rect, rect.actor",
    ),
  )) {
    setSvgStroke(shape, tokens.border);
    shape.setAttribute("fill", tokens.surface);
    setRoundedRect(shape, tokens.radius);
  }

  for (const note of Array.from(doc.querySelectorAll("rect.note"))) {
    setSvgStroke(note, tokens.accent);
    note.setAttribute("fill", tokens.accentBg);
    setRoundedRect(note, tokens.radius);
  }

  for (const labelBox of Array.from(doc.querySelectorAll("rect.labelBox"))) {
    setSvgStroke(labelBox, tokens.accent);
    labelBox.setAttribute("fill", tokens.accentBg);
    setRoundedRect(labelBox, Math.max(2, tokens.radius));
  }

  for (const line of Array.from(
    doc.querySelectorAll(
      ".edgePath .path, .flowchart-link, .messageLine0, .messageLine1, .messageLine2, .messageLine3, .actor-line",
    ),
  )) {
    setSvgStroke(line, tokens.line);
    line.setAttribute("fill", "none");
  }

  for (const loopLine of Array.from(doc.querySelectorAll(".loopLine"))) {
    setSvgStroke(loopLine, tokens.accent);
    loopLine.setAttribute("fill", "none");
    loopLine.setAttribute("stroke-dasharray", "4 4");
  }

  for (const dashed of Array.from(
    doc.querySelectorAll(".messageLine1, .messageLine3, .edge-pattern-dashed"),
  )) {
    dashed.setAttribute("stroke-dasharray", "4 4");
  }

  for (const marker of Array.from(
    doc.querySelectorAll(
      ".marker, .marker path, .marker polygon, .arrowheadPath, .arrowMarkerPath",
    ),
  )) {
    marker.setAttribute("fill", tokens.line);
    marker.setAttribute("stroke", tokens.line);
  }

  const style = doc.createElementNS("http://www.w3.org/2000/svg", "style");
  style.setAttribute("data-rua-mdp-style", "true");
  style.textContent = ruaMermaidSvgCss(tokens, svg.getAttribute("id") || "");
  const styleElements = Array.from(svg.children).filter(
    (child) => child.tagName.toLowerCase() === "style",
  );
  const lastStyle = styleElements[styleElements.length - 1];
  if (lastStyle?.nextSibling) {
    svg.insertBefore(style, lastStyle.nextSibling);
  } else {
    svg.insertBefore(style, svg.firstChild);
  }
}

function setSvgStroke(element: Element, color: string) {
  element.setAttribute("stroke", color);
  element.setAttribute("stroke-width", `${MDP_DIAGRAM_STROKE_WIDTH}px`);
}

function setRoundedRect(element: Element, radius: number) {
  if (element.tagName.toLowerCase() !== "rect") return;
  element.setAttribute("rx", String(radius));
  element.setAttribute("ry", String(radius));
}

function cssIdSelector(id: string): string {
  if (!id) return "";
  return `#${id.replace(/([^A-Za-z0-9_-])/g, "\\$1")} `;
}

function ruaMermaidSvgCss(tokens: MermaidStyleTokens, svgId: string): string {
  const scope = cssIdSelector(svgId);
  return `
${scope}* {
  font-family: ${tokens.fontFamily} !important;
}
${scope}text,
${scope}tspan,
${scope}.label,
${scope}.nodeLabel,
${scope}.actor,
${scope}.actor-box,
${scope}.messageText,
${scope}.noteText,
${scope}.labelText,
${scope}.loopText,
${scope}.edgeLabel {
  fill: ${tokens.text} !important;
  color: ${tokens.text} !important;
  font-size: ${MDP_DIAGRAM_FONT_SIZE} !important;
  font-weight: 500 !important;
  letter-spacing: 0 !important;
}
${scope}.edgeLabel,
${scope}.edgeLabel p {
  background-color: ${tokens.surface} !important;
}
${scope}.edgeLabel rect,
${scope}.labelBkg {
  fill: ${tokens.surface} !important;
}
${scope}.flowchart-link,
${scope}.edgePath .path,
${scope}.messageLine0,
${scope}.messageLine1,
${scope}.messageLine2,
${scope}.messageLine3,
${scope}.actor-line {
  stroke: ${tokens.line} !important;
  stroke-width: ${MDP_DIAGRAM_STROKE_WIDTH}px !important;
}
${scope}.loopLine {
  stroke: ${tokens.accent} !important;
  stroke-width: ${MDP_DIAGRAM_STROKE_WIDTH}px !important;
  stroke-dasharray: 4 4 !important;
}
${scope}.node rect,
${scope}.node polygon,
${scope}.node circle,
${scope}.node ellipse,
${scope}.cluster rect,
${scope}rect.actor {
  fill: ${tokens.surface} !important;
  stroke: ${tokens.border} !important;
  stroke-width: ${MDP_DIAGRAM_STROKE_WIDTH}px !important;
}
${scope}rect.note,
${scope}rect.labelBox {
  fill: ${tokens.accentBg} !important;
  stroke: ${tokens.accent} !important;
  stroke-width: ${MDP_DIAGRAM_STROKE_WIDTH}px !important;
}
${scope}.marker,
${scope}.marker path,
${scope}.marker polygon,
${scope}.arrowheadPath,
${scope}.arrowMarkerPath {
  fill: ${tokens.line} !important;
  stroke: ${tokens.line} !important;
}
`;
}

export function sanitizeMermaidSvgForRender(svg: string): string {
  return DOMPurify.sanitize(normalizeMermaidSvgForRender(svg), {
    USE_PROFILES: { svg: true },
  });
}

export function mermaidRenderConfig() {
  return {
    startOnLoad: false,
    securityLevel: "strict" as const,
    theme: "base" as const,
    flowchart: {
      htmlLabels: true,
    },
    sequence: {
      actorFontFamily: cssFontTokenValue(
        "--font-sans",
        "system-ui, sans-serif",
      ),
      actorFontSize: MDP_DIAGRAM_FONT_SIZE,
      messageFontFamily: cssFontTokenValue(
        "--font-sans",
        "system-ui, sans-serif",
      ),
      messageFontSize: MDP_DIAGRAM_FONT_SIZE,
      noteFontFamily: cssFontTokenValue("--font-sans", "system-ui, sans-serif"),
      noteFontSize: MDP_DIAGRAM_FONT_SIZE,
    },
    themeVariables: mermaidThemeVariables(),
  };
}

function MermaidDiagram({
  source,
  fallback,
}: {
  source: string;
  fallback: string;
}) {
  const { isDark, colorScheme, skin } = useTheme();
  const [svg, setSvg] = useState("");
  const [error, setError] = useState("");
  const renderIdPrefix = useMemo(() => {
    mermaidRenderCounter += 1;
    return `rua-mdp-mermaid-${mermaidRenderCounter}`;
  }, []);
  const renderCountRef = useRef(0);
  const previousSourceRef = useRef(source);
  const mounted = useRef(true);
  const themeKey = `${isDark ? "dark" : "light"}:${colorScheme}:${skin}`;

  useEffect(() => {
    mounted.current = true;
    const sourceChanged = previousSourceRef.current !== source;
    previousSourceRef.current = source;
    if (sourceChanged) {
      setSvg("");
    }
    setError("");
    if (import.meta.env.MODE === "test") return;

    const renderId = `${renderIdPrefix}-${renderCountRef.current++}`;
    let cancelled = false;
    const frame = window.requestAnimationFrame(() => {
      void import("mermaid")
        .then(async ({ default: mermaid }) => {
          mermaid.initialize(mermaidRenderConfig());
          try {
            return await mermaid.render(renderId, source);
          } catch (originalError) {
            const repaired = repairMermaidSource(source);
            if (repaired === source) throw originalError;
            return mermaid.render(`${renderId}-repaired`, repaired);
          }
        })
        .then(({ svg }) => {
          if (cancelled || !mounted.current) return;
          setSvg(sanitizeMermaidSvgForRender(svg));
          setError("");
        })
        .catch((err) => {
          if (cancelled || !mounted.current) return;
          setError(
            `Diagram parse failed: ${
              err instanceof Error ? err.message : String(err)
            }`,
          );
        });
    });

    return () => {
      cancelled = true;
      mounted.current = false;
      window.cancelAnimationFrame(frame);
    };
  }, [renderIdPrefix, source, themeKey]);

  if (svg && !error) {
    return (
      <div
        className="rua-mdp-mermaid"
        dangerouslySetInnerHTML={{ __html: svg }}
      />
    );
  }

  if (!error) {
    return <DiagramLoading />;
  }

  return <DiagramFallback error={error} fallback={fallback} />;
}

function DiagramLoading() {
  return (
    <div className="rua-mdp-diagram-loading" aria-label="Rendering diagram" />
  );
}

function DiagramFallback({
  error,
  fallback,
}: {
  error: string;
  fallback: string;
}) {
  return (
    <div className="rua-mdp-diagram-fallback">
      {error && <div className="rua-mdp-diagram-error">{error}</div>}
      <pre className="rua-mdp-diagram-lines">{fallback}</pre>
    </div>
  );
}

function MarkdownPlusChart({
  block,
  renderMarkdown,
  showCaption = true,
}: {
  block: MarkdownPlusBlock;
  renderMarkdown: MarkdownRenderer;
  showCaption?: boolean;
}) {
  const chart = parseCsv(block.body);
  const title = block.params.title;
  const type = block.params.type || "bar";

  if (!chart) {
    return <>{renderMarkdown(block.raw)}</>;
  }

  return (
    <section
      className="rua-mdp-block rua-mdp-chart"
      aria-label={title || `@chart ${type}`}
    >
      {type === "line" || type === "area" ? (
        <LineChart chart={chart} area={type === "area"} />
      ) : type === "pie" ? (
        <PieChart chart={chart} />
      ) : (
        <BarChart chart={chart} />
      )}
      {showCaption && title && <div className="rua-mdp-caption">{title}</div>}
    </section>
  );
}

function numericSeries(chart: ChartData) {
  const seriesNames = chart.headers.slice(1);
  const rows = chart.rows.map((row) => ({
    category: row[0] || "",
    values: row.slice(1).map((value) => Number.parseFloat(value)),
  }));
  const values = rows.flatMap((row) => row.values).filter(Number.isFinite);
  const max = Math.max(...values, 0);
  return { seriesNames, rows, max: max || 1 };
}

function BarChart({ chart }: { chart: ChartData }) {
  const { seriesNames, rows, max } = numericSeries(chart);
  const multiSeries = seriesNames.length > 1;
  return (
    <div className="rua-mdp-bar-chart">
      {rows.map((row, rowIndex) => (
        <div className="rua-mdp-bar-row" key={`${row.category}-${rowIndex}`}>
          <div className="rua-mdp-bar-category">{row.category}</div>
          <div className="rua-mdp-bar-series">
            {row.values.map((value, valueIndex) => {
              const safeValue = Number.isFinite(value) ? value : 0;
              const width = `${Math.max(1, (safeValue / max) * 100)}%`;
              return (
                <div
                  className={[
                    "rua-mdp-bar-item",
                    multiSeries
                      ? "rua-mdp-bar-item--multi"
                      : "rua-mdp-bar-item--single",
                  ].join(" ")}
                  key={seriesNames[valueIndex] || valueIndex}
                >
                  {multiSeries && (
                    <span className="rua-mdp-bar-name">
                      {seriesNames[valueIndex]}
                    </span>
                  )}
                  <span className="rua-mdp-bar-track">
                    <span className="rua-mdp-bar-fill" style={{ width }} />
                  </span>
                  <span className="rua-mdp-bar-value">
                    {row.values[valueIndex]}
                  </span>
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </div>
  );
}

function LineChart({
  chart,
  area = false,
}: {
  chart: ChartData;
  area?: boolean;
}) {
  const { seriesNames, rows, max } = numericSeries(chart);
  const width = 640;
  const height = 220;
  const padX = 34;
  const padY = 24;
  const plotW = width - padX * 2;
  const plotH = height - padY * 2;

  const paths = seriesNames.map((name, seriesIndex) => {
    const points = rows.map((row, index) => {
      const x =
        padX +
        (rows.length <= 1 ? plotW / 2 : (plotW * index) / (rows.length - 1));
      const raw = row.values[seriesIndex];
      const y = padY + plotH - ((Number.isFinite(raw) ? raw : 0) / max) * plotH;
      return [x, y] as const;
    });
    const pointString = points.map(([x, y]) => `${x},${y}`).join(" ");
    const areaPath =
      points.length > 0
        ? `M ${points[0][0]} ${height - padY} L ${points
            .map(([x, y]) => `${x} ${y}`)
            .join(" L ")} L ${points[points.length - 1][0]} ${height - padY} Z`
        : "";
    return { name, points, pointString, areaPath };
  });

  return (
    <div className="rua-mdp-line-chart-wrap">
      <svg
        className="rua-mdp-line-chart"
        viewBox={`0 0 ${width} ${height}`}
        role="img"
      >
        {[0, 1, 2, 3].map((line) => {
          const y = padY + (plotH * line) / 3;
          return (
            <line
              className="rua-mdp-chart-grid"
              x1={padX}
              x2={width - padX}
              y1={y}
              y2={y}
              key={line}
            />
          );
        })}
        {paths.map((path, index) => (
          <g
            key={path.name || index}
            className={`rua-mdp-series-${index % 4}`}
          >
            {area && <path className="rua-mdp-area" d={path.areaPath} />}
            <polyline className="rua-mdp-line" points={path.pointString} />
            {path.points.map(([x, y], pointIndex) => (
              <circle
                className="rua-mdp-point"
                cx={x}
                cy={y}
                r="3.5"
                key={pointIndex}
              />
            ))}
          </g>
        ))}
      </svg>
      <Legend names={seriesNames} />
    </div>
  );
}

function PieChart({ chart }: { chart: ChartData }) {
  const { rows, max } = numericSeries(chart);
  const total =
    rows.reduce(
      (sum, row) => sum + (Number.isFinite(row.values[0]) ? row.values[0] : 0),
      0,
    ) || max;
  let cursor = 0;
  const stops = rows
    .map((row, index) => {
      const value = Number.isFinite(row.values[0]) ? row.values[0] : 0;
      const start = (cursor / total) * 100;
      cursor += value;
      const end = (cursor / total) * 100;
      return `var(--mdp-series-${index % 4}) ${start}% ${end}%`;
    })
    .join(", ");
  return (
    <div className="rua-mdp-pie-wrap">
      <div
        className="rua-mdp-pie"
        style={{ background: `conic-gradient(${stops})` }}
      />
      <Legend names={rows.map((row) => row.category)} />
    </div>
  );
}

function Legend({ names }: { names: string[] }) {
  if (names.length <= 1) return null;
  return (
    <div className="rua-mdp-legend">
      {names.map((name, index) => (
        <span
          className={`rua-mdp-legend-item rua-mdp-series-${index % 4}`}
          key={name}
        >
          {name}
        </span>
      ))}
    </div>
  );
}

export function MarkdownPlusBlockView({
  block,
  collapsibleRenderBlocks = false,
  resolveImageSrc,
  renderMarkdown,
}: {
  block: MarkdownPlusBlock;
  collapsibleRenderBlocks?: boolean;
  resolveImageSrc?: (source: string) => string;
  renderMarkdown: MarkdownRenderer;
}) {
  const view =
    block.type === "figure" ? (
      <MarkdownPlusFigure
        block={block}
        renderMarkdown={renderMarkdown}
        resolveImageSrc={resolveImageSrc}
        showCaption={!collapsibleRenderBlocks}
      />
    ) : block.type === "diagram" ? (
      <MarkdownPlusDiagram
        block={block}
        showCaption={!collapsibleRenderBlocks}
      />
    ) : (
      <MarkdownPlusChart
        block={block}
        renderMarkdown={renderMarkdown}
        showCaption={!collapsibleRenderBlocks}
      />
    );

  if (!collapsibleRenderBlocks) return view;

  const title =
    block.type === "figure" ? block.params.caption : block.params.title;

  return (
    <ChatRenderBlock
      className="rua-render-block--markdown-plus"
      ariaLabel={`@${block.type} block`}
      title={title}
    >
      {view}
    </ChatRenderBlock>
  );
}
