import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import RuaCard from "./index";

describe("RuaCard", () => {
  it("renders header, tags, body, and footer content", () => {
    render(
      <RuaCard
        icon={<span data-testid="card-icon" />}
        title="Runtime"
        tags={[<span key="tag">enabled</span>]}
        footer={<button type="button">Configure</button>}
      >
        <p>Runtime description</p>
      </RuaCard>,
    );

    expect(screen.getByTestId("card-icon")).toBeVisible();
    expect(screen.getByText("Runtime")).toBeVisible();
    expect(screen.getByText("enabled")).toBeVisible();
    expect(screen.getByText("Runtime description")).toBeVisible();
    expect(screen.getByRole("button", { name: "Configure" })).toBeVisible();
  });

  it("fires whole-card clicks only when hoverable and enabled", () => {
    const onClick = vi.fn();
    const { container, rerender } = render(
      <RuaCard hoverable onClick={onClick}>
        click me
      </RuaCard>,
    );

    fireEvent.click(container.firstElementChild!);
    expect(onClick).toHaveBeenCalledTimes(1);

    rerender(<RuaCard onClick={onClick}>not hoverable</RuaCard>);
    fireEvent.click(container.firstElementChild!);
    expect(onClick).toHaveBeenCalledTimes(1);

    rerender(
      <RuaCard hoverable disabled onClick={onClick}>
        disabled
      </RuaCard>,
    );
    fireEvent.click(container.firstElementChild!);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("prevents action and footer clicks from triggering the card click", () => {
    const onClick = vi.fn();
    const actionClick = vi.fn();
    const footerClick = vi.fn();

    render(
      <RuaCard
        hoverable
        onClick={onClick}
        actions={[
          <button key="action" type="button" onClick={actionClick}>
            Action
          </button>,
        ]}
        footer={
          <button type="button" onClick={footerClick}>
            Footer
          </button>
        }
      >
        body
      </RuaCard>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Action" }));
    fireEvent.click(screen.getByRole("button", { name: "Footer" }));

    expect(actionClick).toHaveBeenCalledTimes(1);
    expect(footerClick).toHaveBeenCalledTimes(1);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("passes hover callbacks through to the root card", () => {
    const onMouseEnter = vi.fn();
    const onMouseLeave = vi.fn();
    const { container } = render(
      <RuaCard onMouseEnter={onMouseEnter} onMouseLeave={onMouseLeave}>
        hover me
      </RuaCard>,
    );

    fireEvent.mouseEnter(container.firstElementChild!);
    fireEvent.mouseLeave(container.firstElementChild!);

    expect(onMouseEnter).toHaveBeenCalledTimes(1);
    expect(onMouseLeave).toHaveBeenCalledTimes(1);
  });
});
