import { getApiToken, getApiUrl } from "./config";
import { buildAuthHeaders } from "./authHeaders";

const CHANNEL_BYTES = 256 * 1024;
const TOTAL_BYTES = 2 * 1024 * 1024;
const MAX_CHANNELS = 64;
type Channel = {
  frame: Record<string, unknown>;
  sent: boolean;
  controller: ReadableStreamDefaultController<Uint8Array>;
  resolve: (response: Response) => void;
  reject: (error: Error) => void;
  cleanup: () => void;
};

/** One socket per document + authenticated resident context. Domain adapters
 * retain their parsers/cursors; they never own physical sockets or HTTP streams. */
export class ConsoleTransport {
  private socket?: WebSocket;
  private ready = false;
  private sequence = 0;
  private generation = 0;
  private channels = new Map<string, Channel>();
  private retry?: ReturnType<typeof setTimeout>;
  private idle?: ReturnType<typeof setTimeout>;
  private authentication?: ReturnType<typeof setTimeout>;
  private delay = 1000;
  private fatal?: Error;

  private readonly url: string;
  private readonly token: string;
  constructor(url: string, token: string) {
    this.url = url;
    this.token = token;
  }

  private send(frame: Record<string, unknown>) {
    if (!this.ready || !this.socket || this.socket.bufferedAmount > TOTAL_BYTES)
      return false;
    this.socket.send(JSON.stringify(frame));
    return true;
  }

  private connect() {
    if (this.socket || this.retry || this.fatal || !this.channels.size) return;
    const generation = ++this.generation;
    const socket = new WebSocket(this.url);
    this.socket = socket;
    this.authentication = setTimeout(() => socket.close(), 6000);
    socket.onopen = () => {
      if (generation !== this.generation) return;
      socket.send(
        JSON.stringify({ type: "auth", version: 1, token: this.token }),
      );
    };
    socket.onmessage = (event) => {
      if (generation !== this.generation) return;
      let frame: {
        type: string;
        id?: string;
        version?: number;
        status?: number;
        content_type?: string;
        data?: string;
        detail?: string;
      };
      try {
        frame = JSON.parse(String(event.data));
      } catch {
        this.close(new Error("Invalid Console Transport frame"));
        return;
      }
      if (frame.type === "fatal") {
        this.close(new Error(frame.detail || "Console Transport rejected"));
        return;
      }
      if (frame.type === "ready") {
        if (frame.version !== 1) {
          this.close(new Error("Unsupported Console Transport version"));
          return;
        }
        clearTimeout(this.authentication);
        this.ready = true;
        this.delay = 1000;
        for (const channel of this.channels.values()) {
          if (!channel.sent) channel.sent = this.send(channel.frame);
        }
        return;
      }
      const id = frame.id;
      const channel = id ? this.channels.get(id) : undefined;
      if (!id || !channel) return; // Cancelled/obsolete logical request.
      switch (frame.type) {
        case "head":
          channel.resolve(
            new Response(channelStream.get(channel), {
              status: frame.status,
              headers: {
                "Content-Type": frame.content_type || "application/json",
              },
            }),
          );
          break;
        case "data": {
          const encoded = frame.data || "";
          if (encoded.length > 24 * 1024) {
            this.finish(id, new Error("Console frame too large"));
            break;
          }
          const bytes = Uint8Array.from(atob(encoded), (c) => c.charCodeAt(0));
          const buffered = [...this.channels.values()].reduce(
            (total, c) =>
              total + CHANNEL_BYTES - (c.controller.desiredSize ?? 0),
            0,
          );
          if (
            (channel.controller.desiredSize ?? 0) < bytes.length ||
            buffered + bytes.length > TOTAL_BYTES
          ) {
            this.finish(
              id,
              new Error(
                "Console channel overflow; reconcile from cursor or snapshot",
              ),
            );
          } else channel.controller.enqueue(bytes);
          break;
        }
        case "end":
          this.finish(id);
          break;
        case "gap":
        case "error":
          this.finish(
            id,
            new Error(frame.detail || "Console operation failed"),
          );
          break;
        default:
          this.close(new Error("Unsupported Console Transport frame"));
      }
    };
    socket.onerror = () => socket.close();
    socket.onclose = () => {
      if (generation !== this.generation) return;
      clearTimeout(this.authentication);
      this.ready = false;
      this.socket = undefined;
      // Never replay a dispatched operation. Cursor-aware subscription adapters
      // reconcile and acquire fresh logical requests after transport loss.
      for (const [id, channel] of this.channels) {
        if (channel.sent)
          this.finish(id, new Error("Console Transport disconnected"));
      }
      this.retry = setTimeout(() => {
        this.retry = undefined;
        this.connect();
      }, this.delay);
      this.delay = Math.min(10000, this.delay * 2);
    };
  }

