import { request } from "../request";

export interface UserInfo {
  remark: string;
  username: string;
}

export interface PendingEntry {
  user_id: string;
  channel: string;
  timestamp: number;
  first_message: string;
  remark: string;
  username: string;
}

export interface ACLData {
  whitelist: Record<string, UserInfo>;
  blacklist: Record<string, UserInfo>;
  pending: PendingEntry[];
}

function accessControlPath(path = "", agentId?: string): string {
  const prefix = agentId
    ? `/agents/${encodeURIComponent(agentId)}/access-control`
    : "/access-control";
  return `${prefix}${path}`;
}

export const accessControlApi = {
  getAclAll: (agentId?: string) =>
    request<Record<string, ACLData>>(accessControlPath("", agentId)),

  getAclChannel: (channel: string, agentId?: string) =>
    request<ACLData>(
      accessControlPath(`/${encodeURIComponent(channel)}`, agentId),
    ),

  addAclWhitelist: (
    entries: {
      channel: string;
      user_id: string;
      remark?: string;
      username?: string;
    }[],
    agentId?: string,
  ) =>
    request(accessControlPath("/whitelist/add", agentId), {
      method: "POST",
      body: JSON.stringify({ entries }),
    }),

  removeAclWhitelist: (
    entries: { channel: string; user_id: string }[],
    agentId?: string,
  ) =>
    request(accessControlPath("/whitelist/remove", agentId), {
      method: "POST",
      body: JSON.stringify({ entries }),
    }),

  addAclBlacklist: (
    entries: {
      channel: string;
      user_id: string;
      remark?: string;
      username?: string;
    }[],
    agentId?: string,
  ) =>
    request(accessControlPath("/blacklist/add", agentId), {
      method: "POST",
      body: JSON.stringify({ entries }),
    }),

  removeAclBlacklist: (
    entries: { channel: string; user_id: string }[],
    agentId?: string,
  ) =>
    request(accessControlPath("/blacklist/remove", agentId), {
      method: "POST",
      body: JSON.stringify({ entries }),
    }),

  updateAclRemark: (
    channel: string,
    userId: string,
    remark: string,
    agentId?: string,
  ) =>
    request(accessControlPath("/remark", agentId), {
      method: "POST",
      body: JSON.stringify({ channel, user_id: userId, remark }),
    }),

  getAclAllPending: (agentId?: string) =>
    request<PendingEntry[]>(accessControlPath("/pending/all", agentId)),

  approveAclPending: (
    entries: { channel: string; user_id: string; remark?: string }[],
    agentId?: string,
  ) =>
    request(accessControlPath("/pending/approve", agentId), {
      method: "POST",
      body: JSON.stringify({ entries }),
    }),

  denyAclPending: (
    entries: { channel: string; user_id: string; remark?: string }[],
    agentId?: string,
  ) =>
    request(accessControlPath("/pending/deny", agentId), {
      method: "POST",
      body: JSON.stringify({ entries }),
    }),

  dismissAclPending: (
    entries: { channel: string; user_id: string }[],
    agentId?: string,
  ) =>
    request(accessControlPath("/pending/dismiss", agentId), {
      method: "POST",
      body: JSON.stringify({ entries }),
    }),

  updatePendingRemark: (
    channel: string,
    userId: string,
    remark: string,
    agentId?: string,
  ) =>
    request(accessControlPath("/pending/remark", agentId), {
      method: "POST",
      body: JSON.stringify({ channel, user_id: userId, remark }),
    }),

  updateUsername: (
    channel: string,
    userId: string,
    username: string,
    agentId?: string,
  ) =>
    request(accessControlPath("/username", agentId), {
      method: "POST",
      body: JSON.stringify({ channel, user_id: userId, username }),
    }),
};
