import { Radio, RadioGroup } from "@/ui";
import { useTranslation } from "react-i18next";
import type { ApprovalMode } from "../../../../api/modules/security";
import { CheckCircle2, Rocket, ShieldQuestion } from "lucide-react";
interface ApprovalModeSectionProps {
  value?: ApprovalMode;
  onChange?: (mode: ApprovalMode) => void;
}

const MODE_OPTIONS: {
  value: ApprovalMode;
  icon: React.ReactNode;
  titleKey: string;
  descKey: string;
}[] = [
  {
    value: "ask_first",
    icon: <ShieldQuestion />,
    titleKey: "security.approvalMode.askFirstTitle",
    descKey: "security.approvalMode.askFirstDesc",
  },
  {
    value: "hands_off",
    icon: <CheckCircle2 />,
    titleKey: "security.approvalMode.handsOffTitle",
    descKey: "security.approvalMode.handsOffDesc",
  },
  {
    value: "just_do_it",
    icon: <Rocket />,
    titleKey: "security.approvalMode.justDoItTitle",
    descKey: "security.approvalMode.justDoItDesc",
  },
];

export function ApprovalModeSection({
  value = "hands_off",
  onChange,
}: ApprovalModeSectionProps) {
  const { t } = useTranslation();

  return (
    <div style={{ marginBottom: 16 }}>
      <span style={{ display: "block", marginBottom: 8, fontWeight: 600 }}>
        {t("security.approvalMode.title")}
      </span>
      <span
        style={{
          display: "block",
          marginBottom: 12,
          fontSize: 13,
          color: "var(--text-2)",
        }}
      >
        {t("security.approvalMode.description")}
      </span>
      <RadioGroup
        value={value}
        onChange={(value) => onChange?.(value as ApprovalMode)}
      >
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "var(--space-2)",
          }}
        >
          {MODE_OPTIONS.map((opt) => (
            <Radio key={opt.value} value={opt.value}>
              <div
                style={{ display: "inline-flex", alignItems: "center", gap: 4 }}
              >
                {opt.icon}
                <span style={{ fontWeight: 600 }}>{t(opt.titleKey)}</span>
              </div>
              <br />
              <span style={{ fontSize: 12, color: "var(--text-2)" }}>
                {t(opt.descKey)}
              </span>
            </Radio>
          ))}
        </div>
      </RadioGroup>
    </div>
  );
}
