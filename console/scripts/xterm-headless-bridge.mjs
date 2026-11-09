import headlessPkg from "@xterm/headless";
import { SerializeAddon } from "@xterm/addon-serialize";
import readline from "node:readline";

const { Terminal } = headlessPkg;

const terminals = new Map();
const canonicalTheme = {
  background: "#242424",
  cursor: "#e9ecef",
  foreground: "#e9ecef",
};
const colorKeyByOsc = new Map([
  [10, "foreground"],
  [11, "background"],
  [12, "cursor"],
]);

function encodeReply(data) {
  return Buffer.from(data).toString("base64");
}

function formatColorReply(identifier, value) {
  const match = /^#([0-9a-f]{6})$/i.exec(value);
  if (!match) throw new Error(`invalid terminal color: ${value}`);
  const channels = [0, 2, 4].map((offset) => {
    const channel = match[1].slice(offset, offset + 2);
    return `${channel}${channel}`;
  });
  return `\x1b]${identifier};rgb:${channels.join("/")}\x1b\\`;
}

function installTopMarginScrollUpCompatibility(terminal) {
  return terminal.parser.registerCsiHandler({ final: "S" }, (params) => {
    const inputHandler = terminal._core?._inputHandler;
    const bufferService = terminal._core?._bufferService;
    const buffer = inputHandler?._activeBuffer;
    if (!inputHandler || !bufferService || !buffer || buffer.scrollTop !== 0) {
      return false;
    }
    let count = params[0];
    if (Array.isArray(count)) count = count[0];
    count = Number.isInteger(count) && count > 0 ? count : 1;
    while (count-- > 0) {
      bufferService.scroll(inputHandler._eraseAttrData());
    }
    inputHandler._dirtyRowTracker.markRangeDirty(
      buffer.scrollTop,
      buffer.scrollBottom,
    );
    return true;
  });
}

function getTerminal(sessionId) {
  const entry = terminals.get(sessionId);
  if (!entry) {
    throw new Error(`terminal session not found: ${sessionId}`);
  }
  return entry;
}

async function handle(command) {
  switch (command.op) {
    case "create": {
      if (!Number.isInteger(command.scrollback) || command.scrollback < 0) {
        throw new Error("scrollback must be a non-negative integer");
      }
      const terminal = new Terminal({
        allowProposedApi: true,
        cols: Math.max(1, command.cols ?? 100),
        rows: Math.max(1, command.rows ?? 30),
        scrollback: command.scrollback,
        theme: { ...canonicalTheme },
      });
      const serializeAddon = new SerializeAddon();
      const replies = [];
      const palette = { ...canonicalTheme };
      const disposables = [
        terminal.onData((data) => replies.push(encodeReply(data))),
        installTopMarginScrollUpCompatibility(terminal),
        ...[10, 11, 12].map((identifier) =>
          terminal.parser.registerOscHandler(identifier, (data) => {
            if (data.trim() !== "?") return false;
            const key = colorKeyByOsc.get(identifier);
            replies.push(
              encodeReply(formatColorReply(identifier, palette[key])),
            );
            return true;
          }),
        ),
      ];
      terminal.loadAddon(serializeAddon);
      terminals.set(command.sessionId, {
        terminal,
        palette,
        serializeAddon,
        replies,
        disposables,
      });
      return {};
    }
    case "write": {
      const entry = getTerminal(command.sessionId);
      entry.replies.length = 0;
      const data = Buffer.from(command.data ?? "", "base64");
      await new Promise((resolve) => entry.terminal.write(data, resolve));
      return { replies: entry.replies.splice(0) };
    }
    case "resize": {
      const { terminal } = getTerminal(command.sessionId);
      terminal.resize(
        Math.max(1, command.cols ?? 100),
        Math.max(1, command.rows ?? 30),
      );
      return {};
    }
    case "set_scrollback": {
      const { terminal } = getTerminal(command.sessionId);
      terminal.options.scrollback = Math.max(0, command.scrollback ?? 0);
      return {};
    }
    case "set_palette": {
      const entry = getTerminal(command.sessionId);
      entry.replies.length = 0;
      entry.palette.foreground = command.foreground;
      entry.palette.background = command.background;
      entry.palette.cursor = command.cursor;
      entry.terminal.options.theme = { ...entry.palette };
      if (entry.terminal.modes.sendFocusMode) {
        entry.replies.push(encodeReply("\x1b[I"));
      }
      return { replies: entry.replies.splice(0) };
    }
    case "serialize": {
      const { serializeAddon } = getTerminal(command.sessionId);
      return { snapshot: serializeAddon.serialize() };
    }
    case "dispose": {
      const entry = terminals.get(command.sessionId);
      if (entry) {
        for (const disposable of entry.disposables) disposable.dispose();
        entry.terminal.dispose();
        terminals.delete(command.sessionId);
      }
      return {};
    }
    default:
      throw new Error(`unknown op: ${command.op}`);
  }
}

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

for await (const line of rl) {
  if (!line.trim()) continue;
  let id = null;
  try {
    const command = JSON.parse(line);
    id = command.id;
    const result = await handle(command);
    process.stdout.write(`${JSON.stringify({ id, ok: true, ...result })}\n`);
  } catch (error) {
    process.stdout.write(
      `${JSON.stringify({
        id,
        ok: false,
        error: error instanceof Error ? error.message : String(error),
      })}\n`,
    );
  }
}
