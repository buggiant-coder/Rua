import { describe, expect, it } from "vitest";
import en from "./en.json";
import ja from "./ja.json";
import ru from "./ru.json";
import zh from "./zh.json";

const locales = { en, ja, ru, zh };

const requiredKeys = [
  "historyReplacementEnabled",
  "historyReplacementEnabledTooltip",
  "activeWorkPlanInjectionEnabled",
  "activeWorkPlanInjectionEnabledTooltip",
  "toolResultProjectionTruncationEnabled",
  "toolResultProjectionTruncationEnabledTooltip",
  "toolResultProjectionTruncationBatchSize",
  "toolResultProjectionTruncationBatchSizeTooltip",
  "toolResultProjectionTruncationBatchSizeRequired",
  "toolResultCompactRecentN",
  "toolResultCompactRecentNTooltip",
  "toolResultCompactRecentNRequired",
  "toolResultCompactOldMaxBytes",
  "toolResultCompactOldMaxBytesTooltip",
  "thinkingTurnsToKeep",
  "thinkingTurnsToKeepTooltip",
  "thinkingTurnsAll",
  "thinkingTurnsNone",
  "earlyBackgroundCompactionEnabled",
  "earlyBackgroundCompactionEnabledTooltip",
  "earlyCompactionTargetRatio",
  "earlyCompactionTargetRatioTooltip",
  "earlyCompactionTargetRatioRequired",
  "earlyCompactionTargetThreshold",
  "earlyCompactionTargetThresholdTooltip",
  "earlyCompactionTargetThresholdPlaceholder",
] as const;

describe("context-management translations", () => {
  it.each(Object.entries(locales))(
    "%s defines every context-management label and explanation",
    (_locale, messages) => {
      for (const key of requiredKeys) {
        expect(messages.agentConfig[key]).toBeTruthy();
      }
    },
  );

  it("describes second-pass truncation without a discard tier", () => {
    expect(
      zh.agentConfig.toolResultProjectionTruncationEnabledTooltip,
    ).toContain("所有更旧的工具结果");
    expect(
      en.agentConfig.toolResultProjectionTruncationEnabledTooltip,
    ).not.toMatch(/empty result|call reference/i);
  });

  it("names early background compaction explicitly in Chinese", () => {
    expect(zh.agentConfig.earlyBackgroundCompactionEnabled).toBe(
      "提前后台压缩",
    );
    expect(zh.agentConfig.historyReplacementEnabled).toBe("历史过程替换");
  });
});
