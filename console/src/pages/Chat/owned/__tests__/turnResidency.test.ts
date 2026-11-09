import { expect, it } from "vitest";
import { retainConversationTurns } from "../turnResidency";

it("keeps a generous history until the high watermark, then reclaims whole distant turns with headroom", () => {
  const keys = Array.from({ length: 12 }, (_, index) => String(index));
  const previous = new Set(keys.slice(0, 10));
  const below = retainConversationTurns({
    keys: keys.slice(0, 10),
    previous,
    visibleStart: 8,
    visibleEnd: 9,
    protectedKeys: new Set(),
    high: 10,
    low: 7,
    buffer: 1,
  });
  expect([...below]).toEqual([...previous]);
  const full = retainConversationTurns({
    keys,
    previous: new Set(keys),
    visibleStart: 9,
    visibleEnd: 11,
    protectedKeys: new Set(["0"]),
    high: 10,
    low: 7,
    buffer: 1,
  });
  expect(full.size).toBe(7);
  expect(full.has("0")).toBe(true);
  expect(full.has("10")).toBe(true);
  expect(full.has("11")).toBe(true);
  expect(full.has("1")).toBe(false);
});

it("restores the reading destination and protects it from immediate eviction", () => {
  const keys = Array.from({ length: 30 }, (_, i) => String(i));
  const next = retainConversationTurns({
    keys,
    previous: new Set(keys.slice(-10)),
    visibleStart: 3,
    visibleEnd: 4,
    protectedKeys: new Set(["29"]),
    high: 12,
    low: 8,
    buffer: 2,
  });
  for (const id of ["1", "2", "3", "4", "5", "6", "29"])
    expect(next.has(id)).toBe(true);
  expect(next.size).toBeLessThanOrEqual(12);
});
