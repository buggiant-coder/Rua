import type {
  TerminalAttachResponse,
  TerminalSession,
} from "../../api/modules/terminal";
import {
  reduceTerminalStreamFrame,
  type TerminalStreamCursor,
  type TerminalStreamFrame,
} from "../stream";
import { recordTerminalDiagnostic } from "../diagnostics";

export type TerminalViewerRuntimeState =
  | "new"
  | "attaching"
  | "hydrating"
  | "live"
  | "suspended"
  | "resyncing"
  | "disposed";

export type TerminalResyncReason =
  | "stream_gap"
  | "stream_invalid"
  | "transport_failure"
  | "viewer_lag";

export interface TerminalSurfaceOptions {
  cols: number;
  rows: number;
  scrollbackLines: number;
  diagnosticKey?: string;
}

export interface TerminalSurface {
  mount(host: HTMLElement): Promise<void>;
  reveal(): void;
  unmount(host: HTMLElement): void;
  refresh(): void;
  write(data: string | Uint8Array): Promise<void>;
  resize(cols: number, rows: number): void;
  fit(): { cols: number; rows: number } | null;
  setLockedGeometry(locked: boolean): void;
  setScreenReaderMode(enabled: boolean): void;
  setSelectionListener(listener?: (hasSelection: boolean) => void): void;
  getSelection(): string;
  setInputHandler(enabled: boolean, handler?: (data: string) => void): void;
  getViewportMetrics(): {
    baseY: number;
    cols: number;
    rows: number;
    viewportY: number;
  };
  scrollToTop(): void;
  scrollPages(count: number): void;
  scrollToBottom(): void;
  focus(): void;
  dispose(): void;
}

export interface TerminalSurfaceFactory {
  create(options: TerminalSurfaceOptions): TerminalSurface;
}

export interface TerminalActivityConnection {
  close(): void;
}

export interface TerminalActivityTransport {
  open(
    cursor: TerminalStreamCursor,
    handlers: {
      onFrame: (frame: TerminalStreamFrame) => void;
      onError: (error: unknown) => void;
    },
  ): TerminalActivityConnection;
}

export interface TerminalRuntimeLifecycleCounters {
  hydrationCount: number;
  resyncCount: number;
  subscriptionOpenCount: number;
  subscriptionCloseCount: number;
  surfaceCreateCount: number;
  surfaceDisposeCount: number;
}

export interface TerminalWriterMetrics {
  acknowledgedBytes: number;
  batchCount: number;
  highWatermarkBytes: number;
  inFlightBytes: number;
  lowWatermarkBytes: number;
  queuedBytes: number;
  receivedBytes: number;
}

export interface TerminalViewerRuntimeOptions {
  key: string;
  sessionId?: string;
  viewerId?: string;
  placement?: "local" | "remote";
  scrollbackLines: number;
  surfaceFactory: TerminalSurfaceFactory;
  onSession?: (session: TerminalSession) => void;
  onFrame?: (frame: TerminalStreamFrame, cursor: TerminalStreamCursor) => void;
  onInput?: (data: string) => void | Promise<void>;
  onTransportError?: (error: unknown) => void;
  onRecoveryComplete?: (reason: TerminalResyncReason) => void;
  onRecoveryError?: (reason: TerminalResyncReason, error: unknown) => void;
  writerBatchDelayMs?: number;
  writerBatchMaxBytes?: number;
  writerHighWatermarkBytes?: number;
  writerLowWatermarkBytes?: number;
}

interface PendingLiveFrame {
  frame: TerminalStreamFrame;
  byteLength: number;
  resolve: (kind: "apply" | "duplicate") => void;
  reject: (error: unknown) => void;
}

const DEFAULT_WRITER_BATCH_DELAY_MS = 4;
const DEFAULT_WRITER_BATCH_MAX_BYTES = 64 * 1024;
const DEFAULT_WRITER_HIGH_WATERMARK_BYTES = 1024 * 1024;
const DEFAULT_WRITER_LOW_WATERMARK_BYTES = 256 * 1024;

export class TerminalViewerLagError extends Error {
  constructor() {
    super("terminal viewer lag budget exceeded");
    this.name = "TerminalViewerLagError";
  }
}

export class TerminalViewerFrameError extends Error {
  readonly classification: "gap" | "invalid";

  constructor(classification: "gap" | "invalid") {
    super(`terminal viewer received a ${classification} frame`);
    this.name = "TerminalViewerFrameError";
    this.classification = classification;
  }
}

