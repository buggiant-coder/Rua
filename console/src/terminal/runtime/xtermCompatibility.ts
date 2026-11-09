import type { Terminal } from "@xterm/xterm";

type XtermInternals = Terminal & {
  _core: {
    _bufferService: {
      scroll(eraseAttr: unknown): void;
    };
    _viewport?: {
      _sync(ydisp?: number): void;
    };
    coreService: {
      decPrivateModes: {
        synchronizedOutput: boolean;
      };
    };
    _inputHandler: {
      _activeBuffer: {
        scrollBottom: number;
        scrollTop: number;
      };
      _dirtyRowTracker: {
        markRangeDirty(start: number, end: number): void;
      };
      _eraseAttrData(): unknown;
    };
  };
};

function includesMode(params: (number | number[])[], mode: number) {
  return params.some((value) =>
    Array.isArray(value) ? value.includes(mode) : value === mode,
  );
}

function installSynchronizedOutputViewportCompatibility(terminal: Terminal) {
  const internal = terminal as XtermInternals;
  let viewport: XtermInternals["_core"]["_viewport"];
  let originalSync: ((ydisp?: number) => void) | undefined;
  let patchedSync: ((ydisp?: number) => void) | undefined;
  let syncPending = false;

  const installPatch = () => {
    const nextViewport = internal._core?._viewport;
    if (!nextViewport || nextViewport === viewport) return;
    if (viewport && originalSync && viewport._sync === patchedSync) {
      viewport._sync = originalSync;
    }
    viewport = nextViewport;
    originalSync = viewport._sync;
    patchedSync = (ydisp?: number) => {
      if (internal._core.coreService.decPrivateModes.synchronizedOutput) {
        syncPending = true;
        return;
      }
      originalSync?.call(viewport, ydisp);
    };
    viewport._sync = patchedSync;
  };

  const modeHandler = terminal.parser.registerCsiHandler(
    { prefix: "?", final: "h" },
    (params) => {
      if (includesMode(params, 2026)) installPatch();
      return false;
    },
  );
  const renderHandler = terminal.onRender(() => {
    if (
      !syncPending ||
      internal._core.coreService.decPrivateModes.synchronizedOutput
    ) {
      return;
    }
    syncPending = false;
    originalSync?.call(viewport);
  });

  return {
    dispose() {
      modeHandler.dispose();
      renderHandler.dispose();
      if (viewport && originalSync && viewport._sync === patchedSync) {
        viewport._sync = originalSync;
      }
    },
  };
}

function installTopMarginScrollUpCompatibility(terminal: Terminal) {
  return terminal.parser.registerCsiHandler({ final: "S" }, (params) => {
    const internal = terminal as XtermInternals;
    const inputHandler = internal._core?._inputHandler;
    const bufferService = internal._core?._bufferService;
    const buffer = inputHandler?._activeBuffer;
    if (!inputHandler || !bufferService || !buffer || buffer.scrollTop !== 0) {
      return false;
    }
    let count = params[0];
    if (Array.isArray(count)) count = count[0];
    count = typeof count === "number" && count > 0 ? count : 1;
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

function installAuthorityReplySuppression(terminal: Terminal) {
  const handlers = [
    terminal.parser.registerCsiHandler({ final: "c" }, () => true),
    terminal.parser.registerCsiHandler({ prefix: ">", final: "c" }, () => true),
    terminal.parser.registerCsiHandler({ final: "n" }, () => true),
    terminal.parser.registerCsiHandler({ prefix: "?", final: "n" }, () => true),
    ...[10, 11, 12].map((identifier) =>
      terminal.parser.registerOscHandler(
        identifier,
        (data) => data.trim() === "?",
      ),
    ),
  ];
  return {
    dispose() {
      for (const handler of handlers) handler.dispose();
    },
  };
}

export function installXtermCompatibility(terminal: Terminal) {
  const handlers = [
    installSynchronizedOutputViewportCompatibility(terminal),
    installTopMarginScrollUpCompatibility(terminal),
    installAuthorityReplySuppression(terminal),
  ];
  return {
    dispose() {
      for (const handler of handlers) handler.dispose();
    },
  };
}
