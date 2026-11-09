import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import ContentTransitionFrame from "./ContentTransitionFrame";

describe("ContentTransitionFrame", () => {
  it("animates only after committed content identity changes", () => {
    const { container, rerender } = render(
      <ContentTransitionFrame transitionKey="chat-a">
        <div>Chat A</div>
      </ContentTransitionFrame>,
    );
    const frame = container.firstElementChild as HTMLElement;
    expect(frame).not.toHaveClass("is-entering");

    rerender(
      <ContentTransitionFrame transitionKey="chat-b">
        <div>Chat B</div>
      </ContentTransitionFrame>,
    );
    expect(frame).toHaveClass("is-entering");

    fireEvent.animationEnd(frame);
    expect(frame).not.toHaveClass("is-entering");
  });
});
