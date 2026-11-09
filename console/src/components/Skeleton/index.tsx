import type { CSSProperties } from "react";
import styles from "./index.module.less";

export type SkeletonVariant = "list" | "card" | "detail" | "table";

export interface SkeletonProps {
  variant?: SkeletonVariant;
  count?: number;
  className?: string;
  style?: CSSProperties;
}

function defaultCount(variant: SkeletonVariant): number {
  switch (variant) {
    case "card":
      return 4;
    case "detail":
      return 1;
    case "table":
      return 6;
    case "list":
    default:
      return 5;
  }
}

function Bar({ className }: { className?: string }) {
  return <div className={`${styles.shimmer} ${className ?? ""}`} />;
}

function ListSkeleton({ count }: { count: number }) {
  return (
    <div className={styles.listRoot}>
      {Array.from({ length: count }).map((_, i) => (
        <div key={i} className={styles.listRow}>
          <div
            className={`${styles.shimmer} ${styles.avatarBlock}`}
            style={{ width: 40, height: 40 }}
          />
          <div className={styles.listRowText}>
            <Bar className={styles.barTitle} />
            <Bar className={styles.barSub} />
          </div>
        </div>
      ))}
    </div>
  );
}

function CardSkeleton({ count }: { count: number }) {
  return (
    <div className={styles.cardGrid}>
      {Array.from({ length: count }).map((_, i) => (
        <div key={i} className={styles.cardItem}>
          <div className={styles.cardHead}>
            <div
              className={`${styles.shimmer} ${styles.avatarBlock}`}
              style={{ width: 32, height: 32 }}
            />
            <Bar className={styles.cardTitleBar} />
          </div>
          <div className={styles.paragraphRows}>
            <div
              className={`${styles.shimmer} ${styles.paragraphRow}`}
              style={{ width: "100%" }}
            />
            <div
              className={`${styles.shimmer} ${styles.paragraphRow}`}
              style={{ width: "92%" }}
            />
            <div
              className={`${styles.shimmer} ${styles.paragraphRow}`}
              style={{ width: "70%" }}
            />
          </div>
        </div>
      ))}
    </div>
  );
}

function DetailSkeleton({ count }: { count: number }) {
  return (
    <div className={styles.detailRoot}>
      {Array.from({ length: count }).map((_, i) => (
        <div key={i} className={styles.detailBlock}>
          <Bar className={styles.detailTitle} />
          <div className={styles.paragraphRows}>
            <div
              className={`${styles.shimmer} ${styles.paragraphRow}`}
              style={{ width: "100%" }}
            />
            <div
              className={`${styles.shimmer} ${styles.paragraphRow}`}
              style={{ width: "96%" }}
            />
            <div
              className={`${styles.shimmer} ${styles.paragraphRow}`}
              style={{ width: "88%" }}
            />
            <div
              className={`${styles.shimmer} ${styles.paragraphRow}`}
              style={{ width: "60%" }}
            />
          </div>
        </div>
      ))}
    </div>
  );
}

function TableSkeleton({ count }: { count: number }) {
  return (
    <div className={styles.tableRoot}>
      <div className={`${styles.tableRow} ${styles.tableHeader}`}>
        <Bar className={styles.tableCell} />
        <Bar className={styles.tableCell} />
        <Bar className={styles.tableCell} />
        <Bar className={styles.tableCellNarrow} />
      </div>
      {Array.from({ length: count }).map((_, i) => (
        <div key={i} className={styles.tableRow}>
          <Bar className={styles.tableCell} />
          <Bar className={styles.tableCell} />
          <Bar className={styles.tableCell} />
          <Bar className={styles.tableCellNarrow} />
        </div>
      ))}
    </div>
  );
}

export function Skeleton({
  variant = "list",
  count,
  className,
  style,
}: SkeletonProps) {
  const n = count ?? defaultCount(variant);
  const rootClass = `${styles.root} ${className ?? ""}`.trim();

  let body;
  switch (variant) {
    case "card":
      body = <CardSkeleton count={n} />;
      break;
    case "detail":
      body = <DetailSkeleton count={n} />;
      break;
    case "table":
      body = <TableSkeleton count={n} />;
      break;
    case "list":
    default:
      body = <ListSkeleton count={n} />;
      break;
  }

  return (
    <div
      className={rootClass}
      style={style}
      aria-busy="true"
      aria-live="polite"
    >
      {body}
    </div>
  );
}

export default Skeleton;
