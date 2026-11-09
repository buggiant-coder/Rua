import { expect, test, type Page } from "@playwright/test";
import headlessPackage from "@xterm/headless";
import { SerializeAddon } from "@xterm/addon-serialize";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

const EXPECTED_XTERM_VERSIONS = {
  "@xterm/addon-fit": "0.11.0",
  "@xterm/addon-serialize": "0.14.0",
  "@xterm/addon-unicode11": "0.9.0",
  "@xterm/addon-webgl": "0.19.0",
  "@xterm/headless": "6.0.0",
  "@xterm/xterm": "6.0.0",
} as const;

const { Terminal: HeadlessTerminal } = headlessPackage;

function writeHeadless(
  terminal: InstanceType<typeof HeadlessTerminal>,
  data: string | Uint8Array,
): Promise<void> {
  return new Promise((resolve) => terminal.write(data, resolve));
}

function headlessState(terminal: InstanceType<typeof HeadlessTerminal>) {
  const active = terminal.buffer.active;
  return {
    allLines: Array.from(
      { length: active.length },
      (_, index) => active.getLine(index)?.translateToString(true) ?? "",
    ),
    baseY: active.baseY,
    bufferType: active.type,
    cursorX: active.cursorX,
    cursorY: active.cursorY,
    modes: { ...terminal.modes },
    viewportY: active.viewportY,
  };
}

async function browserState(page: Page, id: string) {
  return page.evaluate((terminalId) => {
    return window.__terminalCanary.state(terminalId);
  }, id);
}

test.beforeEach(async ({ page }) => {
  await page.goto("/tests/terminal-canary/");
  await expect
    .poll(() => page.evaluate(() => Boolean(window.__terminalCanary)))
    .toBe(true);
});

test("pins one exact browser and headless xterm compatibility set", async () => {
  const packageJson = JSON.parse(
    await readFile(new URL("../../package.json", import.meta.url), "utf8"),
  ) as { dependencies: Record<string, string> };
  const packageLock = JSON.parse(
    await readFile(new URL("../../package-lock.json", import.meta.url), "utf8"),
  ) as {
    packages: Record<string, { version?: string }>;
  };

  for (const [name, version] of Object.entries(EXPECTED_XTERM_VERSIONS)) {
    expect(packageJson.dependencies[name]).toBe(version);
    expect(packageLock.packages[`node_modules/${name}`]?.version).toBe(version);
  }

  const installedXtermPackages = Object.entries(packageLock.packages)
    .filter(([path]) => path.includes("node_modules/@xterm/"))
    .map(([path, metadata]) => [path, metadata.version]);
  expect(installedXtermPackages).toHaveLength(
    Object.keys(EXPECTED_XTERM_VERSIONS).length,
  );
});

test("browser viewer suppresses authority-owned replies but forwards real input", async ({
  page,
}) => {
  await page.evaluate(() =>
    window.__terminalCanary.createRuntime(
      "authority-reply-suppression",
      [
        "\x1b[c",
        "\x1b[>c",
        "\x1b[5n",
        "\x1b[6n",
        "\x1b[?6n",
        "\x1b]10;?\x07",
        "\x1b]11;?\x07",
        "\x1b]12;?\x07",
      ].join(""),
    ),
  );

  expect(
    await page.evaluate(() =>
      window.__terminalCanary.runtimeInput("authority-reply-suppression"),
    ),
  ).toEqual([]);

  await page.evaluate(() =>
    window.__terminalCanary.runtimeFocus("authority-reply-suppression"),
  );
  await page.keyboard.press("Escape");
  await page.keyboard.press("Control+c");
  await page.keyboard.type("viewer-input");

  await expect
    .poll(() =>
      page.evaluate(() =>
        window.__terminalCanary.runtimeInput("authority-reply-suppression"),
      ),
    )
    .toEqual(expect.arrayContaining(["\x1b", "\x03", "v"]));
});

test("top-margin CSI S moves leaving rows into normal-buffer scrollback", async ({
  page,
}) => {
  await page.evaluate(async () => {
    await window.__terminalCanary.create({
      id: "top-margin-scroll-up",
      cols: 20,
      rows: 3,
      scrollback: 10,
    });
    await window.__terminalCanary.write(
      "top-margin-scroll-up",
      "one\r\ntwo\r\nthree\x1b[1;3r\x1b[S",
    );
  });

  const state = await browserState(page, "top-margin-scroll-up");
  expect(state.baseY).toBe(1);
  expect(state.allLines[0]).toBe("one");
  expect(state.visibleLines).toEqual(["two", "three", ""]);
});

