import "./spinner.css";

interface SpinnerProps {
  size?: "sm" | "md";
}

export default function Spinner({ size = "sm" }: SpinnerProps) {
  return <span className={`rua-spinner rua-spinner--${size}`} />;
}
