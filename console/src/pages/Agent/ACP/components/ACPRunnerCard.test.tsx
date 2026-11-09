import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { ACPRunnerInfo } from "../../../../api/types";
import { ACPRunnerCard } from "./ACPRunnerCard";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

function runner(): ACPRunnerInfo {
  return {
    key: "codex",
    enabled: true,
    command: "npx",
    args: ["-y", "@agentclientprotocol/codex-acp"],
    env: {},
    trusted: false,
    tool_parse_mode: "call_detail",
    stdio_buffer_limit_bytes: null,
    is_builtin: true,
  };
}

describe("ACPRunnerCard", () => {
  it("keeps legacy parse mode out of the normal runner form", () => {
    render(
      <ACPRunnerCard
        runner={runner()}
        onToggle={vi.fn()}
        onDelete={vi.fn()}
        onInstall={vi.fn()}
        onUpdate={vi.fn().mockResolvedValue(true)}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByText("codex"));

    expect(screen.getAllByText("common.enabled").length).toBeGreaterThan(0);
    expect(screen.queryByText("acp.check.unavailable")).not.toBeInTheDocument();
    expect(screen.getByText("acp.form.command")).toBeVisible();
    expect(
      screen.queryByText("acp.form.toolParseMode"),
    ).not.toBeInTheDocument();
  });
});