function cursorsEqual(left: TerminalStreamCursor, right: TerminalStreamCursor) {
  return (
    left.next_activity_sequence === right.next_activity_sequence &&
    left.next_output_offset === right.next_output_offset
  );
}

export class TerminalViewerRuntime {
  readonly key: string;

  private readonly options: TerminalViewerRuntimeOptions;
  private stateValue: TerminalViewerRuntimeState = "new";
  private cursorValue: TerminalStreamCursor | null = null;
  private surface: TerminalSurface | null = null;
  private boundHost: HTMLElement | null = null;
  private lockedGeometry: boolean | null = null;
  private disposed = false;
  private inputEnabled = false;
  private inputQueue: string[] = [];
  private inputDraining = false;
  private inputSuppressed = false;
  private onInput?: (data: string) => void | Promise<void>;
  private screenReaderMode = false;
  private selectionListener?: (hasSelection: boolean) => void;
  private activityConnection: TerminalActivityConnection | null = null;
  private activityTransport: TerminalActivityTransport | null = null;
  private activityGeneration = 0;
  private transportFailureReported = false;
  private writerQueue: PendingLiveFrame[] = [];
  private writerTimer: number | null = null;
  private writerDraining = false;
  private writerLagged = false;
  private writerIdleResolvers: Array<() => void> = [];
  private onFrame?: (
    frame: TerminalStreamFrame,
    cursor: TerminalStreamCursor,
  ) => void;
  private onTransportError?: (error: unknown) => void;
  private onRecoveryComplete?: (reason: TerminalResyncReason) => void;
  private onRecoveryError?: (
    reason: TerminalResyncReason,
    error: unknown,
  ) => void;
  private requestAttachment?: (
    reason: TerminalResyncReason,
  ) => Promise<TerminalAttachResponse>;
  private resyncPromise: Promise<void> | null = null;
  private initialAttachPromise: Promise<void> | null = null;
  private documentSuspended = false;
  private readonly writerBatchDelayMs: number;
  private readonly writerBatchMaxBytes: number;
  private readonly writerMetricsValue: TerminalWriterMetrics;
  private readonly counters: TerminalRuntimeLifecycleCounters = {
    hydrationCount: 0,
    resyncCount: 0,
    subscriptionOpenCount: 0,
    subscriptionCloseCount: 0,
    surfaceCreateCount: 0,
    surfaceDisposeCount: 0,
  };

  constructor(options: TerminalViewerRuntimeOptions) {
    this.options = options;
    this.key = options.key;
    this.onFrame = options.onFrame;
    this.onInput = options.onInput;
    this.onTransportError = options.onTransportError;
    this.onRecoveryComplete = options.onRecoveryComplete;
    this.onRecoveryError = options.onRecoveryError;
    this.writerBatchDelayMs =
      options.writerBatchDelayMs ?? DEFAULT_WRITER_BATCH_DELAY_MS;
    this.writerBatchMaxBytes =
      options.writerBatchMaxBytes ?? DEFAULT_WRITER_BATCH_MAX_BYTES;
    const highWatermarkBytes =
      options.writerHighWatermarkBytes ?? DEFAULT_WRITER_HIGH_WATERMARK_BYTES;
    const lowWatermarkBytes =
      options.writerLowWatermarkBytes ?? DEFAULT_WRITER_LOW_WATERMARK_BYTES;
    if (lowWatermarkBytes >= highWatermarkBytes) {
      throw new Error(
        "terminal writer low watermark must be below its high watermark",
      );
    }
    this.writerMetricsValue = {
      acknowledgedBytes: 0,
      batchCount: 0,
      highWatermarkBytes,
      inFlightBytes: 0,
      lowWatermarkBytes,
      queuedBytes: 0,
      receivedBytes: 0,
    };
  }

  get state() {
    return this.stateValue;
  }

  get cursor() {
    return this.cursorValue ? { ...this.cursorValue } : null;
  }

  get lifecycleCounters(): TerminalRuntimeLifecycleCounters {
    return { ...this.counters };
  }

  get writerMetrics(): TerminalWriterMetrics {
    return { ...this.writerMetricsValue };
  }

