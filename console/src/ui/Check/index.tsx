import { forwardRef } from "react";
import "./check.css";

export default forwardRef<
  HTMLInputElement,
  Omit<React.InputHTMLAttributes<HTMLInputElement>, "type">
>(function Check({ className, children, ...props }, ref) {
  return (
    <label
      className={`rua-check${props.disabled ? " rua-check--disabled" : ""}${
        className ? ` ${className}` : ""
      }`}
    >
      <input
        ref={ref}
        type="checkbox"
        className="rua-check-input"
        {...props}
      />
      <span className="rua-check-box" />
      {children && <span className="rua-check-label">{children}</span>}
    </label>
  );
});
