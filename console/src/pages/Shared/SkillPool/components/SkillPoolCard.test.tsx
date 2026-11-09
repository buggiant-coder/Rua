import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { PoolSkillSpec } from "../../../../api/types";
import { SkillPoolCard, poolSkillIconUrl } from "./SkillPoolCard";

describe("poolSkillIconUrl", () => {
  it("reads icon_url from pool install metadata", () => {
    const skill: PoolSkillSpec = {
      name: "writer",
      description: "Drafts text",
      content: "---\nname: writer\n---\n",
      source: "pool",
      protected: false,
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

    expect(poolSkillIconUrl(skill)).toBe("https://example.test/icon.png");
  });

  it("falls back when pool install metadata has no string icon url", () => {
    const skill: PoolSkillSpec = {
      name: "writer",
      content: "---\nname: writer\n---\n",
      source: "pool",
      protected: false,
      install_meta: {
        installed_from: "clawhub",
        source_url: "https://example.test/writer/SKILL.md",
        installed_at: "2026-06-18T00:00:00Z",
        installed_version: "1.2.3",
        market_meta: {
          icon_url: null,
        },
      },
    };

    expect(poolSkillIconUrl(skill)).toBe("");
  });

  it("renders pool install source on the card", () => {
    const skill: PoolSkillSpec = {
      name: "writer",
      description: "Drafts text",
      content: "---\nname: writer\n---\n",
      source: "customized",
      protected: false,
      install_meta: {
        installed_from: "skillhub",
        source_url: "https://example.test/writer/SKILL.md",
        installed_at: "2026-06-18T00:00:00Z",
        installed_version: "1.2.3",
      },
    };

    render(
      <SkillPoolCard
        skill={skill}
        isSelected={false}
        batchMode={false}
        onToggleSelect={() => {}}
        onEdit={() => {}}
        onBroadcast={() => {}}
        onDelete={() => {}}
      />,
    );

    expect(screen.getByText("writer")).toBeVisible();
    expect(screen.getByText("skillhub")).toBeVisible();
  });
});