test("round trips headless state into browser xterm before open", async ({
  page,
}) => {
  const headless = new HeadlessTerminal({
    allowProposedApi: true,
    cols: 24,
    rows: 4,
    scrollback: 4,
  });
  const serializeAddon = new SerializeAddon();
  headless.loadAddon(
    serializeAddon as unknown as Parameters<typeof headless.loadAddon>[0],
  );

  await writeHeadless(
    headless,
    new TextEncoder().encode(
      "\x1b[?2004hline-0\r\nline-1\r\nline-2\r\nline-3\r\nline-4\r\nline-5\r\nline-6\r\nline-7\r\n中文-line-8\r\n\x1b[31mline-9\x1b[0m",
    ),
  );
  await writeHeadless(headless, "\x1b[?1049halt-screen\x1b[4;10H");
  const snapshot = serializeAddon.serialize();
  const sourceState = headlessState(headless);

  await page.evaluate(
    ({ serialized }) =>
      window.__terminalCanary.create({
        id: "round-trip",
        cols: 24,
        rows: 4,
        scrollback: 4,
        snapshot: serialized,
      }),
    { serialized: snapshot },
  );

  const restored = await browserState(page, "round-trip");
  expect(restored.bufferType).toBe("alternate");
  expect(restored.visibleLines).toEqual(
    sourceState.allLines.slice(sourceState.baseY, sourceState.baseY + 4),
  );
  expect(restored.cursorX).toBe(sourceState.cursorX);
  expect(restored.cursorY).toBe(sourceState.cursorY);
  expect(restored.modes.bracketedPasteMode).toBe(true);
  expect(restored.serialized).toContain("\x1b[31m");
  expect(restored.serialized).toContain("\x1b[?1049h");
  expect(["loaded", "fallback"]).toContain(restored.webgl);

  await page.evaluate(() =>
    window.__terminalCanary.write("round-trip", "\x1b[?1049l-live"),
  );
  const continued = await browserState(page, "round-trip");
  expect(continued.bufferType).toBe("normal");
  expect(continued.allLines.join("\n")).not.toContain("line-0");
  expect(continued.allLines.join("\n")).toContain("中文-line-8");
  expect(continued.allLines.join("\n")).toContain("line-9-live");
  expect(continued.foregroundPaletteIndexes).toContain(1);
  expect(continued.baseY).toBeLessThanOrEqual(4);
});

test("parses the complete attach tail before first reveal", async ({
  page,
}) => {
  const headless = new HeadlessTerminal({
    allowProposedApi: true,
    cols: 30,
    rows: 6,
    scrollback: 20,
  });
  const serializeAddon = new SerializeAddon();
  headless.loadAddon(
    serializeAddon as unknown as Parameters<typeof headless.loadAddon>[0],
  );
  await writeHeadless(headless, "\x1b[?2004hsnapshot-screen");
  const snapshot = serializeAddon.serialize();

  await page.evaluate(
    ({ serialized }) =>
      window.__terminalCanary.create({
        id: "attach-tail-before-reveal",
        cols: 30,
        rows: 6,
        scrollback: 20,
        snapshot: serialized,
        tail: ["\r\nordered-tail", "\x1b[31m-red\x1b[0m"],
      }),
    { serialized: snapshot },
  );

  const revealed = await browserState(page, "attach-tail-before-reveal");
  expect(revealed.visibleLines.join("\n")).toContain("snapshot-screen");
  expect(revealed.visibleLines.join("\n")).toContain("ordered-tail-red");
  expect(revealed.foregroundPaletteIndexes).toContain(1);

  await page.evaluate(() =>
    window.__terminalCanary.write(
      "attach-tail-before-reveal",
      "\r\npositioned-live",
    ),
  );
  expect(
    (await browserState(page, "attach-tail-before-reveal")).visibleLines.join(
      "\n",
    ),
  ).toContain("positioned-live");
});

test("keeps the old screen visible until a hydrated replacement is committed", async ({
  page,
}) => {
  await page.evaluate(() =>
    window.__terminalCanary.create({
      id: "atomic-replacement",
      cols: 30,
      rows: 6,
      scrollback: 20,
      snapshot: "old-visible-screen",
    }),
  );
  const oldHost = page.locator('[data-terminal-id="atomic-replacement"]');
  const screenshotHash = async () =>
    createHash("sha256")
      .update(await oldHost.screenshot())
      .digest("hex");
  const before = await screenshotHash();

  const headless = new HeadlessTerminal({
    allowProposedApi: true,
    cols: 30,
    rows: 6,
    scrollback: 20,
  });
  const serializeAddon = new SerializeAddon();
  headless.loadAddon(
    serializeAddon as unknown as Parameters<typeof headless.loadAddon>[0],
  );
  await writeHeadless(
    headless,
    "\x1b[?2004hnormal-buffer\r\n\x1b[?1049hreplacement-screen\x1b[31m-red\x1b[0m",
  );
  const snapshot = serializeAddon.serialize();

  await page.evaluate(
    ({ serialized }) =>
      window.__terminalCanary.prepareReplacement({
        id: "atomic-replacement",
        cols: 30,
        rows: 6,
        scrollback: 20,
        snapshot: serialized,
      }),
    { serialized: snapshot },
  );

  expect(await screenshotHash()).toBe(before);
  expect(
    (await browserState(page, "atomic-replacement")).visibleLines.join("\n"),
  ).toContain("old-visible-screen");

  await page.evaluate(() =>
    window.__terminalCanary.commitReplacement("atomic-replacement"),
  );
  const restored = await browserState(page, "atomic-replacement");
  expect(restored.bufferType).toBe("alternate");
  expect(restored.visibleLines.join("\n")).toContain("replacement-screen-red");
  expect(restored.modes.bracketedPasteMode).toBe(true);
  expect(restored.foregroundPaletteIndexes).toContain(1);
});

