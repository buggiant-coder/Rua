export type StorageChatScope = "agent" | "project";
export type StorageChatSort = "size_desc" | "updated_asc";

export type StorageChatAddress =
  | { scope: { kind: "agent"; agent_id: string }; chat_id: string }
  | { scope: { kind: "project"; project_id: string }; chat_id: string };

export interface StorageChatOwner {
  kind: StorageChatScope;
  id: string;
  name: string;
}

export interface StorageChatItem {
  address: StorageChatAddress;
  owner: StorageChatOwner;
  name: string;
  pinned: boolean;
  updated_at: string;
  session_bytes: number;
  task_bytes: number;
  tool_log_bytes: number;
  media_bytes: number;
  attributed_bytes: number;
  reclaimable_bytes: number;
  total_bytes: number;
  deletable: boolean;
}

export type StorageChatListResponse = StorageChatItem[];

export interface StorageSharedMedia {
  address: StorageChatAddress;
  media_ids: string[];
}

export interface StoragePreviewResponse {
  chat_count: number;
  attributed_bytes: number;
  reclaimable_bytes: number;
  shared_media_preserved: StorageSharedMedia[];
  skipped: Array<{ address: StorageChatAddress; reason: string }>;
}

export interface StorageDeleteIssue {
  address: StorageChatAddress;
  reason: string;
}

export interface StorageDeleteResponse {
  deleted: StorageChatAddress[];
  skipped: StorageDeleteIssue[];
  failed: StorageDeleteIssue[];
  pending: StorageDeleteIssue[];
  freed_bytes: number;
  shared_media_preserved: StorageSharedMedia[];
}
