import { useTranslation } from "react-i18next";
import "./enable-disable-switch.css";

interface EnableDisableSwitchProps {
  enabled: boolean;
  onChange: (enabled: boolean) => void;
  disabled?: boolean;
  className?: string;

  enabledLabel?: string;

  disabledLabel?: string;
}

export default function EnableDisableSwitch({
  enabled,
  onChange,
  disabled,
  className,
  enabledLabel,
  disabledLabel,
}: EnableDisableSwitchProps) {
  const { t } = useTranslation();
  return (
    <span
      className={[
        "rua-enable-disable-switch",
        disabled ? "rua-enable-disable-switch--disabled" : "",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <button
        type="button"
        aria-pressed={!enabled}
        className={!enabled ? "is-selected" : undefined}
        disabled={disabled}
        onClick={() => onChange(false)}
      >
        {disabledLabel ?? t("common.disable")}
      </button>
      <button
        type="button"
        aria-pressed={enabled}
        className={enabled ? "is-selected is-enabled" : undefined}
        disabled={disabled}
        onClick={() => onChange(true)}
      >
        {enabledLabel ?? t("common.enable")}
      </button>
    </span>
  );
}