test("document resume discards delayed callbacks and continues the retained terminal", async ({
  page,
}, testInfo) => {
  await page.evaluate(() =>
    window.__terminalCanary.createRuntime(
      "document-resume",
      "old-visible-screen",
    ),
  );
  const host = page.locator('[data-terminal-runtime-id="document-resume"]');
  const screenshotHash = async () =>
    createHash("sha256")
      .update(await host.screenshot())
      .digest("hex");
  const before = await screenshotHash();
  await host.locator(".xterm").evaluate((element) => {
    (element as HTMLElement).dataset.runtimeInstance = "old";
  });

  await page.evaluate(() => {
    window.__terminalCanary.runtimeSuspend("document-resume");
    window.__terminalCanary.runtimeEmitDelayed(
      "document-resume",
      "hidden-backlog",
    );
  });
  await page.waitForTimeout(100);
  expect(await screenshotHash()).toBe(before);
  await expect(host.locator('[data-runtime-instance="old"]')).toHaveCount(1);

  await page.evaluate(async () => {
    await window.__terminalCanary.runtimeResume("document-resume");
    window.__terminalCanary.runtimeEmitDelayed(
      "document-resume",
      "\r\nhidden-backlog",
    );
  });
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.__terminalCanary.runtimeCounters("document-resume"),
      ),
    )
    .toMatchObject({
      hydrationCount: 1,
      openCount: 2,
      resyncCount: 0,
    });
  await expect(host.locator('[data-runtime-instance="old"]')).toHaveCount(1);
  await expect(host.locator(".xterm")).toBeVisible();
  if (testInfo.project.name === "chromium") {
    await expect.poll(screenshotHash).not.toBe(before);
  }
});

test("gap, transport, and lag recovery each create one atomic replacement", async ({
  page,
}) => {
  for (const reason of ["gap", "transport", "lag"] as const) {
    const id = `recovery-${reason}`;
    await page.evaluate(
      async ({ runtimeId, recoveryReason }) => {
        await window.__terminalCanary.createRuntime(
          runtimeId,
          "old-screen",
          recoveryReason === "lag" ? 8 : 64,
        );
        const old = document.querySelector(
          `[data-terminal-runtime-id="${runtimeId}"] .xterm`,
        ) as HTMLElement | null;
        if (!old) throw new Error("old runtime xterm is missing");
        old.dataset.runtimeInstance = "old";
        window.__terminalCanary.runtimeSetReplacement(
          runtimeId,
          `\x1b[2J\x1b[Hcurrent-${recoveryReason}`,
        );
        if (recoveryReason === "gap") {
          window.__terminalCanary.runtimeEmitDelayed(runtimeId, "gap", 1, 0);
          window.__terminalCanary.runtimeEmitDelayed(runtimeId, "gap", 1, 0);
        } else if (recoveryReason === "transport") {
          window.__terminalCanary.runtimeFail(runtimeId);
        } else {
          window.__terminalCanary.runtimeEmitDelayed(
            runtimeId,
            "0123456789abcdef",
          );
        }
      },
      { runtimeId: id, recoveryReason: reason },
    );

    await expect
      .poll(() =>
        page.evaluate((runtimeId) => {
          return window.__terminalCanary.runtimeCounters(runtimeId);
        }, id),
      )
      .toMatchObject({
        hydrationCount: 2,
        openCount: 2,
        resyncCount: 1,
      });
    const host = page.locator(`[data-terminal-runtime-id="${id}"]`);
    await expect(host.locator('[data-runtime-instance="old"]')).toHaveCount(0);
    await expect(host.locator(".xterm")).toBeVisible();
  }
});

test("local and remote placements share one browser runtime lifecycle", async ({
  page,
}) => {
  await page.evaluate(async () => {
    for (const placement of ["local", "remote"]) {
      const id = `${placement}-placement-runtime`;
      await window.__terminalCanary.createRuntime(id, `${placement}-snapshot`);
      const old = document.querySelector(
        `[data-terminal-runtime-id="${id}"] .xterm`,
      ) as HTMLElement | null;
      if (!old) throw new Error(`${placement} runtime xterm is missing`);
      old.dataset.runtimeInstance = "old";
      window.__terminalCanary.runtimeSetReplacement(
        id,
        `\x1b[2J\x1b[H${placement}-current`,
      );
      window.__terminalCanary.runtimeFail(id);
    }
  });

  const counters = [];
  for (const placement of ["local", "remote"]) {
    const id = `${placement}-placement-runtime`;
    await expect
      .poll(() =>
        page.evaluate((runtimeId) => {
          return window.__terminalCanary.runtimeCounters(runtimeId);
        }, id),
      )
      .toMatchObject({
        hydrationCount: 2,
        openCount: 2,
        resyncCount: 1,
      });
    const host = page.locator(`[data-terminal-runtime-id="${id}"]`);
    await expect(host.locator('[data-runtime-instance="old"]')).toHaveCount(0);
    await expect(host.locator(".xterm")).toBeVisible();
    counters.push(
      await page.evaluate((runtimeId) => {
        return window.__terminalCanary.runtimeCounters(runtimeId);
      }, id),
    );
  }
  expect(counters[0]).toEqual(counters[1]);
});

test("holds DEC synchronized output until the transaction ends", async ({
  page,
}, testInfo) => {
  await page.evaluate(() =>
    window.__terminalCanary.create({
      id: "synchronized-output",
      cols: 40,
      rows: 10,
      scrollback: 20,
      snapshot: "old-screen",
    }),
  );
  const host = page.locator('[data-terminal-id="synchronized-output"]');
  const screenshotHash = async () =>
    createHash("sha256")
      .update(await host.screenshot())
      .digest("hex");

  const before = await screenshotHash();
  await page.evaluate(() =>
    window.__terminalCanary.write(
      "synchronized-output",
      "\x1b[?2026h\x1b[2J\x1b[Hnew-screen",
    ),
  );
  await page.waitForTimeout(100);
  expect(await screenshotHash()).toBe(before);

  await page.evaluate(() =>
    window.__terminalCanary.write("synchronized-output", "\x1b[?2026l"),
  );
  await expect
    .poll(
      async () =>
        (await browserState(page, "synchronized-output")).visibleLines[0],
    )
    .toBe("new-screen");
  if (testInfo.project.name === "chromium") {
    await expect.poll(screenshotHash).not.toBe(before);
  }
});