  setEventHandlers(handlers: {
    onFrame?: (
      frame: TerminalStreamFrame,
      cursor: TerminalStreamCursor,
    ) => void;
    onTransportError?: (error: unknown) => void;
    onRecoveryComplete?: (reason: TerminalResyncReason) => void;
    onRecoveryError?: (reason: TerminalResyncReason, error: unknown) => void;
  }) {
    this.onFrame = handlers.onFrame;
    this.onTransportError = handlers.onTransportError;
    this.onRecoveryComplete = handlers.onRecoveryComplete;
    this.onRecoveryError = handlers.onRecoveryError;
  }

  setRecoveryProvider(
    provider: (reason: TerminalResyncReason) => Promise<TerminalAttachResponse>,
  ) {
    this.requestAttachment = provider;
  }

  ensureAttached(
    requestAttachment: () => Promise<TerminalAttachResponse>,
    transport: TerminalActivityTransport,
  ) {
    this.ensureActive();
    this.activityTransport = transport;
    if (this.stateValue === "live" || this.stateValue === "suspended") {
      return Promise.resolve();
    }
    if (this.stateValue === "resyncing") {
      return this.resyncPromise ?? Promise.resolve();
    }
    if (this.stateValue === "attaching" || this.stateValue === "hydrating") {
      return this.initialAttachPromise ?? Promise.resolve();
    }
    if (this.initialAttachPromise) return this.initialAttachPromise;
    this.transition("attaching");
    recordTerminalDiagnostic({
      event: "attach_start",
      ...this.diagnosticIdentity(),
    });
    const attach = Promise.resolve()
      .then(requestAttachment)
      .then((attachment) => this.hydrate(attachment))
      .then(() => {
        recordTerminalDiagnostic({
          event: "attach_complete",
          ...this.diagnosticIdentity(),
          cursor_after: this.cursorValue ?? undefined,
          lifecycle: "hydrated",
        });
        if (this.documentSuspended) {
          this.transition("suspended", "document_hidden");
        } else {
          this.startActivity(transport);
        }
      })
      .catch((error) => {
        if (!this.disposed) this.transition("new", "initial_attach_failed");
        throw error;
      })
      .finally(() => {
        if (this.initialAttachPromise === attach) {
          this.initialAttachPromise = null;
        }
      });
    this.initialAttachPromise = attach;
    return attach;
  }

  async bind(host: HTMLElement) {
    this.ensureActive();
    this.boundHost = host;
    if (this.surface) {
      await this.surface.mount(host);
      this.surface.reveal();
    }
  }

  unbind(host: HTMLElement) {
    if (this.boundHost !== host) return;
    this.surface?.unmount(host);
    this.boundHost = null;
  }

  setInputEnabled(enabled: boolean, onInput = this.options.onInput) {
    if (!enabled) this.inputQueue = [];
    this.inputEnabled = enabled;
    this.onInput = onInput;
    this.syncInputHandler();
  }

  focus() {
    this.surface?.focus();
  }

  refresh() {
    this.surface?.refresh();
  }

  scrollToTop() {
    this.surface?.scrollToTop();
  }

  scrollPages(count: number) {
    this.surface?.scrollPages(count);
  }

  scrollToBottom() {
    this.surface?.scrollToBottom();
  }

  resize(cols: number, rows: number) {
    this.surface?.resize(cols, rows);
  }

  fit() {
    return this.surface?.fit() ?? null;
  }

  setLockedGeometry(locked: boolean) {
    this.lockedGeometry = locked;
    this.surface?.setLockedGeometry(locked);
  }

  setScreenReaderMode(enabled: boolean) {
    this.screenReaderMode = enabled;
    this.surface?.setScreenReaderMode(enabled);
  }

  setSelectionListener(listener?: (hasSelection: boolean) => void) {
    this.selectionListener = listener;
    this.surface?.setSelectionListener(listener);
  }

  getSelection() {
    return this.surface?.getSelection() ?? "";
  }

  startActivity(transport: TerminalActivityTransport) {
    this.ensureActive();
    if (this.activityConnection) return;
    this.activityTransport = transport;
    if (this.documentSuspended) {
      this.transition("suspended", "document_hidden");
      return;
    }
    if (this.stateValue !== "live" || !this.cursorValue) {
      throw new Error("terminal viewer runtime cannot subscribe before live");
    }
    this.transportFailureReported = false;
    const generation = this.activityGeneration + 1;
    this.activityGeneration = generation;
    const fail = (error: unknown) => {
      if (generation !== this.activityGeneration) return;
      if (this.transportFailureReported) return;
      this.transportFailureReported = true;
      this.stopActivity();
      this.onTransportError?.(error);
      const reason = this.resyncReason(error);
      void this.requestResync(reason).catch(() => undefined);
    };
    this.activityConnection = transport.open(this.cursorValue, {
      onFrame: (frame) => {
        if (generation !== this.activityGeneration) return;
        void this.applyLiveFrame(frame)
          .then((kind) => {
            if (kind === "apply" && this.cursorValue) {
              this.onFrame?.(frame, { ...this.cursorValue });
            }
          })
          .catch(fail);
      },
      onError: fail,
    });
    this.counters.subscriptionOpenCount += 1;
  }

