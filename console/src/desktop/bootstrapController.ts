export type BootstrapPhase = "starting" | "ready" | "error" | "timeout";

export interface RuntimeStatus {
  generation: number;
  port: number | null;
  error: string | null;
  terminated: boolean;
}

export interface BootstrapState {
  phase: BootstrapPhase;
  elapsedSeconds: number;
  error: string;
}

export interface BootstrapDependencies {
  getRuntimeStatus: () => Promise<RuntimeStatus>;
  restartRuntime: () => Promise<void>;
  selectRuaRoot: () => Promise<RuaRootSelection>;
  checkReadiness: (baseUrl: string, signal: AbortSignal) => Promise<boolean>;
  navigate: (url: string) => void;
  now?: () => number;
  sleep?: (milliseconds: number) => Promise<void>;
  pollIntervalMs?: number;
  timeoutMs?: number;
  formatRuntimeTimeout?: (seconds: number) => string;
}

export interface RuaRootSelection {
  status: "selected" | "cancelled";
  ruaRoot: string | null;
}

const DEFAULT_POLL_INTERVAL_MS = 500;
const DEFAULT_TIMEOUT_MS = 120_000;

export class BootstrapController {
  private readonly dependencies: Required<
    Pick<
      BootstrapDependencies,
      "now" | "sleep" | "pollIntervalMs" | "timeoutMs" | "formatRuntimeTimeout"
    >
  > &
    Omit<
      BootstrapDependencies,
      "now" | "sleep" | "pollIntervalMs" | "timeoutMs" | "formatRuntimeTimeout"
    >;
  private runId = 0;
  private requestAbort: AbortController | null = null;
  private listeners = new Set<(state: BootstrapState) => void>();
  private currentState: BootstrapState = {
    phase: "starting",
    elapsedSeconds: 0,
    error: "",
  };

  constructor(dependencies: BootstrapDependencies) {
    this.dependencies = {
      ...dependencies,
      now: dependencies.now ?? Date.now,
      sleep:
        dependencies.sleep ??
        ((milliseconds) =>
          new Promise((resolve) => window.setTimeout(resolve, milliseconds))),
      pollIntervalMs: dependencies.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS,
      timeoutMs: dependencies.timeoutMs ?? DEFAULT_TIMEOUT_MS,
      formatRuntimeTimeout:
        dependencies.formatRuntimeTimeout ??
        ((seconds) =>
          `Rua Runtime did not become ready within ${seconds} seconds.`),
    };
  }

  get state(): BootstrapState {
    return { ...this.currentState };
  }

  subscribe(listener: (state: BootstrapState) => void): () => void {
    this.listeners.add(listener);
    listener(this.state);
    return () => this.listeners.delete(listener);
  }

  start(): Promise<void> {
    return this.run(false);
  }

  retry(): Promise<void> {
    return this.run(true);
  }

  async changeRuaRoot(): Promise<void> {
    this.stop();
    try {
      const selection = await this.dependencies.selectRuaRoot();
      if (selection.status === "selected") {
        await this.run(false);
      }
    } catch (error) {
      this.fail("error", errorMessage(error));
    }
  }

  stop(): void {
    this.runId += 1;
    this.requestAbort?.abort();
    this.requestAbort = null;
  }

  private async run(restart: boolean): Promise<void> {
    this.stop();
    const runId = this.runId;
    this.update({ phase: "starting", elapsedSeconds: 0, error: "" });

    if (restart) {
      try {
        await this.dependencies.restartRuntime();
      } catch (error) {
        if (this.isCurrent(runId)) {
          this.fail("error", errorMessage(error));
        }
        return;
      }
    }

    if (!this.isCurrent(runId)) return;
    const startedAt = this.dependencies.now();

    while (this.isCurrent(runId)) {
      let status: RuntimeStatus;
      try {
        status = await this.dependencies.getRuntimeStatus();
      } catch (error) {
        if (this.isCurrent(runId)) {
          this.fail("error", errorMessage(error));
        }
        return;
      }
      if (!this.isCurrent(runId)) return;

      if (status.error) {
        this.fail("error", status.error);
        return;
      }

      if (status.port) {
        const baseUrl = `http://127.0.0.1:${status.port}`;
        this.requestAbort = new AbortController();
        const ready = await this.dependencies
          .checkReadiness(baseUrl, this.requestAbort.signal)
          .catch(() => false);
        this.requestAbort = null;
        if (!this.isCurrent(runId)) return;
        if (ready) {
          this.update({
            phase: "ready",
            elapsedSeconds: this.elapsedSeconds(startedAt),
            error: "",
          });
          this.dependencies.navigate(
            desktopConsoleUrl(baseUrl, this.dependencies.now()),
          );
          return;
        }
      }

      const elapsedMs = this.dependencies.now() - startedAt;
      if (elapsedMs >= this.dependencies.timeoutMs) {
        this.fail(
          "timeout",
          this.dependencies.formatRuntimeTimeout(
            Math.ceil(this.dependencies.timeoutMs / 1000),
          ),
        );
        return;
      }

      this.update({
        phase: "starting",
        elapsedSeconds: Math.floor(elapsedMs / 1000),
        error: "",
      });
      await this.dependencies.sleep(this.dependencies.pollIntervalMs);
    }
  }

  private isCurrent(runId: number): boolean {
    return this.runId === runId;
  }

  private elapsedSeconds(startedAt: number): number {
    return Math.floor((this.dependencies.now() - startedAt) / 1000);
  }

  private fail(phase: "error" | "timeout", error: string): void {
    this.update({
      phase,
      elapsedSeconds: this.currentState.elapsedSeconds,
      error,
    });
  }

  private update(state: BootstrapState): void {
    this.currentState = state;
    for (const listener of this.listeners) {
      listener(this.state);
    }
  }
}

export function desktopConsoleUrl(
  baseUrl: string,
  cacheBuster: number,
): string {
  return `${baseUrl.replace(/\/+$/, "")}/console/?desktop=1&_=${cacheBuster}`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
