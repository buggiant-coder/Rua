import { useState, useEffect, useCallback } from "react";
import type { ColumnDef } from "@/ui/Table";
import { useTranslation } from "react-i18next";

import {
  conflictsApi,
  type PendingConflictEntry,
} from "../../../api/modules/sync";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { RuaCard } from "@/components/RuaCard";
import { EmptyState } from "@/components/EmptyState";
import { Btn, Loading, Modal, Table, Tag, TextArea, toast } from "@/ui";

function DiffView({ winner, loser }: { winner: string; loser: string }) {
  const { t } = useTranslation();
  return (
    <div style={{ display: "flex", gap: 16 }}>
      <div style={{ flex: 1 }}>
        <span style={{ display: "block", marginBottom: 4, fontWeight: 600 }}>
          {t("conflicts.winner", "Version A (Winner)")}
        </span>
        <pre
          style={{
            background: "var(--card-bg, #f5f5f5)",
            padding: 12,
            fontSize: 12,
            fontFamily: "var(--font-mono)",
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
            maxHeight: 300,
            overflow: "auto",
            border: "1px solid var(--border-color, #d9d9d9)",
          }}
        >
          {winner}
        </pre>
      </div>
      <div style={{ flex: 1 }}>
        <span style={{ display: "block", marginBottom: 4, fontWeight: 600 }}>
          {t("conflicts.loser", "Version B (Loser)")}
        </span>
        <pre
          style={{
            background: "var(--card-bg, #f5f5f5)",
            padding: 12,
            fontSize: 12,
            fontFamily: "var(--font-mono)",
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
            maxHeight: 300,
            overflow: "auto",
            border: "1px solid var(--border-color, #d9d9d9)",
          }}
        >
          {loser}
        </pre>
      </div>
    </div>
  );
}

function ResolveModal({
  conflict,
  open,
  onClose,
  onResolved,
}: {
  conflict: PendingConflictEntry;
  open: boolean;
  onClose: () => void;
  onResolved: () => void;
}) {
  const { t } = useTranslation();
  const [resolving, setResolving] = useState(false);
  const [editMode, setEditMode] = useState(false);
  const [editText, setEditText] = useState("");

  const handleResolve = async (chosen: string) => {
    setResolving(true);
    try {
      await conflictsApi.resolve(conflict.id, chosen);
      toast.success(t("conflicts.resolved", "Conflict resolved"));
      onResolved();
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : "Failed to resolve conflict",
      );
    } finally {
      setResolving(false);
    }
  };

  const handleEdit = () => {
    setEditMode(true);
    setEditText(conflict.winner_text);
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      footer={null}
      title={t("conflicts.resolveTitle", "Resolve Conflict")}
      width={800}
      destroyOnClose
    >
      <div style={{ marginBottom: 16 }}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "var(--space-2)",
          }}
        >
          <Tag>{conflict.live_path}</Tag>
          <span style={{ color: "var(--text-2)" }}>
            {new Date(conflict.created_at_ms).toLocaleString()}
          </span>
        </div>
      </div>

      <DiffView winner={conflict.winner_text} loser={conflict.loser_text} />

      {editMode ? (
        <div style={{ marginTop: 16 }}>
          <span style={{ display: "block", marginBottom: 4, fontWeight: 600 }}>
            {t("conflicts.editLabel", "Edit resolution")}
          </span>
          <TextArea
            value={editText}
            onChange={(e) => setEditText(e.target.value)}
            rows={6}
            style={{ fontFamily: "var(--font-mono)", fontSize: 12 }}
          />
          <div style={{ display: "flex", gap: "var(--space-2)", marginTop: 8 }}>
            <Btn loading={resolving} onClick={() => handleResolve(editText)}>
              {t("conflicts.submitEdit", "Submit Edit")}
            </Btn>
            <Btn onClick={() => setEditMode(false)}>
              {t("common.cancel", "Cancel")}
            </Btn>
          </div>
        </div>
      ) : (
        <div style={{ display: "flex", gap: "var(--space-2)", marginTop: 16 }}>
          <Btn
            loading={resolving}
            onClick={() => handleResolve(conflict.winner_text)}
          >
            {t("conflicts.keepWinner", "Keep Version A")}
          </Btn>
          <Btn
            loading={resolving}
            onClick={() => handleResolve(conflict.loser_text)}
          >
            {t("conflicts.keepLoser", "Keep Version B")}
          </Btn>
          <Btn onClick={handleEdit}>
            {t("conflicts.edit", "Edit & Resolve")}
          </Btn>
        </div>
      )}
    </Modal>
  );
}

export default function SyncConflictsPage() {
  const { t } = useTranslation();
  const [conflicts, setConflicts] = useState<PendingConflictEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<PendingConflictEntry | null>(null);

  const fetchPending = useCallback(async () => {
    setLoading(true);
    try {
      const res = await conflictsApi.getPending();
      setConflicts(res.conflicts);
    } catch {
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchPending();
  }, [fetchPending]);

  const handleResolved = () => {
    setSelected(null);
    fetchPending();
  };

  const columns: ColumnDef<PendingConflictEntry>[] = [
    {
      title: t("conflicts.path", "Path"),
      dataIndex: "live_path",
      key: "live_path",
      ellipsis: true,
    },
    {
      title: t("conflicts.recordId", "Record"),
      dataIndex: "record_id",
      key: "record_id",
      ellipsis: true,
      width: 200,
    },
    {
      title: t("conflicts.createdAt", "Created"),
      dataIndex: "created_at_ms",
      key: "created_at_ms",
      width: 180,
      render: (ms: number) => new Date(ms).toLocaleString(),
    },
    {
      title: t("conflicts.actions", "Actions"),
      key: "actions",
      width: 120,
      render: (_: unknown, record: PendingConflictEntry) => (
        <Btn size="sm" onClick={() => setSelected(record)}>
          {t("conflicts.review", "Review")}
        </Btn>
      ),
    },
  ];

  return (
    <Page>
      <PageHeader
        parent={t("deviceSync.title", "Device Sync")}
        current={t("conflicts.pending", "Pending Conflicts")}
        onRefresh={fetchPending}
        refreshing={loading}
      />
      <PageBody>
        <RuaCard>
          {loading ? (
            <div style={{ textAlign: "center", padding: 40 }}>
              <Loading />
            </div>
          ) : conflicts.length === 0 ? (
            <EmptyState
              description={t(
                "conflicts.noPending",
                "No pending conflicts to review",
              )}
            />
          ) : (
            <Table<PendingConflictEntry>
              columns={columns}
              dataSource={conflicts}
              rowKey="id"
              pagination={{ pageSize: 20 }}
              size="sm"
            />
          )}
        </RuaCard>

        {selected && (
          <ResolveModal
            conflict={selected}
            open={!!selected}
            onClose={() => setSelected(null)}
            onResolved={handleResolved}
          />
        )}
      </PageBody>
    </Page>
  );
}