  replaceActivityTransport(transport: TerminalActivityTransport) {
    this.ensureActive();
    if (this.activityTransport === transport) return;
    const shouldRestart =
      this.activityConnection !== null &&
      this.stateValue === "live" &&
      !this.documentSuspended;
    this.activityTransport = transport;
    if (!shouldRestart) return;
    this.stopActivity();
    this.startActivity(transport);
  }

  stopActivity() {
    if (!this.activityConnection) return;
    this.activityGeneration += 1;
    this.activityConnection.close();
    this.activityConnection = null;
    this.counters.subscriptionCloseCount += 1;
  }

  suspend() {
    if (this.disposed) return;
    this.documentSuspended = true;
    this.stopActivity();
    if (this.stateValue === "live") {
      this.transition("suspended", "document_hidden");
    }
    recordTerminalDiagnostic({
      event: "visibility_change",
      ...this.diagnosticIdentity(),
      visibility_scope: "document",
      visible: false,
    });
  }

  resume() {
    if (this.disposed) {
      return Promise.resolve();
    }
    this.documentSuspended = false;
    if (this.stateValue !== "suspended") return Promise.resolve();
    recordTerminalDiagnostic({
      event: "visibility_change",
      ...this.diagnosticIdentity(),
      visibility_scope: "document",
      visible: true,
    });
    this.transition("live", "document_resume");
    if (this.activityTransport) {
      this.startActivity(this.activityTransport);
    }
    return Promise.resolve();
  }

  requestResync(reason: TerminalResyncReason) {
    this.ensureActive();
    if (this.resyncPromise) return this.resyncPromise;
    this.setInputSuppressed(true);
    if (!this.requestAttachment) {
      return Promise.reject(
        new Error("terminal viewer runtime has no recovery provider"),
      );
    }
    this.stopActivity();
    this.transition("resyncing", reason);
    this.counters.resyncCount += 1;
    recordTerminalDiagnostic({
      event: "recovery_start",
      ...this.diagnosticIdentity(),
      cursor_before: this.cursorValue ?? undefined,
      recovery_reason: reason,
    });
    const recovery = (async () => {
      await this.waitForWriterIdle();
      const attachment = await this.requestAttachment!(reason);
      await this.hydrate(attachment);
      if (this.documentSuspended) {
        this.transition("suspended", "document_hidden");
      } else {
        if (this.activityTransport) {
          this.startActivity(this.activityTransport);
        }
        this.setInputSuppressed(false);
      }
      this.onRecoveryComplete?.(reason);
      recordTerminalDiagnostic({
        event: "recovery_complete",
        ...this.diagnosticIdentity(),
        cursor_after: this.cursorValue ?? undefined,
        recovery_reason: reason,
      });
    })()
      .catch((error) => {
        this.transition(this.surface ? "live" : "new", "recovery_failed");
        this.onRecoveryError?.(reason, error);
        throw error;
      })
      .finally(() => {
        if (this.resyncPromise === recovery) {
          this.resyncPromise = null;
        }
      });
    this.resyncPromise = recovery;
    return recovery;
  }

