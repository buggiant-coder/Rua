import { expect, test } from "@playwright/test";

test("bundled monospace keeps ASCII-art glyphs on one character grid", async ({
  page,
}) => {
  await page.goto("/");

  const metrics = await page.evaluate(async () => {
    const probe = document.createElement("code");
    probe.style.cssText = [
      "position:fixed",
      "left:-10000px",
      "top:0",
      "white-space:pre",
      "font-family:var(--font-mono)",
      "font-size:20px",
      "font-weight:400",
    ].join(";");
    document.body.appendChild(probe);

    const fontFamily = getComputedStyle(probe).fontFamily;
    const loadedFaces = await document.fonts.load(
      '20px "Rua Mono"',
      "0 ─┌│×►",
    );
    const widths = ["0", " ", "─", "┌", "│", "×", "►"].map((glyph) => {
      probe.textContent = glyph;
      return probe.getBoundingClientRect().width;
    });
    probe.remove();

    return { fontFamily, loadedFaceCount: loadedFaces.length, widths };
  });

  expect(metrics.fontFamily).toContain('"Rua Mono"');
  expect(metrics.loadedFaceCount).toBeGreaterThan(0);
  expect(
    Math.max(...metrics.widths) - Math.min(...metrics.widths),
  ).toBeLessThan(0.1);
});
