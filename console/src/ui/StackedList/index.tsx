import type { HTMLAttributes, ReactNode } from "react";
import "./stacked-list.css";

export interface StackedListProps extends HTMLAttributes<HTMLDivElement> {
  children?: ReactNode;
}

export default function StackedList({
  children,
  className,
  ...rest
}: StackedListProps) {
  return (
    <div
      className={`stacked-list${className ? ` ${className}` : ""}`}
      {...rest}
    >
      {children}
    </div>
  );
}
