import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ChatRenderBlock } from "./ChatRenderBlock";

describe("ChatRenderBlock", () => {
  it("collapses content and keeps the click inside the render block", () => {
    const onParentClick = vi.fn();
    const { container } = render(
      <div onClick={onParentClick}>
        <ChatRenderBlock title="Artifact">
          <pre>content</pre>
        </ChatRenderBlock>
      </div>,
    );

    const body = container.querySelector(".rua-render-block-body");

    fireEvent.click(screen.getByRole("button", { name: "Collapse block" }));

    expect(body).toHaveAttribute("hidden");
    expect(
      screen.getByRole("button", { name: "Expand block" }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(onParentClick).not.toHaveBeenCalled();
  });
});