test("defers viewport DOM synchronization until synchronized output ends", async ({
  page,
}) => {
  await page.evaluate(async () => {
    await window.__terminalCanary.create({
      id: "synchronized-viewport",
      cols: 40,
      rows: 6,
      scrollback: 80,
    });
    await window.__terminalCanary.write(
      "synchronized-viewport",
      Array.from({ length: 30 }, (_, index) => `line-${index}\r\n`).join(""),
    );
    window.__terminalCanary.scrollLines("synchronized-viewport", -8);
  });

  const before = await browserState(page, "synchronized-viewport");
  const beforeScrollTop = await page.evaluate(() =>
    window.__terminalCanary.viewportScrollTop("synchronized-viewport"),
  );
  expect(before.viewportY).toBeLessThan(before.baseY);
  expect(beforeScrollTop).toBeGreaterThan(0);

  await page.evaluate(() =>
    window.__terminalCanary.write(
      "synchronized-viewport",
      "\x1b[?2026h\x1b[?1049h\x1b[2J\x1b[Halternate-screen",
    ),
  );
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );

  expect(
    await page.evaluate(() =>
      window.__terminalCanary.viewportScrollTop("synchronized-viewport"),
    ),
  ).toBe(beforeScrollTop);

  await page.evaluate(() =>
    window.__terminalCanary.write(
      "synchronized-viewport",
      "\x1b[?1049l\x1b[?2026l",
    ),
  );
  await expect
    .poll(() =>
      page.evaluate(
        () => window.__terminalCanary.state("synchronized-viewport").viewportY,
      ),
    )
    .toBe(before.viewportY);
});

test("renders the Codex prompt background and status region stably", async ({
  page,
}) => {
  await page.evaluate(async () => {
    await window.__terminalCanary.create({
      id: "codex-status",
      cols: 50,
      rows: 6,
      scrollback: 20,
    });
    await window.__terminalCanary.write(
      "codex-status",
      [
        "\x1b[48;2;36;36;36m> explain the runtime\x1b[0m\r\n",
        "\x1b[?2026hworking...\r\nmodel: codex | project: rua\x1b[?2026l",
      ].join(""),
    );
  });

  const state = await browserState(page, "codex-status");
  expect(state.backgroundRgb).toContain(0x242424);
  expect(state.visibleLines.join("\n")).toContain("working...");
  expect(state.visibleLines.join("\n")).toContain(
    "model: codex | project: rua",
  );
});

test("browser xterm follows Rua theme tokens without recreating its runtime", async ({
  page,
}) => {
  await page.evaluate(async () => {
    const root = document.documentElement;
    root.style.setProperty("--surface", "#f8f9fa");
    document.body.style.setProperty("--surface", "#f8f9fa");
    root.style.setProperty("--text", "#101418");
    root.style.setProperty("--primary", "#228be6");
    await window.__terminalCanary.createRuntime("rua-theme", "theme");
    window.__terminalCanary.runtimeSetLockedGeometry(
      "rua-theme",
      true,
      30,
      6,
    );
  });

  expect(
    await page.evaluate(() =>
      window.__terminalCanary.runtimeTheme("rua-theme"),
    ),
  ).toMatchObject({
    background: "#f8f9fa",
    cursor: "#228be6",
    foreground: "#101418",
    selectionBackground: "#228be6",
  });
  const terminal = page.locator('[data-terminal-runtime-id="rua-theme"]');
  await expect(terminal.locator(".xterm-scrollable-element")).toHaveCSS(
    "background-color",
    "rgb(248, 249, 250)",
  );
  await expect(terminal.locator(".xterm-viewport")).toHaveCSS(
    "background-color",
    "rgb(248, 249, 250)",
  );
  const counters = await page.evaluate(() =>
    window.__terminalCanary.runtimeCounters("rua-theme"),
  );

  await page.evaluate(() => {
    const root = document.documentElement;
    root.style.setProperty("--surface", "#242424");
    document.body.style.setProperty("--surface", "#242424");
    root.style.setProperty("--text", "#e9ecef");
    root.style.setProperty("--primary", "#74c0fc");
    root.classList.add("dark-mode");
  });

  await expect
    .poll(() =>
      page.evaluate(() => window.__terminalCanary.runtimeTheme("rua-theme")),
    )
    .toMatchObject({
      background: "#242424",
      cursor: "#74c0fc",
      foreground: "#e9ecef",
      selectionBackground: "#74c0fc",
    });
  await expect(terminal.locator(".xterm-scrollable-element")).toHaveCSS(
    "background-color",
    "rgb(36, 36, 36)",
  );
  await expect(terminal.locator(".xterm-viewport")).toHaveCSS(
    "background-color",
    "rgb(36, 36, 36)",
  );
  expect(
    await page.evaluate(() =>
      window.__terminalCanary.runtimeCounters("rua-theme"),
    ),
  ).toEqual(counters);
});

