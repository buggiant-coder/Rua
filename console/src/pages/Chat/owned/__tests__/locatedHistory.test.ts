import { expect, it } from "vitest";
import { historyWindowGap, joinHistoryWindow } from "../locatedHistory";
import type { ChatHistory } from "@/api/types";
function page(start: number, end: number): ChatHistory {
  return {
    oldest_turn_ordinal: start,
    newest_turn_ordinal: end,
    has_more_before: start > 1,
    messages: Array.from({ length: end - start + 1 }, (_, index) => ({
      id: "m" + (index + start),
      role: "user",
      content: "message",
    })),
  };
}
it("joins overlapping windows in order while current messages remain authoritative", () => {
  const current = page(5, 10);
  const earlier = joinHistoryWindow(current, page(1, 7));
  expect(earlier.messages.map((m) => m.id)).toEqual(
    page(1, 10).messages.map((m) => m.id),
  );
  expect(earlier.messages[4]).toBe(current.messages[0]);
  const later = joinHistoryWindow(current, page(8, 15));
  expect(later.messages.map((m) => m.id)).toEqual(
    page(5, 15).messages.map((m) => m.id),
  );
});
it("represents unloaded ranges in either direction", () => {
  expect(historyWindowGap(page(21, 40), page(1, 10))).toMatchObject({
    beforeMessageId: "m21",
    count: 10,
    beforeTurnOrdinal: 21,
  });
  expect(historyWindowGap(page(1, 10), page(21, 40))).toMatchObject({
    beforeMessageId: "m21",
    count: 10,
    beforeTurnOrdinal: 21,
  });
  expect(historyWindowGap(page(11, 20), page(1, 10))).toBeNull();
});
it("joins a window enclosing the current range without reordering its newer suffix", () => {
  expect(
    joinHistoryWindow(page(5, 10), page(1, 15)).messages.map((m) => m.id),
  ).toEqual(page(1, 15).messages.map((m) => m.id));
});
