import { forwardRef } from "react";
import "./textarea.css";

export default forwardRef<
  HTMLTextAreaElement,
  React.TextareaHTMLAttributes<HTMLTextAreaElement>
>(function TextArea(
  {
    className,
    autoComplete = "off",
    autoCapitalize = "off",
    autoCorrect = "off",
    spellCheck = false,
    ...props
  },
  ref,
) {
  return (
    <textarea
      ref={ref}
      className={`rua-textarea${className ? ` ${className}` : ""}`}
      autoComplete={autoComplete}
      autoCapitalize={autoCapitalize}
      autoCorrect={autoCorrect}
      spellCheck={spellCheck}
      {...props}
    />
  );
});
