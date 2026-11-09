import { useState, useRef, useEffect, useCallback } from "react";
import "./timepicker.css";

interface TimePickerProps {
  value?: any;
  onChange?: (...args: any[]) => void;
  format?: string;
  placeholder?: string;
  disabled?: boolean;
  minuteStep?: number;
  className?: string;
  needConfirm?: boolean;
  style?: React.CSSProperties;
}

function pad(n: number) {
  return n < 10 ? "0" + n : "" + n;
}

function parseTime(s: string): { hour: number; minute: number } | null {
  const m = s.match(/^(\d{1,2}):(\d{2})$/);
  if (!m) return null;
  const h = +m[1];
  const min = +m[2];
  if (h < 0 || h > 23 || min < 0 || min > 59) return null;
  return { hour: h, minute: min };
}

function formatTime(hour: number, minute: number, fmt: string) {
  return fmt.replace("HH", pad(hour)).replace("mm", pad(minute));
}

function ScrollColumn({
  items,
  selected,
  onSelect,
  label,
}: {
  items: number[];
  selected: number | null;
  onSelect: (v: number) => void;
  label: string;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const selectedRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (selectedRef.current && listRef.current) {
      const container = listRef.current;
      const el = selectedRef.current;
      container.scrollTop =
        el.offsetTop - container.clientHeight / 2 + el.clientHeight / 2;
    }
  }, [selected]);

  return (
    <div
      className="rua-ui-timepicker-column"
      role="listbox"
      aria-label={label}
      ref={listRef}
    >
      {items.map((v) => {
        const isSelected = v === selected;
        const cls = [
          "rua-ui-timepicker-option",
          isSelected && "rua-ui-timepicker-option--selected",
        ]
          .filter(Boolean)
          .join(" ");
        return (
          <button
            key={v}
            type="button"
            ref={isSelected ? selectedRef : undefined}
            className={cls}
            role="option"
            aria-selected={isSelected}
            onClick={() => onSelect(v)}
          >
            {pad(v)}
          </button>
        );
      })}
    </div>
  );
}

export default function TimePicker({
  value,
  onChange,
  format = "HH:mm",
  placeholder = "Select time",
  disabled = false,
  minuteStep = 1,
  className,
}: TimePickerProps) {
  const [open, setOpen] = useState(false);
  const normalizedValue =
    value && typeof value === "object" && typeof value.format === "function"
      ? value.format("HH:mm")
      : value;
  const parsed = normalizedValue ? parseTime(String(normalizedValue)) : null;
  const [selHour, setSelHour] = useState<number | null>(parsed?.hour ?? null);
  const [selMinute, setSelMinute] = useState<number | null>(
    parsed?.minute ?? null,
  );
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open && parsed) {
      setSelHour(parsed.hour);
      setSelMinute(parsed.minute);
    }
  }, [open]);

  const closeOnOutside = useCallback((e: MouseEvent) => {
    if (
      containerRef.current &&
      !containerRef.current.contains(e.target as Node)
    ) {
      setOpen(false);
    }
  }, []);

  useEffect(() => {
    if (open) {
      document.addEventListener("mousedown", closeOnOutside);
      return () => document.removeEventListener("mousedown", closeOnOutside);
    }
  }, [open, closeOnOutside]);

  const hours = Array.from({ length: 24 }, (_, i) => i);
  const step = Math.max(1, Math.min(60, minuteStep));
  const minutes = Array.from(
    { length: Math.ceil(60 / step) },
    (_, i) => i * step,
  );

  function handleHourSelect(h: number) {
    setSelHour(h);
    const m = selMinute ?? 0;
    onChange?.(formatTime(h, m, format));
  }

  function handleMinuteSelect(m: number) {
    setSelMinute(m);
    const h = selHour ?? 0;
    onChange?.(formatTime(h, m, format));
  }

  const display = parsed ? formatTime(parsed.hour, parsed.minute, format) : "";

  const wrapCls = ["rua-ui-timepicker", className].filter(Boolean).join(" ");

  return (
    <div ref={containerRef} className={wrapCls}>
      <input
        type="text"
        className="rua-input rua-ui-timepicker-input"
        value={display}
        placeholder={placeholder}
        disabled={disabled}
        readOnly
        onClick={() => !disabled && setOpen((o) => !o)}
      />
      {open && (
        <div className="rua-ui-timepicker-panel">
          <ScrollColumn
            items={hours}
            selected={selHour}
            onSelect={handleHourSelect}
            label="Hour"
          />
          <ScrollColumn
            items={minutes}
            selected={selMinute}
            onSelect={handleMinuteSelect}
            label="Minute"
          />
        </div>
      )}
    </div>
  );
}
