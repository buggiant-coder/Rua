import { describe, expect, it } from "vitest";

import {
  isSupportedSkillUrl,
  MAX_TAG_LENGTH,
  MAX_TAGS,
  SKILL_TAG_FILTER_PREFIX,
  SUPPORTED_SKILL_URL_PREFIXES,
} from "./skill";

describe("SUPPORTED_SKILL_URL_PREFIXES", () => {
  it("keeps Rust skill hub import sources explicit and https-only", () => {
    expect(SUPPORTED_SKILL_URL_PREFIXES).toContain("https://skills.sh/");
    expect(SUPPORTED_SKILL_URL_PREFIXES).toContain("https://github.com/");
    expect(SUPPORTED_SKILL_URL_PREFIXES).toContain(
      "https://modelscope.cn/skills/",
    );

    for (const prefix of SUPPORTED_SKILL_URL_PREFIXES) {
      expect(prefix).toMatch(/^https:\/\//);
    }
  });
});

describe("isSupportedSkillUrl", () => {
  it.each([
    ["skills.sh", "https://skills.sh/my-skill", true],
    ["clawhub", "https://clawhub.ai/skill", true],
    ["github", "https://github.com/user/repo", true],
    ["modelscope", "https://modelscope.cn/skills/my-skill", true],
    ["unsupported", "https://example.com/skill", false],
    ["http", "http://skills.sh/my-skill", false],
    ["partial prefix", "https://skills.sh.other.com/skill", false],
    ["empty", "", false],
  ] as const)("returns %s support as %s", (_label, url, expected) => {
    expect(isSupportedSkillUrl(url)).toBe(expected);
  });
});

describe("skill tag limits", () => {
  it("pins client constants used by pool/workspace skill filters", () => {
    expect(SKILL_TAG_FILTER_PREFIX).toBe("tag:");
    expect(MAX_TAGS).toBe(10);
    expect(MAX_TAG_LENGTH).toBe(20);
  });
});
