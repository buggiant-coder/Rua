import dayjs from "dayjs";
import type { TFunction } from "i18next";
import {
  CHANNEL_COLORS,
  mapChannelTagColor,
} from "../../../../constants/channel";
import { formatTime, type Session } from "./constants";
import styles from "../index.module.less";
import { MessageSquare, Pencil, Trash2 } from "lucide-react";
import { Check, Btn, StackedList, Tag, Tooltip } from "@/ui";

const truncateId = (id: string): string => {
  if (!id) return "";
  if (id.length <= 12) return id;
  return `${id.slice(0, 6)}…${id.slice(-4)}`;
};

const normalizeIso = (ts: string | null | undefined): string | null => {
  if (!ts) return null;
  if (/[Z+-]\d{2}:?\d{2}$/.test(ts) || ts.endsWith("Z")) return ts;
  return ts + "Z";
};

const formatRelative = (ts: string | null | undefined): string | null => {
  const norm = normalizeIso(ts);
  if (!norm) return null;
  const d = dayjs(norm);
  if (!d.isValid()) return null;
  return d.fromNow();
};

interface SessionListProps {
  sessions: Session[];
  batchMode: boolean;
  selectedKeys: React.Key[];
  onSelectChange: (keys: React.Key[]) => void;
  onOpenChat: (chatId: string) => void;
  onEdit: (session: Session) => void;
  onDelete: (chatId: string) => void;
  t: TFunction;
}

export function SessionList({
  sessions,
  batchMode,
  selectedKeys,
  onSelectChange,
  onOpenChat,
  onEdit,
  onDelete,
  t,
}: SessionListProps) {
  const selectedSet = new Set(selectedKeys);

  const toggleOne = (id: string) => {
    if (selectedSet.has(id)) {
      onSelectChange(selectedKeys.filter((k) => k !== id));
    } else {
      onSelectChange([...selectedKeys, id]);
    }
  };

  return (
    <StackedList className={styles.list}>
      {sessions.map((s) => {
        const isSelected = selectedSet.has(s.id);
        const channel = s.channel || "default";
        const tagColor = mapChannelTagColor(CHANNEL_COLORS[channel]);
        const created = formatTime(s.created_at);
        const lastActive = formatRelative(s.updated_at);
        const updatedFull = formatTime(s.updated_at);

        return (
          <div
            key={s.id}
            className={`${styles.listItem} ${
              isSelected ? styles.listItemSelected : ""
            }`}
          >
            {batchMode && (
              <Check
                className={styles.listItemCheckbox}
                checked={isSelected}
                onChange={() => toggleOne(s.id)}
              />
            )}

            <div className={styles.listItemMain}>
              <div className={styles.listItemTopRow}>
                <span className={styles.sessionName}>
                  {s.name || t("sessions.fieldName") || "—"}
                </span>
              </div>

              <div className={styles.listItemMeta}>
                <Tooltip title={s.id} placement="top-start">
                  <span
                    className={styles.idChip}
                    style={{ cursor: "pointer" }}
                    onClick={() => navigator.clipboard.writeText(s.id)}
                  >
                    {truncateId(s.id)}
                  </span>
                </Tooltip>
                <Tag className={styles.channelTag} color={tagColor}>
                  {channel}
                </Tag>
                <span className={styles.metaDot}>·</span>
                {s.user_id && (
                  <span className={styles.metaItem}>
                    <span className={styles.metaLabel}>user</span>
                    <span className={styles.metaValue}>{s.user_id}</span>
                  </span>
                )}
                <span className={styles.metaDot}>·</span>
                <span className={styles.metaItem}>
                  <span className={styles.metaLabel}>
                    {t("sessions.columnCreatedAt")}
                  </span>
                  <span className={styles.metaValue}>{created}</span>
                </span>
                {lastActive && (
                  <>
                    <span className={styles.metaDot}>·</span>
                    <Tooltip title={updatedFull}>
                      <span className={styles.metaItem}>
                        <span className={styles.metaLabel}>
                          {t("sessions.columnUpdatedAt")}
                        </span>
                        <span className={styles.metaValue}>{lastActive}</span>
                      </span>
                    </Tooltip>
                  </>
                )}
              </div>
            </div>

            <div className={styles.listItemActions}>
              <Tooltip title={t("sessions.openChat")}>
                <Btn
                  size="sm"
                  variant="outline"
                  icon={<MessageSquare />}
                  aria-label={t("sessions.openChat")}
                  onClick={() => onOpenChat(s.id)}
                />
              </Tooltip>
              <Tooltip title={t("common.edit")}>
                <Btn
                  size="sm"
                  variant="outline"
                  icon={<Pencil />}
                  aria-label={t("common.edit")}
                  onClick={() => onEdit(s)}
                />
              </Tooltip>
              <Tooltip title={t("common.delete")}>
                <Btn
                  size="sm"
                  variant="danger"
                  icon={<Trash2 />}
                  aria-label={t("common.delete")}
                  onClick={() => onDelete(s.id)}
                />
              </Tooltip>
            </div>
          </div>
        );
      })}
    </StackedList>
  );
}
