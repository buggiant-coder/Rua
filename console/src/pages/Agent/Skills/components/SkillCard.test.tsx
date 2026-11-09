import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SkillSpec } from "../../../../api/types";
import { getSkillVisual, skillIconUrl } from "./SkillCard";

describe("skill market icons", () => {
  it("reads icon_url from install metadata", () => {
    const skill: Pick<SkillSpec, "install_meta"> = {
      install_meta: {
        installed_from: "clawhub",
        source_url: "https://example.test/writer/SKILL.md",
        installed_at: "2026-06-18T00:00:00Z",
        installed_version: "1.2.3",
        market_meta: {
          icon_url: "https://example.test/icon.png",
        },
      },
    };

    expect(skillIconUrl(skill)).toBe("https://example.test/icon.png");
  });

  it("renders remote icon when a market icon url is present", () => {
    const { container } = render(
      getSkillVisual(
        "writer",
        "---\nname: writer\n---\n",
        undefined,
        "https://example.test/icon.png",
      ),
    );

    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      "https://example.test/icon.png",
    );
  });
});
