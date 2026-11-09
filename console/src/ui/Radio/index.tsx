import { createContext, forwardRef, useContext } from "react";
import "./radio.css";

interface RadioGroupProps {
  value?: string;
  onChange?: (value: string) => void;
  disabled?: boolean;
  className?: string;
  children: React.ReactNode;
}

const RadioGroupContext = createContext<{
  value?: string;
  onChange?: (value: string) => void;
  disabled?: boolean;
}>({});

export function RadioGroup({
  value,
  onChange,
  disabled,
  className,
  children,
}: RadioGroupProps) {
  return (
    <RadioGroupContext.Provider value={{ value, onChange, disabled }}>
      <div
        role="radiogroup"
        className={`rua-radio-group${className ? ` ${className}` : ""}`}
      >
        {children}
      </div>
    </RadioGroupContext.Provider>
  );
}

export default forwardRef<
  HTMLInputElement,
  Omit<React.InputHTMLAttributes<HTMLInputElement>, "type">
>(function Radio({ className, children, value, ...props }, ref) {
  const group = useContext(RadioGroupContext);
  const isDisabled = props.disabled ?? group.disabled;
  const isChecked =
    group.value !== undefined ? group.value === value : props.checked;

  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    if (group.onChange && value !== undefined) {
      group.onChange(String(value));
    }
    props.onChange?.(e);
  };

  return (
    <label
      className={`rua-radio${isDisabled ? " rua-radio--disabled" : ""}${
        className ? ` ${className}` : ""
      }`}
    >
      <input
        ref={ref}
        type="radio"
        className="rua-radio-input"
        value={value}
        checked={isChecked}
        disabled={isDisabled}
        {...props}
        onChange={handleChange}
      />
      <span className="rua-radio-circle" />
      {children && <span className="rua-radio-label">{children}</span>}
    </label>
  );
});
