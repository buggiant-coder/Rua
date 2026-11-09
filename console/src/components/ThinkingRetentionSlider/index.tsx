import { Slider } from "@/ui";
import { useTranslation } from "react-i18next";

interface ThinkingRetentionSliderProps {
  value: number;
  onChange: (value: number) => void;
  disabled?: boolean;
}

const NONE_POSITION = 4;
const MAX_TURNS = 20;
const MAX_POSITION = MAX_TURNS + NONE_POSITION;

function toPosition(value: number): number {
  if (value < 0) return 0;
  if (value === 0) return NONE_POSITION;
  return Math.min(value, MAX_TURNS) + NONE_POSITION;
}

function fromPosition(position: number): number {
  if (position < NONE_POSITION) return -1;
  if (position === NONE_POSITION) return 0;
  return position - NONE_POSITION;
}

export function ThinkingRetentionSlider({
  value,
  onChange,
  disabled = false,
}: ThinkingRetentionSliderProps) {
  const { t } = useTranslation();
  const normalized = value < 0 ? -1 : Math.min(value, MAX_TURNS);
  const position = toPosition(normalized);
  const valueText =
    normalized === -1
      ? t("agentConfig.thinkingTurnsAll")
      : normalized === 0
      ? t("agentConfig.thinkingTurnsNone")
      : t("agentConfig.thinkingTurnsCount", { count: normalized });
  const currentMark = normalized <= 0 ? valueText : normalized.toString();

  return (
    <Slider
      min={0}
      max={MAX_POSITION}
      step={1}
      disabled={disabled}
      value={position}
      marks={{
        0: t("agentConfig.thinkingTurnsAll"),
        [NONE_POSITION]: t("agentConfig.thinkingTurnsNone"),
        [position]: currentMark,
        [MAX_POSITION]: MAX_TURNS.toString(),
      }}
      ariaLabel={t("agentConfig.thinkingTurnsToKeep")}
      ariaValueMin={-1}
      ariaValueMax={MAX_TURNS}
      ariaValueNow={normalized}
      ariaValueText={valueText}
      onChange={(next: number) => onChange(fromPosition(next))}
    />
  );
}
