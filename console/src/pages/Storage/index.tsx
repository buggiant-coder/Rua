import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Database,
  FolderKanban,
  HardDrive,
  LockKeyhole,
  Trash2,
} from "lucide-react";
import {
  api,
  type StorageChatItem,
  type StorageChatSort,
  type StoragePreviewResponse,
} from "@/api";
import { EmptyState } from "@/components/EmptyState";
import { Page, PageBody } from "@/components/Page";
import { PageHeader } from "@/components/PageHeader";
import { Btn, Check, Loading, Modal, Select, Tag, toast } from "@/ui";
import styles from "./index.module.less";

type AgeFilter = "all" | "7" | "30" | "90";

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    size /= 1024;
    index += 1;
  }
  return `${size >= 10 ? size.toFixed(1) : size.toFixed(2)} ${units[index]}`;
}

function beforeFor(filter: AgeFilter): string | undefined {
  if (filter === "all") return undefined;
  const before = new Date();
  before.setDate(before.getDate() - Number(filter));
  return before.toISOString();
}

function addressKey(item: StorageChatItem): string {
  return `${item.owner.kind}:${item.owner.id}:${item.address.chat_id}`;
}

export default function StoragePage() {
  const { t, i18n } = useTranslation();
  const [items, setItems] = useState<StorageChatItem[]>([]);
  const [preview, setPreview] = useState<StoragePreviewResponse | null>(null);
  const [age, setAge] = useState<AgeFilter>("all");
  const [sort, setSort] = useState<StorageChatSort>("size_desc");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());
  const [deletePreview, setDeletePreview] =
    useState<StoragePreviewResponse | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deleteLoading, setDeleteLoading] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const response = await api.listStorageChats({
        sort,
        before: beforeFor(age),
      });
      setItems(response);
      const available = new Set(response.map(addressKey));
      setSelectedKeys(
        (current) => new Set([...current].filter((key) => available.has(key))),
      );
      setPreview(
        response.length > 0
          ? await api.previewStorageChats(response.map((item) => item.address))
          : {
              chat_count: 0,
              attributed_bytes: 0,
              reclaimable_bytes: 0,
              shared_media_preserved: [],
              skipped: [],
            },
      );
    } catch (reason) {
      setPreview(null);
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setLoading(false);
    }
  }, [age, sort]);

  useEffect(() => {
    void load();
  }, [load]);

  const totalBytes = useMemo(() => preview?.attributed_bytes ?? 0, [preview]);
  const selectedItems = useMemo(
    () => items.filter((item) => selectedKeys.has(addressKey(item))),
    [items, selectedKeys],
  );
  const selectableItems = useMemo(
    () => items.filter((item) => !item.pinned && item.deletable),
    [items],
  );
  const selectedAddresses = useMemo(
    () => selectedItems.map((item) => item.address),
    [selectedItems],
  );

  const toggleSelectAll = useCallback(() => {
    setSelectedKeys((current) => {
      if (
        selectableItems.length > 0 &&
        selectableItems.every((item) => current.has(addressKey(item)))
      ) {
        return new Set();
      }
      return new Set(selectableItems.map(addressKey));
    });
  }, [selectableItems]);

  const openDelete = useCallback(async () => {
    if (selectedAddresses.length === 0) return;
    setDeleteLoading(true);
    setAcknowledged(false);
    try {
      setDeletePreview(await api.previewStorageChats(selectedAddresses));
      setDeleteOpen(true);
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setDeleteLoading(false);
    }
  }, [selectedAddresses]);

  const closeDelete = useCallback(() => {
    if (deleteLoading) return;
    setDeleteOpen(false);
    setDeletePreview(null);
    setAcknowledged(false);
  }, [deleteLoading]);

  const confirmDelete = useCallback(async () => {
    if (selectedAddresses.length === 0 || !acknowledged) return;
    setDeleteLoading(true);
    try {
      const result = await api.deleteStorageChats(selectedAddresses);
      const summary = t(
        "storage.deleteBatchResult",
        "Deleted {{deleted}}, skipped {{skipped}}, failed {{failed}}, pending {{pending}}. Freed {{size}} locally.",
        {
          deleted: result.deleted.length,
          skipped: result.skipped.length,
          failed: result.failed.length,
          pending: result.pending.length,
          size: formatBytes(result.freed_bytes),
        },
      );
      if (result.failed.length > 0 || result.deleted.length === 0) {
        toast.warning(summary);
      } else {
        toast.success(summary);
      }
      setDeleteOpen(false);
      setDeletePreview(null);
      setSelectedKeys(new Set());
      setAcknowledged(false);
      await load();
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setDeleteLoading(false);
    }
  }, [acknowledged, load, selectedAddresses, t]);
  return (
    <Page className={styles.page}>
      <PageHeader
        current={t("storage.title", "Storage")}
        onRefresh={() => void load()}
      />
      <PageBody className={styles.body}>
        <section className={styles.hero}>
          <div>
            <h1>{t("storage.heading", "Chat storage")}</h1>
            <p>
              {t(
                "storage.description",
                "Review local disk usage by Agent and Project Chat.",
              )}
            </p>
          </div>
          <div className={styles.filters}>
            <Select<AgeFilter>
              className={styles.ageFilter}
              value={age}
              onChange={(value) => value && setAge(value)}
              aria-label={t("storage.ageFilter", "Age filter")}
              options={[
                { value: "all", label: t("storage.allChats", "All Chats") },
                {
                  value: "7",
                  label: t("storage.olderThan7", "Older than 7 days"),
                },
                {
                  value: "30",
                  label: t("storage.olderThan30", "Older than 30 days"),
                },
                {
                  value: "90",
                  label: t("storage.olderThan90", "Older than 90 days"),
                },
              ]}
            />
            <Select<StorageChatSort>
              className={styles.sortFilter}
              value={sort}
              onChange={(value) => value && setSort(value)}
              aria-label={t("storage.sort", "Sort")}
              options={[
                {
                  value: "size_desc",
                  label: t("storage.largestFirst", "Largest first"),
                },
                {
                  value: "updated_asc",
                  label: t("storage.oldestFirst", "Oldest first"),
                },
              ]}
            />
            <Btn
              className={styles.actionButton}
              variant="outline"
              disabled={selectableItems.length === 0}
              onClick={toggleSelectAll}
            >
              {t("storage.selectAll", "Select all")}
            </Btn>
            <Btn
              className={styles.actionButton}
              variant="danger"
              icon={<Trash2 size={16} aria-hidden="true" />}
              disabled={selectedItems.length === 0}
              loading={deleteLoading && !deleteOpen}
              onClick={() => void openDelete()}
            >
              {t("storage.deleteSelected", "Delete")}
            </Btn>
          </div>
        </section>

        <section
          className={styles.metrics}
          aria-label={t("storage.overview", "Overview")}
        >
          <article className={styles.metricCard}>
            <HardDrive aria-hidden="true" />
            <div>
              <span>{t("storage.localUsage", "Local Chat usage")}</span>
              <strong>{formatBytes(totalBytes)}</strong>
            </div>
          </article>
          <article className={styles.metricCard}>
            <Database aria-hidden="true" />
            <div>
              <span>{t("storage.chatCount", "Chats")}</span>
              <strong>{items.length}</strong>
            </div>
          </article>
          <article className={styles.metricCard}>
            <LockKeyhole aria-hidden="true" />
            <div>
              <span>{t("storage.reclaimable", "Reclaimable")}</span>
              <strong>{formatBytes(preview?.reclaimable_bytes ?? 0)}</strong>
            </div>
          </article>
        </section>

        {loading ? (
          <div className={styles.center}>
            <Loading />
          </div>
        ) : error ? (
          <EmptyState
            variant="error"
            description={error}
            onRetry={() => void load()}
          />
        ) : items.length === 0 ? (
          <EmptyState
            description={t("storage.empty", "No Chats match these filters.")}
          />
        ) : (
          <section
            className={styles.list}
            aria-label={t("storage.chatList", "Chat storage list")}
          >
            {items.map((item) => (
              <article
                className={`${styles.chatCard} ${
                  selectedKeys.has(addressKey(item)) ? styles.selected : ""
                }`}
                key={`${item.owner.kind}:${item.owner.id}:${item.address.chat_id}`}
              >
                <div className={styles.identity}>
                  <Check
                    checked={selectedKeys.has(addressKey(item))}
                    disabled={item.pinned || !item.deletable}
                    aria-label={t("storage.selectChat", "Select {{name}}", {
                      name: item.name || item.address.chat_id,
                    })}
                    onChange={(event) => {
                      const key = addressKey(item);
                      setSelectedKeys((current) => {
                        const next = new Set(current);
                        if (event.target.checked) next.add(key);
                        else next.delete(key);
                        return next;
                      });
                    }}
                  />
                  <div className={styles.icon}>
                    <FolderKanban aria-hidden="true" />
                  </div>
                  <div className={styles.titleBlock}>
                    <div className={styles.titleRow}>
                      <h2>{item.name || item.address.chat_id}</h2>
                      {item.pinned ? (
                        <Tag color="warning">
                          {t("storage.pinned", "Pinned")}
                        </Tag>
                      ) : null}
                    </div>
                    <p>
                      {item.owner.kind === "project"
                        ? t("storage.projectOwner", "Project: {{owner}}", {
                            owner: item.owner.name || item.owner.id,
                          })
                        : t("storage.agentOwner", "Agent: {{owner}}", {
                            owner: item.owner.name || item.owner.id,
                          })}
                    </p>
                    <time dateTime={item.updated_at}>
                      {t("storage.lastActive", "Last active {{time}}", {
                        time: new Intl.DateTimeFormat(i18n.language, {
                          dateStyle: "medium",
                          timeStyle: "short",
                        }).format(new Date(item.updated_at)),
                      })}
                    </time>
                  </div>
                </div>
                <dl className={styles.breakdown}>
                  <div>
                    <dt>{t("storage.session", "Session")}</dt>
                    <dd>{formatBytes(item.session_bytes)}</dd>
                  </div>
                  <div>
                    <dt>{t("storage.tasks", "Tasks")}</dt>
                    <dd>{formatBytes(item.task_bytes)}</dd>
                  </div>
                  <div>
                    <dt>{t("storage.toolLogs", "Tool logs")}</dt>
                    <dd>{formatBytes(item.tool_log_bytes)}</dd>
                  </div>
                  <div>
                    <dt>{t("storage.media", "Media")}</dt>
                    <dd>{formatBytes(item.media_bytes)}</dd>
                  </div>
                  <div>
                    <dt>{t("storage.attributed", "Attributed")}</dt>
                    <dd>{formatBytes(item.attributed_bytes)}</dd>
                  </div>
                  <div className={styles.total}>
                    <dt>{t("storage.reclaimable", "Reclaimable")}</dt>
                    <dd>{formatBytes(item.reclaimable_bytes)}</dd>
                  </div>
                </dl>
              </article>
            ))}
          </section>
        )}
        <Modal
          open={deleteOpen}
          onClose={closeDelete}
          title={t("storage.deleteTitle", "Permanently delete selected Chats?")}
          footer={
            <>
              <Btn
                variant="ghost"
                onClick={closeDelete}
                disabled={deleteLoading}
              >
                {t("common.cancel", "Cancel")}
              </Btn>
              <Btn
                variant="danger-filled"
                disabled={!acknowledged}
                loading={deleteLoading}
                onClick={() => void confirmDelete()}
              >
                {t("storage.deleteConfirm", "Delete permanently")}
              </Btn>
            </>
          }
        >
          <div className={styles.deleteBody}>
            <p className={styles.warning}>
              {t(
                "storage.deleteWarning",
                "This permanently deletes the Chat and its associated local data. The deletion will propagate to paired devices and cannot be undone.",
              )}
            </p>
            <dl className={styles.deleteSummary}>
              <div>
                <dt>{t("storage.chatCount", "Chats")}</dt>
                <dd>{deletePreview?.chat_count ?? selectedItems.length}</dd>
              </div>
              <div>
                <dt>{t("storage.attributed", "Attributed")}</dt>
                <dd>{formatBytes(deletePreview?.attributed_bytes ?? 0)}</dd>
              </div>
              <div>
                <dt>{t("storage.reclaimable", "Reclaimable")}</dt>
                <dd>{formatBytes(deletePreview?.reclaimable_bytes ?? 0)}</dd>
              </div>
            </dl>
            {deletePreview?.shared_media_preserved.length ? (
              <p className={styles.preserved}>
                {t(
                  "storage.sharedMediaPreserved",
                  "Shared Media is preserved because another Chat still references it.",
                )}
              </p>
            ) : null}
            <Check
              checked={acknowledged}
              onChange={(event) => setAcknowledged(event.target.checked)}
            >
              {t(
                "storage.acknowledgeDelete",
                "I understand this deletion is permanent and affects paired devices.",
              )}
            </Check>
          </div>
        </Modal>
      </PageBody>
    </Page>
  );
}
