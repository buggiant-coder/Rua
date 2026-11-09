/** Keep HTTP/1.1 capacity available for local reads while remote media waits.
 * The permit covers the entire response body, not just receipt of headers. */
const MAX_ACTIVE_TRANSFERS = 2;
const MAX_QUEUED_TRANSFERS = 128;
let active = 0;
const queue: Array<() => void> = [];

export function withTransferSlot<T>(
  operation: () => Promise<T>,
  signal?: AbortSignal,
): Promise<T> {
  if (signal?.aborted)
    return Promise.reject(new DOMException("Aborted", "AbortError"));
  if (active >= MAX_ACTIVE_TRANSFERS && queue.length >= MAX_QUEUED_TRANSFERS) {
    return Promise.reject(new Error("Media transfer queue is full"));
  }
  return new Promise<T>((resolve, reject) => {
    const abort = () => {
      const index = queue.indexOf(start);
      if (index >= 0) queue.splice(index, 1);
      signal?.removeEventListener("abort", abort);
      reject(new DOMException("Aborted", "AbortError"));
    };
    const start = () => {
      signal?.removeEventListener("abort", abort);
      if (signal?.aborted) {
        abort();
        return;
      }
      active++;
      void operation()
        .then(resolve, reject)
        .finally(() => {
          active--;
          queue.shift()?.();
        });
    };
    if (active < MAX_ACTIVE_TRANSFERS) start();
    else {
      queue.push(start);
      signal?.addEventListener("abort", abort, { once: true });
    }
  });
}
