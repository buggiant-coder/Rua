import { useTranslation } from "react-i18next";
import type { InstallTarget } from "../useHubInstall";
import styles from "./TargetToggle.module.less";
import { Btn } from "@/ui";

interface TargetToggleProps {
  target: InstallTarget;
  onChange: (next: InstallTarget) => void;
  size?: "sm" | "md" | "lg";
}

export function TargetToggle({ target, onChange, size }: TargetToggleProps) {
  const { t } = useTranslation();
  return (
    <div className={styles.targetToggle}>
      <Btn
        size={size}
        variant={target === "pool" ? "filled" : "outline"}
        onClick={() => onChange("pool")}
      >
        {t("market.targetPool")}
      </Btn>
      <Btn
        size={size}
        variant={target === "workspace" ? "filled" : "outline"}
        onClick={() => onChange("workspace")}
      >
        {t("market.targetWorkspace")}
      </Btn>
    </div>
  );
}
