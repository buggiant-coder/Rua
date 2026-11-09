import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { consoleApi } from "./console";

describe("consoleApi agent-scoped routes", () => {
  afterEach(() => vi.clearAllMocks());

  it("loads push messages for the explicit agent with an encoded path", async () => {
    await consoleApi.getPushMessages("agent/a");

    expect(request).toHaveBeenCalledWith(
      "/agents/agent%2Fa/console/push-messages",
    );
  });

  it("loads chat-scoped approval snapshots through the authority endpoint", async () => {
    await consoleApi.getApprovalSnapshot("agent/a", "chat/1");

    expect(request).toHaveBeenCalledWith(
      "/agents/agent%2Fa/approvals/snapshot?chat_id=chat%2F1",
    );
  });

  it("submits decisions through the approval control endpoint", async () => {
    const matcher = {
      type: "bash_command_family" as const,
      program: "git",
      subcommand: "status",
    };

    await consoleApi.decideApproval("agent/a", "approval/1", {
      decision: "allow_chat",
      decided_by: "webui",
      matcher,
    });

    expect(request).toHaveBeenCalledWith(
      "/agents/agent%2Fa/approvals/approval%2F1/decide",
      {
        method: "POST",
        body: JSON.stringify({
          decision: "allow_chat",
          decided_by: "webui",
          matcher,
        }),
      },
    );
  });

  it("manages agent-scoped permission rules", async () => {
    await consoleApi.listApprovalRules("agent/a");
    await consoleApi.createPermissionRule("agent/a", {
      effect: "deny",
      enabled: true,
      label: "Block WebSearch",
      matcher: { type: "tool_name", pattern: "WebSearch" },
    });
    await consoleApi.deleteApprovalRule("agent/a", "rule/1");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent%2Fa/permission-rules",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2Fa/permission-rules",
      {
        method: "POST",
        body: JSON.stringify({
          effect: "deny",
          enabled: true,
          label: "Block WebSearch",
          matcher: { type: "tool_name", pattern: "WebSearch" },
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent%2Fa/permission-rules/rule%2F1",
      { method: "DELETE" },
    );
  });
});
