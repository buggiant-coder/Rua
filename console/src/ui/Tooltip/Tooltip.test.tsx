import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { __resetMobileDetectionForTests } from "@/utils/isMobile";
import Tooltip from "./index";

describe("Tooltip mobile interactions", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", "/?_mobile=1");
    __resetMobileDetectionForTests();
  });

  afterEach(() => {
    vi.useRealTimers();
    window.history.replaceState({}, "", "/");
    document.documentElement.classList.remove("mobile-mode");
    __resetMobileDetectionForTests();
  });

  it("does not show an action tooltip after a normal tap", () => {
    const onClick = vi.fn();
    render(
      <Tooltip title="Action help">
        <button type="button" onClick={onClick}>
          Run
        </button>
      </Tooltip>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Run" }));

    expect(onClick).toHaveBeenCalledOnce();
    expect(screen.queryByText("Action help")).not.toBeInTheDocument();
  });

  it("shows a tooltip on long press without triggering the action", () => {
    vi.useFakeTimers();
    const onClick = vi.fn();
    render(
      <Tooltip title="Action help">
        <button type="button" onClick={onClick}>
          Run
        </button>
      </Tooltip>,
    );
    const button = screen.getByRole("button", { name: "Run" });

    fireEvent.touchStart(button, {
      touches: [{ clientX: 10, clientY: 10 }],
    });
    act(() => {
      vi.advanceTimersByTime(600);
    });
    fireEvent.touchEnd(button);
    fireEvent.click(button);

    expect(screen.getByText("Action help")).toBeInTheDocument();
    expect(onClick).not.toHaveBeenCalled();
  });
});