  async hydrate(attachment: TerminalAttachResponse) {
    this.ensureActive();
    this.transition("attaching");
    if (!attachment.snapshot) {
      this.transition("new", "snapshot_missing");
      throw new Error("terminal attach response is missing its snapshot");
    }

    this.transition("hydrating");
    this.writerLagged = false;
    this.counters.hydrationCount += 1;
    const replacement = this.options.surfaceFactory.create({
      cols: attachment.snapshot.cols,
      rows: attachment.snapshot.rows,
      scrollbackLines: this.options.scrollbackLines,
      diagnosticKey: this.key,
    });
    this.counters.surfaceCreateCount += 1;
    recordTerminalDiagnostic({
      event: "surface_change",
      ...this.diagnosticIdentity(),
      surface_action: "create",
      snapshot_cursor: attachment.snapshot.cursor,
      live_cursor: attachment.live_cursor,
    });
    replacement.setInputHandler(
      this.inputEnabled && !this.inputSuppressed,
      (data) => this.enqueueInput(data),
    );
    replacement.setScreenReaderMode(this.screenReaderMode);
    if (this.lockedGeometry !== null) {
      replacement.setLockedGeometry(this.lockedGeometry);
    }
    replacement.setSelectionListener(this.selectionListener);
    let cursor = { ...attachment.snapshot.cursor };

    try {
      if (this.boundHost) {
        await replacement.mount(this.boundHost);
      }
      if (attachment.snapshot.data) {
        await replacement.write(attachment.snapshot.data);
      }
      for (const frame of attachment.tail) {
        cursor = (await this.applyFrame(replacement, cursor, frame)).cursor;
      }
      if (!cursorsEqual(cursor, attachment.live_cursor)) {
        throw new Error(
          "terminal attach tail did not reach the authority live cursor",
        );
      }
    } catch (error) {
      replacement.dispose();
      this.counters.surfaceDisposeCount += 1;
      recordTerminalDiagnostic({
        event: "surface_change",
        ...this.diagnosticIdentity(),
        surface_action: "dispose",
      });
      this.transition(this.surface ? "live" : "new", "hydration_failed");
      throw error;
    }

    const previous = this.surface;
    this.surface = replacement;
    this.cursorValue = { ...attachment.live_cursor };
    this.options.onSession?.(attachment.session);
    this.transition("live");
    if (this.boundHost) {
      replacement.reveal();
      recordTerminalDiagnostic({
        event: "surface_change",
        ...this.diagnosticIdentity(),
        surface_action: previous ? "swap" : "reveal",
        cursor_after: this.cursorValue,
      });
    }
    if (previous) {
      previous.dispose();
      this.counters.surfaceDisposeCount += 1;
      recordTerminalDiagnostic({
        event: "surface_change",
        ...this.diagnosticIdentity(),
        surface_action: "dispose",
      });
    }
  }

  applyLiveFrame(frame: TerminalStreamFrame) {
    this.ensureActive();
    if (this.stateValue !== "live" || !this.surface || !this.cursorValue) {
      return Promise.reject(new Error("terminal viewer runtime is not live"));
    }
    if (this.writerLagged) {
      return Promise.reject(new TerminalViewerLagError());
    }
    const byteLength = this.frameByteLength(frame);
    this.writerMetricsValue.receivedBytes += byteLength;
    const projectedBytes =
      this.writerMetricsValue.queuedBytes +
      this.writerMetricsValue.inFlightBytes +
      byteLength;
    if (projectedBytes > this.writerMetricsValue.highWatermarkBytes) {
      const error = new TerminalViewerLagError();
      this.writerLagged = true;
      this.rejectQueuedWriterFrames(error);
      recordTerminalDiagnostic({
        event: "writer_enqueue",
        ...this.diagnosticIdentity(),
        queue_depth: 0,
        queued_bytes: 0,
        in_flight_bytes: this.writerMetricsValue.inFlightBytes,
        acknowledged_bytes: this.writerMetricsValue.acknowledgedBytes,
        high_watermark_bytes: this.writerMetricsValue.highWatermarkBytes,
        recovery_reason: "viewer_lag_high_watermark",
      });
      return Promise.reject(error);
    }
    this.writerMetricsValue.queuedBytes += byteLength;
    const pending = new Promise<"apply" | "duplicate">((resolve, reject) => {
      this.writerQueue.push({ frame, byteLength, resolve, reject });
    });
    recordTerminalDiagnostic({
      event: "writer_enqueue",
      ...this.diagnosticIdentity(),
      queue_depth: this.writerQueue.length,
      queued_bytes: this.writerMetricsValue.queuedBytes,
      in_flight_bytes: this.writerMetricsValue.inFlightBytes,
      acknowledged_bytes: this.writerMetricsValue.acknowledgedBytes,
      high_watermark_bytes: this.writerMetricsValue.highWatermarkBytes,
    });
    this.scheduleWriterDrain();
    return pending;
  }

