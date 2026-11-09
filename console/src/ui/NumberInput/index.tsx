import { forwardRef, useCallback } from "react";
import type { InputHTMLAttributes, KeyboardEvent } from "react";
import { ChevronUp, ChevronDown } from "lucide-react";
import "./number-input.css";

interface NumberInputProps
  extends Omit<
    InputHTMLAttributes<HTMLInputElement>,
    "value" | "onChange" | "step"
  > {
  value?: number | null;
  onChange?: (value: number | null) => void;
  onPressEnter?: () => void;
  step?: number;
  inputStep?: InputHTMLAttributes<HTMLInputElement>["step"];
}

export default forwardRef<HTMLInputElement, NumberInputProps>(
  function NumberInput(
    {
      className,
      value,
      onChange,
      onPressEnter,
      onKeyDown,
      step = 1,
      inputStep = "any",
      min,
      max,
      disabled,
      ...props
    },
    ref,
  ) {
    const clamp = useCallback(
      (v: number) => {
        let clamped = v;
        if (min !== undefined) clamped = Math.max(Number(min), clamped);
        if (max !== undefined) clamped = Math.min(Number(max), clamped);
        return clamped;
      },
      [min, max],
    );

    const increment = () => {
      const cur = value ?? 0;
      onChange?.(clamp(cur + Number(step)));
    };

    const decrement = () => {
      const cur = value ?? 0;
      onChange?.(clamp(cur - Number(step)));
    };

    return (
      <div
        className={`rua-number-input-wrapper${
          className ? ` ${className}` : ""
        }`}
      >
        <input
          ref={ref}
          type="number"
          className="rua-number-input"
          value={value ?? ""}
          onChange={(e) => {
            const raw = e.target.value;
            onChange?.(raw === "" ? null : Number(raw));
          }}
          onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
            if (e.key === "Enter") onPressEnter?.();
            onKeyDown?.(e);
          }}
          step={inputStep}
          min={min}
          max={max}
          disabled={disabled}
          {...props}
        />
        <div className="rua-number-input-steppers">
          <button
            type="button"
            className="rua-number-input-step-up"
            onClick={increment}
            tabIndex={-1}
            disabled={disabled}
            aria-label="Increase"
          >
            <ChevronUp size={12} />
          </button>
          <button
            type="button"
            className="rua-number-input-step-down"
            onClick={decrement}
            tabIndex={-1}
            disabled={disabled}
            aria-label="Decrease"
          >
            <ChevronDown size={12} />
          </button>
        </div>
      </div>
    );
  },
);
