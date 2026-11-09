import type { ReactNode } from "react";
import "./loading.css";

export interface LoadingProps {
  spinning?: boolean;
  children?: ReactNode;
  tip?: ReactNode;
  size?: "small" | "default" | "large" | "sm" | "md" | "lg";
  className?: string;
}

export function Loading({
  spinning = true,
  children,
  tip,
  size = "default",
  className,
}: LoadingProps) {
  const sizeClass = size !== "default" ? `rua-loading-${size}` : "";

  const spinner = (
    <div className={`rua-loading ${sizeClass}`}>
      <div className="rua-loading-dot" />
      {tip && <div className="rua-loading-tip">{tip}</div>}
    </div>
  );

  if (!children) {
    if (!spinning) return null;
    return <div className={className}>{spinner}</div>;
  }

  return (
    <div className={`rua-loading-nested ${className ?? ""}`.trim()}>
      {spinning && <div className="rua-loading-mask">{spinner}</div>}
      <div className={spinning ? "rua-loading-blur" : ""}>{children}</div>
    </div>
  );
}

export default Loading;
