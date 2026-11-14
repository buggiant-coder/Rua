import { afterEach, describe, expect, it, vi } from "vitest";
import { request } from "../request";
import {
  toolSelectionApi,
  type ToolSelectionPolicyPreview,
} from "./toolSelection";

vi.mock("../request", () => ({ request: vi.fn() }));

describe("Tool selection configuration routes", () => {
  afterEach(() => vi.clearAllMocks());

  it("reads without mutating and commits the exact server preview token and candidate", async () => {
    const candidate = {
      mode: "default_enabled" as const,
      disabled_tools: [{ kind: "builtin" as const, name: "phone_take_photo" }],
      disabled_capabilities: ["computer_use"],
    };
    const preview: ToolSelectionPolicyPreview = {
      expected: "opaque-policy-revision",
      current: { mode: "restricted", allowed_tools: [] },
      candidate: { ...candidate, future_setting: true },
    };
    await toolSelectionApi.get("agent/a");
    await toolSelectionApi.preview("agent/a", candidate);
    await toolSelectionApi.apply("agent/a", preview);
    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent%2Fa/tools/selection-policy",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2Fa/tools/selection-policy/preview",
      {
        method: "POST",
        body: JSON.stringify({ selection_policy: candidate }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent%2Fa/tools/selection-policy",
      {
        method: "PUT",
        body: JSON.stringify({
          expected: preview.expected,
          candidate: preview.candidate,
        }),
      },
    );
  });
});
