import assert from "node:assert/strict";
import { execFile as execFileCallback } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { promisify } from "node:util";
import { WebSocket } from "undici";

const execFile = promisify(execFileCallback);
const cdpUrl = requiredEnv("RUA_ANDROID_E2E_CDP_URL").replace(/\/$/, "");
const baseUrl = requiredEnv("RUA_ANDROID_E2E_WEB_BASE_URL").replace(
  /\/$/,
  "",
);
let remoteDeviceId = process.env.RUA_ANDROID_E2E_REMOTE_DEVICE_ID;
let hostApiToken;
const artifactDir = requiredEnv("RUA_ANDROID_E2E_ARTIFACT_DIR");
const secretProbe = requiredEnv("RUA_ANDROID_E2E_SECRET_PROBE");
const hostApiUrl = requiredEnv("RUA_ANDROID_E2E_HOST_API_URL").replace(
  /\/$/,
  "",
);
const remoteApiUrl = requiredEnv("RUA_ANDROID_E2E_REMOTE_API_URL").replace(
  /\/$/,
  "",
);
const adbSerial = requiredEnv("ADB_SERIAL");
const androidActivity =
  process.env.RUA_ANDROID_ACTIVITY ?? "com.rua.mobile/.MainActivity";
const timeoutMs = Number(process.env.RUA_ANDROID_E2E_UI_TIMEOUT ?? 120_000);

const diagnosticsStorageKey = "rua-terminal-diagnostics";
const activeTerminalSelector =
  ".rua-terminal-viewport-slot.is-active .rua-terminal-xterm";

function requiredEnv(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function adb(...args) {
  await execFile("adb", ["-s", adbSerial, ...args], {
    timeout: timeoutMs,
  });
}

async function requestJson(base, method, requestPath, body) {
  const response = await fetch(`${base}${requestPath}`, {
    method,
    headers: {
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      ...(base === hostApiUrl && hostApiToken ? { Authorization: `Bearer ${hostApiToken}` } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  assert(
    response.ok,
    `${method} ${requestPath} failed with ${response.status}: ${text.slice(
      0,
      200,
    )}`,
  );
  return text ? JSON.parse(text) : null;
}

async function apiJson(method, requestPath, body) {
  return requestJson(hostApiUrl, method, requestPath, body);
}

async function waitForJson(
  base,
  requestPath,
  predicate,
  message,
  timeout = timeoutMs,
) {
  const deadline = Date.now() + timeout;
  let latest;
  let lastError;
  while (Date.now() < deadline) {
    try {
      latest = await requestJson(base, "GET", requestPath);
      if (predicate(latest)) return latest;
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  throw new Error(
    `${message}; latest=${JSON.stringify(latest)}; lastError=${
      lastError instanceof Error ? lastError.message : String(lastError ?? "")
    }`,
  );
}

async function waitForApi(
  requestPath,
  predicate,
  message,
  timeout = timeoutMs,
) {
  return waitForJson(hostApiUrl, requestPath, predicate, message, timeout);
}

function sessionShortId(sessionId) {
  return sessionId.replace(/^terminal-session-/, "").slice(0, 8);
}

function executionDevice(placement, deviceId) {
  return placement === "local"
    ? { kind: "local" }
    : { kind: "remote", device_id: deviceId };
}

function latest(items, predicate) {
  return items.filter(predicate).at(-1);
}

function safeDiagnostics(items) {
  const scalarKeys = [
    "at",
    "event",
    "session_key",
    "session_id",
    "viewer_id",
    "placement",
    "activity_sequence",
    "start_offset",
    "end_offset",
    "frame_classification",
    "recovery_reason",
    "queue_depth",
    "focused",
    "composing",
    "renderer",
    "lifecycle",
    "state_before",
    "state_after",
    "surface_action",
    "visibility_scope",
    "visible",
    "queued_bytes",
    "in_flight_bytes",
    "acknowledged_bytes",
    "batch_bytes",
    "high_watermark_bytes",
    "write_callback_completed",
  ];
  return items.map((item) => {
    const safe = {};
    for (const key of scalarKeys) {
      if (item[key] !== undefined) safe[key] = item[key];
    }
    if (item.cursor_before) safe.cursor_before = { ...item.cursor_before };
    if (item.cursor_after) safe.cursor_after = { ...item.cursor_after };
    if (item.viewport) safe.viewport = { ...item.viewport };
    return safe;
  });
}

function safeWriterMetrics(items) {
  return items
    .filter((item) => item.event === "writer_complete")
    .map((item) => ({
      at: item.at,
      session_key: item.session_key,
      placement: item.placement,
      queued_bytes: item.queued_bytes,
      in_flight_bytes: item.in_flight_bytes,
      acknowledged_bytes: item.acknowledged_bytes,
      batch_bytes: item.batch_bytes,
      write_callback_completed: item.write_callback_completed,
      viewport: item.viewport ? { ...item.viewport } : undefined,
    }));
}

class CdpClient {
  constructor(webSocketUrl) {
    this.webSocketUrl = webSocketUrl;
    this.nextId = 1;
    this.pending = new Map();
    this.socket = null;
  }

  async connect() {
    this.socket = new WebSocket(this.webSocketUrl);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("CDP WebSocket connection timed out")),
        timeoutMs,
      );
      this.socket.addEventListener(
        "open",
        () => {
          clearTimeout(timer);
          resolve();
        },
        { once: true },
      );
      this.socket.addEventListener(
        "error",
        () => {
          clearTimeout(timer);
          reject(new Error("CDP WebSocket connection failed"));
        },
        { once: true },
      );
    });
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      if (message.error) {
        pending.reject(
          new Error(
            `${pending.method} failed: ${
              message.error.message ?? "unknown error"
            }`,
          ),
        );
      } else {
        pending.resolve(message.result ?? {});
      }
    });
    this.socket.addEventListener("close", () => {
      for (const pending of this.pending.values()) {
        clearTimeout(pending.timer);
        pending.reject(new Error(`${pending.method} failed: CDP disconnected`));
      }
      this.pending.clear();
    });
  }

  send(method, params = {}) {
    assert(this.socket, "CDP socket is not connected");
    const id = this.nextId;
    this.nextId += 1;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer, method });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.socket?.close();
  }
}

async function connectPageTarget() {
  const response = await fetch(`${cdpUrl}/json/list`);
  assert(response.ok, `CDP target list failed: ${response.status}`);
  const targets = await response.json();
  const target =
    targets.find(
      (candidate) =>
        candidate.type === "page" && candidate.url?.startsWith(baseUrl),
    ) ?? targets.find((candidate) => candidate.type === "page");
  assert(
    target?.webSocketDebuggerUrl,
    "Android WebView exposed no page target",
  );
  const client = new CdpClient(target.webSocketDebuggerUrl);
  await client.connect();
  await client.send("Page.enable");
  await client.send("Runtime.enable");
  await client.send("DOM.enable");
  return client;
}

async function evaluate(client, expression) {
  const result = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) {
    throw new Error(
      `WebView evaluation failed: ${
        result.exceptionDetails.text ?? "unknown exception"
      }`,
    );
  }
  return result.result?.value;
}

async function waitFor(client, expression, message, timeout = timeoutMs) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate(client, expression)) return;
    await sleep(100);
  }
  throw new Error(message);
}

async function navigate(client, url) {
  const previousTimeOrigin = await evaluate(client, "performance.timeOrigin");
  await client.send("Page.navigate", { url });
  await waitFor(
    client,
    `performance.timeOrigin !== ${previousTimeOrigin} &&
      document.readyState === "complete" &&
      location.href.startsWith(${JSON.stringify(url)})`,
    `WebView did not navigate to ${url}`,
  );
}

async function reload(client) {
  const previousTimeOrigin = await evaluate(client, "performance.timeOrigin");
  await client.send("Page.reload", { ignoreCache: true });
  await waitFor(
    client,
    `performance.timeOrigin !== ${previousTimeOrigin} &&
      document.readyState === "complete"`,
    "WebView reload did not complete",
  );
}

