import { forwardRef } from "react";
import "./toggle.css";

export default forwardRef<
  HTMLInputElement,
  Omit<React.InputHTMLAttributes<HTMLInputElement>, "type">
>(function Toggle({ className, ...props }, ref) {
  return (
    <label
      className={`rua-toggle${
        props.disabled ? " rua-toggle--disabled" : ""
      }${className ? ` ${className}` : ""}`}
    >
      <input
        ref={ref}
        type="checkbox"
        className="rua-toggle-input"
        {...props}
      />
      <span className="rua-toggle-track">
        <span className="rua-toggle-thumb" />
      </span>
    </label>
  );
});
