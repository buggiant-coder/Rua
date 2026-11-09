import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { terminalApi } from "./terminal";

describe("terminalApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses root terminal session routes with explicit execution devices", async () => {
    await terminalApi.listSessions({ kind: "local" });
    await terminalApi.listSessions({ kind: "remote", device_id: "device-b" });
    await terminalApi.openSession({
      execution_device: { kind: "remote", device_id: "device-b" },
      browser_viewer_id: "browser-a",
      cwd: "/tmp/work",
    });
    await terminalApi.attachSession("session/1", "browser-b", 4096, {
      kind: "remote",
      device_id: "device-b",
    });
    await terminalApi.detachSession("session/1", "browser-b", {
      kind: "remote",
      device_id: "device-b",
    });
    await terminalApi.takeInputLease("session/1", "browser-b", {
      kind: "remote",
      device_id: "device-b",
    });
    await terminalApi.adaptPtyGeometry("session/1", "browser-b", 100, 30, {
      kind: "remote",
      device_id: "device-b",
    });
    await terminalApi.updatePalette(
      "session/1",
      "browser-b",
      {
        foreground: "#101418",
        background: "#f8f9fa",
        cursor: "#228be6",
      },
      {
        kind: "remote",
        device_id: "device-b",
      },
    );
    await terminalApi.renameSession("session/1", "Deploy shell", {
      kind: "remote",
      device_id: "device-b",
    });
    await terminalApi.sendInput("session/1", "browser-b", "echo hello\n", {
      kind: "remote",
      device_id: "device-b",
    });
    await terminalApi.killSession("session/1", "browser-b", {
      kind: "remote",
      device_id: "device-b",
    });

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/terminal/sessions?execution_device=local",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/terminal/sessions?execution_device=device-b",
    );
    expect(request).toHaveBeenNthCalledWith(3, "/terminal/sessions", {
      method: "POST",
      body: JSON.stringify({
        execution_device: { kind: "remote", device_id: "device-b" },
        browser_viewer_id: "browser-a",
        cwd: "/tmp/work",
      }),
    });
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/terminal/sessions/session%2F1/attach",
      {
        method: "POST",
        body: JSON.stringify({
          browser_viewer_id: "browser-b",
          execution_device: { kind: "remote", device_id: "device-b" },
          scrollback_lines: 4096,
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/terminal/sessions/session%2F1/detach",
      {
        method: "POST",
        body: JSON.stringify({
          browser_viewer_id: "browser-b",
          execution_device: { kind: "remote", device_id: "device-b" },
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/terminal/sessions/session%2F1/take-input-lease",
      {
        method: "POST",
        body: JSON.stringify({
          browser_viewer_id: "browser-b",
          execution_device: { kind: "remote", device_id: "device-b" },
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      7,
      "/terminal/sessions/session%2F1/adapt-pty-geometry",
      {
        method: "POST",
        body: JSON.stringify({
          browser_viewer_id: "browser-b",
          cols: 100,
          rows: 30,
          execution_device: { kind: "remote", device_id: "device-b" },
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      8,
      "/terminal/sessions/session%2F1/palette",
      {
        method: "POST",
        body: JSON.stringify({
          browser_viewer_id: "browser-b",
          palette: {
            foreground: "#101418",
            background: "#f8f9fa",
            cursor: "#228be6",
          },
          execution_device: { kind: "remote", device_id: "device-b" },
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      9,
      "/terminal/sessions/session%2F1/rename",
      {
        method: "POST",
        body: JSON.stringify({
          display_name: "Deploy shell",
          execution_device: { kind: "remote", device_id: "device-b" },
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      10,
      "/terminal/sessions/session%2F1/input",
      {
        method: "POST",
        body: JSON.stringify({
          browser_viewer_id: "browser-b",
          data: "echo hello\n",
          execution_device: { kind: "remote", device_id: "device-b" },
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      11,
      "/terminal/sessions/session%2F1?browser_viewer_id=browser-b&execution_device=device-b",
      { method: "DELETE" },
    );
  });
});
