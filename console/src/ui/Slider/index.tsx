import { useRef, useCallback } from "react";
import type { CSSProperties, ReactNode } from "react";
import "./slider.css";

interface SliderProps {
  value?: number | number[];
  onChange?: (value: any) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  className?: string;
  marks?: Record<number, ReactNode>;
  style?: CSSProperties;
  range?: boolean;
  tooltip?: Record<string, unknown>;
  ariaLabel?: string;
  ariaLabels?: [string, string];
  ariaValueMin?: number;
  ariaValueMax?: number;
  ariaValueNow?: number;
  ariaValueText?: string;
}

type SliderMark = {
  value: number;
  label: ReactNode;
};

function clamp(v: number, lo: number, hi: number) {
  return Math.min(hi, Math.max(lo, v));
}

function snap(v: number, step: number, min: number) {
  return Math.round((v - min) / step) * step + min;
}

function formatSliderValue(v: number) {
  if (Number.isInteger(v)) return v.toString();
  return v.toFixed(2).replace(/\.?0+$/, "");
}

function sameValue(a: number, b: number, step: number) {
  return Math.abs(a - b) <= Math.max(Math.abs(step) / 1000, Number.EPSILON);
}

export default function Slider({
  value: valueProp,
  onChange,
  min = 0,
  max = 100,
  step = 1,
  disabled,
  className,
  style,
  marks,
  range = false,
  ariaLabel,
  ariaLabels,
  ariaValueMin,
  ariaValueMax,
  ariaValueNow,
  ariaValueText,
}: SliderProps) {
  const rawRange = Array.isArray(valueProp) ? valueProp : [];
  const rangeStart = clamp(rawRange[0] ?? min, min, max);
  const rangeEnd = clamp(rawRange[1] ?? max, rangeStart, max);
  const singleValue = Array.isArray(valueProp) ? rangeEnd : valueProp ?? min;
  const clampedValue = clamp(singleValue, min, max);
  const currentValues = range ? [rangeStart, rangeEnd] : [clampedValue];
  const trackRef = useRef<HTMLDivElement>(null);
  const activeThumbRef = useRef<0 | 1 | null>(null);

  const emitValue = useCallback(
    (thumb: 0 | 1, nextValue: number) => {
      const next = clamp(snap(nextValue, step, min), min, max);
      if (!range) {
        onChange?.(next);
        return;
      }
      onChange?.(
        thumb === 0
          ? [Math.min(next, rangeEnd), rangeEnd]
          : [rangeStart, Math.max(next, rangeStart)],
      );
    },
    [max, min, onChange, range, rangeEnd, rangeStart, step],
  );

  const resolve = useCallback(
    (clientX: number, thumb: 0 | 1) => {
      const track = trackRef.current;
      if (!track) return;
      const rect = track.getBoundingClientRect();
      if (rect.width <= 0) return;
      const ratio = clamp((clientX - rect.left) / rect.width, 0, 1);
      const raw = min + ratio * (max - min);
      emitValue(thumb, raw);
    },
    [emitValue, min, max],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (disabled) return;
      e.preventDefault();
      const explicitThumb = (e.target as HTMLElement).closest<HTMLElement>(
        "[data-slider-thumb]",
      )?.dataset.sliderThumb;
      const track = trackRef.current;
      const rect = track?.getBoundingClientRect();
      const raw = rect
        ? min + clamp((e.clientX - rect.left) / rect.width, 0, 1) * (max - min)
        : clampedValue;
      const thumb: 0 | 1 =
        explicitThumb === "0" || explicitThumb === "1"
          ? (Number(explicitThumb) as 0 | 1)
          : range && Math.abs(raw - rangeEnd) < Math.abs(raw - rangeStart)
          ? 1
          : 0;
      activeThumbRef.current = thumb;
      e.currentTarget.setPointerCapture(e.pointerId);
      resolve(e.clientX, thumb);
    },
    [clampedValue, disabled, max, min, range, rangeEnd, rangeStart, resolve],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (
        disabled ||
        activeThumbRef.current === null ||
        !e.currentTarget.hasPointerCapture(e.pointerId)
      ) {
        return;
      }
      resolve(e.clientX, activeThumbRef.current);
    },
    [disabled, resolve],
  );

  const onPointerUp = useCallback((e: React.PointerEvent) => {
    activeThumbRef.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  }, []);

  const onThumbKeyDown = useCallback(
    (thumb: 0 | 1, e: React.KeyboardEvent) => {
      if (disabled) return;
      const current = range
        ? thumb === 0
          ? rangeStart
          : rangeEnd
        : clampedValue;
      let next: number | null = null;
      if (e.key === "ArrowLeft" || e.key === "ArrowDown") {
        next = current - step;
      } else if (e.key === "ArrowRight" || e.key === "ArrowUp") {
        next = current + step;
      } else if (e.key === "Home") {
        next = min;
      } else if (e.key === "End") {
        next = max;
      }
      if (next === null) return;
      e.preventDefault();
      emitValue(thumb, next);
    },
    [
      clampedValue,
      disabled,
      emitValue,
      max,
      min,
      range,
      rangeEnd,
      rangeStart,
      step,
    ],
  );

  const toPercent = (value: number) =>
    max > min ? ((value - min) / (max - min)) * 100 : 0;
  const pct = toPercent(clampedValue);
  const startPct = toPercent(rangeStart);
  const endPct = toPercent(rangeEnd);
  const markEntries = Object.entries(marks ?? {})
    .map(([key, label]): SliderMark | null => {
      const numeric = Number(key);
      return Number.isFinite(numeric) ? { value: numeric, label } : null;
    })
    .filter((mark): mark is SliderMark => mark !== null);

  const completeMarks = [...markEntries];
  for (const boundary of [min, max]) {
    if (!completeMarks.some((mark) => sameValue(mark.value, boundary, step))) {
      completeMarks.push({
        value: boundary,
        label: formatSliderValue(boundary),
      });
    }
  }
  for (const current of currentValues) {
    if (!completeMarks.some((mark) => sameValue(mark.value, current, step))) {
      completeMarks.push({
        value: current,
        label: formatSliderValue(current),
      });
    }
  }
  completeMarks.sort((a, b) => a.value - b.value);

  return (
    <div
      className={`rua-ui-slider${
        disabled ? " rua-ui-slider--disabled" : ""
      }${className ? ` ${className}` : ""}`}
      style={style}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      <div className="rua-ui-slider-track" ref={trackRef}>
        {range && (
          <div
            className="rua-ui-slider-range-before"
            style={{ width: `${startPct}%` }}
          />
        )}
        <div
          className="rua-ui-slider-fill"
          style={
            range
              ? { left: `${startPct}%`, width: `${endPct - startPct}%` }
              : { width: `${pct}%` }
          }
        />
        {(range ? [rangeStart, rangeEnd] : [clampedValue]).map(
          (thumbValue, index) => (
            <div
              key={index}
              role="slider"
              tabIndex={disabled ? -1 : 0}
              aria-label={range ? ariaLabels?.[index] : ariaLabel}
              aria-valuemin={
                range && index === 1 ? rangeStart : ariaValueMin ?? min
              }
              aria-valuemax={
                range && index === 0 ? rangeEnd : ariaValueMax ?? max
              }
              aria-valuenow={range ? thumbValue : ariaValueNow ?? thumbValue}
              aria-valuetext={range ? undefined : ariaValueText}
              aria-disabled={disabled}
              data-slider-thumb={index}
              className={`rua-ui-slider-thumb rua-ui-slider-thumb--${
                index + 1
              } rua-slider-handle-${index + 1}`}
              style={{ left: `${toPercent(thumbValue)}%` }}
              onKeyDown={(event) => onThumbKeyDown(index as 0 | 1, event)}
            />
          ),
        )}
      </div>
      <div className="rua-ui-slider-marks" aria-hidden="true">
        {completeMarks.map((mark) => {
          const markPct =
            max > min
              ? ((clamp(mark.value, min, max) - min) / (max - min)) * 100
              : 0;
          const isMin = sameValue(mark.value, min, step);
          const isMax = sameValue(mark.value, max, step);
          const isCurrent = currentValues.some((current) =>
            sameValue(mark.value, current, step),
          );
          return (
            <span
              key={`${mark.value}`}
              className={[
                "rua-ui-slider-mark",
                isMin ? "rua-ui-slider-mark--min" : "",
                isMax ? "rua-ui-slider-mark--max" : "",
                isCurrent ? "rua-ui-slider-mark--current" : "",
              ]
                .filter(Boolean)
                .join(" ")}
              style={{ left: `${markPct}%` }}
            >
              {mark.label}
            </span>
          );
        })}
      </div>
    </div>
  );
}
