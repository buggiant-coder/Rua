import type { ReactNode } from "react";
import styles from "./index.module.less";

export interface PageProps {
  children: ReactNode;
  className?: string;
}

export function Page({ children, className }: PageProps) {
  return (
    <div className={`${styles.page} ${className ?? ""}`.trim()}>{children}</div>
  );
}

export interface PageBodyProps {
  children: ReactNode;
  className?: string;
}

export function PageBody({ children, className }: PageBodyProps) {
  return (
    <div className={`${styles.pageBody} ${className ?? ""}`.trim()}>
      {children}
    </div>
  );
}
