import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ThinkingRetentionSlider } from ".";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

describe("ThinkingRetentionSlider", () => {
  it("adds visual distance between keep-all and keep-none values", () => {
    const onChange = vi.fn();
    const { container } = render(
      <ThinkingRetentionSlider value={0} onChange={onChange} />,
    );

    expect(
      container.querySelector('.rua-ui-slider-mark[style*="16.666"]'),
    ).toHaveTextContent("agentConfig.thinkingTurnsNone");

    const slider = screen.getByRole("slider", {
      name: "agentConfig.thinkingTurnsToKeep",
    });
    expect(slider).toHaveAttribute("aria-valuenow", "0");
    fireEvent.keyDown(slider, { key: "ArrowLeft" });
    expect(onChange).toHaveBeenCalledWith(-1);
  });
});
