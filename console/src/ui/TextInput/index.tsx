import {
  forwardRef,
  useImperativeHandle,
  useRef,
  useState,
  type InputHTMLAttributes,
} from "react";
import { X } from "lucide-react";
import "./text-input.css";

interface TextInputProps extends InputHTMLAttributes<HTMLInputElement> {
  allowClear?: boolean;
  clearLabel?: string;
  onClear?: () => void;
}

export default forwardRef<HTMLInputElement, TextInputProps>(function TextInput(
  {
    autoComplete = "off",
    autoCapitalize = "off",
    autoCorrect = "off",
    spellCheck = false,
    allowClear = false,
    className,
    clearLabel = "Clear",
    defaultValue,
    onChange,
    onClear,
    value,
    ...props
  },
  ref,
) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [uncontrolledValue, setUncontrolledValue] = useState(
    defaultValue ?? "",
  );
  const currentValue = value === undefined ? uncontrolledValue : value;
  const hasValue = String(currentValue).length > 0;

  useImperativeHandle(ref, () => inputRef.current as HTMLInputElement);

  if (!allowClear) {
    return (
      <input
        ref={inputRef}
        className={`rua-input${className ? ` ${className}` : ""}`}
        defaultValue={defaultValue}
        value={value}
        onChange={onChange}
        autoComplete={autoComplete}
        autoCapitalize={autoCapitalize}
        autoCorrect={autoCorrect}
        spellCheck={spellCheck}
        {...props}
      />
    );
  }

  const input = (
    <input
      ref={inputRef}
      className={`rua-input${className ? ` ${className}` : ""}`}
      value={currentValue}
      onChange={(event) => {
        if (value === undefined) setUncontrolledValue(event.target.value);
        onChange?.(event);
      }}
      autoComplete={autoComplete}
      autoCapitalize={autoCapitalize}
      autoCorrect={autoCorrect}
      spellCheck={spellCheck}
      {...props}
    />
  );

  return (
    <span className="rua-input-clearable">
      {input}
      {hasValue && !props.disabled && !props.readOnly && (
        <button
          type="button"
          className="rua-input-clear"
          aria-label={clearLabel}
          onClick={() => {
            if (value === undefined) setUncontrolledValue("");
            onClear?.();
            inputRef.current?.focus();
          }}
        >
          <X size={14} aria-hidden="true" />
        </button>
      )}
    </span>
  );
});

export type { TextInputProps };
