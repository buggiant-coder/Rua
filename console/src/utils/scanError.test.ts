import { beforeEach, describe, expect, it, vi } from "vitest";
import { Modal } from "@/ui";
import {
  checkScanWarnings,
  handleScanError,
  tryParseScanError,
} from "./scanError";

vi.mock("@/ui", () => ({
  Modal: {
    error: vi.fn(),
    warning: vi.fn(),
  },
}));

const t = vi.fn((key: string, params?: Record<string, unknown>) =>
  params ? `${key}:${JSON.stringify(params)}` : key,
) as never;

const findings = [
  {
    title: "Dangerous command",
    description: "runs rm",
    file_path: "SKILL.md",
    line_number: 3,
  },
];

describe("scanError utilities", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("parses scanner failure JSON from an Error message", () => {
    const parsed = tryParseScanError(
      new Error(
        `request failed: ${JSON.stringify({
          type: "security_scan_failed",
          findings,
        })}`,
      ),
    );

    expect(parsed).toMatchObject({
      type: "security_scan_failed",
      findings,
    });
  });

  it("ignores non-errors, malformed JSON, and unrelated error objects", () => {
    expect(tryParseScanError("security_scan_failed")).toBeNull();
    expect(tryParseScanError(new Error("prefix {bad json"))).toBeNull();
    expect(
      tryParseScanError(
        new Error(JSON.stringify({ type: "different_error", findings })),
      ),
    ).toBeNull();
  });

  it("shows the scan error modal when handleScanError recognizes the payload", () => {
    const handled = handleScanError(
      new Error(
        JSON.stringify({
          type: "security_scan_failed",
          findings,
        }),
      ),
      t,
    );

    expect(handled).toBe(true);
    expect(Modal.error).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "security.skillScanner.scanError.title",
        width: 640,
        centered: true,
      }),
    );
  });

  it("returns false without opening a modal for ordinary errors", () => {
    expect(handleScanError(new Error("plain failure"), t)).toBe(false);
    expect(Modal.error).not.toHaveBeenCalled();
  });

  it("shows latest warn-mode findings unless the skill is whitelisted", async () => {
    const fetchAlerts = vi.fn().mockResolvedValue([
      {
        skill_name: "demo",
        action: "blocked",
        findings: [{ ...findings[0], title: "blocked" }],
      },
      {
        skill_name: "demo",
        action: "warned",
        findings: [{ ...findings[0], title: "first warning" }],
      },
      {
        skill_name: "demo",
        action: "warned",
        findings: [{ ...findings[0], title: "latest warning" }],
      },
    ]);
    const fetchScannerCfg = vi.fn().mockResolvedValue({ whitelist: [] });

    await checkScanWarnings("demo", fetchAlerts, fetchScannerCfg, t);

    expect(Modal.warning).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "security.skillScanner.scanError.title",
        width: 640,
      }),
    );

    vi.clearAllMocks();
    fetchScannerCfg.mockResolvedValue({
      whitelist: [{ skill_name: "demo" }],
    });

    await checkScanWarnings("demo", fetchAlerts, fetchScannerCfg, t);

    expect(Modal.warning).not.toHaveBeenCalled();
  });

  it("does not fail callers when warning lookup throws", async () => {
    await expect(
      checkScanWarnings(
        "demo",
        vi.fn().mockRejectedValue(new Error("network")),
        vi.fn(),
        t,
      ),
    ).resolves.toBeUndefined();
  });
});
