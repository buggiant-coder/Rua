import type { ReactNode } from "react";
import "./form-field.css";

interface FormFieldProps {
  label?: string;
  error?: string;
  required?: boolean;
  children: ReactNode;
  className?: string;
}

export default function FormField({
  label,
  error,
  required,
  children,
  className,
}: FormFieldProps) {
  return (
    <div className={`rua-field${className ? ` ${className}` : ""}`}>
      {label && (
        <label
          className={`rua-field-label${
            required ? " rua-field-required" : ""
          }`}
        >
          {label}
        </label>
      )}
      {children}
      {error && <span className="rua-field-error">{error}</span>}
    </div>
  );
}
