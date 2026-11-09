import { beforeEach, describe, expect, it, vi } from "vitest";
import { copyText } from "./clipboard";

describe("copyText", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    Object.defineProperty(window, "isSecureContext", {
      value: true,
      configurable: true,
    });
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    });
    Object.defineProperty(document, "execCommand", {
      value: vi.fn(),
      configurable: true,
    });
  });

  it("uses the Clipboard API in secure contexts", async () => {
    await expect(copyText("hello")).resolves.toBe(true);

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("hello");
  });

  it("falls back to execCommand in non-secure contexts", async () => {
    Object.defineProperty(window, "isSecureContext", {
      value: false,
      configurable: true,
    });
    const execSpy = vi.mocked(document.execCommand).mockReturnValue(true);

    await expect(copyText("mobile copy")).resolves.toBe(true);

    expect(execSpy).toHaveBeenCalledWith("copy");
    expect(document.querySelector("textarea")).toBeNull();
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
  });

  it("falls back when Clipboard API writing rejects", async () => {
    (
      navigator.clipboard.writeText as ReturnType<typeof vi.fn>
    ).mockRejectedValue(new Error("denied"));
    vi.mocked(document.execCommand).mockReturnValue(true);

    await expect(copyText("fallback")).resolves.toBe(true);
  });

  it("returns false when both copy paths fail", async () => {
    (
      navigator.clipboard.writeText as ReturnType<typeof vi.fn>
    ).mockRejectedValue(new Error("denied"));
    vi.mocked(document.execCommand).mockReturnValue(false);

    await expect(copyText("nope")).resolves.toBe(false);
  });
});