test("WebGL context loss keeps the same viewer runtime and screen", async ({
  page,
}) => {
  await page.evaluate(async () => {
    window.localStorage.setItem("rua-terminal-diagnostics", "1");
    delete window.__ruaTerminalDiagnostics;
    await window.__terminalCanary.createRuntime(
      "webgl-context-loss",
      "stable-screen",
    );
  });
  const host = page.locator('[data-terminal-runtime-id="webgl-context-loss"]');
  await host.locator(".xterm").evaluate((element) => {
    (element as HTMLElement).dataset.runtimeInstance = "original";
  });
  const before = await page.evaluate(() =>
    window.__terminalCanary.runtimeCounters("webgl-context-loss"),
  );
  const lost = await host.locator("canvas").evaluateAll((canvases) => {
    for (const canvas of canvases) {
      const context = canvas.getContext("webgl2") ?? canvas.getContext("webgl");
      const extension = context?.getExtension("WEBGL_lose_context");
      if (extension) {
        extension.loseContext();
        return true;
      }
    }
    return false;
  });
  test.skip(!lost, "WebGL is unavailable in this browser project");

  await page.waitForTimeout(100);
  await expect(host.locator('[data-runtime-instance="original"]')).toHaveCount(
    1,
  );
  await expect(host.locator(".xterm")).toBeVisible();
  expect(
    await page.evaluate(() =>
      window.__terminalCanary.runtimeCounters("webgl-context-loss"),
    ),
  ).toEqual(before);
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window.__ruaTerminalDiagnostics ?? [])
          .filter((event) => event.event === "renderer_change")
          .map((event) => event.renderer),
      ),
    )
    .toEqual(expect.arrayContaining(["webgl", "dom"]));
});

test("keeps ED2 viewport intent and leaves ED3 at the new bottom", async ({
  page,
}) => {
  await page.evaluate(async () => {
    await window.__terminalCanary.create({
      id: "erase-display",
      cols: 20,
      rows: 4,
      scrollback: 20,
    });
    await window.__terminalCanary.write(
      "erase-display",
      "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix",
    );
    window.__terminalCanary.scrollLines("erase-display", -2);
  });

  const before = await browserState(page, "erase-display");
  expect(before.viewportY).toBeLessThan(before.baseY);

  await page.evaluate(() =>
    window.__terminalCanary.write("erase-display", "\x1b[2J"),
  );
  const afterEd2 = await browserState(page, "erase-display");
  expect(afterEd2.viewportY).toBe(before.viewportY);
  expect(afterEd2.baseY).toBe(before.baseY);

  await page.evaluate(() =>
    window.__terminalCanary.write("erase-display", "\x1b[3Jafter-ed3"),
  );
  const afterEd3 = await browserState(page, "erase-display");
  expect(afterEd3.baseY).toBe(0);
  expect(afterEd3.viewportY).toBe(afterEd3.baseY);
  expect(afterEd3.visibleLines.join("\n")).toContain("after-ed3");
});

test("parses split control sequences and preserves sustained write order", async ({
  page,
}) => {
  await page.evaluate(async () => {
    await window.__terminalCanary.create({
      id: "contiguous",
      cols: 30,
      rows: 6,
      scrollback: 20,
    });
    await window.__terminalCanary.create({
      id: "split",
      cols: 30,
      rows: 6,
      scrollback: 20,
    });
  });

  const payload = "\x1b[31mred\x1b[0m\r\n\x1b[?1049halt\x1b[?1049l-normal";
  await page.evaluate(
    ({ complete, chunks }) =>
      Promise.all([
        window.__terminalCanary.write("contiguous", complete),
        window.__terminalCanary.writeChunks("split", chunks),
      ]),
    { complete: payload, chunks: Array.from(payload) },
  );

  const contiguous = await browserState(page, "contiguous");
  const split = await browserState(page, "split");
  expect(split.serialized).toBe(contiguous.serialized);
  expect(split.visibleLines).toEqual(contiguous.visibleLines);

  const chunks = Array.from({ length: 256 }, (_, index) => `${index};`);
  const callbacks = await page.evaluate(
    (values) => window.__terminalCanary.writeBurst("split", values),
    chunks,
  );
  expect(callbacks).toEqual(chunks.map((_, index) => index));
  expect((await browserState(page, "split")).serialized).toContain("255;");
});

test("preserves CJK composition, control keys, and selection", async ({
  page,
}) => {
  await page.evaluate(async () => {
    await window.__terminalCanary.create({
      id: "input",
      cols: 30,
      rows: 6,
      scrollback: 20,
    });
    await window.__terminalCanary.write("input", "select-me");
    window.__terminalCanary.focus("input");
  });

  const textarea = page.locator(
    '[data-terminal-id="input"] .xterm-helper-textarea',
  );
  await expect(textarea).toBeFocused();
  await textarea.evaluate((element) => {
    const input = element as HTMLTextAreaElement;
    input.dispatchEvent(
      new CompositionEvent("compositionstart", {
        bubbles: true,
        data: "",
      }),
    );
    input.value = "中文";
    input.dispatchEvent(
      new CompositionEvent("compositionupdate", {
        bubbles: true,
        data: "中文",
      }),
    );
    input.dispatchEvent(
      new InputEvent("input", {
        bubbles: true,
        data: "中文",
        inputType: "insertCompositionText",
        isComposing: true,
      }),
    );
    input.dispatchEvent(
      new CompositionEvent("compositionend", {
        bubbles: true,
        data: "中文",
      }),
    );
  });
  await expect
    .poll(async () => (await browserState(page, "input")).input.join(""))
    .toContain("中文");

  await page.keyboard.press("Escape");
  await page.keyboard.press("Control+c");
  await expect
    .poll(async () => (await browserState(page, "input")).input)
    .toEqual(expect.arrayContaining(["\x1b", "\x03"]));

  await page.evaluate(() =>
    window.__terminalCanary.select("input", 0, 0, "select".length),
  );
  expect((await browserState(page, "input")).selection).toBe("select");
});

