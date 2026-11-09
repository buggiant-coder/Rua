import type { ReactNode } from "react";
import "./result.css";

interface ResultProps {
  status?: "success" | "error" | "info" | "warning";
  title?: ReactNode;
  subTitle?: ReactNode;
  extra?: ReactNode;
  className?: string;
  icon?: ReactNode;
}

const STATUS_ICONS: Record<string, string> = {
  success: "✓",
  error: "✕",
  info: "i",
  warning: "!",
};

export default function Result({
  status = "info",
  title,
  subTitle,
  extra,
  className,
  icon,
}: ResultProps) {
  const cls = ["rua-ui-result", className].filter(Boolean).join(" ");

  return (
    <div className={cls}>
      <div className={`rua-ui-result-icon rua-ui-result-icon--${status}`}>
        {icon ?? STATUS_ICONS[status]}
      </div>
      <div className="rua-ui-result-title">{title}</div>
      {subTitle && <div className="rua-ui-result-subtitle">{subTitle}</div>}
      {extra && <div className="rua-ui-result-extra">{extra}</div>}
    </div>
  );
}
