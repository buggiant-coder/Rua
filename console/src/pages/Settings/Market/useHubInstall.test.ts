import { describe, expect, it } from "vitest";
import { marketMeta } from "./useHubInstall";
import type { MarketResult } from "../../../api/modules/market";

describe("marketMeta", () => {
  it("preserves market icon metadata for installed skills", () => {
    const result: MarketResult = {
      source: "clawhub",
      slug: "writer",
      name: "Writer",
      description: "Drafts text",
      source_url: "https://example.test/writer/SKILL.md",
      version: "1.2.3",
      author: "Market Author",
      icon_url: "https://example.test/icon.png",
      stats: { downloads: 42 },
    };

    expect(marketMeta(result)).toEqual({
      source: "clawhub",
      slug: "writer",
      name: "Writer",
      author: "Market Author",
      icon_url: "https://example.test/icon.png",
      stats: { downloads: 42 },
    });
  });
});