test("desktop wheel uses xterm scrollback without an outer vertical scroller", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.createRuntime(
      "desktop-wheel",
      Array.from({ length: 120 }, (_, index) => `line-${index}\r\n`).join(""),
    );
  });

  const host = page.locator(
    '[data-terminal-runtime-id="desktop-wheel"] .rua-terminal-xterm-host',
  );
  await expect(host).toHaveCSS("overflow-y", "hidden");
  const before = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("desktop-wheel"),
  );
  await host.locator(".xterm-screen").hover();
  await page.mouse.wheel(0, -160);
  const immediatelyAfterWheel = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("desktop-wheel"),
  );
  await page.waitForTimeout(40);
  const afterSmoothScroll = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("desktop-wheel"),
  );
  expect(afterSmoothScroll.viewportY).toBeLessThan(
    immediatelyAfterWheel.viewportY,
  );
  expect(afterSmoothScroll.viewportY).toBeLessThan(before.viewportY);
});

test("mobile long press selects without focusing terminal input", async ({
  page,
  context,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile-chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.createRuntime(
      "mobile-long-press",
      "select-me",
    );
  });

  const screen = page.locator(
    '[data-terminal-runtime-id="mobile-long-press"] .xterm-screen',
  );
  const bounds = await screen.boundingBox();
  expect(bounds).not.toBeNull();
  const point = {
    x: bounds!.x + Math.min(24, bounds!.width / 2),
    y: bounds!.y + Math.min(12, bounds!.height / 2),
  };
  const client = await context.newCDPSession(page);
  await client.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [point],
  });
  await page.waitForTimeout(450);

  await expect
    .poll(() =>
      page.evaluate(() =>
        window.__terminalCanary.runtimeSelection("mobile-long-press"),
      ),
    )
    .not.toBe("");
  const textarea = page.locator(
    '[data-terminal-runtime-id="mobile-long-press"] .xterm-helper-textarea',
  );
  expect(
    await textarea.evaluate((element) => document.activeElement === element),
  ).toBe(false);

  await client.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });
  await page.waitForTimeout(50);
  expect(
    await textarea.evaluate((element) => document.activeElement === element),
  ).toBe(false);

  await client.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [{ x: point.x + 12, y: point.y }],
  });
  await client.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });
  await expect(textarea).toBeFocused();
});

test("mobile passive viewer hands vertical scrolling to the outer viewport only at inner boundaries", async ({
  page,
  context,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile-chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.createRuntime(
      "mobile-nested-scroll",
      Array.from({ length: 120 }, (_, index) => `line-${index}\r\n`).join(""),
      1024 * 1024,
      200,
    );
    window.__terminalCanary.runtimeSetHostSize(
      "mobile-nested-scroll",
      280,
      120,
    );
    window.__terminalCanary.runtimeSetLockedGeometry(
      "mobile-nested-scroll",
      true,
      120,
      40,
    );
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
  });

  const host = page.locator(
    '[data-terminal-runtime-id="mobile-nested-scroll"] .rua-terminal-xterm-host',
  );
  const bounds = await host.boundingBox();
  expect(bounds).not.toBeNull();
  const initialMetrics = await page.evaluate(() =>
    window.__terminalCanary.runtimeScrollMetrics("mobile-nested-scroll"),
  );
  expect(initialMetrics.overflowX).toBe("auto");
  expect(initialMetrics.overflowY).toBe("auto");
  expect(initialMetrics.scrollWidth).toBeGreaterThan(
    initialMetrics.clientWidth,
  );
  expect(initialMetrics.scrollHeight).toBeGreaterThan(
    initialMetrics.clientHeight,
  );
  expect(initialMetrics.scrollTop).toBe(
    initialMetrics.scrollHeight - initialMetrics.clientHeight,
  );

  const client = await context.newCDPSession(page);
  const center = {
    x: bounds!.x + bounds!.width / 2,
    y: bounds!.y + bounds!.height / 2,
  };
  const drag = async (deltaX: number, deltaY: number) => {
    await client.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: [center],
    });
    await client.send("Input.dispatchTouchEvent", {
      type: "touchMove",
      touchPoints: [{ x: center.x + deltaX, y: center.y + deltaY }],
    });
    await client.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
  };

  const innerBefore = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("mobile-nested-scroll"),
  );
  const outerBefore = await page.evaluate(
    () =>
      window.__terminalCanary.runtimeScrollMetrics("mobile-nested-scroll")
        .scrollTop,
  );
  await drag(0, 24);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__terminalCanary.runtimeViewport("mobile-nested-scroll")
            .viewportY,
      ),
    )
    .toBeLessThan(innerBefore.viewportY);
  expect(
    await page.evaluate(
      () =>
        window.__terminalCanary.runtimeScrollMetrics("mobile-nested-scroll")
          .scrollTop,
    ),
  ).toBe(outerBefore);

  await page.evaluate(() => {
    window.__terminalCanary.runtimeSetInnerScroll(
      "mobile-nested-scroll",
      "top",
    );
    window.__terminalCanary.runtimeSetOuterScroll(
      "mobile-nested-scroll",
      "bottom",
    );
  });
  const outerBottom = await page.evaluate(
    () =>
      window.__terminalCanary.runtimeScrollMetrics("mobile-nested-scroll")
        .scrollTop,
  );
  await drag(0, 60);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__terminalCanary.runtimeScrollMetrics("mobile-nested-scroll")
            .scrollTop,
      ),
    )
    .toBeLessThan(outerBottom);
  await page.waitForTimeout(350);

  await page.evaluate(() => {
    window.__terminalCanary.runtimeSetInnerScroll(
      "mobile-nested-scroll",
      "bottom",
    );
    window.__terminalCanary.runtimeSetOuterScroll(
      "mobile-nested-scroll",
      "top",
    );
  });
  await drag(0, -60);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__terminalCanary.runtimeScrollMetrics("mobile-nested-scroll")
            .scrollTop,
      ),
    )
    .toBeGreaterThan(0);

  const horizontalBefore = await page.evaluate(
    () =>
      window.__terminalCanary.runtimeScrollMetrics("mobile-nested-scroll")
        .scrollLeft,
  );
  await drag(-80, 0);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__terminalCanary.runtimeScrollMetrics("mobile-nested-scroll")
            .scrollLeft,
      ),
    )
    .toBeGreaterThan(horizontalBefore);
});

