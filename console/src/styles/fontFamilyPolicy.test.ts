import { readdirSync, readFileSync } from "node:fs";
import { extname, join } from "node:path";

import { describe, expect, it } from "vitest";

const SOURCE_ROOT = join(process.cwd(), "src");
const SOURCE_EXTENSIONS = new Set([".css", ".less", ".ts", ".tsx"]);
const EXCLUDED_FILES = new Set([
  "desktop/bootstrap.css",
  "styles/fontFamilyPolicy.test.ts",
  "styles/theme.css",
  "terminal/runtime/xtermSurface.ts",
]);
const ALLOWED_CSS_VALUES = new Set([
  "inherit",
  "var(--font-mono)",
  "var(--font-sans)",
]);
const ALLOWED_JS_VALUES = new Set(["var(--font-mono)", "var(--font-sans)"]);

describe("Console font-family policy", () => {
  it("applies the sans token at the document root and native controls", () => {
    const layout = readFileSync(join(SOURCE_ROOT, "styles/layout.css"), "utf8");

    expect(layout).toMatch(
      /html,\s*body,\s*#root\s*\{[^}]*font-family:\s*var\(--font-sans\)/s,
    );
    expect(layout).toMatch(
      /button,\s*input,\s*textarea,\s*select\s*\{[^}]*font:\s*inherit/s,
    );
  });

  it("uses shared tokens for every page-level font declaration", () => {
    const violations: string[] = [];

    for (const path of sourceFiles(SOURCE_ROOT)) {
      const relativePath = path.slice(SOURCE_ROOT.length + 1);
      if (EXCLUDED_FILES.has(relativePath)) continue;
      const source = readFileSync(path, "utf8");
      for (const match of source.matchAll(/font-family\s*:\s*([^;}]+)[;}]/gi)) {
        const value = (match[1]?.trim() ?? "").replace(/\s*!important$/, "");
        if (value.startsWith("${tokens.fontFamily")) continue;
        if (!ALLOWED_CSS_VALUES.has(value)) {
          const line = source.slice(0, match.index).split("\n").length;
          violations.push(`${relativePath}:${line}: font-family: ${value}`);
        }
      }
      for (const match of source.matchAll(
        /fontFamily\s*(?:=|:)\s*["']([^"']+)["']/g,
      )) {
        const value = match[1]?.trim() ?? "";
        if (!ALLOWED_JS_VALUES.has(value)) {
          const line = source.slice(0, match.index).split("\n").length;
          violations.push(`${relativePath}:${line}: fontFamily: ${value}`);
        }
      }
    }

    expect(violations).toEqual([]);
  });
});

function sourceFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return SOURCE_EXTENSIONS.has(extname(entry.name)) ? [path] : [];
  });
}
