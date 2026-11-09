import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SliderWithValue } from "./SliderWithValue";

const sliderMock = vi.hoisted(() => vi.fn());

vi.mock("@/ui", () => ({
  Slider: (props: {
    value?: number;
    min?: number;
    max?: number;
    step?: number;
    marks?: Record<number, React.ReactNode>;
    onChange?: (value: number) => void;
  }) => {
    sliderMock(props);
    return (
      <div>
        <input
          aria-label="slider"
          type="range"
          value={props.value ?? props.min ?? 0}
          min={props.min}
          max={props.max}
          step={props.step}
          onChange={(event) => props.onChange?.(Number(event.target.value))}
        />
        <div data-testid="marks">
          {Object.entries(props.marks ?? {}).map(([key, value]) => (
            <span key={key}>{value}</span>
          ))}
        </div>
      </div>
    );
  },
}));

describe("SliderWithValue", () => {
  it("renders current/min/max marks through the owned slider component", () => {
    render(<SliderWithValue value={0.75} min={0} max={1} step={0.01} />);

    expect(screen.getByTestId("marks")).toHaveTextContent("0");
    expect(screen.getByTestId("marks")).toHaveTextContent("0.75");
    expect(screen.getByTestId("marks")).toHaveTextContent("1");
    expect(sliderMock).toHaveBeenCalledWith(
      expect.objectContaining({
        value: 0.75,
        min: 0,
        max: 1,
        step: 0.01,
        tooltip: { open: false },
      }),
    );
  });

  it("formats integer and decimal marks with Rust console semantics", () => {
    const { rerender } = render(
      <SliderWithValue value={1.5} min={0} max={2} step={0.01} />,
    );
    expect(screen.getByText("1.50")).toBeInTheDocument();

    rerender(<SliderWithValue value={2} min={0} max={2} step={0.01} />);

    expect(screen.getByTestId("marks")).toHaveTextContent("2");
    expect(screen.queryByText("2.00")).not.toBeInTheDocument();
  });

  it("forwards slider changes as numbers", () => {
    const onChange = vi.fn();
    render(
      <SliderWithValue
        value={0.5}
        min={0}
        max={1}
        step={0.01}
        onChange={onChange}
      />,
    );

    fireEvent.change(screen.getByLabelText("slider"), {
      target: { value: "0.8" },
    });

    expect(onChange).toHaveBeenCalledWith(0.8);
  });
});
