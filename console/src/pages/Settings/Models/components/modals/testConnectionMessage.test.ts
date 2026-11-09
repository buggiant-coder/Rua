import { describe, it, expect } from "vitest";
import type { TFunction } from "i18next";
import {
  getLocalizedTestConnectionMessage,
  getTestConnectionFailureDetail,
} from "./testConnectionMessage";

describe("getTestConnectionFailureDetail", () => {
  it("treats generic failures case-insensitively", () => {
    expect(getTestConnectionFailureDetail("connection failed")).toBeNull();
  });

  it("ignores trailing periods on generic failures", () => {
    expect(getTestConnectionFailureDetail("Connection failed.")).toBeNull();
  });

  it("drops redundant generic detail after stripping the prefix", () => {
    expect(
      getTestConnectionFailureDetail("Connection failed: Connection failed"),
    ).toBeNull();
  });

  it("uses localized success text instead of the provider's English message", () => {
    const t = ((key: string) => key) as unknown as TFunction;
    expect(
      getLocalizedTestConnectionMessage(
        { success: true, message: "Connection successful" },
        t,
      ),
    ).toBe("models.testConnectionSuccess");
  });

  it("keeps useful failure detail in the localized message", () => {
    const t = ((key: string, options?: { message?: string }) =>
      `${key}:${options?.message ?? ""}`) as unknown as TFunction;
    expect(
      getLocalizedTestConnectionMessage(
        { success: false, message: "Connection failed: HTTP 401" },
        t,
      ),
    ).toBe("models.testConnectionFailedWithMessage:HTTP 401");
  });
});