test("mobile passive viewer keeps a sparse shell prompt visible", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile-chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.createRuntime(
      "mobile-sparse-prompt",
      "developer@host ~ % \r\ndeveloper@host ~ % ",
      1024 * 1024,
      200,
    );
    window.__terminalCanary.runtimeSetHostSize(
      "mobile-sparse-prompt",
      280,
      120,
    );
    window.__terminalCanary.runtimeSetLockedGeometry(
      "mobile-sparse-prompt",
      true,
      120,
      40,
    );
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
  });

  const metrics = await page.evaluate(() =>
    window.__terminalCanary.runtimeScrollMetrics("mobile-sparse-prompt"),
  );
  expect(metrics.scrollHeight).toBeGreaterThan(metrics.clientHeight);
  expect(metrics.scrollTop).toBe(0);
});

test("mobile terminal headers stay viewport-wide while only xterm scrolls horizontally", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile-chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.createRuntime(
      "mobile-width-containment",
      "wide terminal",
    );
    window.__terminalCanary.runtimeWrapInTerminalPage(
      "mobile-width-containment",
      320,
      240,
    );
    window.__terminalCanary.runtimeSetLockedGeometry(
      "mobile-width-containment",
      true,
      120,
      40,
    );
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
  });

  const metrics = await page.evaluate(() =>
    window.__terminalCanary.runtimePageMetrics("mobile-width-containment"),
  );
  expect(metrics.hostScrollWidth).toBeGreaterThan(metrics.hostClientWidth);
  expect(metrics.pageWidth).toBe(metrics.shellClientWidth);
  expect(metrics.shellScrollWidth).toBe(metrics.shellClientWidth);
  expect(metrics.pageScrollWidth).toBe(metrics.pageClientWidth);
  expect(metrics.pageHeaderWidth).toBe(metrics.pageClientWidth);
  expect(metrics.sessionHeaderWidth).toBe(metrics.pageClientWidth);
  for (const containment of [
    metrics.pageContainment,
    metrics.pageHeaderContainment,
    metrics.bodyContainment,
    metrics.viewportContainment,
    metrics.sessionHeaderContainment,
    metrics.xtermContainment,
    metrics.hostContainment,
  ]) {
    expect(containment.minWidth).toBe("0px");
    expect(containment.maxWidth).toBe("100%");
  }
});

test("first mobile history gesture moves incrementally instead of jumping to top", async ({
  page,
  context,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile-chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.createRuntime(
      "mobile-first-history-scroll",
      Array.from({ length: 120 }, (_, index) => `line-${index}\r\n`).join(""),
      1024 * 1024,
      200,
    );
    window.__terminalCanary.runtimeSetLockedGeometry(
      "mobile-first-history-scroll",
      true,
      30,
      40,
    );
  });

  const host = page.locator(
    '[data-terminal-runtime-id="mobile-first-history-scroll"] .rua-terminal-xterm-host',
  );
  const bounds = await host.boundingBox();
  expect(bounds).not.toBeNull();
  const before = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("mobile-first-history-scroll"),
  );
  expect(before.viewportY).toBeGreaterThan(10);

  const start = {
    x: bounds!.x + bounds!.width / 2,
    y: bounds!.y + bounds!.height / 2,
  };
  const client = await context.newCDPSession(page);
  await client.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [start],
  });
  await client.send("Input.dispatchTouchEvent", {
    type: "touchMove",
    touchPoints: [{ x: start.x, y: start.y + 24 }],
  });
  await client.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });

  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__terminalCanary.runtimeViewport("mobile-first-history-scroll")
            .viewportY,
      ),
    )
    .toBeLessThan(before.viewportY);
  const after = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("mobile-first-history-scroll"),
  );
  expect(after.viewportY).toBeLessThan(before.viewportY);
  expect(after.viewportY).toBeGreaterThan(0);
  expect(before.viewportY - after.viewportY).toBeLessThanOrEqual(3);
});

