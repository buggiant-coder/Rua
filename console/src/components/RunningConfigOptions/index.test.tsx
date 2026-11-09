import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ChatRunningConfig } from "@/api/types";
import { RunningConfigOptions } from ".";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

describe("RunningConfigOptions", () => {
  it("keeps zero when the continuous work limit is disabled", () => {
    const changed = vi.fn();
    function Harness() {
      const [limit, setLimit] = useState(1);
      return (
        <RunningConfigOptions
          value={
            {
              max_iters: limit,
              auto_continue_on_text_only: false,
              context_compact: {},
              tool_result_compact: {},
            } as ChatRunningConfig
          }
          openSections={{
            runtime: true,
            compaction: false,
            aggressive: false,
            token: false,
          }}
          onToggleSection={vi.fn()}
          onRunningChange={(key, value) => {
            changed(key, value);
            if (key === "max_iters") setLimit(Number(value));
          }}
          onContextChange={vi.fn()}
          onToolResultChange={vi.fn()}
          onThinkingTurnsChange={vi.fn()}
        />
      );
    }
    render(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: "Decrease" }));
    expect(changed).toHaveBeenCalledWith("max_iters", 0);
    expect(screen.getByRole("spinbutton")).toHaveValue(0);
    expect(screen.getByRole("spinbutton")).toHaveAttribute("min", "0");
  });
});
