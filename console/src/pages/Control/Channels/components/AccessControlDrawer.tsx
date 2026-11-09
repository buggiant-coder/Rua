import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Btn, Check, Drawer, Table, Tabs, TextInput, toast } from "@/ui";
import api from "../../../../api";
import type {
  ACLData,
  PendingEntry,
} from "../../../../api/modules/accessControl";
import styles from "../index.module.less";

type ListName = "whitelist" | "blacklist";

interface AccessControlDrawerProps {
  open: boolean;
  agentId?: string | null;
  onClose: () => void;
}

interface AclMember {
  key: string;
  channel: string;
  user_id: string;
  username: string;
  remark: string;
  list: ListName;
}

interface Draft {
  channel: string;
  userId: string;
  username: string;
  remark: string;
}

const emptyDraft: Draft = {
  channel: "",
  userId: "",
  username: "",
  remark: "",
};

function entryKey(channel: string, userId: string): string {
  return `${channel}\u0000${userId}`;
}

function entryPayload(key: string): { channel: string; user_id: string } {
  const [channel, user_id] = key.split("\u0000");
  return { channel, user_id };
}

function flattenMembers(snapshot: Record<string, ACLData>, list: ListName) {
  return Object.entries(snapshot).flatMap(([channel, acl]) =>
    Object.entries(acl[list]).map(([user_id, info]) => ({
      key: entryKey(channel, user_id),
      channel,
      user_id,
      username: info.username,
      remark: info.remark,
      list,
    })),
  );
}

