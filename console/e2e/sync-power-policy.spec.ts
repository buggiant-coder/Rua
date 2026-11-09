import { test, expect } from "./fixtures";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "node:net";
import type { APIRequestContext } from "@playwright/test";

type JsonObject = Record<string, unknown>;
type BackendHandle = { base: string; stop: () => Promise<void> };

function asObject(value: unknown): JsonObject {
  expect(value).toBeTruthy();
  expect(typeof value).toBe("object");
  expect(Array.isArray(value)).toBe(false);
  return value as JsonObject;
}

function hasDiagnosticsContract(value: JsonObject): boolean {
  return Boolean(
    value.host_profile &&
      value.effective_policy &&
      value.runtime_state &&
      value.background_grace &&
      typeof value.warm_suspended === "boolean" &&
      value.scheduled,
  );
}

async function configureAndReadDiagnostics(
  request: APIRequestContext,
  base: string,
): Promise<JsonObject> {
  const enable = await request.post(`${base}/api/sync/enable`);
  expect(enable.ok()).toBeTruthy();

  const policy = await request.put(`${base}/api/sync/power-policy`, {
    data: {
      mode: "power_saver",
      background_flush_grace_secs: 60,
    },
  });
  expect(policy.ok()).toBeTruthy();

  const before = await request.get(`${base}/api/sync/diagnostics`);
  expect(before.ok()).toBeTruthy();
  return asObject(await before.json());
}

async function chooseBackend(
  request: APIRequestContext,
  baseURL: string | undefined,
): Promise<BackendHandle> {
  const externalBase = baseURL ?? "http://127.0.0.1:8090";
  const externalDiagnostics = await configureAndReadDiagnostics(
    request,
    externalBase,
  );
  if (hasDiagnosticsContract(externalDiagnostics)) {
    return { base: externalBase, stop: async () => {} };
  }

  const local = await startCurrentBackend(request);
  await configureAndReadDiagnostics(request, local.base);
  return local;
}

async function startCurrentBackend(
  request: APIRequestContext,
): Promise<BackendHandle> {
  const repoRoot = fileURLToPath(new URL("../../", import.meta.url));
  const port = await freePort();
  const workingDir = mkdtempSync(join(tmpdir(), "rua-sync-power-policy-"));
  const child = spawn(
    "cargo",
    [
      "run",
      "-p",
      "rua-server",
      "--",
      "--debug",
      "--no-auth",
      "--port",
      String(port),
      "--rua-root",
      workingDir,
      "--console-static-dir",
      "console/dist",
    ],
    {
      cwd: repoRoot,
      env: { ...process.env, RUST_LOG: process.env.RUST_LOG ?? "warn" },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const logs: string[] = [];
  collectLogs(child, logs);
  const base = `http://127.0.0.1:${port}`;

  try {
    await waitForBackend(request, base, logs);
  } catch (error) {
    await stopChild(child);
    throw error;
  }

  return {
    base,
    stop: () => stopChild(child),
  };
}

function collectLogs(
  child: ChildProcessWithoutNullStreams,
  logs: string[],
): void {
  for (const stream of [child.stdout, child.stderr]) {
    stream.on("data", (chunk) => {
      logs.push(chunk.toString());
      if (logs.length > 40) {
        logs.shift();
      }
    });
  }
}

async function waitForBackend(
  request: APIRequestContext,
  base: string,
  logs: string[],
): Promise<void> {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    const response = await request
      .get(`${base}/api/version`, { timeout: 1_000 })
      .catch(() => null);
    if (response?.ok()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `timed out waiting for current rua-server at ${base}\n${logs.join("")}`,
  );
}

async function stopChild(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  child.kill("SIGTERM");
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 5_000)),
  ]);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
  }
}

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") {
        server.close(() => reject(new Error("failed to allocate TCP port")));
        return;
      }
      const port = address.port;
      server.close(() => resolve(port));
    });
  });
}

test.describe("sync power policy diagnostics", () => {
  test("diagnostics expose policy intent and runtime state", async ({
    request,
    baseURL,
  }) => {
    test.setTimeout(180_000);
    const backend = await chooseBackend(request, baseURL);
    try {
      const initial = await configureAndReadDiagnostics(request, backend.base);

      expect(initial.host_profile).toBeTruthy();
      expect(asObject(initial.effective_policy).mode).toBe("power_saver");
      expect(asObject(initial.runtime_state).state).toBeTruthy();
      expect(asObject(initial.background_grace).pending).toBe(false);
      expect(typeof initial.warm_suspended).toBe("boolean");
      expect(asObject(initial.scheduled).allow_background_scheduled_sync).toBe(
        false,
      );
      expect(asObject(initial.inbox)).toBeTruthy();
      expect(asObject(initial.merge)).toBeTruthy();
      expect(asObject(initial.peers)).toBeTruthy();

      const background = await request.post(
        `${backend.base}/api/platform/lifecycle`,
        {
          data: { event: "background" },
        },
      );
      expect(background.ok()).toBeTruthy();

      const afterBackground = await request.get(
        `${backend.base}/api/sync/diagnostics`,
      );
      expect(afterBackground.ok()).toBeTruthy();
      const backgroundBody = asObject(await afterBackground.json());

      if (backgroundBody.host_profile === "mobile-apk") {
        expect(asObject(backgroundBody.runtime_state).state).toBe(
          "warm_suspended",
        );
        expect(asObject(backgroundBody.runtime_state).pending_grace).toBe(true);
        expect(asObject(backgroundBody.background_grace).pending).toBe(true);
        expect(backgroundBody.warm_suspended).toBe(true);

        const foreground = await request.post(
          `${backend.base}/api/platform/lifecycle`,
          {
            data: { event: "foreground" },
          },
        );
        expect(foreground.ok()).toBeTruthy();

        const afterForeground = await request.get(
          `${backend.base}/api/sync/diagnostics`,
        );
        expect(afterForeground.ok()).toBeTruthy();
        const foregroundBody = asObject(await afterForeground.json());
        expect(asObject(foregroundBody.runtime_state).state).toBe("active");
        expect(asObject(foregroundBody.background_grace).pending).toBe(false);
        expect(foregroundBody.warm_suspended).toBe(false);
      } else {
        expect(asObject(backgroundBody.runtime_state).state).toBe("active");
        expect(asObject(backgroundBody.background_grace).pending).toBe(false);
        expect(backgroundBody.warm_suspended).toBe(false);
      }
    } finally {
      await backend.stop();
    }
  });
});
