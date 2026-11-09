import { useState, useRef, useEffect, useCallback } from "react";
import dayjs, { type Dayjs } from "dayjs";
import { ArrowRight, CalendarDays } from "lucide-react";
import { useTranslation } from "react-i18next";
import "./datepicker.css";

interface DatePickerProps {
  value?: string | null;
  onChange?: (value: string) => void;
  format?: string;
  placeholder?: string;
  disabled?: boolean;
  disabledDate?: (current: Dayjs) => boolean;
  className?: string;
  popupClassName?: string;
  ariaLabel?: string;
  panelLabel?: string;
}

interface RangePickerProps {
  value?: [Dayjs | null, Dayjs | null] | null;
  onChange?: (value: [Dayjs | null, Dayjs | null] | null) => void;
  format?: string;
  placeholder?: [string, string];
  disabled?: boolean;
  disabledDate?: (current: Dayjs) => boolean;
  className?: string;
  popupClassName?: string;
}

function pad(n: number) {
  return n < 10 ? "0" + n : "" + n;
}

function formatDate(y: number, m: number, d: number, fmt: string) {
  return fmt
    .replace("YYYY", "" + y)
    .replace("MM", pad(m + 1))
    .replace("DD", pad(d));
}

function parseDate(
  s: string,
): { year: number; month: number; day: number } | null {
  const m = s.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (!m) return null;
  return { year: +m[1], month: +m[2] - 1, day: +m[3] };
}

function daysInMonth(year: number, month: number) {
  return new Date(year, month + 1, 0).getDate();
}

function firstDayOfWeek(year: number, month: number) {
  return new Date(year, month, 1).getDay();
}

function intlLocale(language: string): string {
  if (language.startsWith("zh")) return "zh-CN";
  if (language.startsWith("ja")) return "ja-JP";
  if (language.startsWith("ru")) return "ru-RU";
  return "en-US";
}

