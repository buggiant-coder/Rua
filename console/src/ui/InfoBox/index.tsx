import type { ReactNode } from "react";
import { Info, AlertTriangle, CheckCircle, XCircle } from "lucide-react";
import "./infobox.css";

type InfoBoxType = "info" | "warning" | "success" | "error";

interface InfoBoxProps {
  type: InfoBoxType;
  icon?: ReactNode;
  title?: string;
  children?: ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

const DEFAULT_ICONS: Record<InfoBoxType, ReactNode> = {
  info: <Info size={16} />,
  warning: <AlertTriangle size={16} />,
  success: <CheckCircle size={16} />,
  error: <XCircle size={16} />,
};

export default function InfoBox({
  type,
  icon,
  title,
  children,
  className,
  style,
}: InfoBoxProps) {
  const displayIcon = icon === undefined ? DEFAULT_ICONS[type] : icon;

  return (
    <div
      className={`rua-infobox rua-infobox--${type}${
        className ? ` ${className}` : ""
      }`}
      style={style}
    >
      {displayIcon && (
        <span className="rua-infobox__icon">{displayIcon}</span>
      )}
      <div className="rua-infobox__body">
        {title && <div className="rua-infobox__title">{title}</div>}
        {children && <div className="rua-infobox__content">{children}</div>}
      </div>
    </div>
  );
}
