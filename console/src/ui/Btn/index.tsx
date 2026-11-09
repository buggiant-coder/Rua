import { forwardRef } from "react";
import type { ReactNode, ButtonHTMLAttributes } from "react";
import "./btn.css";

interface BtnProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "filled" | "outline" | "ghost" | "danger" | "danger-filled";
  size?: "sm" | "md" | "lg";
  icon?: ReactNode;
  loading?: boolean;
}

export default forwardRef<HTMLButtonElement, BtnProps>(function Btn(
  {
    variant = "filled",
    size = "md",
    icon,
    loading,
    disabled,
    className,
    children,
    ...rest
  },
  ref,
) {
  const iconOnly = !!icon && !children;
  const cls = [
    "rua-btn",
    `rua-btn--${variant}`,
    size !== "md" && `rua-btn--${size}`,
    iconOnly && "rua-btn--icon-only",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <button ref={ref} className={cls} disabled={disabled || loading} {...rest}>
      {loading && <span className="rua-btn__spinner" />}
      {icon && !loading && icon}
      {children}
    </button>
  );
});
