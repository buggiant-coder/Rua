import { act, fireEvent, render } from "@testing-library/react";
import { useRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  __resetConversationViewportForTests,
  useConversationViewport,
} from "../useConversationViewport";

const turns: Parameters<typeof useConversationViewport>[0]["turns"] = [];

describe("conversation viewport event isolation", () => {
  beforeEach(() => {
    __resetConversationViewportForTests();
  });

  function setup() {
    const rendered = vi.fn();
    function Harness() {
      const scrollRef = useRef<HTMLDivElement>(null);
      const viewport = useConversationViewport({
        viewKey: "selection-isolation",
        turns,
        scrollRef,
        initialScroll: { followLatest: false },
      });
      rendered();
      return (
        <>
          <div ref={scrollRef} data-testid="scroller">
            <div ref={viewport.listRef}>
              <p tabIndex={0}>Message body</p>
            </div>
          </div>
          <textarea aria-label="Composer" />
        </>
      );
    }
    const result = render(<Harness />);
    rendered.mockClear();
    return { ...result, rendered };
  }

  it("ignores composer caret events without rendering or measuring history", () => {
    const { getByLabelText, getByTestId, rendered } = setup();
    const measure = vi.spyOn(getByTestId("scroller"), "getBoundingClientRect");
    act(() => window.getSelection()?.removeAllRanges());
    getByLabelText("Composer").focus();
    rendered.mockClear();
    measure.mockClear();
    for (let index = 0; index < 20; index += 1) {
      fireEvent(
        getByLabelText("Composer"),
        new Event("selectionchange", { bubbles: true }),
      );
    }
    expect(rendered).not.toHaveBeenCalled();
    expect(measure).not.toHaveBeenCalled();
  });

  it("refreshes message focus protection even without a text selection", () => {
    const { getByText, getByLabelText, rendered } = setup();
    act(() => getByText("Message body").focus());
    expect(rendered).toHaveBeenCalled();
    rendered.mockClear();
    act(() => getByLabelText("Composer").focus());
    expect(rendered).toHaveBeenCalled();
  });

  it("ignores outside selections before attempting to capture an anchor", () => {
    const { getByTestId, rendered } = setup();
    const measure = vi.spyOn(getByTestId("scroller"), "getBoundingClientRect");
    const outside = document.createElement("p");
    outside.textContent = "Outside the conversation";
    document.body.append(outside);
    try {
      act(() => {
        const range = document.createRange();
        range.selectNodeContents(outside);
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range);
        document.dispatchEvent(new Event("selectionchange"));
      });
      expect(rendered).not.toHaveBeenCalled();
      expect(measure).not.toHaveBeenCalled();
    } finally {
      outside.remove();
      window.getSelection()?.removeAllRanges();
    }
  });

  it("refreshes when a message selection is created and cleared, but not on later caret events", () => {
    const { getByText, getByLabelText, rendered } = setup();
    act(() => {
      const range = document.createRange();
      range.selectNodeContents(getByText("Message body"));
      window.getSelection()?.removeAllRanges();
      window.getSelection()?.addRange(range);
      document.dispatchEvent(new Event("selectionchange"));
    });
    expect(rendered).toHaveBeenCalled();
    rendered.mockClear();
    act(() => {
      window.getSelection()?.removeAllRanges();
      document.dispatchEvent(new Event("selectionchange"));
    });
    expect(rendered).toHaveBeenCalled();
    rendered.mockClear();
    fireEvent(
      getByLabelText("Composer"),
      new Event("selectionchange", { bubbles: true }),
    );
    expect(rendered).not.toHaveBeenCalled();
  });
});
