import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { formatFileSize, formatTimeAgo } from "./utils";

dayjs.extend(relativeTime);

describe("workspace component utilities", () => {
  describe("formatFileSize", () => {
    it("formats byte, KB, and MB boundaries without changing Rust labels", () => {
      expect(formatFileSize(0)).toBe("0 B");
      expect(formatFileSize(512)).toBe("512 B");
      expect(formatFileSize(1024)).toBe("1.0 KB");
      expect(formatFileSize(1536)).toBe("1.5 KB");
      expect(formatFileSize(1024 * 1024)).toBe("1.0 MB");
      expect(formatFileSize(2.5 * 1024 * 1024)).toBe("2.5 MB");
    });
  });

  describe("formatTimeAgo", () => {
    beforeEach(() => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-18T12:00:00Z"));
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it("returns relative times for numeric and string timestamps", () => {
      expect(formatTimeAgo(new Date("2026-06-18T11:55:00Z").getTime())).toBe(
        "5 minutes ago",
      );
      expect(formatTimeAgo("2026-06-18T11:00:00Z")).toBe("an hour ago");
    });

    it("returns a stable placeholder for invalid timestamps", () => {
      expect(formatTimeAgo("not-a-date")).toBe("-");
    });
  });
});
