import styles from "../index.module.less";
import { Slider } from "@/ui";

interface SliderWithValueProps {
  value?: number;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  onChange?: (value: number) => void;
}

export function SliderWithValue({
  value,
  min = 0,
  max = 100,
  step,
  disabled,
  onChange,
}: SliderWithValueProps) {
  const formatted = value !== undefined ? formatValue(value) : "";
  const marks: Record<number, React.ReactNode> = {
    [min]: String(formatValue(min)),
    [max]: String(formatValue(max)),
  };

  if (value !== undefined && value !== min && value !== max) {
    marks[value] = (
      <span className={styles.sliderCurrentMark}>{formatted}</span>
    );
  }

  return (
    <Slider
      value={value}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      marks={marks}
      tooltip={{ open: false }}
      onChange={onChange}
    />
  );
}

function formatValue(v: number) {
  if (Number.isInteger(v)) return v.toString();
  return v.toFixed(2);
}