  waitForWriterIdle() {
    if (
      !this.writerDraining &&
      this.writerQueue.length === 0 &&
      this.writerMetricsValue.inFlightBytes === 0
    ) {
      return Promise.resolve();
    }
    return new Promise<void>((resolve) => {
      this.writerIdleResolvers.push(resolve);
    });
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.inputQueue = [];
    this.resyncPromise = null;
    this.stopActivity();
    if (this.writerTimer !== null) {
      window.clearTimeout(this.writerTimer);
      this.writerTimer = null;
    }
    this.rejectQueuedWriterFrames(
      new Error("terminal viewer runtime is disposed"),
    );
    if (this.surface) {
      this.surface.dispose();
      this.counters.surfaceDisposeCount += 1;
    }
    this.surface = null;
    this.boundHost = null;
    this.cursorValue = null;
    this.transition("disposed");
  }

  private async applyFrame(
    surface: TerminalSurface,
    cursor: TerminalStreamCursor,
    frame: TerminalStreamFrame,
  ) {
    const reduction = reduceTerminalStreamFrame(cursor, frame);
    if (reduction.kind === "duplicate") {
      return { cursor, kind: "duplicate" as const };
    }
    if (reduction.kind !== "apply") {
      throw new Error(
        `terminal frame is ${reduction.kind} at activity ${frame.activity_sequence}`,
      );
    }
    if (reduction.data) {
      await surface.write(reduction.data);
    } else if (frame.type === "pty_geometry_changed") {
      surface.resize(frame.geometry.cols, frame.geometry.rows);
    }
    return { cursor: reduction.cursor, kind: "apply" as const };
  }

  private frameByteLength(frame: TerminalStreamFrame) {
    return frame.type === "output"
      ? Math.max(0, frame.end_offset - frame.start_offset)
      : 32;
  }

  private scheduleWriterDrain() {
    if (this.writerDraining || this.writerTimer !== null) return;
    this.writerTimer = window.setTimeout(() => {
      this.writerTimer = null;
      void this.drainWriter();
    }, this.writerBatchDelayMs);
  }

  private async drainWriter() {
    if (this.writerDraining || this.writerLagged) return;
    this.writerDraining = true;
    try {
      while (this.writerQueue.length > 0 && !this.writerLagged) {
        if (!this.surface || !this.cursorValue || this.stateValue !== "live") {
          this.rejectQueuedWriterFrames(
            new Error("terminal viewer runtime is not live"),
          );
          return;
        }
        const first = this.shiftWriterFrame();
        const firstReduction = reduceTerminalStreamFrame(
          this.cursorValue,
          first.frame,
        );
        if (firstReduction.kind === "duplicate") {
          first.resolve("duplicate");
          continue;
        }
        if (firstReduction.kind !== "apply") {
          const error = new TerminalViewerFrameError(firstReduction.kind);
          first.reject(error);
          this.rejectQueuedWriterFrames(error);
          return;
        }
        if (!firstReduction.data) {
          if (first.frame.type === "pty_geometry_changed") {
            this.surface.resize(
              first.frame.geometry.cols,
              first.frame.geometry.rows,
            );
          }
          this.cursorValue = firstReduction.cursor;
          first.resolve("apply");
          continue;
        }

        const batch = [first];
        const chunks = [firstReduction.data];
        let batchBytes = firstReduction.data.byteLength;
        let batchCursor = firstReduction.cursor;
        while (this.writerQueue.length > 0) {
          const next = this.writerQueue[0];
          if (
            next.frame.type !== "output" ||
            batchBytes + next.byteLength > this.writerBatchMaxBytes
          ) {
            break;
          }
          const reduction = reduceTerminalStreamFrame(batchCursor, next.frame);
          if (reduction.kind !== "apply" || !reduction.data) break;
          batch.push(this.shiftWriterFrame());
          chunks.push(reduction.data);
          batchBytes += reduction.data.byteLength;
          batchCursor = reduction.cursor;
        }

        this.writerMetricsValue.inFlightBytes = batchBytes;
        try {
          await this.surface.write(this.combineChunks(chunks, batchBytes));
        } catch (error) {
          for (const item of batch) item.reject(error);
          this.rejectQueuedWriterFrames(error);
          return;
        } finally {
          this.writerMetricsValue.inFlightBytes = 0;
        }
        this.cursorValue = batchCursor;
        this.writerMetricsValue.acknowledgedBytes += batchBytes;
        this.writerMetricsValue.batchCount += 1;
        const viewport = this.surface.getViewportMetrics();
        recordTerminalDiagnostic({
          event: "writer_complete",
          ...this.diagnosticIdentity(),
          queue_depth: this.writerQueue.length,
          queued_bytes: this.writerMetricsValue.queuedBytes,
          in_flight_bytes: 0,
          acknowledged_bytes: this.writerMetricsValue.acknowledgedBytes,
          batch_bytes: batchBytes,
          high_watermark_bytes: this.writerMetricsValue.highWatermarkBytes,
          write_callback_completed: true,
          viewport: {
            cols: viewport.cols,
            rows: viewport.rows,
            base_y: viewport.baseY,
            viewport_y: viewport.viewportY,
            bottom_offset: Math.max(0, viewport.baseY - viewport.viewportY),
            following_output: viewport.baseY === viewport.viewportY,
          },
        });
        for (const item of batch) item.resolve("apply");
      }
    } finally {
      this.writerDraining = false;
      if (this.writerQueue.length > 0 && !this.writerLagged) {
        this.scheduleWriterDrain();
      }
      this.resolveWriterIdle();
    }
  }

