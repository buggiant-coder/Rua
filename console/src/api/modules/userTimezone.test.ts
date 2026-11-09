import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { userTimezoneApi } from "./userTimezone";

describe("userTimezoneApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses the shared user-timezone config route", async () => {
    await userTimezoneApi.getUserTimezone();
    await userTimezoneApi.updateUserTimezone("Asia/Shanghai");

    expect(request).toHaveBeenNthCalledWith(1, "/config/user-timezone");
    expect(request).toHaveBeenNthCalledWith(2, "/config/user-timezone", {
      method: "PUT",
      body: JSON.stringify({ timezone: "Asia/Shanghai" }),
    });
  });
});
