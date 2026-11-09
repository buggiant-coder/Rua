import { beforeEach, describe, expect, it, vi } from "vitest";
import { request } from "../request";
import { userInboxApi } from "./userInbox";

vi.mock("../request", () => ({ request: vi.fn() }));

describe("userInboxApi", () => {
  beforeEach(() => vi.clearAllMocks());

  it("uses the global Inbox lifecycle routes", async () => {
    vi.mocked(request).mockResolvedValue({});

    await userInboxApi.list({ cursor: "cursor", limit: 25, read: false });
    await userInboxApi.get("message/id");
    await userInboxApi.setRead("message/id", true);
    await userInboxApi.delete("message/id");
    await userInboxApi.unreadCount();
    await userInboxApi.markAllRead();

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/inbox/messages?cursor=cursor&limit=25&read=false",
    );
    expect(request).toHaveBeenNthCalledWith(2, "/inbox/messages/message%2Fid");
    expect(request).toHaveBeenNthCalledWith(3, "/inbox/messages/message%2Fid", {
      method: "PATCH",
      body: JSON.stringify({ read: true }),
    });
    expect(request).toHaveBeenNthCalledWith(4, "/inbox/messages/message%2Fid", {
      method: "DELETE",
    });
    expect(request).toHaveBeenNthCalledWith(5, "/inbox/unread-count");
    expect(request).toHaveBeenNthCalledWith(6, "/inbox/read-all", {
      method: "POST",
    });
  });
});
