import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import TextInput from ".";

describe("TextInput", () => {
  it.each([false, true])(
    "disables input assistance (allowClear=%s)",
    (allowClear) => {
      render(<TextInput allowClear={allowClear} defaultValue="qwen3.5" />);
      const input = screen.getByRole("textbox");
      expect(input).toHaveAttribute("autocomplete", "off");
      expect(input).toHaveAttribute("autocapitalize", "off");
      expect(input).toHaveAttribute("autocorrect", "off");
      expect(input).toHaveAttribute("spellcheck", "false");
      fireEvent.change(input, { target: { value: "qwen3.5-plus" } });
      expect(input).toHaveValue("qwen3.5-plus");
    },
  );

  it("clears an uncontrolled value and restores focus", () => {
    const onClear = vi.fn();
    render(
      <TextInput
        allowClear
        clearLabel="Clear title"
        defaultValue="Existing title"
        onClear={onClear}
      />,
    );

    const input = screen.getByRole("textbox");
    fireEvent.click(screen.getByRole("button", { name: "Clear title" }));

    expect(input).toHaveValue("");
    expect(input).toHaveFocus();
    expect(onClear).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "Clear title" })).toBeNull();
  });
});