async function readDiagnostics(client) {
  return (
    (await evaluate(client, "window.__ruaTerminalDiagnostics ?? []")) ?? []
  );
}

async function latestAppliedCursor(client, sessionId) {
  const diagnostics = await readDiagnostics(client);
  return latest(
    diagnostics,
    (item) =>
      item.event === "frame_classified" &&
      item.session_id === sessionId &&
      item.frame_classification === "apply" &&
      item.cursor_after,
  )?.cursor_after;
}

async function waitForOffsetAdvance(client, sessionId, beforeOffset) {
  await waitFor(
    client,
    `(window.__ruaTerminalDiagnostics ?? []).some((item) =>
      item.event === "frame_classified" &&
      item.session_id === ${JSON.stringify(sessionId)} &&
      item.frame_classification === "apply" &&
      (item.cursor_after?.next_output_offset ?? -1) > ${beforeOffset}
    )`,
    "terminal output cursor did not advance",
  );
}

async function clickSelector(client, selector, last = false) {
  const clicked = await evaluate(
    client,
    `(() => {
      const items = Array.from(document.querySelectorAll(${JSON.stringify(
        selector,
      )}));
      const element = items[${last ? "items.length - 1" : "0"}];
      if (!element) return false;
      element.click();
      return true;
    })()`,
  );
  assert(clicked, `element is missing: ${selector}`);
}

async function chooseSelectOption(client, selector, label) {
  await clickSelector(client, selector);
  await waitFor(
    client,
    `Array.from(document.querySelectorAll(".rua-ui-select-option")).some(
      (element) => element.textContent?.includes(${JSON.stringify(label)})
    )`,
    `select option did not appear: ${label}`,
  );
  const clicked = await evaluate(
    client,
    `(() => {
      const element = Array.from(
        document.querySelectorAll(".rua-ui-select-option")
      ).find((candidate) =>
        candidate.textContent?.includes(${JSON.stringify(label)})
      );
      if (!element) return false;
      element.click();
      return true;
    })()`,
  );
  assert(clicked, `select option is missing: ${label}`);
}

async function activateSession(client, sessionId) {
  const shortId = sessionShortId(sessionId);
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await chooseSelectOption(client, ".rua-terminal-session-select", shortId);
    const stableUntil = Date.now() + 750;
    let stable = true;
    while (Date.now() < stableUntil) {
      const selected = await evaluate(
        client,
        `document.querySelector(".rua-terminal-session-select")
          ?.textContent?.includes(${JSON.stringify(shortId)}) === true`,
      );
      if (!selected) {
        stable = false;
        break;
      }
      await sleep(100);
    }
    if (stable) return;
  }
  throw new Error(`terminal session did not remain active: ${shortId}`);
}

async function selectExecutionDevice(client, devices, placement, deviceId) {
  const remoteDevices = devices.filter((device) => !device.is_self);
  const remoteIndex = remoteDevices.findIndex(
    (device) => device.device_id === deviceId,
  );
  const optionIndex = placement === "local" ? 0 : remoteIndex + 1;
  assert(
    placement === "local" || remoteIndex >= 0,
    `${placement} execution device is missing`,
  );
  await waitFor(
    client,
    'document.querySelector(".rua-terminal-device-select") !== null',
    "paired device selector did not become available",
  );
  await clickSelector(client, ".rua-terminal-device-select");
  await waitFor(
    client,
    `document.querySelectorAll(".rua-ui-select-option").length > ${optionIndex}`,
    `${placement} execution device option did not appear`,
  );
  const clicked = await evaluate(
    client,
    `(() => {
      const option = document.querySelectorAll(
        ".rua-ui-select-option"
      )[${optionIndex}];
      if (!option) return false;
      option.click();
      return true;
    })()`,
  );
  assert(clicked, `${placement} execution device option is missing`);
  if (placement === "remote") {
    await waitFor(
      client,
      `Object.entries(localStorage).some(
        ([key, value]) =>
          key.startsWith("rua-terminal-active-execution-device:") &&
          value === ${JSON.stringify(`remote:${deviceId}`)}
      )`,
      "remote execution device did not become active",
    );
  }
  await sleep(250);
}

async function createSession(client, placement) {
  const before = await readDiagnostics(client);
  const existingSessionIds = new Set(
    before
      .filter((item) => item.event === "attach_complete")
      .map((item) => item.session_id),
  );

  await waitFor(
    client,
    'document.querySelector(".rua-terminal-page-header button:has(.lucide-plus)")?.disabled === false',
    "terminal profiles did not enable session creation",
  );
  await clickSelector(client, ".rua-terminal-page-header button:has(.lucide-plus)");

  const dialogDeadline = Date.now() + 2_000;
  while (Date.now() < dialogDeadline) {
    const state = await evaluate(
      client,
      `({
        dialog: document.querySelector('[role="dialog"] [role="combobox"]') !== null,
        attached: (window.__ruaTerminalDiagnostics ?? []).some(
          (item) =>
            item.event === "attach_complete" &&
            item.placement === ${JSON.stringify(placement)} &&
            item.session_id &&
            !${JSON.stringify(Array.from(existingSessionIds))}.includes(
              item.session_id
            )
        )
      })`,
    );
    if (state.attached) break;
    if (state.dialog) {
      await clickSelector(client, '[role="dialog"] [role="combobox"]');
      await waitFor(
        client,
        'document.querySelector(".rua-ui-select-option:not(.rua-ui-select-option--disabled)") !== null',
        "terminal profile options did not load",
      );
      await clickSelector(client, ".rua-ui-select-option:not(.rua-ui-select-option--disabled)");
      await waitFor(
        client,
        `(() => {
          const buttons = document.querySelectorAll(
            '[role="dialog"] .rua-terminal-kill-confirm-footer button'
          );
          const button = buttons[buttons.length - 1];
          return button && !button.disabled;
        })()`,
        "terminal profile confirmation did not become enabled",
      );
      await clickSelector(
        client,
        '[role="dialog"] .rua-terminal-kill-confirm-footer button',
        true,
      );
      break;
    }
    await sleep(100);
  }

  await waitFor(
    client,
    `(window.__ruaTerminalDiagnostics ?? []).some(
      (item) =>
        item.event === "attach_complete" &&
        item.placement === ${JSON.stringify(placement)} &&
        item.session_id &&
        !${JSON.stringify(Array.from(existingSessionIds))}.includes(
          item.session_id
        )
    )`,
    `${placement} terminal did not attach`,
  );

  const diagnostics = await readDiagnostics(client);
  const attached = latest(
    diagnostics,
    (item) =>
      item.event === "attach_complete" &&
      item.placement === placement &&
      item.session_id &&
      !existingSessionIds.has(item.session_id),
  );
  assert(attached?.session_id, `${placement} attach did not expose a session`);
  assert(attached.viewer_id, `${placement} attach did not expose a viewer`);

  await activateSession(client, attached.session_id);
  await waitFor(
    client,
    `document.querySelector(${JSON.stringify(
      `${activeTerminalSelector} textarea`,
    )}) !== null`,
    `${placement} terminal textarea did not mount`,
  );
  const focused = await evaluate(
    client,
    `document.activeElement === document.querySelector(${JSON.stringify(
      `${activeTerminalSelector} textarea`,
    )})`,
  );
  assert.equal(focused, false, `${placement} terminal focused during attach`);

  return {
    placement,
    sessionId: attached.session_id,
    viewerId: attached.viewer_id,
    sessionKey: attached.session_key,
    initialCursor: attached.cursor_after,
  };
}

