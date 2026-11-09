import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import Slider from "./index";

describe("Slider", () => {
  it("renders min, max, and current value marks below the track", () => {
    const { container } = render(
      <Slider value={0.75} min={0} max={1} step={0.01} />,
    );

    expect(screen.getByText("0")).toBeInTheDocument();
    expect(screen.getByText("0.75")).toBeInTheDocument();
    expect(screen.getByText("1")).toBeInTheDocument();
    expect(
      container.querySelector(".rua-ui-slider-mark--current"),
    ).toHaveTextContent("0.75");
  });

  it("uses supplied mark labels and highlights the current mark", () => {
    const { container } = render(
      <Slider
        value={1}
        min={0.5}
        max={2}
        step={0.5}
        marks={{ 0.5: "0.5x", 1: "1x", 1.5: "1.5x", 2: "2x" }}
      />,
    );

    expect(screen.getByText("0.5x")).toBeInTheDocument();
    expect(screen.getByText("2x")).toBeInTheDocument();
    expect(
      container.querySelector(".rua-ui-slider-mark--current"),
    ).toHaveTextContent("1x");
  });

  it("renders and updates two ordered thumbs in range mode", () => {
    const onChange = vi.fn();
    render(
      <Slider
        range
        value={[2, 5]}
        min={1}
        max={20}
        onChange={onChange}
        ariaLabels={["Recent boundary", "Snip boundary"]}
      />,
    );

    const recent = screen.getByRole("slider", { name: "Recent boundary" });
    const snip = screen.getByRole("slider", { name: "Snip boundary" });
    expect(recent).toHaveAttribute("aria-valuenow", "2");
    expect(recent).toHaveAttribute("aria-valuemax", "5");
    expect(snip).toHaveAttribute("aria-valuemin", "2");

    fireEvent.keyDown(recent, { key: "ArrowRight" });
    expect(onChange).toHaveBeenCalledWith([3, 5]);

    fireEvent.keyDown(snip, { key: "Home" });
    expect(onChange).toHaveBeenCalledWith([2, 2]);
  });
});
