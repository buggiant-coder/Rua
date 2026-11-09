import { describe, expect, it } from "vitest";
import { groupConversationTurns } from "../conversationTurns";
import type { ConversationProjectionItem } from "../projection";

const user = (id: string, turn_id: string): ConversationProjectionItem => ({
  kind: "user",
  role: "user",
  id,
  turn_id,
  content: [],
});
const answer = (id: string, turn_id: string): ConversationProjectionItem => ({
  kind: "assistant",
  role: "assistant",
  id,
  turn_id,
  output: [],
  variants: [],
  status: "completed",
});

describe("whole conversation turns", () => {
  it("keeps every part of a canonical turn and connected user answers in one residency unit", () => {
    const items = [
      user("u1", "t1"),
      answer("a1", "t1"),
      user("follow", "t1"),
      answer("a2", "t1"),
      user("u2", "t2"),
      answer("ask", "t2"),
      user("answer", "t3"),
      answer("a3", "t3"),
      user("u4", "t4"),
    ];
    const groups = groupConversationTurns(items, (item) => item.id === "ask");
    expect(
      groups.map((group) => group.items.map((entry) => entry.item.id)),
    ).toEqual([
      ["u1", "a1", "follow", "a2"],
      ["u2", "ask", "answer", "a3"],
      ["u4"],
    ]);
    expect(groups[1].turnIds).toEqual(["t2", "t3"]);
  });
});
