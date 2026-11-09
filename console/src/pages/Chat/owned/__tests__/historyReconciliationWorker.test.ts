import { describe, expect, it, vi } from "vitest";
import { HistoryReconciliationWorker } from "../historyReconciliationWorker";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
}

describe("HistoryReconciliationWorker", () => {
  it("coalesces requests without blocking independent work", async () => {
    const firstHistory = deferred();
    const refreshHistory = vi
      .fn<() => Promise<void>>()
      .mockReturnValueOnce(firstHistory.promise)
      .mockResolvedValue(undefined);
    const firstApproval = deferred();
    const refreshApproval = vi
      .fn<() => Promise<void>>()
      .mockReturnValueOnce(firstApproval.promise)
      .mockResolvedValue(undefined);
    const worker = new HistoryReconciliationWorker({
      refreshHistory,
      refreshApproval,
    });

    worker.requestHistory();
    worker.requestHistory();
    worker.requestApproval();
    const consumedFrames: string[] = [];
    consumedFrames.push("text-after-refresh-request", "terminal-after-request");
    await flush();
    expect(refreshHistory).toHaveBeenCalledTimes(1);
    expect(refreshApproval).toHaveBeenCalledTimes(1);
    expect(consumedFrames).toEqual([
      "text-after-refresh-request",
      "terminal-after-request",
    ]);

    firstHistory.resolve();
    firstApproval.resolve();
    await flush();
    expect(refreshHistory).toHaveBeenCalledTimes(2);
  });

  it("does not start queued work after cancellation", async () => {
    const pending = deferred();
    const refreshHistory = vi.fn(async () => {
      await pending.promise;
    });
    const worker = new HistoryReconciliationWorker({
      refreshHistory,
      refreshApproval: async () => undefined,
    });
    worker.requestHistory();
    worker.requestHistory();
    worker.cancel();
    pending.resolve();
    await flush();
    expect(refreshHistory).toHaveBeenCalledTimes(1);
  });

  it("reports failure and accepts a later retry", async () => {
    const onError = vi.fn();
    const refreshHistory = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValue(undefined);
    const worker = new HistoryReconciliationWorker({
      refreshHistory,
      refreshApproval: async () => undefined,
      onError,
    });
    worker.requestHistory();
    await flush();
    expect(onError).toHaveBeenCalledOnce();
    worker.requestHistory();
    await flush();
    expect(refreshHistory).toHaveBeenCalledTimes(2);
  });
});
