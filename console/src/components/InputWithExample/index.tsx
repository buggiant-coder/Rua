import { ClipboardPaste } from "lucide-react";
import { forwardRef, useCallback, useMemo } from "react";
import TextInput from "@/ui/TextInput";
import { Tooltip } from "@/ui";

export interface InputWithExampleProps
  extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "value"> {
  value?: unknown;
  example?: string;
  exampleTooltip?: string;
  onPressEnter?: (e: React.KeyboardEvent<HTMLInputElement>) => void;
}

export const InputWithExample = forwardRef<
  HTMLInputElement,
  InputWithExampleProps
>(function InputWithExample(
  {
    example,
    exampleTooltip,
    placeholder,
    value,
    onChange,
    onPressEnter,
    onKeyDown,
    style,
    ...rest
  },
  ref,
) {
  const exampleValue =
    example ?? (typeof placeholder === "string" ? placeholder : "");

  const fillExample = useCallback(() => {
    if (!exampleValue) return;
    const synthetic = {
      target: { value: exampleValue },
      currentTarget: { value: exampleValue },
    } as React.ChangeEvent<HTMLInputElement>;
    onChange?.(synthetic);
  }, [exampleValue, onChange]);

  const isEmpty = value === undefined || value === null || value === "";

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Enter") onPressEnter?.(e);
      onKeyDown?.(e);
    },
    [onPressEnter, onKeyDown],
  );

  const button = useMemo(() => {
    if (!exampleValue) return undefined;
    const visible = isEmpty;
    const btn = (
      <span
        role="button"
        tabIndex={-1}
        onMouseDown={(e) => e.preventDefault()}
        onClick={visible ? fillExample : undefined}
        style={{
          cursor: visible ? "pointer" : "default",
          color: "var(--text-3)",
          display: "inline-flex",
          alignItems: "center",
          visibility: visible ? "visible" : "hidden",
          pointerEvents: visible ? "auto" : "none",
        }}
      >
        <ClipboardPaste size={14} />
      </span>
    );
    return exampleTooltip ? (
      <Tooltip title={exampleTooltip} open={visible ? undefined : false}>
        {btn}
      </Tooltip>
    ) : (
      btn
    );
  }, [exampleValue, isEmpty, fillExample, exampleTooltip]);

  const inputValue = value as string | number | readonly string[] | undefined;

  return (
    <div style={{ position: "relative", display: "flex", width: "100%" }}>
      <TextInput
        ref={ref}
        placeholder={placeholder}
        value={inputValue}
        onChange={onChange}
        onKeyDown={onPressEnter || onKeyDown ? handleKeyDown : undefined}
        style={{
          ...style,
          paddingRight: button ? 32 : undefined,
          width: "100%",
        }}
        {...rest}
      />
      {button && (
        <span
          style={{
            position: "absolute",
            right: 8,
            top: "50%",
            transform: "translateY(-50%)",
            lineHeight: 0,
          }}
        >
          {button}
        </span>
      )}
    </div>
  );
});

export interface PasswordWithExampleProps
  extends React.InputHTMLAttributes<HTMLInputElement> {
  example?: string;
  exampleTooltip?: string;
}

export const PasswordWithExample = forwardRef<
  HTMLInputElement,
  PasswordWithExampleProps
>(function PasswordWithExample(
  { example, exampleTooltip, placeholder, value, onChange, style, ...rest },
  ref,
) {
  const exampleValue =
    example ?? (typeof placeholder === "string" ? placeholder : "");

  const fillExample = useCallback(() => {
    if (!exampleValue) return;
    const synthetic = {
      target: { value: exampleValue },
      currentTarget: { value: exampleValue },
    } as React.ChangeEvent<HTMLInputElement>;
    onChange?.(synthetic);
  }, [exampleValue, onChange]);

  const isEmpty = value === undefined || value === null || value === "";

  const trigger = useMemo(() => {
    if (!exampleValue) return undefined;
    const visible = isEmpty;
    const btn = (
      <span
        role="button"
        tabIndex={-1}
        onMouseDown={(e) => e.preventDefault()}
        onClick={visible ? fillExample : undefined}
        style={{
          cursor: visible ? "pointer" : "default",
          color: "var(--text-3)",
          display: "inline-flex",
          alignItems: "center",
          visibility: visible ? "visible" : "hidden",
          pointerEvents: visible ? "auto" : "none",
        }}
      >
        <ClipboardPaste size={14} />
      </span>
    );
    return exampleTooltip ? (
      <Tooltip title={exampleTooltip} open={visible ? undefined : false}>
        {btn}
      </Tooltip>
    ) : (
      btn
    );
  }, [exampleValue, isEmpty, fillExample, exampleTooltip]);

  return (
    <div style={{ position: "relative", display: "flex", width: "100%" }}>
      <TextInput
        ref={ref}
        type="password"
        placeholder={placeholder}
        value={value}
        onChange={onChange}
        style={{
          ...style,
          paddingRight: trigger ? 32 : undefined,
          width: "100%",
        }}
        {...rest}
      />
      {trigger && (
        <span
          style={{
            position: "absolute",
            right: 8,
            top: "50%",
            transform: "translateY(-50%)",
            lineHeight: 0,
          }}
        >
          {trigger}
        </span>
      )}
    </div>
  );
});
