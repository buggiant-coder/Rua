import type { ColumnDef } from "@/ui/Table";
import type { TFunction } from "i18next";
import { formatTime, type Session } from "./constants";
import {
  CHANNEL_COLORS,
  mapChannelTagColor,
} from "../../../../constants/channel";
import styles from "../index.module.less";
import { Pencil, Trash2 } from "lucide-react";
import { Btn, Tag, Tooltip } from "@/ui";

const truncateId = (id: string): string => {
  if (!id) return "";
  if (id.length <= 12) return id;
  return `${id.slice(0, 6)}…${id.slice(-4)}`;
};

interface ColumnHandlers {
  onEdit: (session: Session) => void;
  onDelete: (chatId: string) => void;
  t: TFunction;
}

const toUTCTime = (ts: string | null | undefined): number => {
  if (!ts) return 0;
  const normalized =
    /[Z+-]\d{2}:?\d{2}$/.test(ts) || ts.endsWith("Z") ? ts : ts + "Z";
  return new Date(normalized).getTime();
};

export const createColumns = (
  handlers: ColumnHandlers,
): ColumnDef<Session>[] => {
  const { t } = handlers;

  return [
    {
      title: t("sessions.columnId"),
      dataIndex: "id",
      key: "id",
      width: 160,
      render: (id: string) => {
        if (!id) return "-";
        return (
          <Tooltip title={id} placement="top-start">
            <span
              className={styles.idCell}
              style={{ cursor: "pointer" }}
              onClick={() => navigator.clipboard.writeText(id)}
            >
              {truncateId(id)}
            </span>
          </Tooltip>
        );
      },
    },
    {
      title: t("sessions.columnName"),
      dataIndex: "name",
      key: "name",
      width: 200,
    },
    {
      title: t("sessions.columnUserId"),
      dataIndex: "user_id",
      key: "user_id",
      width: 150,
    },
    {
      title: t("sessions.columnChannel"),
      dataIndex: "channel",
      key: "channel",
      width: 120,
      render: (channel: string) => (
        <Tag color={mapChannelTagColor(CHANNEL_COLORS[channel])}>{channel}</Tag>
      ),
    },
    {
      title: t("sessions.columnCreatedAt"),
      dataIndex: "created_at",
      key: "created_at",
      width: 180,
      render: (timestamp: string | number | null) => formatTime(timestamp),
      sorter: (a: Session, b: Session) =>
        toUTCTime(a.created_at) - toUTCTime(b.created_at),
    },
    {
      title: t("sessions.columnUpdatedAt"),
      dataIndex: "updated_at",
      key: "updated_at",
      width: 180,
      render: (timestamp: string | number | null) => formatTime(timestamp),
      sorter: (a: Session, b: Session) =>
        toUTCTime(a.updated_at) - toUTCTime(b.updated_at),
      defaultSortOrder: "descend",
    },
    {
      title: t("sessions.columnAction"),
      key: "action",
      width: 120,
      align: "center",
      fixed: "right",
      render: (_: unknown, record: Session) => (
        <div className={styles.actionColumn}>
          <Tooltip title={t("common.edit")}>
            <Btn
              variant="outline"
              size="sm"
              icon={<Pencil />}
              onClick={() => handlers.onEdit(record)}
            />
          </Tooltip>
          <Tooltip title={t("common.delete")}>
            <Btn
              variant="danger"
              size="sm"
              icon={<Trash2 />}
              onClick={() => handlers.onDelete(record.id)}
            />
          </Tooltip>
        </div>
      ),
    },
  ];
};