  private shiftWriterFrame() {
    const item = this.writerQueue.shift();
    if (!item) throw new Error("terminal writer queue is empty");
    this.writerMetricsValue.queuedBytes = Math.max(
      0,
      this.writerMetricsValue.queuedBytes - item.byteLength,
    );
    return item;
  }

  private rejectQueuedWriterFrames(error: unknown) {
    if (this.writerTimer !== null) {
      window.clearTimeout(this.writerTimer);
      this.writerTimer = null;
    }
    const queued = this.writerQueue.splice(0);
    this.writerMetricsValue.queuedBytes = 0;
    for (const item of queued) item.reject(error);
    this.resolveWriterIdle();
  }

  private combineChunks(chunks: Uint8Array[], byteLength: number) {
    if (chunks.length === 1) return chunks[0];
    const combined = new Uint8Array(byteLength);
    let offset = 0;
    for (const chunk of chunks) {
      combined.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return combined;
  }

  private resolveWriterIdle() {
    if (
      this.writerDraining ||
      this.writerQueue.length > 0 ||
      this.writerMetricsValue.inFlightBytes > 0
    ) {
      return;
    }
    const resolvers = this.writerIdleResolvers.splice(0);
    for (const resolve of resolvers) resolve();
  }

  private resyncReason(error: unknown): TerminalResyncReason {
    if (error instanceof TerminalViewerLagError) return "viewer_lag";
    if (error instanceof TerminalViewerFrameError) {
      return error.classification === "gap" ? "stream_gap" : "stream_invalid";
    }
    return "transport_failure";
  }

  private setInputSuppressed(suppressed: boolean) {
    if (suppressed) this.inputQueue = [];
    this.inputSuppressed = suppressed;
    this.syncInputHandler();
  }

  private enqueueInput(data: string) {
    if (this.disposed || !this.inputEnabled || this.inputSuppressed) return;
    this.inputQueue.push(data);
    void this.drainInput();
  }

  private async drainInput() {
    if (this.inputDraining) return;
    this.inputDraining = true;
    try {
      while (this.inputQueue.length > 0) {
        const data = this.inputQueue.shift()!;
        await this.onInput?.(data);
      }
    } catch (error) {
      // A failed request may already have reached the shell. Never replay it,
      // or send the remainder of a partially delivered command automatically.
      this.inputQueue = [];
      if (!this.disposed) this.onTransportError?.(error);
    } finally {
      this.inputDraining = false;
    }
  }

  private syncInputHandler() {
    this.surface?.setInputHandler(
      this.inputEnabled && !this.inputSuppressed,
      (data) => this.enqueueInput(data),
    );
  }

  private ensureActive() {
    if (this.disposed) {
      throw new Error("terminal viewer runtime is disposed");
    }
  }

  private transition(
    next: TerminalViewerRuntimeState,
    recoveryReason?: string,
  ) {
    if (this.stateValue === next) return;
    const previous = this.stateValue;
    this.stateValue = next;
    recordTerminalDiagnostic({
      event: "runtime_state",
      ...this.diagnosticIdentity(),
      state_before: previous,
      state_after: next,
      cursor_after: this.cursorValue ?? undefined,
      recovery_reason: recoveryReason,
    });
  }

  private diagnosticIdentity() {
    return {
      session_key: this.key,
      session_id: this.options.sessionId,
      viewer_id: this.options.viewerId,
      placement: this.options.placement,
    };
  }
}
