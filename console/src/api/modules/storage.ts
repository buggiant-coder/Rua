import { request } from "../request";
import type {
  StorageChatAddress,
  StorageChatListResponse,
  StorageChatSort,
  StorageDeleteResponse,
  StoragePreviewResponse,
} from "../types/storage";

export const storageApi = {
  listStorageChats: (options: { sort: StorageChatSort; before?: string }) => {
    const search = new URLSearchParams(
      options.sort === "size_desc"
        ? { sort_by: "size", sort_order: "desc" }
        : { sort_by: "updated_at", sort_order: "asc" },
    );
    if (options.before) search.set("before", options.before);
    return request<StorageChatListResponse>(`/storage/chats?${search}`);
  },
  previewStorageChats: (addresses: StorageChatAddress[]) =>
    request<StoragePreviewResponse>("/storage/chats/preview", {
      method: "POST",
      body: JSON.stringify({ addresses }),
    }),
  deleteStorageChats: (addresses: StorageChatAddress[]) =>
    request<StorageDeleteResponse>("/storage/chats/delete", {
      method: "POST",
      body: JSON.stringify({ addresses, acknowledge_data_loss: true }),
    }),
};
