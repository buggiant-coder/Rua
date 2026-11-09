import type {
  ConversationAssistantResponse,
  ConversationProjectionItem,
} from "./projection";

export type ConversationTurnView = {
  key: string;
  turnIds: string[];
  items: Array<{ item: ConversationProjectionItem; index: number }>;
};

// A displayed AskUser answer may join two canonical Turns. Keep that connected
// group indivisible too; a canonical Turn must never span two residency units.
export function groupConversationTurns(
  items: readonly ConversationProjectionItem[],
  hasAskUser: (item: ConversationAssistantResponse) => boolean,
): ConversationTurnView[] {
  const groups: ConversationTurnView[] = [];
  const groupByTurn = new Map<string, number>();
  for (const [index, item] of items.entries()) {
    const previous = items[index - 1];
    const startsTurn =
      item.kind === "acp_history_preview" ||
      (item.kind === "user" &&
        !(previous?.kind === "assistant" && hasAskUser(previous)));
    let groupIndex = groups.length - 1;
    const existing = item.turn_id ? groupByTurn.get(item.turn_id) : undefined;
    if (existing !== undefined && existing < groupIndex) {
      const merged = groups.splice(existing + 1);
      const target = groups[existing];
      for (const group of merged) {
        target.items.push(...group.items);
        for (const turnId of group.turnIds) {
          if (!target.turnIds.includes(turnId)) target.turnIds.push(turnId);
          groupByTurn.set(turnId, existing);
        }
      }
      groupIndex = existing;
    } else if (groups.length === 0 || (startsTurn && existing === undefined)) {
      groups.push({ key: item.id, turnIds: [], items: [] });
      groupIndex = groups.length - 1;
    }
    const group = groups[groupIndex];
    group.items.push({ item, index });
    if (item.turn_id && !group.turnIds.includes(item.turn_id)) {
      group.turnIds.push(item.turn_id);
      groupByTurn.set(item.turn_id, groupIndex);
    }
  }
  return groups;
}
