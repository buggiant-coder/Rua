export type ReconciliationKind = "history" | "approval";

export interface HistoryReconciliationWorkerOptions {
  refreshHistory: () => Promise<void>;
  refreshApproval: () => Promise<void>;
  onError?: (kind: ReconciliationKind, error: unknown) => void;
}

export class HistoryReconciliationWorker {
  private readonly options: HistoryReconciliationWorkerOptions;
  private cancelled = false;
  private historyRunning = false;
  private historyDirty = false;
  private approvalRunning = false;
  private approvalDirty = false;

  constructor(options: HistoryReconciliationWorkerOptions) {
    this.options = options;
  }

  requestHistory(): void {
    if (this.cancelled) return;
    this.historyDirty = true;
    void this.drainHistory();
  }

  requestApproval(): void {
    if (this.cancelled) return;
    this.approvalDirty = true;
    void this.drainApproval();
  }

  cancel(): void {
    this.cancelled = true;
    this.historyDirty = false;
    this.approvalDirty = false;
  }

  private async drainHistory(): Promise<void> {
    if (this.historyRunning) return;
    this.historyRunning = true;
    try {
      while (!this.cancelled && this.historyDirty) {
        this.historyDirty = false;
        try {
          await this.options.refreshHistory();
        } catch (error) {
          if (!this.cancelled) this.options.onError?.("history", error);
        }
      }
    } finally {
      this.historyRunning = false;
    }
  }

  private async drainApproval(): Promise<void> {
    if (this.approvalRunning) return;
    this.approvalRunning = true;
    try {
      while (!this.cancelled && this.approvalDirty) {
        this.approvalDirty = false;
        try {
          await this.options.refreshApproval();
        } catch (error) {
          if (!this.cancelled) this.options.onError?.("approval", error);
        }
      }
    } finally {
      this.approvalRunning = false;
    }
  }
}
