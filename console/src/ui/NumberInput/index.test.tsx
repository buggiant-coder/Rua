import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import NumberInput from ".";

describe("NumberInput", () => {
  it("uses step for spinner increments without constraining typed values", () => {
    const onChange = vi.fn();
    const { container } = render(
      <NumberInput
        min={1000}
        step={1024}
        value={1_000_000}
        onChange={onChange}
      />,
    );

    const input = container.querySelector("input");
    expect(input).not.toBeNull();
    expect(input).toHaveAttribute("step", "any");
    expect(input?.validity.stepMismatch).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "Increase" }));
    expect(onChange).toHaveBeenCalledWith(1_001_024);
  });
});
