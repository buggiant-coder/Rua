import "./tag.css";

export interface TagProps {
  color?: "primary" | "danger" | "success" | "warning" | "default";
  closable?: boolean;
  onClose?: () => void;
  children?: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

export default function Tag({
  color = "default",
  closable,
  onClose,
  children,
  className,
  style,
}: TagProps) {
  return (
    <span
      className={`rua-tag rua-tag--${color}${
        className ? ` ${className}` : ""
      }`}
      style={style}
    >
      {children}
      {closable && (
        <span className="rua-tag__close" onClick={onClose} role="button">
          &times;
        </span>
      )}
    </span>
  );
}
