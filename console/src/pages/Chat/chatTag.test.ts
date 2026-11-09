import { describe, expect, it } from "vitest";

import { nextChatTag } from "./chatTag";

describe("nextChatTag", () => {
  it("cycles through none, red, yellow, green, and none", () => {
    expect(nextChatTag(undefined)).toBe("red");
    expect(nextChatTag("none")).toBe("red");
    expect(nextChatTag("red")).toBe("yellow");
    expect(nextChatTag("yellow")).toBe("green");
    expect(nextChatTag("green")).toBe("none");
  });
});
