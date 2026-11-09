import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ApprovalModeSection } from "./ApprovalModeSection";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => {
      const labels: Record<string, string> = {
        "security.approvalMode.title": "Approval mode",
        "security.approvalMode.description":
          "Choose how tool calls are approved.",
        "security.approvalMode.askFirstTitle": "Ask First",
        "security.approvalMode.askFirstDesc":
          "Require authorization before every tool call.",
        "security.approvalMode.handsOffTitle": "Hands-off",
        "security.approvalMode.handsOffDesc":
          "Request authorization only for tool calls that hit enabled safety rules.",
        "security.approvalMode.justDoItTitle": "Just-Do-IT!",
        "security.approvalMode.justDoItDesc":
          "Run fully automatically (except explicitly disabled tools).",
      };
      return labels[key] ?? key;
    },
  }),
}));

describe("ApprovalModeSection", () => {
  it("renders three approval presets and selects Hands-off by default", () => {
    render(<ApprovalModeSection />);

    expect(screen.getByText("Approval mode")).toBeVisible();
    expect(screen.getByRole("radio", { name: /Ask First/ })).not.toBeChecked();
    expect(
      screen.queryByRole("radio", { name: /Balanced/ }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Hands-off/ })).toBeChecked();
    expect(
      screen.getByRole("radio", { name: /Just-Do-IT!/ }),
    ).not.toBeChecked();
    expect(
      screen.getByText("Require authorization before every tool call."),
    ).toBeVisible();
    expect(
      screen.getByText(
        "Request authorization only for tool calls that hit enabled safety rules.",
      ),
    ).toBeVisible();
    expect(
      screen.getByText(
        "Run fully automatically (except explicitly disabled tools).",
      ),
    ).toBeVisible();
    expect(
      screen.queryByRole("radio", { name: /Strict/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("radio", { name: /Smart/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("radio", { name: /Custom/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("radio", { name: /^Off/ }),
    ).not.toBeInTheDocument();
  });

  it("emits the selected approval preset", () => {
    const onChange = vi.fn();

    render(<ApprovalModeSection value="ask_first" onChange={onChange} />);

    fireEvent.click(screen.getByRole("radio", { name: /Hands-off/ }));

    expect(onChange).toHaveBeenCalledWith("hands_off");
  });
});
