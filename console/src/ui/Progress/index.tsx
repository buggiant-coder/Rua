import "./progress.css";

interface ProgressProps {
  percent: number;
  color?: string;
  size?: "sm" | "md";
  className?: string;
  showInfo?: boolean;
  status?: string;
  strokeColor?: string;
  strokeWidth?: number;
}

export default function Progress({
  percent,
  color,
  size = "md",
  className,
}: ProgressProps) {
  const clamped = Math.max(0, Math.min(100, percent));
  const cls = [
    "rua-ui-progress",
    size !== "md" && "rua-ui-progress--sm",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className={cls}>
      <div className="rua-ui-progress-track">
        <div
          className="rua-ui-progress-bar"
          style={{
            width: `${clamped}%`,
            ...(color ? { background: color } : {}),
          }}
        />
      </div>
    </div>
  );
}
