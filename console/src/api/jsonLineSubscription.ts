import { subscribeConsoleResponse } from "./consoleSubscription";

export function subscribeJsonLines({
  path,
  onMessage,
  onConnected,
}: {
  path: string;
  onMessage: (message: unknown) => void;
  onConnected?: (reconnected: boolean) => void;
}): { reconnect: () => void; stop: () => void } {
  return subscribeConsoleResponse(
    path,
    async (response, reconnected, signal) => {
      if (!response.body) throw new Error("Subscription body unavailable");
      const reader = response.body.getReader();
      const cancel = () => {
        void reader.cancel().catch(() => {});
      };
      signal.addEventListener("abort", cancel, { once: true });
      const decoder = new TextDecoder();
      let buffer = "";
      const consume = (line: string) => {
        if (!line.trim()) return;
        let message: unknown;
        try {
          message = JSON.parse(line);
        } catch {
          return;
        }
        onMessage(message);
      };
      onConnected?.(reconnected);
      try {
        while (!signal.aborted) {
          const { done, value } = await reader.read();
          if (signal.aborted) return;
          buffer += decoder.decode(value, { stream: !done });
          if (buffer.length > 1024 * 1024)
            throw new Error("Console JSON line overflow");
          let newline = buffer.indexOf("\n");
          while (newline >= 0 && !signal.aborted) {
            consume(buffer.slice(0, newline));
            buffer = buffer.slice(newline + 1);
            newline = buffer.indexOf("\n");
          }
          if (done) {
            if (!signal.aborted) consume(buffer);
            return;
          }
        }
      } finally {
        await reader.cancel().catch(() => {});
        reader.releaseLock();
      }
    },
  );
}