test("mobile terminal scrolling keeps moving briefly after a fast swipe", async ({
  page,
  context,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile-chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.createRuntime(
      "mobile-scroll-inertia",
      Array.from({ length: 200 }, (_, index) => `line-${index}\r\n`).join(""),
      1024 * 1024,
      300,
    );
  });

  const host = page.locator(
    '[data-terminal-runtime-id="mobile-scroll-inertia"] .rua-terminal-xterm-host',
  );
  const bounds = await host.boundingBox();
  expect(bounds).not.toBeNull();
  const start = {
    x: bounds!.x + bounds!.width / 2,
    y: bounds!.y + bounds!.height / 2,
  };
  const client = await context.newCDPSession(page);
  await client.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [start],
  });
  await page.waitForTimeout(16);
  await client.send("Input.dispatchTouchEvent", {
    type: "touchMove",
    touchPoints: [{ x: start.x, y: start.y + 64 }],
  });
  await client.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });

  const atRelease = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("mobile-scroll-inertia"),
  );
  expect(atRelease.viewportY).toBeLessThan(atRelease.baseY);
  await page.waitForTimeout(180);
  const afterInertia = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("mobile-scroll-inertia"),
  );
  expect(afterInertia.viewportY).toBeLessThan(atRelease.viewportY);
});

test("mobile input takeover preserves bottom and history viewport intent", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile-chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.createRuntime(
      "mobile-takeover-scroll",
      Array.from(
        { length: 120 },
        (_, index) => `line-${index}-${"x".repeat(180)}\r\n`,
      ).join(""),
      1024 * 1024,
      200,
    );
    window.__terminalCanary.runtimeSetHostSize(
      "mobile-takeover-scroll",
      280,
      120,
    );
    window.__terminalCanary.runtimeSetLockedGeometry(
      "mobile-takeover-scroll",
      true,
      120,
      40,
    );
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
    window.__terminalCanary.runtimeSetInnerScroll(
      "mobile-takeover-scroll",
      "bottom",
    );
    window.__terminalCanary.runtimeSetOuterScroll(
      "mobile-takeover-scroll",
      "bottom",
    );
  });

  const before = await page.evaluate(() => ({
    outer: window.__terminalCanary.runtimeScrollMetrics(
      "mobile-takeover-scroll",
    ),
    viewport: window.__terminalCanary.runtimeViewport("mobile-takeover-scroll"),
  }));
  expect(before.viewport.viewportY).toBe(before.viewport.baseY);
  expect(before.outer.scrollTop).toBe(
    before.outer.scrollHeight - before.outer.clientHeight,
  );

  await page.evaluate(() =>
    window.__terminalCanary.runtimeSetGeometryLocked(
      "mobile-takeover-scroll",
      false,
    ),
  );
  await page.waitForTimeout(180);
  await page.evaluate(() =>
    window.__terminalCanary.runtimeFit("mobile-takeover-scroll"),
  );
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );

  const afterBottomTakeover = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("mobile-takeover-scroll"),
  );
  expect(afterBottomTakeover.baseY).toBeGreaterThan(0);
  expect(afterBottomTakeover.viewportY).toBe(afterBottomTakeover.baseY);

  await page.evaluate(async () => {
    window.__terminalCanary.runtimeSetLockedGeometry(
      "mobile-takeover-scroll",
      true,
      120,
      40,
    );
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
    window.__terminalCanary.runtimeSetInnerScroll(
      "mobile-takeover-scroll",
      "bottom",
    );
    window.__terminalCanary.runtimeScrollInnerLines(
      "mobile-takeover-scroll",
      -8,
    );
    window.__terminalCanary.runtimeSetOuterScroll(
      "mobile-takeover-scroll",
      "bottom",
    );
  });

  const beforeHistoryTakeover = await page.evaluate(() =>
    window.__terminalCanary.runtimeViewport("mobile-takeover-scroll"),
  );
  expect(beforeHistoryTakeover.baseY - beforeHistoryTakeover.viewportY).toBe(8);

  await page.evaluate(() =>
    window.__terminalCanary.runtimeSetGeometryLocked(
      "mobile-takeover-scroll",
      false,
    ),
  );
  await page.waitForTimeout(180);
  await page.evaluate(() =>
    window.__terminalCanary.runtimeFit("mobile-takeover-scroll"),
  );
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );

  await expect
    .poll(() =>
      page.evaluate(() => {
        const viewport = window.__terminalCanary.runtimeViewport(
          "mobile-takeover-scroll",
        );
        return viewport.baseY - viewport.viewportY;
      }),
    )
    .toBe(8);
});

test("mobile activation is explicit and background output preserves scrollback", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile-chromium");
  await page.evaluate(async () => {
    await window.__terminalCanary.create({
      id: "mobile-intent",
      cols: 24,
      rows: 4,
      scrollback: 40,
    });
    await window.__terminalCanary.write(
      "mobile-intent",
      Array.from({ length: 20 }, (_, index) => `line-${index}\r\n`).join(""),
    );
    window.__terminalCanary.scrollLines("mobile-intent", -3);
  });

  const textarea = page.locator(
    '[data-terminal-id="mobile-intent"] .xterm-helper-textarea',
  );
  expect(
    await textarea.evaluate((element) => document.activeElement === element),
  ).toBe(false);
  const before = await browserState(page, "mobile-intent");
  await page.evaluate(() =>
    window.__terminalCanary.write("mobile-intent", "background-output"),
  );
  const afterOutput = await browserState(page, "mobile-intent");
  expect(afterOutput.viewportY).toBe(before.viewportY);

  await page.setViewportSize({ width: 412, height: 420 });
  await page.setViewportSize({ width: 412, height: 732 });
  const afterViewportAnimation = await browserState(page, "mobile-intent");
  expect(afterViewportAnimation.viewportY).toBe(before.viewportY);

  await page.evaluate(() => window.__terminalCanary.focus("mobile-intent"));
  await expect(textarea).toBeFocused();
});
