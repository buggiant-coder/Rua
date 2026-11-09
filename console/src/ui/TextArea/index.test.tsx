import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import TextArea from ".";

describe("TextArea", () => {
  it("disables text assistance even when optional props are undefined", () => {
    render(
      <TextArea
        autoComplete={undefined}
        defaultValue={'{"model":"qwen3.5"}'}
      />,
    );
    const input = screen.getByRole("textbox");
    expect(input).toHaveAttribute("autocomplete", "off");
    expect(input).toHaveAttribute("autocapitalize", "off");
    expect(input).toHaveAttribute("autocorrect", "off");
    expect(input).toHaveAttribute("spellcheck", "false");
    expect(input).toHaveValue('{"model":"qwen3.5"}');
  });
});
