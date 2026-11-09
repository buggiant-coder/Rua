import { describe, expect, it } from "vitest";

import { getAgentDisplayName } from "./agentDisplayName";
import {
  extractFriendlyMessage,
  extractRawDetail,
  parseErrorDetail,
} from "./error";
import { formatCompact } from "./formatNumber";
import { stripFrontmatter } from "./markdown";
describe("getAgentDisplayName", () => {
  it("uses Rust API names directly and falls back to id", () => {
    expect(getAgentDisplayName({ id: "default", name: "Default Agent" })).toBe(
      "Default Agent",
    );
    expect(getAgentDisplayName({ id: "agent-1", name: "" })).toBe("agent-1");
    expect(
      getAgentDisplayName({
        id: "agent-2",
        name: undefined,
      } as Parameters<typeof getAgentDisplayName>[0]),
    ).toBe("agent-2");
  });
});

describe("stripFrontmatter", () => {
  it("removes YAML frontmatter while preserving markdown body", () => {
    expect(stripFrontmatter("---\ntitle: Test\n---\n# Hello")).toBe("# Hello");
    expect(stripFrontmatter("---\r\ntitle: Test\r\n---\r\n# Hello")).toBe(
      "# Hello",
    );
    expect(stripFrontmatter("# Hello\ncontent")).toBe("# Hello\ncontent");
    expect(stripFrontmatter("---\ntitle: Only\n---\n")).toBe("");
  });
});

describe("formatCompact", () => {
  it.each([
    [-1, "0"],
    [Number.NaN, "0"],
    [999, "999"],
    [1_000, "1K"],
    [1_500, "1.5K"],
    [1_000_000, "1M"],
    [1_250_000, "1.3M"],
    [1_000_000_000, "1B"],
    [1_250_000_000, "1.3B"],
  ] as const)("formats %s as %s", (input, expected) => {
    expect(formatCompact(input)).toBe(expected);
  });
});

describe("error helpers", () => {
  it("extracts structured details from request-style error messages", () => {
    const error = new Error(
      'invalid provider - {"detail":{"provider_id":"missing","kind":"not_found"}}',
    );

    expect(parseErrorDetail(error)).toEqual({
      provider_id: "missing",
      kind: "not_found",
    });
    expect(extractFriendlyMessage(error)).toBe("invalid provider");
    expect(extractRawDetail(error)).toBe(
      JSON.stringify(
        { detail: { provider_id: "missing", kind: "not_found" } },
        null,
        2,
      ),
    );
  });

  it("falls back safely for non-request errors and multiline messages", () => {
    expect(parseErrorDetail("plain")).toBeNull();
    expect(extractFriendlyMessage(new Error("\n first line \n second"))).toBe(
      "first line",
    );
    expect(extractRawDetail(new Error("plain"))).toContain("Error: plain");
  });
});
