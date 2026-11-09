import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { accessControlApi } from "./accessControl";

describe("accessControlApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses QwenPaw-compatible access-control list routes", async () => {
    await accessControlApi.getAclAll();
    await accessControlApi.getAclChannel("wecom/aibot");
    await accessControlApi.getAclAllPending();

    expect(request).toHaveBeenNthCalledWith(1, "/access-control");
    expect(request).toHaveBeenNthCalledWith(2, "/access-control/wecom%2Faibot");
    expect(request).toHaveBeenNthCalledWith(3, "/access-control/pending/all");
  });

  it("uses agent-scoped access-control routes when an agent id is provided", async () => {
    await accessControlApi.getAclAll("agent/1");
    await accessControlApi.getAclChannel("telegram", "agent/1");
    await accessControlApi.approveAclPending(
      [{ channel: "telegram", user_id: "u1" }],
      "agent/1",
    );

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent%2F1/access-control",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2F1/access-control/telegram",
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent%2F1/access-control/pending/approve",
      {
        method: "POST",
        body: JSON.stringify({
          entries: [{ channel: "telegram", user_id: "u1" }],
        }),
      },
    );
  });

  it("posts whitelist and blacklist batch bodies", async () => {
    const entries = [
      {
        channel: "telegram",
        user_id: "u1",
        remark: "trusted",
        username: "Alice",
      },
    ];

    await accessControlApi.addAclWhitelist(entries);
    await accessControlApi.removeAclWhitelist([
      { channel: "telegram", user_id: "u1" },
    ]);
    await accessControlApi.addAclBlacklist(entries);
    await accessControlApi.removeAclBlacklist([
      { channel: "telegram", user_id: "u1" },
    ]);

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/access-control/whitelist/add",
      {
        method: "POST",
        body: JSON.stringify({ entries }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/access-control/whitelist/remove",
      {
        method: "POST",
        body: JSON.stringify({
          entries: [{ channel: "telegram", user_id: "u1" }],
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/access-control/blacklist/add",
      {
        method: "POST",
        body: JSON.stringify({ entries }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/access-control/blacklist/remove",
      {
        method: "POST",
        body: JSON.stringify({
          entries: [{ channel: "telegram", user_id: "u1" }],
        }),
      },
    );
  });

  it("posts pending actions and user metadata updates", async () => {
    await accessControlApi.approveAclPending([
      { channel: "feishu", user_id: "u2", remark: "teammate" },
    ]);
    await accessControlApi.denyAclPending([
      { channel: "feishu", user_id: "u3" },
    ]);
    await accessControlApi.dismissAclPending([
      { channel: "feishu", user_id: "u4" },
    ]);
    await accessControlApi.updatePendingRemark("feishu", "u5", "later");
    await accessControlApi.updateAclRemark("feishu", "u6", "vip");
    await accessControlApi.updateUsername("feishu", "u7", "Bob");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/access-control/pending/approve",
      {
        method: "POST",
        body: JSON.stringify({
          entries: [{ channel: "feishu", user_id: "u2", remark: "teammate" }],
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(2, "/access-control/pending/deny", {
      method: "POST",
      body: JSON.stringify({ entries: [{ channel: "feishu", user_id: "u3" }] }),
    });
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/access-control/pending/dismiss",
      {
        method: "POST",
        body: JSON.stringify({
          entries: [{ channel: "feishu", user_id: "u4" }],
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/access-control/pending/remark",
      {
        method: "POST",
        body: JSON.stringify({
          channel: "feishu",
          user_id: "u5",
          remark: "later",
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(5, "/access-control/remark", {
      method: "POST",
      body: JSON.stringify({ channel: "feishu", user_id: "u6", remark: "vip" }),
    });
    expect(request).toHaveBeenNthCalledWith(6, "/access-control/username", {
      method: "POST",
      body: JSON.stringify({
        channel: "feishu",
        user_id: "u7",
        username: "Bob",
      }),
    });
  });
});
