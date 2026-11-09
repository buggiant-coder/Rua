import { consoleFetch } from "./consoleTransport";

/** Logical subscription recovery lives beside the physical connection owner.
 * The callback owns domain framing/cursors and must reconcile on each attach. */
export function subscribeConsoleResponse(
  path: string,
  onResponse: (
    response: Response,
    reconnected: boolean,
    signal: AbortSignal,
  ) => Promise<void>,
) {
  let stopped = false;
  let attempted = false;
  let delay = 1000;
  let active: AbortController | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  const connect = async () => {
    if (stopped) return;
    const controller = new AbortController();
    active = controller;
    const reconnected = attempted;
    attempted = true;
    let connectedAt: number | undefined;
    try {
      const response = await consoleFetch(path, { signal: controller.signal });
      if (stopped || active !== controller || controller.signal.aborted) {
        await response.body?.cancel().catch(() => {});
        return;
      }
      if (!response.ok) {
        await response.body?.cancel();
        throw new Error("Console subscription unavailable");
      }
      connectedAt = Date.now();
      await onResponse(response, reconnected, controller.signal);
    } catch (error) {
      if (error && typeof error === "object" && "terminal" in error)
        stopped = true;
    } finally {
      controller.abort();
    }
    if (!stopped && active === controller) {
      if (connectedAt !== undefined && Date.now() - connectedAt >= 10000)
        delay = 1000;
      retry = setTimeout(() => void connect(), delay);
      delay = Math.min(10000, delay * 2);
    }
  };
  void connect();
  return {
    reconnect: () => {
      clearTimeout(retry);
      active?.abort();
      void connect();
    },
    stop: () => {
      stopped = true;
      clearTimeout(retry);
      active?.abort();
    },
  };
}
