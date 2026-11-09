import { expect, it, vi } from "vitest";
import { withTransferSlot } from "./transferAdmission";

it("holds only two HTTP transfers through body consumption and removes cancelled queued work", async () => {
  const releases: Array<() => void> = [];
  const run = vi.fn(
    () => new Promise<void>((resolve) => releases.push(resolve)),
  );
  const first = withTransferSlot(run);
  const second = withTransferSlot(run);
  const cancelled = new AbortController();
  const third = withTransferSlot(run, cancelled.signal).catch(
    (error: Error) => error,
  );
  const fourth = withTransferSlot(run);
  expect(run).toHaveBeenCalledTimes(2);
  cancelled.abort();
  expect((await third).name).toBe("AbortError");
  releases[0]();
  await first;
  await vi.waitFor(() => expect(run).toHaveBeenCalledTimes(3));
  releases[1]();
  releases[2]();
  await Promise.all([second, fourth]);
});