  fetch(path: string, init: RequestInit = {}): Promise<Response> {
    if (this.fatal) return Promise.reject(this.fatal);
    if (init.signal?.aborted)
      return Promise.reject(new DOMException("Aborted", "AbortError"));
    if (this.channels.size >= MAX_CHANNELS)
      return Promise.reject(new Error("Console channel capacity exhausted"));
    clearTimeout(this.idle);
    const id = String(++this.sequence);
    return new Promise<Response>((resolve, reject) => {
      let controller!: ReadableStreamDefaultController<Uint8Array>;
      const stream = new ReadableStream<Uint8Array>(
        {
          start(value) {
            controller = value;
          },
          cancel: () =>
            this.finish(id, new DOMException("Aborted", "AbortError")),
        },
        { highWaterMark: CHANNEL_BYTES, size: (chunk) => chunk.byteLength },
      );
      const abort = () =>
        this.finish(id, new DOMException("Aborted", "AbortError"));
      // Deadline covers socket admission as well as the remote operation.
      const persistent =
        /\/(?:activity|conversation-activity|live)(?:\?|$)/.test(path);
      const deadline = persistent
        ? undefined
        : setTimeout(
            () =>
              this.finish(id, new Error("Console request deadline exceeded")),
            30000,
          );
      const channel: Channel = {
        frame: {
          type: "open",
          id,
          path,
          method: init.method || "GET",
          body: typeof init.body === "string" ? init.body : "",
          agent_id:
            new Headers(init.headers || buildAuthHeaders()).get("X-Agent-Id") ||
            "",
        },
        sent: false,
        controller,
        resolve,
        reject,
        cleanup: () => {
          clearTimeout(deadline);
          init.signal?.removeEventListener("abort", abort);
        },
      };
      channelStream.set(channel, stream);
      this.channels.set(id, channel);
      init.signal?.addEventListener("abort", abort, { once: true });
      if (this.ready) channel.sent = this.send(channel.frame);
      this.connect();
    });
  }

  private finish(id: string, error?: Error) {
    const channel = this.channels.get(id);
    if (!channel) return;
    this.channels.delete(id);
    channel.cleanup();
    if (error) {
      if (channel.sent) this.send({ type: "cancel", id });
      channel.reject(error);
      channel.controller.error(error);
    } else channel.controller.close();
    if (!this.channels.size) {
      this.idle = setTimeout(() => {
        ++this.generation;
        this.socket?.close();
        this.socket = undefined;
        this.ready = false;
      }, 30000);
    }
  }

  close(error = new Error("Console authentication context changed")) {
    this.fatal = Object.assign(error, { terminal: true });
    ++this.generation;
    for (const id of this.channels.keys()) this.finish(id, error);
    clearTimeout(this.retry);
    clearTimeout(this.idle);
    clearTimeout(this.authentication);
    this.socket?.close();
    this.socket = undefined;
    this.ready = false;
  }
}

const channelStream = new WeakMap<Channel, ReadableStream<Uint8Array>>();
let current: { key: string; transport: ConsoleTransport } | undefined;

export function resetConsoleTransport() {
  current?.transport.close();
  current = undefined;
}

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", resetConsoleTransport);
  window.addEventListener("rua:auth-changed", resetConsoleTransport);
  window.addEventListener("storage", (event) => {
    if (event.key === "rua_auth_token") resetConsoleTransport();
  });
}

export function consoleFetch(
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const url = new URL(getApiUrl("/console/transport"), window.location.href);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  const token = getApiToken();
  const key = `${url.href}\n${token}`;
  if (current?.key !== key) {
    resetConsoleTransport();
    current = { key, transport: new ConsoleTransport(url.href, token) };
  }
  return current!.transport.fetch(path, init);
}
