export type ChatSortField = "title" | "created_at" | "updated_at";
export type SortOrder = "asc" | "desc";

export type ChatListSort = {
  sort_by: ChatSortField;
  sort_order: SortOrder;
};

export const DEFAULT_CHAT_LIST_SORT: ChatListSort = {
  sort_by: "updated_at",
  sort_order: "desc",
};

export function chatListSortKey(sort: ChatListSort): string {
  return `${sort.sort_by}:${sort.sort_order}`;
}

export function parseChatListSortKey(key: string): ChatListSort | null {
  const [sortBy, sortOrder] = key.split(":");
  if (
    (sortBy === "title" ||
      sortBy === "created_at" ||
      sortBy === "updated_at") &&
    (sortOrder === "asc" || sortOrder === "desc")
  ) {
    return { sort_by: sortBy, sort_order: sortOrder };
  }
  return null;
}
