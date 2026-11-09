import {
  TerminalViewerRuntime,
  type TerminalViewerRuntimeOptions,
} from "./viewerRuntime";

export class TerminalRuntimeRegistry {
  private readonly runtimes = new Map<string, TerminalViewerRuntime>();

  get(key: string) {
    return this.runtimes.get(key);
  }

  getOrCreate(options: TerminalViewerRuntimeOptions) {
    const existing = this.runtimes.get(options.key);
    if (existing) return existing;
    const runtime = new TerminalViewerRuntime(options);
    this.runtimes.set(options.key, runtime);
    return runtime;
  }

  dispose(key: string) {
    const runtime = this.runtimes.get(key);
    if (!runtime) return;
    this.runtimes.delete(key);
    runtime.dispose();
  }

  disposeAll() {
    for (const runtime of this.runtimes.values()) runtime.dispose();
    this.runtimes.clear();
  }
}

export const terminalRuntimeRegistry = new TerminalRuntimeRegistry();
