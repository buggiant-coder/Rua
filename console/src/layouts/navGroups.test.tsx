import { isValidElement } from "react";
import { describe, expect, it } from "vitest";
import { NAV_GROUPS } from "./navGroups";

function item(groupKey: string, itemKey: string) {
  const group = NAV_GROUPS.find((candidate) => candidate.key === groupKey);
  return group?.items.find((candidate) => candidate.key === itemKey);
}

describe("NAV_GROUPS", () => {
  it("keeps projects between chat and terminal in the primary group", () => {
    const primary = NAV_GROUPS.find((group) => group.key === "primary");

    expect(primary?.items.map((entry) => entry.key)).toEqual([
      "chat",
      "projects",
      "terminal",
    ]);
  });

  it("uses distinct ACP and terminal icons", () => {
    const acp = item("agent", "acp");
    const terminal = item("primary", "terminal");

    expect(isValidElement(acp?.icon)).toBe(true);
    expect(isValidElement(terminal?.icon)).toBe(true);
    expect(acp?.icon).not.toEqual(terminal?.icon);
  });

  it("places desktop tools immediately after tools for all clients", () => {
    const entries =
      NAV_GROUPS.find((group) => group.key === "agent")?.items ?? [];
    const toolsIndex = entries.findIndex((entry) => entry.key === "tools");
    expect(entries[toolsIndex + 1]).toMatchObject({
      key: "desktop-tools",
      path: "/desktop-tools",
      labelKey: "nav.desktopTools",
    });
    expect(entries[toolsIndex + 1]).not.toHaveProperty("desktopOnly", true);
    expect(item("settings", "desktop")).toBeUndefined();
  });

  it("places About after every other navigation item", () => {
    expect(NAV_GROUPS.flatMap((group) => group.items).at(-1)).toMatchObject({
      key: "about",
      path: "/about",
      labelKey: "nav.about",
    });
  });

  it("places sync in a multi-device group between workspace and settings", () => {
    const orderedKeys = NAV_GROUPS.map((group) => group.key);
    const agentIndex = orderedKeys.indexOf("agent");
    const multiDeviceIndex = orderedKeys.indexOf("multi-device");
    const settingsIndex = orderedKeys.indexOf("settings");

    expect(agentIndex).toBeGreaterThanOrEqual(0);
    expect(multiDeviceIndex).toBeGreaterThan(agentIndex);
    expect(settingsIndex).toBeGreaterThan(multiDeviceIndex);
    expect(
      NAV_GROUPS.find((group) => group.key === "multi-device"),
    ).toMatchObject({
      labelKey: "nav.multiDevice",
      items: [
        {
          key: "device-sync",
          labelKey: "nav.deviceSync",
          path: "/settings/device-sync",
        },
        {
          key: "airdrop-transfer",
          labelKey: "nav.airdropTransfer",
          path: "/settings/airdrop-transfer",
        },
      ],
    });
  });
});
