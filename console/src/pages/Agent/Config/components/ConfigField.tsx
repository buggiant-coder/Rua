import type { ReactNode } from "react";
import styles from "../index.module.less";
import { Tooltip } from "@/ui";
import { HelpCircle } from "lucide-react";

interface ConfigFieldProps {
  label?: string;
  tooltip?: string;
  error?: string;
  required?: boolean;
  children: ReactNode;
  className?: string;
}

export function ConfigField({
  label,
  tooltip,
  error,
  required,
  children,
  className,
}: ConfigFieldProps) {
  return (
    <div className={`${styles.configField}${className ? ` ${className}` : ""}`}>
      {label && (
        <label
          className={`${styles.configFieldLabel}${
            required ? ` ${styles.configFieldRequired}` : ""
          }`}
        >
          {label}
          {tooltip && (
            <Tooltip title={tooltip}>
              <HelpCircle size={14} className={styles.configFieldTooltip} />
            </Tooltip>
          )}
        </label>
      )}
      {children}
      {error && <span className={styles.configFieldError}>{error}</span>}
    </div>
  );
}
