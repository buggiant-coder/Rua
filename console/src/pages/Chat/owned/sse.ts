export type SseChunk = {
  event?: string;
  data: string;
};

export class SseReadIdleTimeoutError extends Error {
  constructor(timeoutMs: number) {
    super(`stream produced no bytes for ${timeoutMs}ms`);
    this.name = "SseReadIdleTimeoutError";
  }
}

export type ReadSseChunksOptions = {
  idleTimeoutMs?: number;
};

function parseSsePart(part: string): SseChunk | null {
  const lines = part.split(/\r?\n/);
  let event: string | undefined;
  const data: string[] = [];

  for (const line of lines) {
    if (!line || line.startsWith(":")) continue;
    const idx = line.indexOf(":");
    if (idx < 0) continue;
    const key = line.slice(0, idx).trim();
    const value = line.slice(idx + 1).replace(/^ /, "");
    if (key === "event") event = value;
    if (key === "data") data.push(value);
  }

  if (data.length === 0) return null;
  return { event, data: data.join("\n") };
}

function splitFirstCompleteFrame(buffer: string): [string, string] | null {
  const sseMatch = buffer.match(/\r?\n\r?\n/);
  if (sseMatch?.index != null) {
    const frame = buffer.slice(0, sseMatch.index);
    const rest = buffer.slice(sseMatch.index + sseMatch[0].length);
    return [frame, rest];
  }

  return null;
}

export async function* readSseChunks(
  body: ReadableStream<Uint8Array> | null,
  options: ReadSseChunksOptions = {},
): AsyncGenerator<SseChunk> {
  if (!body) return;
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  const idleTimeoutMs = options.idleTimeoutMs;

  const read = async (): Promise<ReadableStreamReadResult<Uint8Array>> => {
    if (idleTimeoutMs === undefined) return reader.read();

    let timeoutId: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        reader.read(),
        new Promise<never>((_, reject) => {
          timeoutId = setTimeout(
            () => reject(new SseReadIdleTimeoutError(idleTimeoutMs)),
            idleTimeoutMs,
          );
        }),
      ]);
    } finally {
      if (timeoutId !== undefined) clearTimeout(timeoutId);
    }
  };

  try {
    while (true) {
      const { done, value } = await read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let next = splitFirstCompleteFrame(buffer);
      while (next) {
        const [part, rest] = next;
        buffer = rest;
        const chunk = parseSsePart(part);
        if (chunk) yield chunk;
        next = splitFirstCompleteFrame(buffer);
      }
    }
    buffer += decoder.decode();
    const tail = parseSsePart(buffer);
    if (tail) yield tail;
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
