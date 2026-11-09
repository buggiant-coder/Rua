import { fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { describe, expect, it, vi } from "vitest";
import { InputWithExample, PasswordWithExample } from "./index";

vi.mock("lucide-react", () => ({
  ClipboardPaste: () => <span data-testid="paste-icon" />,
}));

vi.mock("@/ui/TextInput", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  const TextInput = React.forwardRef<
    HTMLInputElement,
    React.InputHTMLAttributes<HTMLInputElement>
  >((props, ref) => <input ref={ref} {...props} />);
  TextInput.displayName = "TextInput";
  return { default: TextInput };
});

vi.mock("@/ui", () => ({
  Tooltip: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
}));

describe("InputWithExample", () => {
  it("fills an explicit example into an empty input", () => {
    const onChange = vi.fn();

    render(
      <InputWithExample
        value=""
        example="https://example.test"
        onChange={onChange}
      />,
    );

    fireEvent.click(
      screen.getByTestId("paste-icon").closest('[role="button"]')!,
    );
    expect(onChange.mock.calls[0][0].target.value).toBe("https://example.test");
  });

  it("uses the placeholder as the fallback example", () => {
    const onChange = vi.fn();

    render(
      <InputWithExample
        value=""
        placeholder="http://localhost:11434"
        onChange={onChange}
      />,
    );

    fireEvent.click(
      screen.getByTestId("paste-icon").closest('[role="button"]')!,
    );
    expect(onChange.mock.calls[0][0].target.value).toBe(
      "http://localhost:11434",
    );
  });

  it("hides the example action for non-empty values and forwards Enter", () => {
    const onChange = vi.fn();
    const onKeyDown = vi.fn();
    const onPressEnter = vi.fn();

    render(
      <InputWithExample
        value="already-set"
        example="unused"
        onChange={onChange}
        onKeyDown={onKeyDown}
        onPressEnter={onPressEnter}
      />,
    );

    const button = screen.getByTestId("paste-icon").closest('[role="button"]')!;
    fireEvent.click(button);
    expect(onChange).not.toHaveBeenCalled();

    fireEvent.keyDown(screen.getByDisplayValue("already-set"), {
      key: "Enter",
    });
    expect(onPressEnter).toHaveBeenCalledTimes(1);
    expect(onKeyDown).toHaveBeenCalledTimes(1);
  });

  it("forwards refs and preserves password input type", () => {
    const inputRef = createRef<HTMLInputElement>();
    const passwordRef = createRef<HTMLInputElement>();

    render(
      <>
        <InputWithExample ref={inputRef} value="" placeholder="plain" />
        <PasswordWithExample ref={passwordRef} value="" placeholder="secret" />
      </>,
    );

    expect(inputRef.current).toBeInstanceOf(HTMLInputElement);
    expect(passwordRef.current).toHaveAttribute("type", "password");
  });
});
