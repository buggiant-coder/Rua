import { useTranslation } from "react-i18next";
import styles from "../../index.module.less";
import { Btn } from "@/ui";

interface LoadingStateProps {
  message: string;
  error?: boolean;
  onRetry?: () => void;
  className?: string;
}

export function LoadingState({
  message,
  error,
  onRetry,
  className,
}: LoadingStateProps) {
  const { t } = useTranslation();

  return (
    <div className={`${styles.loading} ${className || ""}`}>
      <span
        className={styles.loadingText}
        style={{ color: error ? "var(--danger-fg)" : undefined }}
      >
        {message}
      </span>
      {error && onRetry && (
        <Btn onClick={onRetry} style={{ marginTop: 12 }}>
          {t("models.retry")}
        </Btn>
      )}
    </div>
  );
}