export function AccessControlDrawer({
  open,
  agentId,
  onClose,
}: AccessControlDrawerProps) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState("pending");
  const [snapshot, setSnapshot] = useState<Record<string, ACLData>>({});
  const [pending, setPending] = useState<PendingEntry[]>([]);
  const [selectedPending, setSelectedPending] = useState<Set<string>>(
    () => new Set(),
  );
  const [selectedWhitelist, setSelectedWhitelist] = useState<Set<string>>(
    () => new Set(),
  );
  const [selectedBlacklist, setSelectedBlacklist] = useState<Set<string>>(
    () => new Set(),
  );
  const [draft, setDraft] = useState<Draft>(emptyDraft);
  const [busy, setBusy] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const scopedAgentId = agentId || undefined;

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [aclSnapshot, pendingRows] = await Promise.all([
        api.getAclAll(scopedAgentId),
        api.getAclAllPending(scopedAgentId),
      ]);
      setSnapshot(aclSnapshot);
      setPending(pendingRows);
      setSelectedPending(new Set());
      setSelectedWhitelist(new Set());
      setSelectedBlacklist(new Set());
    } catch {
      toast.error(
        t("channels.aclLoadFailed", {
          defaultValue: "Failed to load access control",
        }),
      );
    } finally {
      setLoading(false);
    }
  }, [scopedAgentId]);

  useEffect(() => {
    if (open) void refresh();
  }, [open, refresh]);

  const whitelist = useMemo(
    () => flattenMembers(snapshot, "whitelist"),
    [snapshot],
  );
  const blacklist = useMemo(
    () => flattenMembers(snapshot, "blacklist"),
    [snapshot],
  );

  const toggleSelection = (
    current: Set<string>,
    setCurrent: (next: Set<string>) => void,
    key: string,
    checked: boolean,
  ) => {
    const next = new Set(current);
    if (checked) {
      next.add(key);
    } else {
      next.delete(key);
    }
    setCurrent(next);
  };

  const runAction = async (label: string, action: () => Promise<unknown>) => {
    setBusy(label);
    try {
      await action();
      await refresh();
      toast.success(
        t("channels.aclSaved", { defaultValue: "Access control updated" }),
      );
    } catch {
      toast.error(
        t("channels.aclSaveFailed", {
          defaultValue: "Failed to update access control",
        }),
      );
    } finally {
      setBusy(null);
    }
  };

  const pendingEntries = (keys: Set<string>) =>
    [...keys].map((key) => {
      const payload = entryPayload(key);
      const row = pending.find(
        (entry) =>
          entry.channel === payload.channel &&
          entry.user_id === payload.user_id,
      );
      return { ...payload, remark: row?.remark };
    });

  const memberEntries = (keys: Set<string>) => [...keys].map(entryPayload);

  const updateDraft = (patch: Partial<Draft>) =>
    setDraft((current) => ({ ...current, ...patch }));

  const addManualEntry = async (list: ListName) => {
    const channel = draft.channel.trim();
    const userId = draft.userId.trim();
    if (!channel || !userId) return;
    const entry = {
      channel,
      user_id: userId,
      remark: draft.remark.trim(),
      username: draft.username.trim(),
    };
    await runAction(`add-${list}`, async () => {
      if (list === "whitelist") {
        return api.addAclWhitelist([entry], scopedAgentId);
      }
      return api.addAclBlacklist([entry], scopedAgentId);
    });
    setDraft(emptyDraft);
  };

  const savePendingMetadata = (row: PendingEntry) =>
    runAction(`pending-meta-${row.channel}-${row.user_id}`, async () => {
      await api.updatePendingRemark(
        row.channel,
        row.user_id,
        row.remark,
        scopedAgentId,
      );
      return api.updateUsername(
        row.channel,
        row.user_id,
        row.username,
        scopedAgentId,
      );
    });

  const saveMemberMetadata = (row: AclMember) =>
    runAction(`member-meta-${row.channel}-${row.user_id}`, async () => {
      await api.updateAclRemark(
        row.channel,
        row.user_id,
        row.remark,
        scopedAgentId,
      );
      return api.updateUsername(
        row.channel,
        row.user_id,
        row.username,
        scopedAgentId,
      );
    });

  const pendingColumns = [
    {
      key: "select",
      width: 44,
      render: (_: unknown, row: PendingEntry) => {
        const key = entryKey(row.channel, row.user_id);
        return (
          <Check
            aria-label={t("channels.aclSelectUser", {
              defaultValue: "Select user",
            })}
            checked={selectedPending.has(key)}
            onChange={(event) =>
              toggleSelection(
                selectedPending,
                setSelectedPending,
                key,
                event.target.checked,
              )
            }
          />
        );
      },
    },
    {
      title: t("channels.channel", { defaultValue: "Channel" }),
      dataIndex: "channel",
      width: 100,
    },
    {
      title: t("channels.aclUser", { defaultValue: "User" }),
      render: (_: unknown, row: PendingEntry) => (
        <div className={styles.aclUserCell}>
          <span>{row.user_id}</span>
          <TextInput
            aria-label={t("channels.aclUsername", {
              defaultValue: "Username",
            })}
            value={row.username}
            onChange={(event) =>
              setPending((rows) =>
                rows.map((item) =>
                  item.channel === row.channel && item.user_id === row.user_id
                    ? { ...item, username: event.target.value }
                    : item,
                ),
              )
            }
          />
        </div>
      ),
    },
    {
      title: t("channels.aclRemark", { defaultValue: "Remark" }),
      render: (_: unknown, row: PendingEntry) => (
        <TextInput
          aria-label={t("channels.aclRemark", { defaultValue: "Remark" })}
          value={row.remark}
          onChange={(event) =>
            setPending((rows) =>
              rows.map((item) =>
                item.channel === row.channel && item.user_id === row.user_id
                  ? { ...item, remark: event.target.value }
                  : item,
              ),
            )
          }
        />
      ),
    },
    {
      title: t("channels.aclFirstMessage", { defaultValue: "First message" }),
      dataIndex: "first_message",
      ellipsis: true,
    },
    {
      title: t("channels.aclActions", { defaultValue: "Actions" }),
      width: 256,
      render: (_: unknown, row: PendingEntry) => {
        const entry = [
          { channel: row.channel, user_id: row.user_id, remark: row.remark },
        ];
        return (
          <div className={styles.aclRowActions}>
            <Btn
              size="sm"
              variant="outline"
              onClick={() => savePendingMetadata(row)}
              loading={busy === `pending-meta-${row.channel}-${row.user_id}`}
            >
              {t("common.save", { defaultValue: "Save" })}
            </Btn>
            <Btn
              size="sm"
              onClick={() =>
                runAction("pending-approve", () =>
                  api.approveAclPending(entry, scopedAgentId),
                )
              }
            >
              {t("channels.aclApprove", { defaultValue: "Approve" })}
            </Btn>
            <Btn
              size="sm"
              variant="danger"
              onClick={() =>
                runAction("pending-deny", () =>
                  api.denyAclPending(entry, scopedAgentId),
                )
              }
            >
              {t("channels.aclDeny", { defaultValue: "Deny" })}
            </Btn>
            <Btn
              size="sm"
              variant="ghost"
              onClick={() =>
                runAction("pending-dismiss", () =>
                  api.dismissAclPending(
                    [{ channel: row.channel, user_id: row.user_id }],
                    scopedAgentId,
                  ),
                )
              }
            >
              {t("channels.aclDismiss", { defaultValue: "Dismiss" })}
            </Btn>
          </div>
        );
      },
    },
  ];

  const memberColumns = (
    list: ListName,
    selected: Set<string>,
    setSelected: (next: Set<string>) => void,
  ) => [
    {
      key: "select",
      width: 44,
      render: (_: unknown, row: AclMember) => (
        <Check
          aria-label={t("channels.aclSelectUser", {
            defaultValue: "Select user",
          })}
          checked={selected.has(row.key)}
          onChange={(event) =>
            toggleSelection(
              selected,
              setSelected,
              row.key,
              event.target.checked,
            )
          }
        />
      ),
    },
    {
      title: t("channels.channel", { defaultValue: "Channel" }),
      dataIndex: "channel",
      width: 100,
    },
    {
      title: t("channels.aclUser", { defaultValue: "User" }),
      render: (_: unknown, row: AclMember) => (
        <div className={styles.aclUserCell}>
          <span>{row.user_id}</span>
          <TextInput
            aria-label={t("channels.aclUsername", {
              defaultValue: "Username",
            })}
            value={row.username}
            onChange={(event) =>
              setSnapshot((current) => ({
                ...current,
                [row.channel]: {
                  ...current[row.channel],
                  [list]: {
                    ...current[row.channel]?.[list],
                    [row.user_id]: {
                      ...current[row.channel]?.[list]?.[row.user_id],
                      username: event.target.value,
                    },
                  },
                },
              }))
            }
          />
        </div>
      ),
    },
    {
      title: t("channels.aclRemark", { defaultValue: "Remark" }),
      render: (_: unknown, row: AclMember) => (
        <TextInput
          aria-label={t("channels.aclRemark", { defaultValue: "Remark" })}
          value={row.remark}
          onChange={(event) =>
            setSnapshot((current) => ({
              ...current,
              [row.channel]: {
                ...current[row.channel],
                [list]: {
                  ...current[row.channel]?.[list],
                  [row.user_id]: {
                    ...current[row.channel]?.[list]?.[row.user_id],
                    remark: event.target.value,
                  },
                },
              },
            }))
          }
        />
      ),
    },
    {
      title: t("channels.aclActions", { defaultValue: "Actions" }),
      width: 150,
      render: (_: unknown, row: AclMember) => (
        <div className={styles.aclRowActions}>
          <Btn
            size="sm"
            variant="outline"
            onClick={() => saveMemberMetadata(row)}
            loading={busy === `member-meta-${row.channel}-${row.user_id}`}
          >
            {t("common.save", { defaultValue: "Save" })}
          </Btn>
          <Btn
            size="sm"
            variant="danger"
            onClick={() =>
              runAction(`remove-${list}`, () =>
                list === "whitelist"
                  ? api.removeAclWhitelist(
                      [{ channel: row.channel, user_id: row.user_id }],
                      scopedAgentId,
                    )
                  : api.removeAclBlacklist(
                      [{ channel: row.channel, user_id: row.user_id }],
                      scopedAgentId,
                    ),
              )
            }
          >
            {t("channels.aclRemove", { defaultValue: "Remove" })}
          </Btn>
        </div>
      ),
    },
  ];

  const selectedPendingEntries = pendingEntries(selectedPending);
  const selectedWhitelistEntries = memberEntries(selectedWhitelist);
  const selectedBlacklistEntries = memberEntries(selectedBlacklist);

  const pendingToolbar = (
    <div className={styles.aclToolbar}>
      <Btn
        size="sm"
        disabled={selectedPendingEntries.length === 0}
        onClick={() =>
          runAction("batch-approve", () =>
            api.approveAclPending(selectedPendingEntries, scopedAgentId),
          )
        }
      >
        {t("channels.aclApproveSelected", {
          defaultValue: "Approve selected",
        })}
      </Btn>
      <Btn
        size="sm"
        variant="danger"
        disabled={selectedPendingEntries.length === 0}
        onClick={() =>
          runAction("batch-deny", () =>
            api.denyAclPending(selectedPendingEntries, scopedAgentId),
          )
        }
      >
        {t("channels.aclDenySelected", { defaultValue: "Deny selected" })}
      </Btn>
      <Btn
        size="sm"
        variant="ghost"
        disabled={selectedPendingEntries.length === 0}
        onClick={() =>
          runAction("batch-dismiss", () =>
            api.dismissAclPending(selectedPendingEntries, scopedAgentId),
          )
        }
      >
        {t("channels.aclDismissSelected", {
          defaultValue: "Dismiss selected",
        })}
      </Btn>
    </div>
  );

  const memberToolbar = (
    list: ListName,
    entries: { channel: string; user_id: string }[],
  ) => (
    <div className={styles.aclToolbar}>
      <Btn
        size="sm"
        variant="danger"
        disabled={entries.length === 0}
        onClick={() =>
          runAction(`batch-remove-${list}`, () =>
            list === "whitelist"
              ? api.removeAclWhitelist(entries, scopedAgentId)
              : api.removeAclBlacklist(entries, scopedAgentId),
          )
        }
      >
        {t("channels.aclRemoveSelected", { defaultValue: "Remove selected" })}
      </Btn>
    </div>
  );

  return (
    <Drawer
      open={open}
      onClose={onClose}
      width={880}
      title={t("channels.aclTitle", { defaultValue: "Access control" })}
      footer={
        <div className={styles.formActions}>
          <Btn variant="outline" onClick={refresh} loading={loading}>
            {t("common.refresh", { defaultValue: "Refresh" })}
          </Btn>
          <Btn onClick={onClose}>
            {t("common.close", { defaultValue: "Close" })}
          </Btn>
        </div>
      }
    >
      <div className={styles.aclManualForm}>
        <TextInput
          aria-label={t("channels.channel", { defaultValue: "Channel" })}
          placeholder={t("channels.channel", { defaultValue: "Channel" })}
          value={draft.channel}
          onChange={(event) => updateDraft({ channel: event.target.value })}
        />
        <TextInput
          aria-label={t("channels.aclUserId", { defaultValue: "User ID" })}
          placeholder={t("channels.aclUserId", { defaultValue: "User ID" })}
          value={draft.userId}
          onChange={(event) => updateDraft({ userId: event.target.value })}
        />
        <TextInput
          aria-label={t("channels.aclUsername", { defaultValue: "Username" })}
          placeholder={t("channels.aclUsername", { defaultValue: "Username" })}
          value={draft.username}
          onChange={(event) => updateDraft({ username: event.target.value })}
        />
        <TextInput
          aria-label={t("channels.aclRemark", { defaultValue: "Remark" })}
          placeholder={t("channels.aclRemark", { defaultValue: "Remark" })}
          value={draft.remark}
          onChange={(event) => updateDraft({ remark: event.target.value })}
        />
        <Btn
          variant="outline"
          disabled={!draft.channel.trim() || !draft.userId.trim()}
          onClick={() => addManualEntry("whitelist")}
          loading={busy === "add-whitelist"}
        >
          {t("channels.aclAddWhitelist", {
            defaultValue: "Add whitelist",
          })}
        </Btn>
        <Btn
          variant="danger"
          disabled={!draft.channel.trim() || !draft.userId.trim()}
          onClick={() => addManualEntry("blacklist")}
          loading={busy === "add-blacklist"}
        >
          {t("channels.aclAddBlacklist", {
            defaultValue: "Add blacklist",
          })}
        </Btn>
      </div>

      <Tabs
        activeKey={activeTab}
        onChange={setActiveTab}
        items={[
          {
            key: "pending",
            label: t("channels.aclPending", {
              count: pending.length,
              defaultValue: "Pending ({{count}})",
            }),
            children: (
              <>
                {pendingToolbar}
                <Table
                  rowKey={(row: PendingEntry) =>
                    entryKey(row.channel, row.user_id)
                  }
                  columns={pendingColumns}
                  dataSource={pending}
                  loading={loading}
                  emptyText={t("channels.aclNoPending", {
                    defaultValue: "No pending users",
                  })}
                  scroll={{ x: 920 }}
                />
              </>
            ),
          },
          {
            key: "whitelist",
            label: t("channels.aclWhitelist", {
              count: whitelist.length,
              defaultValue: "Whitelist ({{count}})",
            }),
            children: (
              <>
                {memberToolbar("whitelist", selectedWhitelistEntries)}
                <Table
                  rowKey="key"
                  columns={memberColumns(
                    "whitelist",
                    selectedWhitelist,
                    setSelectedWhitelist,
                  )}
                  dataSource={whitelist}
                  loading={loading}
                  emptyText={t("channels.aclNoWhitelist", {
                    defaultValue: "No whitelist users",
                  })}
                  scroll={{ x: 760 }}
                />
              </>
            ),
          },
          {
            key: "blacklist",
            label: t("channels.aclBlacklist", {
              count: blacklist.length,
              defaultValue: "Blacklist ({{count}})",
            }),
            children: (
              <>
                {memberToolbar("blacklist", selectedBlacklistEntries)}
                <Table
                  rowKey="key"
                  columns={memberColumns(
                    "blacklist",
                    selectedBlacklist,
                    setSelectedBlacklist,
                  )}
                  dataSource={blacklist}
                  loading={loading}
                  emptyText={t("channels.aclNoBlacklist", {
                    defaultValue: "No blacklist users",
                  })}
                  scroll={{ x: 760 }}
                />
              </>
            ),
          },
        ]}
      />
    </Drawer>
  );
}
