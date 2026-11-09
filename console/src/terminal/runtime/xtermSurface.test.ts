import { describe, expect, it, vi } from "vitest";
import { installTerminalPageNavigation } from "./xtermSurface";

function createTerminal() {
  let handler: ((event: KeyboardEvent) => boolean) | undefined;
  const terminal = {
    attachCustomKeyEventHandler: vi.fn(
      (nextHandler: (event: KeyboardEvent) => boolean) => {
        handler = nextHandler;
      },
    ),
  };
  const scrollPages = vi.fn();

  return {
    terminal,
    scrollPages,
    dispatch(init: KeyboardEventInit) {
      const event = new KeyboardEvent("keydown", {
        cancelable: true,
        ...init,
      });
      return {
        event,
        handledByXterm: handler?.(event),
      };
    },
  };
}

describe("installTerminalPageNavigation", () => {
  it("uses unmodified PageUp and PageDown for local scrollback", () => {
    const { terminal, scrollPages, dispatch } = createTerminal();
    installTerminalPageNavigation(terminal, scrollPages);

    const pageUp = dispatch({ key: "PageUp" });
    const pageDown = dispatch({ key: "PageDown" });

    expect(scrollPages.mock.calls).toEqual([[-1], [1]]);
    expect(pageUp.handledByXterm).toBe(false);
    expect(pageDown.handledByXterm).toBe(false);
    expect(pageUp.event.defaultPrevented).toBe(true);
    expect(pageDown.event.defaultPrevented).toBe(true);
  });

  it("leaves modified and unrelated keys to xterm", () => {
    const { terminal, scrollPages, dispatch } = createTerminal();
    installTerminalPageNavigation(terminal, scrollPages);

    expect(dispatch({ key: "PageDown", metaKey: true }).handledByXterm).toBe(
      true,
    );
    expect(dispatch({ key: "PageUp", shiftKey: true }).handledByXterm).toBe(
      true,
    );
    expect(dispatch({ key: "ArrowUp" }).handledByXterm).toBe(true);
    expect(scrollPages).not.toHaveBeenCalled();
  });
});