async function elementCenter(client, selector) {
  const rect = await evaluate(
    client,
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return {
        x: Math.round(rect.left + rect.width / 2),
        y: Math.round(rect.top + rect.height / 2),
        top: rect.top,
        bottom: rect.bottom,
        height: rect.height
      };
    })()`,
  );
  assert(rect, `element has no bounds: ${selector}`);
  assert(rect.height > 0, `element has zero height: ${selector}`);
  return rect;
}

async function touchTap(client, selector) {
  const center = await elementCenter(client, selector);
  await client.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [{ x: center.x, y: center.y }],
  });
  await client.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });
}

async function focusTerminal(client, placement) {
  await touchTap(client, activeTerminalSelector);
  await waitFor(
    client,
    `document.activeElement === document.querySelector(${JSON.stringify(
      `${activeTerminalSelector} textarea`,
    )})`,
    `${placement} terminal did not focus after explicit activation`,
  );
  await waitFor(
    client,
    `(() => {
      const height = Math.round(
        window.visualViewport?.height ?? window.innerHeight
      );
      const cssHeight = Number.parseInt(
        getComputedStyle(document.documentElement)
          .getPropertyValue("--rua-mobile-visual-height"),
        10
      );
      const terminal = document.querySelector(${JSON.stringify(
        activeTerminalSelector,
      )});
      return cssHeight === height &&
        terminal !== null &&
        terminal.getBoundingClientRect().bottom <= height + 1;
    })()`,
    `${placement} terminal did not fit above the Android keyboard`,
  );
}

async function dispatchKey(client, key, code, virtualKeyCode, modifiers = 0) {
  await client.send("Input.dispatchKeyEvent", {
    type: "rawKeyDown",
    key,
    code,
    modifiers,
    windowsVirtualKeyCode: virtualKeyCode,
    nativeVirtualKeyCode: virtualKeyCode,
  });
  await client.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key,
    code,
    modifiers,
    windowsVirtualKeyCode: virtualKeyCode,
    nativeVirtualKeyCode: virtualKeyCode,
  });
}

async function sendApiInput(client, session, data, remoteId) {
  const response = await evaluate(
    client,
    `(async () => {
      const response = await fetch(
        "/api/terminal/sessions/" +
          encodeURIComponent(${JSON.stringify(session.sessionId)}) +
          "/input",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            browser_viewer_id: ${JSON.stringify(session.viewerId)},
            data: ${JSON.stringify(data)},
            execution_device: ${JSON.stringify(
              executionDevice(session.placement, remoteId),
            )}
          })
        }
      );
      return { ok: response.ok, status: response.status };
    })()`,
  );
  assert(response.ok, `terminal input failed with status ${response.status}`);
}

async function sendAndAwaitOutput(client, session, data, remoteId) {
  const cursor = await latestAppliedCursor(client, session.sessionId);
  const beforeOffset =
    cursor?.next_output_offset ??
    session.initialCursor?.next_output_offset ??
    0;
  await sendApiInput(client, session, data, remoteId);
  await waitForOffsetAdvance(client, session.sessionId, beforeOffset);
}

async function readSession(client, session, remoteId) {
  const deviceId = session.placement === "local" ? "local" : remoteId;
  const result = await evaluate(
    client,
    `(async () => {
      const response = await fetch(
        "/api/terminal/sessions?execution_device=" +
          encodeURIComponent(${JSON.stringify(deviceId)}),
        { cache: "no-store" }
      );
      return {
        ok: response.ok,
        status: response.status,
        payload: response.ok ? await response.json() : null
      };
    })()`,
  );
  assert(
    result.ok,
    `terminal session list failed with status ${result.status}`,
  );
  const current = result.payload?.sessions?.find(
    (candidate) => candidate.session_id === session.sessionId,
  );
  assert(current, `terminal session is missing: ${session.sessionId}`);
  return current;
}

async function takeInputLease(client, session, browserViewerId, remoteId) {
  const response = await evaluate(
    client,
    `(async () => {
      const response = await fetch(
        "/api/terminal/sessions/" +
          encodeURIComponent(${JSON.stringify(session.sessionId)}) +
          "/take-input-lease",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            browser_viewer_id: ${JSON.stringify(browserViewerId)},
            execution_device: ${JSON.stringify(
              executionDevice(session.placement, remoteId),
            )}
          })
        }
      );
      return { ok: response.ok, status: response.status };
    })()`,
  );
  assert(
    response.ok,
    `take terminal input lease failed with status ${response.status}`,
  );
}

function countSessionDiagnostics(diagnostics, session, event, lifecycle) {
  return diagnostics.filter(
    (item) =>
      item.event === event &&
      item.session_id === session.sessionId &&
      (lifecycle === undefined || item.lifecycle === lifecycle),
  ).length;
}

async function assertAutomaticGeometryContract(client, session, remoteId) {
  const deviceId = session.placement === "local" ? "local" : remoteId;
  const manualAdaptControl = await evaluate(
    client,
    `Array.from(
      document.querySelectorAll(".rua-terminal-session-actions button")
    ).some((button) =>
      button.querySelector(".lucide-scan") ||
      /adapt pty/i.test(button.getAttribute("aria-label") ?? "")
    )`,
  );
  assert.equal(
    manualAdaptControl,
    false,
    `${session.placement} terminal exposed a manual PTY adapt control`,
  );

  await waitFor(
    client,
    `(async () => {
      const diagnostics = window.__ruaTerminalDiagnostics ?? [];
      const latestFit = diagnostics.filter((item) =>
        item.event === "viewport_fit" &&
        item.session_key === ${JSON.stringify(session.sessionKey)}
      ).at(-1)?.viewport;
      if (!latestFit) return false;
      const response = await fetch(
        "/api/terminal/sessions?execution_device=" +
          encodeURIComponent(${JSON.stringify(deviceId)}),
        { cache: "no-store" }
      );
      if (!response.ok) return false;
      const payload = await response.json();
      const current = payload.sessions?.find((candidate) =>
        candidate.session_id === ${JSON.stringify(session.sessionId)}
      );
      return current?.pty_geometry?.cols === latestFit.cols &&
        current?.pty_geometry?.rows === latestFit.rows;
    })()`,
    `${session.placement} initial viewport geometry did not reach the authority`,
  );
  const beforeAuthoritySession = await readSession(client, session, remoteId);
  const beforeDiagnostics = await readDiagnostics(client);
  const beforeAttach = countSessionDiagnostics(
    beforeDiagnostics,
    session,
    "attach_start",
  );
  const beforeHydrate = countSessionDiagnostics(
    beforeDiagnostics,
    session,
    "attach_complete",
    "hydrated",
  );

  const originalHeight = await evaluate(
    client,
    `(() => {
      const root = document.documentElement;
      const height = Math.round(
        window.visualViewport?.height ?? window.innerHeight
      );
      root.style.setProperty(
        "--rua-mobile-visual-height",
        Math.max(320, height - 160) + "px"
      );
      return height;
    })()`,
  );
  assert(originalHeight > 0, "mobile visual viewport has no measurable height");

  await waitFor(
    client,
    `(async () => {
      const response = await fetch(
        "/api/terminal/sessions?execution_device=" +
          encodeURIComponent(${JSON.stringify(deviceId)}),
        { cache: "no-store" }
      );
      if (!response.ok) return false;
      const payload = await response.json();
      const current = payload.sessions?.find((candidate) =>
        candidate.session_id === ${JSON.stringify(session.sessionId)}
      );
      return current?.pty_geometry?.cols !== ${
        beforeAuthoritySession.pty_geometry.cols
      } ||
        current?.pty_geometry?.rows !== ${
          beforeAuthoritySession.pty_geometry.rows
        };
    })()`,
    `${session.placement} terminal did not synchronize resized geometry`,
  );

  await evaluate(
    client,
    `document.documentElement.style.setProperty(
      "--rua-mobile-visual-height",
      ${JSON.stringify(`${originalHeight}px`)}
    )`,
  );
  await waitFor(
    client,
    `(async () => {
      const response = await fetch(
        "/api/terminal/sessions?execution_device=" +
          encodeURIComponent(${JSON.stringify(deviceId)}),
        { cache: "no-store" }
      );
      if (!response.ok) return false;
      const payload = await response.json();
      const current = payload.sessions?.find((candidate) =>
        candidate.session_id === ${JSON.stringify(session.sessionId)}
      );
      return current?.pty_geometry?.cols === ${
        beforeAuthoritySession.pty_geometry.cols
      } &&
        current?.pty_geometry?.rows === ${
          beforeAuthoritySession.pty_geometry.rows
        };
    })()`,
    `${session.placement} terminal did not restore viewport geometry`,
  );

  const afterDiagnostics = await readDiagnostics(client);
  assert.equal(
    countSessionDiagnostics(afterDiagnostics, session, "attach_start"),
    beforeAttach,
    `${session.placement} geometry resize attached the terminal again`,
  );
  assert.equal(
    countSessionDiagnostics(
      afterDiagnostics,
      session,
      "attach_complete",
      "hydrated",
    ),
    beforeHydrate,
    `${session.placement} geometry resize hydrated terminal history`,
  );

  const latestGeometry = latest(
    afterDiagnostics,
    (item) =>
      item.event === "viewport_fit" &&
      item.session_key === session.sessionKey &&
      item.viewport,
  )?.viewport;
  const authoritySession = await readSession(client, session, remoteId);
  assert.deepEqual(
    authoritySession.pty_geometry,
    latestGeometry
      ? { cols: latestGeometry.cols, rows: latestGeometry.rows }
      : latestGeometry,
    `${session.placement} authority geometry differs from the lease holder viewport`,
  );
}

async function assertPassiveAuthorityGeometry(client, session, remoteId) {
  const authoritySession = await readSession(client, session, remoteId);
  const authorityGeometry = authoritySession.pty_geometry;
  const viewportFitsBefore = (await readDiagnostics(client)).filter(
    (item) =>
      item.event === "viewport_fit" &&
      item.session_key === session.sessionKey &&
      item.viewport?.cols === authorityGeometry.cols &&
      item.viewport?.rows === authorityGeometry.rows,
  ).length;
  await takeInputLease(
    client,
    session,
    `${session.viewerId}-passive-check`,
    remoteId,
  );
  await waitFor(
    client,
    `document.querySelector(
      ${JSON.stringify(
        `${activeTerminalSelector} .rua-terminal-xterm-host--locked-geometry`,
      )}
    ) !== null`,
    `${session.placement} passive viewer did not lock authority geometry`,
  );
  await waitFor(
    client,
    `(window.__ruaTerminalDiagnostics ?? []).filter((item) =>
      item.event === "viewport_fit" &&
      item.session_key === ${JSON.stringify(session.sessionKey)} &&
      item.viewport?.cols === ${authorityGeometry.cols} &&
      item.viewport?.rows === ${authorityGeometry.rows}
    ).length > ${viewportFitsBefore}`,
    `${session.placement} passive viewer did not render authority geometry`,
  );
  await sleep(300);
  assert.deepEqual(
    (await readSession(client, session, remoteId)).pty_geometry,
    authorityGeometry,
    `${session.placement} passive viewer changed PTY geometry`,
  );

  await takeInputLease(client, session, session.viewerId, remoteId);
  await waitFor(
    client,
    `document.querySelector(
      ${JSON.stringify(
        `${activeTerminalSelector} .rua-terminal-xterm-host--locked-geometry`,
      )}
    ) === null`,
    `${session.placement} viewer did not regain geometry ownership`,
  );
}

async function latestSessionWriterMetrics(client, sessionKey) {
  const perf = await readDiagnostics(client);
  return latest(
    perf,
    (item) =>
      item.session_key === sessionKey && item.event === "writer_complete",
  );
}

async function waitForScrollback(client, sessionKey) {
  await waitFor(
    client,
    `(window.__ruaTerminalDiagnostics ?? []).some(
      (item) =>
        item.session_key === ${JSON.stringify(sessionKey)} &&
        item.event === "writer_complete" &&
        (item.viewport?.base_y ?? 0) > 20
    )`,
    "terminal did not accumulate scrollback",
  );
}

async function scrollUp(client) {
  const rect = await elementCenter(client, activeTerminalSelector);
  const startY = Math.round(rect.top + rect.height * 0.2);
  const endY = Math.round(rect.top + rect.height * 0.9);
  await client.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [{ x: rect.x, y: startY }],
  });
  for (let step = 1; step <= 8; step += 1) {
    await client.send("Input.dispatchTouchEvent", {
      type: "touchMove",
      touchPoints: [
        {
          x: rect.x,
          y: Math.round(startY + ((endY - startY) * step) / 8),
        },
      ],
    });
  }
  await client.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });
  await sleep(500);
}

async function clickMobileNavItem(client, iconClass, label) {
  const menuOpened = await evaluate(
    client,
    `(() => {
      const button = document.querySelector('button[aria-label="menu"]');
      button?.click();
      return button !== null;
    })()`,
  );
  assert(menuOpened, "mobile navigation menu is unavailable");
  await waitFor(
    client,
    `document.querySelector(
      ${JSON.stringify(`.rua-nav-drawer button ${iconClass}`)}
    ) !== null`,
    `mobile navigation item did not render: ${label}`,
  );
  const clicked = await evaluate(
    client,
    `(() => {
      const icon = document.querySelector(
        ${JSON.stringify(`.rua-nav-drawer button ${iconClass}`)}
      );
      const button = icon?.closest("button");
      button?.click();
      return button !== null;
    })()`,
  );
  assert(clicked, `mobile navigation item is unavailable: ${label}`);
}

async function assertRouteContinuity(client, session) {
  const switchedAt = await evaluate(client, "new Date().toISOString()");
  await clickMobileNavItem(client, ".lucide-square-pen", "new chat");
  await waitFor(
    client,
    `location.pathname.includes("/chats/") &&
      document.querySelector(".rua-owned-chat-page") !== null`,
    "WebView did not enter the chat route",
  );
  await clickMobileNavItem(client, ".lucide-square-terminal", "terminal");
  await waitFor(
    client,
    `location.pathname === "/terminal" &&
      document.querySelector(${JSON.stringify(
        activeTerminalSelector,
      )}) !== null`,
    "WebView did not return to the terminal route",
  );
  const after = (await readDiagnostics(client)).filter(
    (item) =>
      item.at >= switchedAt && item.event === "attach_start" && item.session_id === session.sessionId,
  ).length;
  assert.equal(after, 0, "route switch reattached the active terminal");
}

async function assertSessionContinuity(
  client,
  firstSession,
  placement,
  remoteId,
) {
  await createSession(client, placement);
  const cursor = await latestAppliedCursor(client, firstSession.sessionId);
  const beforeOffset =
    cursor?.next_output_offset ??
    firstSession.initialCursor?.next_output_offset ??
    0;
  await sendApiInput(
    client,
    firstSession,
    "printf 'background-session-output\\n'\n",
    remoteId,
  );
  const sessionSwitchStartedAt = await evaluate(
    client,
    `new Date().toISOString()`,
  );
  await activateSession(client, firstSession.sessionId);
  await waitFor(
    client,
    `(window.__ruaTerminalDiagnostics ?? []).some((item) =>
      item.event === "frame_classified" &&
      item.frame_classification === "apply" &&
      item.session_id === ${JSON.stringify(firstSession.sessionId)} &&
      (item.cursor_after?.next_output_offset ?? -1) > ${beforeOffset}
    )`,
    "session switch did not recover hidden terminal output",
  );
  const afterDiagnostics = (await readDiagnostics(client)).filter(
    (item) => item.at >= sessionSwitchStartedAt,
  );
  assert.equal(
    countSessionDiagnostics(afterDiagnostics, firstSession, "attach_start"),
    0,
    "session switch reattached an existing runtime",
  );
  assert.equal(
    countSessionDiagnostics(
      afterDiagnostics,
      firstSession,
      "subscription_open",
    ),
    placement === "remote" ? 1 : 0,
    "session switch did not preserve the placement's subscription ownership",
  );
  assert.equal(
    countSessionDiagnostics(
      afterDiagnostics,
      firstSession,
      "subscription_close",
    ),
    placement === "remote" ? 1 : 0,
    "session switch changed the subscription outside the remote authority handoff",
  );
  assert.equal(
    countSessionDiagnostics(afterDiagnostics, firstSession, "recovery_start"),
    0,
    "session switch unnecessarily started snapshot recovery",
  );
  assert.equal(
    countSessionDiagnostics(
      afterDiagnostics,
      firstSession,
      "recovery_complete",
    ),
    0,
    "session switch unnecessarily replaced the retained terminal surface",
  );
}

async function assertBackgroundRecovery(client, session, remoteId) {
  await apiJson("PUT", "/api/sync/power-policy", {
    mode: "custom",
    periodic_interval_secs: 60,
    background_flush_grace_secs: 1,
    allow_background_runtime: false,
    allow_background_scheduled_sync: false,
  });

  const beforeCursor = await latestAppliedCursor(client, session.sessionId);
  assert(
    beforeCursor,
    "remote terminal has no cursor before background recovery",
  );
  const timeOrigin = await evaluate(client, "performance.timeOrigin");
  const recoveryStartedAt = await evaluate(client, "new Date().toISOString()");
  const marker = `rua-background-recovery-${Date.now()}`;

  await sendApiInput(
    client,
    session,
    `sleep 3; printf '${marker}\\n'\n`,
    remoteId,
  );
  await evaluate(
    client,
    `(() => {
      const originalFetch =
        window.__ruaBackgroundRecoveryFetch ?? window.fetch.bind(window);
      window.__ruaBackgroundRecoveryFetch = originalFetch;
      window.__ruaBackgroundResponses = [];
      window.__ruaInjectedAttachFailures = 0;
      window.__ruaInjectedActivityFailures = 0;
      window.__ruaActivityFailureLimit = 1;
      window.fetch = async (input, init) => {
        const url =
          typeof input === "string" ? input : input?.url ?? String(input);
        // Foreground resumes the retained cursor. A transport failure, not
        // visibility alone, asks the runtime to recover from a snapshot.
        if (
          window.__ruaInjectedActivityFailures < window.__ruaActivityFailureLimit &&
          url.includes("/api/terminal/sessions/" + ${JSON.stringify(session.sessionId)} + "/activity?")
        ) {
          window.__ruaInjectedActivityFailures += 1;
          return new Response("acceptance transport unavailable", { status: 503 });
        }
        if (
          window.__ruaInjectedAttachFailures === 0 &&
          url.includes("/api/terminal/sessions/") &&
          url.endsWith("/attach")
        ) {
          window.__ruaInjectedAttachFailures = 1;
          return new Response(
            JSON.stringify({
              detail: "Device Network Runtime is not initialized"
            }),
            {
              status: 503,
              headers: { "Content-Type": "application/json" }
            }
          );
        }
        const response = await originalFetch(input, init);
        if (url.includes("/api/terminal/sessions/") && !response.ok) {
          const body = await response.clone().json().catch(() => ({}));
          window.__ruaBackgroundResponses.push({ status: response.status, detail: body.detail });
        }
        return response;
      };
      return true;
    })()`,
  );

  await adb("shell", "input", "keyevent", "3");
  await waitFor(
    client,
    "document.hidden === true",
    "Android WebView did not enter background",
  );
  const suspended = await waitForApi(
    "/api/platform/device-network-availability",
    (snapshot) => snapshot.state === "suspended",
    "Device Network Runtime did not suspend after background grace",
    60_000,
  );
  const suspendedResidency = await apiJson(
    "GET",
    "/api/platform/foreground-operations",
  );
  assert.equal(suspendedResidency.active_count, 0);
  assert.equal(suspendedResidency.locks_required, false);
  assert.equal(suspendedResidency.residency_state, "suspended");
  await sleep(3_000);

  await adb("shell", "am", "start", "-n", androidActivity);
  await waitFor(
    client,
    "document.hidden === false",
    "Android WebView did not return to foreground",
  );
  const firstReady = await waitForApi(
    "/api/platform/device-network-availability",
    (snapshot) =>
      snapshot.state === "ready" && snapshot.revision > suspended.revision,
    "Device Network Runtime did not become ready after foreground",
    60_000,
  );
  await waitFor(
    client,
    "window.__ruaInjectedAttachFailures === 1",
    "the deterministic runtime-not-ready attach was not exercised",
  );
  await waitFor(
    client,
    `(window.__ruaTerminalDiagnostics ?? []).some((item) =>
      item.at >= ${JSON.stringify(recoveryStartedAt)} &&
      item.event === "runtime_state" &&
      item.session_id === ${JSON.stringify(session.sessionId)} &&
      item.state_after === "live" && item.recovery_reason === "recovery_failed"
    )`,
    "terminal did not report the injected recovery failure",
  );
  const oldSurfaceStillVisible = await evaluate(
    client,
    `(() => {
      const terminal = document.querySelector(${JSON.stringify(
        activeTerminalSelector,
      )});
      if (!terminal) return false;
      const rect = terminal.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && terminal.childElementCount > 0;
    })()`,
  );
  assert(
    oldSurfaceStillVisible,
    "old terminal surface disappeared during recovery",
  );

  await adb("shell", "input", "keyevent", "3");
  await waitFor(client, "document.hidden === true", "fault recovery did not hide the document");
  const secondSuspended = await waitForApi(
    "/api/platform/device-network-availability",
    (snapshot) =>
      snapshot.state === "suspended" && snapshot.revision > firstReady.revision,
    "fault recovery barrier did not publish a newer suspended revision",
    60_000,
  );
  await evaluate(client, "window.__ruaActivityFailureLimit = 2");
  await adb("shell", "am", "start", "-n", androidActivity);
  await waitFor(client, "document.hidden === false", "fault recovery did not resume the document");
  const secondReady = await waitForApi(
    "/api/platform/device-network-availability",
    (snapshot) =>
      snapshot.state === "ready" &&
      snapshot.revision > secondSuspended.revision,
    "fault recovery barrier did not publish a newer ready revision",
    60_000,
  );

  await waitFor(
    client,
    `(window.__ruaTerminalDiagnostics ?? []).some((item) =>
      item.at >= ${JSON.stringify(recoveryStartedAt)} &&
      item.event === "recovery_complete" &&
      item.session_id === ${JSON.stringify(session.sessionId)} &&
      item.recovery_reason === "transport_failure" &&
      (item.cursor_after?.next_output_offset ?? -1) >
        ${beforeCursor.next_output_offset}
    )`,
    "terminal did not recover missed background output",
  );
  assert.equal(
    await evaluate(client, "performance.timeOrigin"),
    timeOrigin,
    "terminal recovery reloaded the WebView",
  );

  const attachment = await evaluate(
    client,
    `(async () => {
      const response = await fetch(
        "/api/terminal/sessions/" +
          encodeURIComponent(${JSON.stringify(session.sessionId)}) +
          "/attach",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            browser_viewer_id: ${JSON.stringify(session.viewerId)},
            scrollback_lines: 5000,
            execution_device: ${JSON.stringify(
              executionDevice("remote", remoteId),
            )}
          })
        }
      );
      if (!response.ok) {
        throw new Error("verification attach failed: " + response.status);
      }
      return response.json();
    })()`,
  );
  const transcript = [
    attachment.snapshot.data,
    ...attachment.tail
      .filter((frame) => frame.type === "output")
      .map((frame) => Buffer.from(frame.data, "base64").toString("utf8")),
  ].join("");
  assert.equal(
    transcript.split(marker).length - 1,
    1,
    "missed background output was absent or duplicated",
  );

  await sendAndAwaitOutput(
    client,
    session,
    `printf 'rua-after-recovery-${Date.now()}\\n'\n`,
    remoteId,
  );
  await evaluate(
    client,
    `(() => {
      window.fetch = window.__ruaBackgroundRecoveryFetch;
      return true;
    })()`,
  );
  await apiJson("PUT", "/api/sync/power-policy", { mode: "power_saver" });

  const diagnostics = (await readDiagnostics(client)).filter(
    (item) =>
      item.at >= recoveryStartedAt && item.session_id === session.sessionId,
  );
  return {
    suspended_revision: suspended.revision,
    first_ready_revision: firstReady.revision,
    second_ready_revision: secondReady.revision,
    injected_runtime_not_ready: 1,
    injected_transport_failures: await evaluate(client, "window.__ruaInjectedActivityFailures"),
    recovery_start_count: diagnostics.filter(
      (item) => item.event === "recovery_start",
    ).length,
    recovery_complete_count: diagnostics.filter(
      (item) => item.event === "recovery_complete",
    ).length,
    cursor_before: beforeCursor,
    cursor_after: await latestAppliedCursor(client, session.sessionId),
    missed_output_count: 1,
    webview_reloaded: false,
  };
}

function chatContains(chat, marker) {
  return JSON.stringify(chat).includes(marker);
}

function chatIsStable(chat) {
  return (chat?.messages ?? []).some((message) => {
    if (
      message?.type === "turn_aborted" ||
      message?.type === "turn_interjected"
    ) {
      return true;
    }
    if (message?.role !== "assistant") return false;
    return message?.status !== "in_progress" && message?.status !== "streaming";
  });
}

function chatIdentitySnapshot(chat) {
  return {
    placement: chat?.meta?.authority_placement
      ? {
          authority_epoch:
            chat.meta.authority_placement.authority_epoch ?? null,
          authority_device_id: chat.meta.authority_placement.device_id ?? null,
        }
      : null,
    messages: (chat?.messages ?? []).map((message) => ({
      id: message?.id ?? null,
      turn_id: message?.turn_id ?? null,
      role: message?.role ?? null,
      type: message?.type ?? null,
      status: message?.status ?? null,
    })),
  };
}

function streamChatChunk(response, marker, finishReason = null) {
  response.write(
    `data: ${JSON.stringify({
      id: "chatcmpl-android-recovery",
      object: "chat.completion.chunk",
      model: "android-recovery-model",
      choices: [
        {
          index: 0,
          delta: marker ? { role: "assistant", content: marker } : {},
          finish_reason: finishReason,
        },
      ],
    })}\n\n`,
  );
}

async function startChatRecoveryMock() {
  const historyMarker = `android-chat-history-${Date.now()}`;
  const replayMarker = `android-chat-replay-${Date.now()}`;
  const liveMarker = `android-chat-live-${Date.now()}`;
  let resolveReplaySent;
  let releaseLive;
  const replaySent = new Promise((resolve) => {
    resolveReplaySent = resolve;
  });
  const liveReleased = new Promise((resolve) => {
    releaseLive = resolve;
  });
  const server = createServer(async (request, response) => {
    if (request.method === "GET") {
      const payload = JSON.stringify({ ok: true });
      response.writeHead(200, {
        "Content-Type": "application/json",
        "Content-Length": Buffer.byteLength(payload),
      });
      response.end(payload);
      return;
    }

    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
    const prompt = JSON.stringify(body.messages ?? []);
    response.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "close",
    });
    if (prompt.includes("android-chat-recovery")) {
      streamChatChunk(response, replayMarker);
      resolveReplaySent();
      await liveReleased;
      streamChatChunk(response, liveMarker);
      streamChatChunk(response, "", "stop");
      response.end("data: [DONE]\n\n");
      return;
    }
    streamChatChunk(response, historyMarker);
    streamChatChunk(response, "", "stop");
    response.end("data: [DONE]\n\n");
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  assert(address && typeof address !== "string");
  return {
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    historyMarker,
    replayMarker,
    liveMarker,
    waitForReplay: async () => {
      let timer;
      try {
        await Promise.race([
          replaySent,
          new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error("Chat fixture did not receive the recovery turn")), 60_000);
          }),
        ]);
      } finally {
        clearTimeout(timer);
      }
    },
    releaseLive: () => releaseLive(),
    close: async () => {
      releaseLive();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

async function assertChatRecovery(client, mobileDeviceId) {
  const mock = await startChatRecoveryMock();
  const providerId = `android-chat-recovery-e2e-${Date.now()}`;
  try {
    await requestJson(remoteApiUrl, "POST", "/api/models/custom-providers", {
      id: providerId,
      name: "Android Chat Recovery E2E",
      base_url: mock.baseUrl,
      api_key: "test-key",
      chat_model: "OpenAIChatModel",
      models: [
        {
          id: "android-recovery-model",
          name: "Android Recovery Model",
        },
      ],
    });
    await requestJson(remoteApiUrl, "PUT", "/api/models/active", {
      provider_id: providerId,
      model: "android-recovery-model",
    });
    const created = await requestJson(
      remoteApiUrl,
      "POST",
      "/api/agents/default/chats",
      {
        name: "Android Chat Recovery E2E",
        session_id: `android-chat-recovery-${Date.now()}`,
        user_id: "android-chat-recovery-e2e",
        channel: "console",
        meta: {},
        model: {
          provider_id: providerId,
          model: "android-recovery-model",
          thinking_strength: "off",
        },
      },
    );
    const chatId = created.id;
    assert(chatId, "remote Chat creation returned no id");
    const firstSubmit = await requestJson(
      remoteApiUrl,
      "POST",
      `/api/agents/default/chats/${encodeURIComponent(chatId)}/console/submit`,
      { chat_id: chatId, message: "android-chat-history" },
    );
    assert(firstSubmit.accepted, "stable Chat history submit was not accepted");
    await waitForJson(
      remoteApiUrl,
      `/api/agents/default/chats/${encodeURIComponent(chatId)}`,
      (chat) => chatContains(chat, mock.historyMarker) && chatIsStable(chat),
      "desktop Chat did not commit stable history",
      60_000,
    );
    await requestJson(remoteApiUrl, "POST", "/api/sync/cycle/target", {
      target_device_id: mobileDeviceId,
    });
    await waitForJson(
      hostApiUrl,
      `/api/agents/default/chats/${encodeURIComponent(chatId)}`,
      (chat) => chatContains(chat, mock.historyMarker),
      "Android did not receive durable Chat history",
      60_000,
    );

    await apiJson("PUT", "/api/sync/power-policy", {
      mode: "custom",
      periodic_interval_secs: 60,
      background_flush_grace_secs: 1,
      allow_background_runtime: false,
      allow_background_scheduled_sync: false,
    });
    await navigate(client, `${baseUrl}/agents/default/chats/${encodeURIComponent(chatId)}`);
    await waitFor(
      client,
      `document.querySelector(".rua-owned-chat-page") !== null &&
        document.body.innerText.includes(${JSON.stringify(
          mock.historyMarker,
        )})`,
      "durable Chat history did not render before backgrounding",
    );
    const timeOrigin = await evaluate(client, "performance.timeOrigin");
    await waitForApi(
      "/api/platform/foreground-operations",
      (snapshot) => snapshot.active_count === 0,
      "hidden Terminal viewer lease did not release before Chat recovery",
      30_000,
    );
    await adb("shell", "input", "keyevent", "3");
    await waitFor(
      client,
      "document.hidden === true",
      "Android WebView did not enter background for Chat recovery",
    );
    const suspended = await waitForApi(
      "/api/platform/device-network-availability",
      (snapshot) => snapshot.state === "suspended",
      "Device Network Runtime did not suspend before Chat recovery",
      60_000,
    );

    const historyVisibleBeforeReady = await evaluate(
      client,
      `document.body.innerText.includes(${JSON.stringify(mock.historyMarker)})`,
    );

    const recoverySubmit = await requestJson(
      remoteApiUrl,
      "POST",
      `/api/agents/default/chats/${encodeURIComponent(chatId)}/console/submit`,
      { chat_id: chatId, message: "android-chat-recovery" },
    );
    assert(recoverySubmit.accepted, "Chat recovery submit was not accepted");
    await mock.waitForReplay();
    assert.equal(
      await evaluate(
        client,
        `document.body.innerText.includes(${JSON.stringify(
          mock.replayMarker,
        )})`,
      ),
      false,
      "Chat replay marker appeared before Device Network readiness",
    );

    await adb("shell", "am", "start", "-n", androidActivity);
    await waitFor(
      client,
      "document.hidden === false",
      "Android WebView did not return to foreground for Chat recovery",
    );
    const ready = await waitForApi(
      "/api/platform/device-network-availability",
      (snapshot) =>
        snapshot.state === "ready" && snapshot.revision > suspended.revision,
      "Device Network Runtime did not become ready for Chat recovery",
      60_000,
    );
    await waitFor(
      client,
      `document.body.innerText.includes(${JSON.stringify(mock.replayMarker)})`,
      "Chat Live did not replay output produced before readiness",
      60_000,
    );
    mock.releaseLive();
    await waitFor(
      client,
      `document.body.innerText.includes(${JSON.stringify(mock.liveMarker)})`,
      "Chat Live did not continue streaming after readiness",
      60_000,
    );
    const remoteStableChat = await waitForJson(
      remoteApiUrl,
      `/api/agents/default/chats/${encodeURIComponent(chatId)}`,
      (chat) => chatContains(chat, mock.liveMarker) && chatIsStable(chat),
      "desktop Chat did not reach a stable terminal state",
      60_000,
    );
    await requestJson(remoteApiUrl, "POST", "/api/sync/cycle/target", {
      target_device_id: mobileDeviceId,
    });
    const mobileStableChat = await waitForJson(
      hostApiUrl,
      `/api/agents/default/chats/${encodeURIComponent(chatId)}`,
      (chat) => chatContains(chat, mock.liveMarker) && chatIsStable(chat),
      "Android stable Chat history did not converge after the authority turn completed",
      60_000,
    );
    try {
      await waitFor(
        client,
        `document.querySelectorAll(".rua-chat-inline-spinner").length === 0`,
        "Chat retained a generating spinner after the authority turn completed",
        15_000,
      );
    } catch (error) {
      const dom = await evaluate(
        client,
        `Array.from(document.querySelectorAll("[data-rua-chat-role]")).map(
          (node) => ({
            role: node.getAttribute("data-rua-chat-role"),
            message_id: node.getAttribute("data-rua-chat-message-id"),
            turn_id: node.getAttribute("data-rua-chat-turn-id"),
            spinner_count:
              node.querySelectorAll(".rua-chat-inline-spinner").length
          })
        )`,
      );
      await writeFile(
        path.join(artifactDir, "chat-recovery-failure.json"),
        JSON.stringify(
          {
            remote: chatIdentitySnapshot(remoteStableChat),
            mobile: chatIdentitySnapshot(mobileStableChat),
            dom,
          },
          null,
          2,
        ),
      );
      throw error;
    }
    await writeFile(
      path.join(artifactDir, "chat-recovery-diagnostics.json"),
      JSON.stringify(
        {
          remote: chatIdentitySnapshot(remoteStableChat),
          mobile: chatIdentitySnapshot(mobileStableChat),
        },
        null,
        2,
      ),
    );
    const renderedText = await evaluate(client, "document.body.innerText");
    assert.equal(
      renderedText.split(mock.historyMarker).length - 1,
      1,
      "durable Chat history was absent or duplicated",
    );
    assert.equal(
      renderedText.split(mock.replayMarker).length - 1,
      1,
      "replayed Chat output was absent or duplicated",
    );
    assert.equal(
      renderedText.split(mock.liveMarker).length - 1,
      1,
      "live Chat output was absent or duplicated",
    );
    assert.equal(
      await evaluate(client, "performance.timeOrigin"),
      timeOrigin,
      "Chat recovery reloaded the WebView",
    );
    await apiJson("PUT", "/api/sync/power-policy", { mode: "power_saver" });
    return {
      chat_id: chatId,
      suspended_revision: suspended.revision,
      ready_revision: ready.revision,
      history_visible_before_ready: historyVisibleBeforeReady,
      history_count: 1,
      replay_count: 1,
      live_count: 1,
      stable_sync_reconciled: true,
      webview_reloaded: false,
    };
  } finally {
    await mock.close();
  }
}

async function maskedScreenshot(client, name) {
  const overlayId = "rua-terminal-e2e-mask";
  const masked = await evaluate(
    client,
    `(() => {
      document.getElementById(${JSON.stringify(overlayId)})?.remove();
      const terminal = document.querySelector(${JSON.stringify(
        activeTerminalSelector,
      )});
      if (!terminal) return false;
      const rect = terminal.getBoundingClientRect();
      const mask = document.createElement("div");
      mask.id = ${JSON.stringify(overlayId)};
      Object.assign(mask.style, {
        position: "fixed",
        left: rect.left + "px",
        top: rect.top + "px",
        width: rect.width + "px",
        height: rect.height + "px",
        background: "#111827",
        zIndex: "2147483647",
        pointerEvents: "none"
      });
      document.body.appendChild(mask);
      return true;
    })()`,
  );
  assert(masked, "terminal screenshot mask could not be installed");
  try {
    const screenshot = await client.send("Page.captureScreenshot", {
      format: "png",
      fromSurface: true,
    });
    assert(screenshot.data, "WebView screenshot returned no data");
    await writeFile(
      path.join(artifactDir, name),
      Buffer.from(screenshot.data, "base64"),
    );
  } finally {
    await evaluate(
      client,
      `document.getElementById(${JSON.stringify(overlayId)})?.remove()`,
    );
  }
}

async function exercisePlacement(client, devices, placement, remoteId) {
  await selectExecutionDevice(client, devices, placement, remoteId);
  const session = await createSession(client, placement);
  await assertAutomaticGeometryContract(client, session, remoteId);
  await assertPassiveAuthorityGeometry(client, session, remoteId);
  await focusTerminal(client, placement);

  const beforeKeyboardOffset =
    (await latestAppliedCursor(client, session.sessionId))
      ?.next_output_offset ?? 0;
  await client.send("Input.insertText", {
    text: `printf '${secretProbe}\\n'`,
  });
  await dispatchKey(client, "Enter", "Enter", 13);
  await waitForOffsetAdvance(client, session.sessionId, beforeKeyboardOffset);

  await sendAndAwaitOutput(
    client,
    session,
    "i=0; while [ \"$i\" -lt 240 ]; do printf 'x\\n'; i=$((i+1)); done\n",
    remoteId,
  );
  await waitForScrollback(client, session.sessionKey);
  await scrollUp(client);
  await sendAndAwaitOutput(client, session, "printf 'x\\n'\n", remoteId);

  const scrolled = await latestSessionWriterMetrics(client, session.sessionKey);
  assert(scrolled, `${placement} terminal has no write performance metadata`);
  assert(
    (scrolled.viewport?.viewport_y ?? 0) > 0,
    `${placement} terminal jumped to scrollback top`,
  );
  assert(
    (scrolled.viewport?.viewport_y ?? 0) < (scrolled.viewport?.base_y ?? 0),
    `${placement} terminal discarded scrollback-reading intent`,
  );

  await sendApiInput(
    client,
    session,
    "i=0; while [ \"$i\" -lt 500 ]; do printf 'x\\n'; i=$((i+1)); sleep 0.01; done\n",
    remoteId,
  );
  await sleep(300);
  const interruptStartedAt = await evaluate(client, `new Date().toISOString()`);
  await dispatchKey(client, "Escape", "Escape", 27);
  await dispatchKey(client, "c", "KeyC", 67, 2);
  await sleep(500);
  const afterInterruptDiagnostics = (await readDiagnostics(client)).filter(
    (item) => item.at >= interruptStartedAt,
  );
  assert.equal(
    countSessionDiagnostics(afterInterruptDiagnostics, session, "attach_start"),
    0,
    `${placement} terminal interrupt reattached terminal history`,
  );
  assert.equal(
    countSessionDiagnostics(
      afterInterruptDiagnostics,
      session,
      "subscription_open",
    ),
    0,
    `${placement} terminal interrupt reopened the activity subscription`,
  );
  assert.equal(
    countSessionDiagnostics(
      afterInterruptDiagnostics,
      session,
      "subscription_close",
    ),
    0,
    `${placement} terminal interrupt closed the activity subscription`,
  );
  const afterInterrupt = await latestSessionWriterMetrics(
    client,
    session.sessionKey,
  );
  assert(
    (afterInterrupt?.viewport?.viewport_y ?? 0) > 0,
    `${placement} terminal interrupt moved the viewport to the top`,
  );

  await assertRouteContinuity(client, session);
  await maskedScreenshot(client, `${placement}-terminal-masked.png`);
  return session;
}

await mkdir(artifactDir, { recursive: true });
const client = await connectPageTarget();

try {
  // The production WebView receives its private resident token at document start.
  // Raw harness fetches must use the same authenticated HTTP boundary as the UI.
  await client.send("Page.addScriptToEvaluateOnNewDocument", { source: `
    (() => {
      const original = window.fetch;
      window.fetch = (input, init = {}) => {
        const url = new URL(typeof input === 'string' ? input : input.url, location.href);
        if (url.origin === location.origin && url.pathname.startsWith('/api/')) {
          const headers = new Headers(init.headers || (input instanceof Request ? input.headers : undefined));
          const token = localStorage.getItem('rua_auth_token');
          if (token) headers.set('Authorization', 'Bearer ' + token);
          init = { ...init, headers };
        }
        return original.call(window, input, init);
      };
    })();
  ` });
  await navigate(client, baseUrl);
  hostApiToken = await evaluate(client, "localStorage.getItem('rua_auth_token')");
  assert(hostApiToken, "Private WebView authentication missing");
  if (!remoteDeviceId) {
    await requestJson(remoteApiUrl, "POST", "/api/sync/enable");
    await apiJson("POST", "/api/sync/enable");
    const remote = await requestJson(remoteApiUrl, "GET", "/api/sync/devices");
    remoteDeviceId = remote.devices.find(device => device.is_self)?.device_id;
    assert(remoteDeviceId, "Host peer identity missing");
    const invitation = await requestJson(remoteApiUrl, "POST", "/api/sync/pairing/initiate");
    await apiJson("POST", "/api/sync/pairing/join", { pin: invitation.pin });
    await waitForApi("/api/sync/devices", value => value.devices.some(device => device.device_id === remoteDeviceId), "Host peer did not pair");
    await waitForApi(`/api/terminal/profiles?execution_device=${encodeURIComponent(remoteDeviceId)}`, value => !!value, "Remote terminal placement unavailable");
  }
  await evaluate(
    client,
    `localStorage.setItem(
      ${JSON.stringify(diagnosticsStorageKey)},
      "1"
    );`,
  );
  await navigate(client, `${baseUrl}/terminal`);
  await waitFor(
    client,
    'document.querySelector(".rua-terminal-page") !== null',
    "terminal page did not render",
  );

  const devicePayload = await evaluate(
    client,
    `(async () => {
      const response = await fetch("/api/sync/devices", { cache: "no-store" });
      if (!response.ok) throw new Error("device list failed");
      return response.json();
    })()`,
  );
  const devices = Array.isArray(devicePayload)
    ? devicePayload
    : devicePayload.devices;
  assert(Array.isArray(devices), "Android device list has an invalid shape");
  assert(
    devices.some((device) => device.is_self),
    "Android device list has no local device",
  );
  const mobileDeviceId = devices.find((device) => device.is_self)?.device_id;
  assert(mobileDeviceId, "Android device list has no local Device id");
  assert(
    devices.some((device) => device.device_id === remoteDeviceId),
    "paired remote device is absent from Android device list",
  );

  const chatRecovery = await assertChatRecovery(client, mobileDeviceId);
  console.log("PASS Chat background recovery");
  await navigate(client, `${baseUrl}/terminal`);
  await waitFor(
    client,
    'document.querySelector(".rua-terminal-page") !== null',
    "terminal page did not render after Chat recovery",
  );

  const localSession = await exercisePlacement(
    client,
    devices,
    "local",
    remoteDeviceId,
  );
  await assertSessionContinuity(client, localSession, "local", remoteDeviceId);
  console.log("PASS local terminal UI and retained session");
  const preRefreshDiagnostics = await readDiagnostics(client);

  await reload(client);
  await waitFor(
    client,
    `document.querySelector(${JSON.stringify(
      activeTerminalSelector,
    )}) !== null &&
      (window.__ruaTerminalDiagnostics ?? []).some((item) =>
        item.event === "attach_complete" &&
        item.session_id === ${JSON.stringify(localSession.sessionId)} &&
        item.placement === "local" &&
        item.lifecycle === "hydrated"
      )`,
    "local terminal did not hydrate after refresh",
  );
  const refreshedDiagnostics = await readDiagnostics(client);
  const refreshedLocal = latest(
    refreshedDiagnostics,
    (item) =>
      item.event === "attach_complete" &&
      item.session_id === localSession.sessionId &&
      item.placement === "local",
  );
  assert(refreshedLocal, "browser refresh did not reattach the local session");
  assert.equal(
    refreshedLocal.lifecycle,
    "hydrated",
    "browser refresh did not hydrate an offscreen replacement",
  );
  const refreshedFocused = await evaluate(
    client,
    `document.activeElement === document.querySelector(${JSON.stringify(
      `${activeTerminalSelector} textarea`,
    )})`,
  );
  assert.equal(
    refreshedFocused,
    false,
    "browser refresh focused the mobile terminal",
  );
  await maskedScreenshot(client, "local-refresh-masked.png");

  const remoteSession = await exercisePlacement(
    client,
    devices,
    "remote",
    remoteDeviceId,
  );
  await assertSessionContinuity(
    client,
    remoteSession,
    "remote",
    remoteDeviceId,
  );
  console.log("PASS remote terminal UI and retained session");
  const backgroundRecovery = await assertBackgroundRecovery(
    client,
    remoteSession,
    remoteDeviceId,
  );

  const diagnostics = safeDiagnostics([
    ...preRefreshDiagnostics,
    ...(await readDiagnostics(client)),
  ]);
  const perf = safeWriterMetrics(diagnostics);
  const summary = {
    local_session_id: localSession.sessionId,
    remote_session_id: remoteSession.sessionId,
    remote_device_id: remoteDeviceId,
    background_recovery: backgroundRecovery,
    chat_recovery: chatRecovery,
    diagnostic_events: diagnostics.length,
    performance_events: perf.length,
    placements: Array.from(
      new Set(diagnostics.map((item) => item.placement).filter(Boolean)),
    ).sort(),
    final_positions: diagnostics
      .filter(
        (item) =>
          item.event === "frame_classified" &&
          item.frame_classification === "apply" &&
          item.cursor_after,
      )
      .map((item) => ({
        session_id: item.session_id,
        placement: item.placement,
        cursor: item.cursor_after,
      }))
      .slice(-4),
  };

  await writeFile(
    path.join(artifactDir, "terminal-diagnostics.json"),
    `${JSON.stringify(diagnostics, null, 2)}\n`,
  );
  await writeFile(
    path.join(artifactDir, "terminal-performance.json"),
    `${JSON.stringify(perf, null, 2)}\n`,
  );
  await writeFile(
    path.join(artifactDir, "terminal-summary.json"),
    `${JSON.stringify(summary, null, 2)}\n`,
  );

  console.log(
    `Android terminal WebView matrix passed: local=${localSession.sessionId} remote=${remoteSession.sessionId}`,
  );
} catch (error) {
  await writeFile(
    path.join(artifactDir, "terminal-failure-responses.json"),
    JSON.stringify(await evaluate(client, "window.__ruaBackgroundResponses ?? []"), null, 2),
  );
  await writeFile(
    path.join(artifactDir, "terminal-failure-diagnostics.json"),
    `${JSON.stringify(safeDiagnostics(await readDiagnostics(client)), null, 2)}\n`,
  );
  throw error;
} finally {
  client.close();
}
