import { request } from "../request";
import type {
  InboxListOptions,
  InboxMessage,
  InboxPage,
} from "../types/userInbox";

function messagePath(id: string): string {
  return `/inbox/messages/${encodeURIComponent(id)}`;
}

export const userInboxApi = {
  list: (options: InboxListOptions = {}) => {
    const query = new URLSearchParams();
    if (options.cursor) query.set("cursor", options.cursor);
    if (options.limit !== undefined) query.set("limit", String(options.limit));
    if (options.read !== undefined) query.set("read", String(options.read));
    const suffix = query.size > 0 ? `?${query.toString()}` : "";
    return request<InboxPage>(`/inbox/messages${suffix}`);
  },

  get: (id: string) => request<InboxMessage>(messagePath(id)),

  setRead: (id: string, read: boolean) =>
    request<InboxMessage>(messagePath(id), {
      method: "PATCH",
      body: JSON.stringify({ read }),
    }),

  delete: (id: string) => request<void>(messagePath(id), { method: "DELETE" }),

  unreadCount: () => request<{ count: number }>("/inbox/unread-count"),

  markAllRead: () =>
    request<{ updated_count: number }>("/inbox/read-all", {
      method: "POST",
    }),
};
