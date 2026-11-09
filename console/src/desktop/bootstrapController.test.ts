import { describe, expect, it } from "vitest";

import {
  BootstrapController,
  desktopConsoleUrl,
  type RuntimeStatus,
} from "./bootstrapController";

function status(overrides: Partial<RuntimeStatus> = {}): RuntimeStatus {
  return {
    generation: 1,
    port: null,
    error: null,
    terminated: false,
    ...overrides,
  };
}

describe("BootstrapController", () => {
  it("polls readiness and navigates to the Runtime-hosted Console", async () => {
    const statuses = [status(), status({ port: 43123 })];
    const navigations: string[] = [];
    let now = 1000;
    const controller = new BootstrapController({
      getRuntimeStatus: async () => statuses.shift() ?? status({ port: 43123 }),
      restartRuntime: async () => {},
      selectRuaRoot: async () => ({
        status: "cancelled",
        ruaRoot: null,
      }),
      checkReadiness: async () => true,
      navigate: (url) => navigations.push(url),
      now: () => (now += 100),
      sleep: async () => {},
    });

    await controller.start();

    expect(controller.state.phase).toBe("ready");
    expect(navigations).toHaveLength(1);
    expect(navigations[0]).toContain(
      "http://127.0.0.1:43123/console/?desktop=1&_=",
    );
  });

  it("surfaces process startup failures", async () => {
    const controller = new BootstrapController({
      getRuntimeStatus: async () =>
        status({ error: "failed to spawn unified rua Runtime" }),
      restartRuntime: async () => {},
      selectRuaRoot: async () => ({
        status: "cancelled",
        ruaRoot: null,
      }),
      checkReadiness: async () => false,
      navigate: () => {},
      sleep: async () => {},
    });

    await controller.start();

    expect(controller.state).toMatchObject({
      phase: "error",
      error: "failed to spawn unified rua Runtime",
    });
  });

  it("reports readiness timeout", async () => {
    let now = 0;
    const controller = new BootstrapController({
      getRuntimeStatus: async () => status(),
      restartRuntime: async () => {},
      selectRuaRoot: async () => ({
        status: "cancelled",
        ruaRoot: null,
      }),
      checkReadiness: async () => false,
      navigate: () => {},
      now: () => (now += 600),
      sleep: async () => {},
      timeoutMs: 1000,
      formatRuntimeTimeout: (seconds) =>
        `Rua Runtime 未能在 ${seconds} 秒内就绪。`,
    });

    await controller.start();

    expect(controller.state.phase).toBe("timeout");
    expect(controller.state.error).toContain("1 秒内就绪");
  });

  it("restarts after failure and ignores a stale polling result", async () => {
    let resolveFirst:
      | ((value: RuntimeStatus | PromiseLike<RuntimeStatus>) => void)
      | undefined;
    const firstStatus = new Promise<RuntimeStatus>((resolve) => {
      resolveFirst = resolve;
    });
    let statusCalls = 0;
    let restarts = 0;
    const navigations: string[] = [];
    const controller = new BootstrapController({
      getRuntimeStatus: () => {
        statusCalls += 1;
        return statusCalls === 1
          ? firstStatus
          : Promise.resolve(status({ generation: 2, port: 43124 }));
      },
      restartRuntime: async () => {
        restarts += 1;
      },
      selectRuaRoot: async () => ({
        status: "cancelled",
        ruaRoot: null,
      }),
      checkReadiness: async () => true,
      navigate: (url) => navigations.push(url),
      sleep: async () => {},
    });

    const staleRun = controller.start();
    const retryRun = controller.retry();
    resolveFirst?.(status({ port: 41000 }));
    await Promise.all([staleRun, retryRun]);

    expect(restarts).toBe(1);
    expect(navigations).toHaveLength(1);
    expect(navigations[0]).toContain(":43124/");
  });

  it("polls the restarted Runtime after selecting a persistent Rua Root", async () => {
    const navigations: string[] = [];
    let selections = 0;
    const controller = new BootstrapController({
      getRuntimeStatus: async () => status({ generation: 2, port: 43125 }),
      restartRuntime: async () => {},
      selectRuaRoot: async () => {
        selections += 1;
        return {
          status: "selected",
          ruaRoot: "/Users/test/Rua Data",
        };
      },
      checkReadiness: async () => true,
      navigate: (url) => navigations.push(url),
      sleep: async () => {},
    });

    await controller.changeRuaRoot();

    expect(selections).toBe(1);
    expect(controller.state.phase).toBe("ready");
    expect(navigations[0]).toContain(":43125/");
  });
});

describe("desktopConsoleUrl", () => {
  it("adds the Desktop Shell marker and cache buster", () => {
    expect(desktopConsoleUrl("http://127.0.0.1:8765/", 123)).toBe(
      "http://127.0.0.1:8765/console/?desktop=1&_=123",
    );
  });
});