function CalendarPanel({
  viewYear,
  viewMonth,
  selected,
  onSelect,
  onPrev,
  onNext,
  onPrevYear,
  onNextYear,
  disabledDate,
  className,
  ariaLabel,
}: {
  viewYear: number;
  viewMonth: number;
  selected: { year: number; month: number; day: number } | null;
  onSelect: (y: number, m: number, d: number) => void;
  onPrev: () => void;
  onNext: () => void;
  onPrevYear: () => void;
  onNextYear: () => void;
  disabledDate?: (current: Dayjs) => boolean;
  className?: string;
  ariaLabel?: string;
}) {
  const { t, i18n } = useTranslation();
  const locale = intlLocale(i18n.resolvedLanguage || i18n.language);
  const total = daysInMonth(viewYear, viewMonth);
  const startDay = firstDayOfWeek(viewYear, viewMonth);
  const today = new Date();
  const todayY = today.getFullYear();
  const todayM = today.getMonth();
  const todayD = today.getDate();

  const cells: (number | null)[] = [];
  for (let i = 0; i < startDay; i++) cells.push(null);
  for (let d = 1; d <= total; d++) cells.push(d);

  const monthLabel = new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "short",
  }).format(new Date(viewYear, viewMonth, 1));
  const weekdayFormatter = new Intl.DateTimeFormat(locale, {
    weekday: "short",
  });
  const weekdays = Array.from({ length: 7 }, (_, index) =>
    weekdayFormatter.format(new Date(2026, 0, 4 + index)).replace(/\.$/, ""),
  );
  const dateLabelFormatter = new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "long",
    day: "numeric",
  });

  return (
    <div
      className={["rua-ui-datepicker-panel", className]
        .filter(Boolean)
        .join(" ")}
      role="dialog"
      aria-label={ariaLabel}
    >
      <div className="rua-ui-datepicker-header">
        <button
          type="button"
          className="rua-ui-datepicker-nav"
          aria-label={t("datePicker.previousYear")}
          title={t("datePicker.previousYear")}
          onClick={onPrevYear}
        >
          &laquo;
        </button>
        <button
          type="button"
          className="rua-ui-datepicker-nav"
          aria-label={t("datePicker.previousMonth")}
          title={t("datePicker.previousMonth")}
          onClick={onPrev}
        >
          &lsaquo;
        </button>
        <span className="rua-ui-datepicker-title">{monthLabel}</span>
        <button
          type="button"
          className="rua-ui-datepicker-nav"
          aria-label={t("datePicker.nextMonth")}
          title={t("datePicker.nextMonth")}
          onClick={onNext}
        >
          &rsaquo;
        </button>
        <button
          type="button"
          className="rua-ui-datepicker-nav"
          aria-label={t("datePicker.nextYear")}
          title={t("datePicker.nextYear")}
          onClick={onNextYear}
        >
          &raquo;
        </button>
      </div>
      <div className="rua-ui-datepicker-weekdays">
        {weekdays.map((weekday, index) => (
          <span key={index} className="rua-ui-datepicker-weekday">
            {weekday}
          </span>
        ))}
      </div>
      <div className="rua-ui-datepicker-grid">
        {cells.map((day, i) => {
          if (day == null)
            return (
              <span
                key={"e" + i}
                className="rua-ui-datepicker-cell rua-ui-datepicker-cell--empty"
              />
            );
          const isToday =
            viewYear === todayY && viewMonth === todayM && day === todayD;
          const isSelected =
            selected != null &&
            viewYear === selected.year &&
            viewMonth === selected.month &&
            day === selected.day;
          const current = dayjs(new Date(viewYear, viewMonth, day));
          const isDisabled = disabledDate?.(current) ?? false;
          const cls = [
            "rua-ui-datepicker-cell",
            isToday && "rua-ui-datepicker-cell--today",
            isSelected && "rua-ui-datepicker-cell--selected",
          ]
            .filter(Boolean)
            .join(" ");
          return (
            <button
              key={day}
              type="button"
              className={cls}
              aria-label={dateLabelFormatter.format(current.toDate())}
              disabled={isDisabled}
              onClick={() => onSelect(viewYear, viewMonth, day)}
            >
              {day}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function RangePicker({
  value,
  onChange,
  format = "YYYY-MM-DD",
  placeholder,
  disabled = false,
  disabledDate,
  className,
  popupClassName,
}: RangePickerProps) {
  const { t } = useTranslation();
  const [startDate, endDate] = value ?? [null, null];
  const labels = placeholder ?? [
    t("datePicker.startDate"),
    t("datePicker.endDate"),
  ];

  const updateRange = (index: 0 | 1, rawValue: string) => {
    const selected = dayjs(rawValue);
    if (!selected.isValid()) return;

    let nextStart = index === 0 ? selected : startDate;
    let nextEnd = index === 1 ? selected : endDate;
    if (nextStart && nextEnd && nextStart.isAfter(nextEnd, "day")) {
      if (index === 0) nextEnd = selected;
      else nextStart = selected;
    }
    onChange?.([nextStart, nextEnd]);
  };

  return (
    <div
      className={["rua-ui-range-picker", className].filter(Boolean).join(" ")}
      role="group"
      aria-label={t("datePicker.dateRange")}
    >
      <DatePickerInner
        value={startDate?.format(format) ?? null}
        onChange={(next) => updateRange(0, next)}
        format={format}
        placeholder={labels[0]}
        disabled={disabled}
        disabledDate={disabledDate}
        className="rua-ui-range-picker-start"
        popupClassName={popupClassName}
        ariaLabel={labels[0]}
        panelLabel={t("datePicker.chooseStartDate")}
      />
      <ArrowRight
        className="rua-ui-range-picker-separator"
        size={14}
        aria-hidden="true"
      />
      <DatePickerInner
        value={endDate?.format(format) ?? null}
        onChange={(next) => updateRange(1, next)}
        format={format}
        placeholder={labels[1]}
        disabled={disabled}
        disabledDate={disabledDate}
        className="rua-ui-range-picker-end"
        popupClassName={popupClassName}
        ariaLabel={labels[1]}
        panelLabel={t("datePicker.chooseEndDate")}
      />
      <CalendarDays
        className="rua-ui-range-picker-icon"
        size={15}
        aria-hidden="true"
      />
    </div>
  );
}

function DatePickerInner({
  value,
  onChange,
  format = "YYYY-MM-DD",
  placeholder,
  disabled = false,
  disabledDate,
  className,
  popupClassName,
  ariaLabel,
  panelLabel,
}: DatePickerProps) {
  const { t } = useTranslation();
  const effectivePlaceholder = placeholder ?? t("datePicker.selectDate");
  const [open, setOpen] = useState(false);
  const parsed = value ? parseDate(value) : null;
  const now = new Date();
  const [viewYear, setViewYear] = useState(parsed?.year ?? now.getFullYear());
  const [viewMonth, setViewMonth] = useState(parsed?.month ?? now.getMonth());
  const containerRef = useRef<HTMLDivElement>(null);
  const parsedYear = parsed?.year;
  const parsedMonth = parsed?.month;

  useEffect(() => {
    if (open && parsedYear !== undefined && parsedMonth !== undefined) {
      setViewYear(parsedYear);
      setViewMonth(parsedMonth);
    }
  }, [open, parsedMonth, parsedYear]);

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

  function handleSelect(y: number, m: number, d: number) {
    onChange?.(formatDate(y, m, d, format));
    setOpen(false);
  }

  function prevMonth() {
    if (viewMonth === 0) {
      setViewMonth(11);
      setViewYear((y) => y - 1);
    } else {
      setViewMonth((m) => m - 1);
    }
  }

  function nextMonth() {
    if (viewMonth === 11) {
      setViewMonth(0);
      setViewYear((y) => y + 1);
    } else {
      setViewMonth((m) => m + 1);
    }
  }

  const display = parsed
    ? formatDate(parsed.year, parsed.month, parsed.day, format)
    : "";

  const wrapCls = ["rua-ui-datepicker", className].filter(Boolean).join(" ");

  return (
    <div ref={containerRef} className={wrapCls}>
      <input
        type="text"
        className="rua-input rua-ui-datepicker-input"
        value={display}
        placeholder={effectivePlaceholder}
        disabled={disabled}
        aria-label={ariaLabel ?? effectivePlaceholder}
        readOnly
        onClick={() => !disabled && setOpen((o) => !o)}
      />
      {open && (
        <CalendarPanel
          viewYear={viewYear}
          viewMonth={viewMonth}
          selected={parsed}
          onSelect={handleSelect}
          onPrev={prevMonth}
          onNext={nextMonth}
          onPrevYear={() => setViewYear((y) => y - 1)}
          onNextYear={() => setViewYear((y) => y + 1)}
          disabledDate={disabledDate}
          className={popupClassName}
          ariaLabel={panelLabel ?? t("datePicker.chooseDate")}
        />
      )}
    </div>
  );
}

const DatePicker = Object.assign(DatePickerInner, { RangePicker });
export default DatePicker;
