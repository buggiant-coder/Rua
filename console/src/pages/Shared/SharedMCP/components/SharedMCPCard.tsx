import type { MCPClientInfo } from "../../../../api/types";
import { useTranslation } from "react-i18next";
import { useState } from "react";
import { useTheme } from "../../../../contexts/ThemeContext";
import { RuaCard } from "@/components/RuaCard";
import styles from "../index.module.less";
import { Btn, Check, Modal, TextArea, Tooltip } from "@/ui";
interface SharedMCPCardProps {
  client: MCPClientInfo;
  onDelete: (client: MCPClientInfo) => void;
  onUpdate: (key: string, updates: Partial<MCPClientInfo>) => Promise<boolean>;
  isHovered: boolean;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  selectable?: boolean;
  selected?: boolean;
  onSelect?: (client: MCPClientInfo) => void;
}

export function SharedMCPCard({
  client,
  onDelete,
  onUpdate,
  onMouseEnter,
  onMouseLeave,
  selectable,
  selected,
  onSelect,
}: SharedMCPCardProps) {
  const { t } = useTranslation();
  const { isDark } = useTheme();
  const [jsonModalOpen, setJsonModalOpen] = useState(false);
  const [deleteModalOpen, setDeleteModalOpen] = useState(false);
  const [editedJson, setEditedJson] = useState("");
  const [isEditing, setIsEditing] = useState(false);
  const [enteredFromEdit, setEnteredFromEdit] = useState(false);

  const isRemote =
    client.transport === "streamable_http" || client.transport === "sse";
  const clientType = isRemote ? "Remote" : "Local";

  const handleCardClick = () => {
    if (selectable && onSelect) {
      onSelect(client);
      return;
    }
    const jsonStr = JSON.stringify(client, null, 2);
    setEditedJson(jsonStr);
    setIsEditing(false);
    setEnteredFromEdit(false);
    setJsonModalOpen(true);
  };

  const handleSaveJson = async () => {
    try {
      const parsed = JSON.parse(editedJson);
      const { key: _key, ...updates } = parsed;
      const success = await onUpdate(client.key, updates);
      if (success) {
        setJsonModalOpen(false);
        setIsEditing(false);
      }
    } catch {
      alert("Invalid JSON format");
    }
  };

  const clientJson = JSON.stringify(client, null, 2);

  const titleNode = (
    <Tooltip title={client.name}>
      <span className={styles.mcpTitle}>{client.name}</span>
    </Tooltip>
  );

  const tagNodes = [
    <span
      key="type"
      className={`${styles.typeBadge} ${
        isRemote ? styles.remote : styles.local
      }`}
    >
      {clientType}
    </span>,
  ];

  const actionNodes = selectable
    ? [
        <Check
          key="check"
          checked={!!selected}
          onChange={() => onSelect?.(client)}
          onClick={(e) => e.stopPropagation()}
        />,
      ]
    : undefined;

  return (
    <>
      <RuaCard
        hoverable
        selected={!!selected}
        onClick={handleCardClick}
        onMouseEnter={onMouseEnter}
        onMouseLeave={onMouseLeave}
        className={styles.mcpCard}
        title={titleNode}
        tags={tagNodes}
        actions={actionNodes}
        footer={
          !selectable ? (
            <>
              <Btn
                variant="outline"
                onClick={(e) => {
                  e.stopPropagation();
                  const jsonStr = JSON.stringify(client, null, 2);
                  setEditedJson(jsonStr);
                  setIsEditing(true);
                  setEnteredFromEdit(true);
                  setJsonModalOpen(true);
                }}
              >
                {t("common.edit")}
              </Btn>
              <Btn
                variant="danger"
                onClick={(e) => {
                  e.stopPropagation();
                  setDeleteModalOpen(true);
                }}
              >
                {t("common.delete")}
              </Btn>
            </>
          ) : undefined
        }
      >
        <p className={styles.mcpDescription}>{client.description || "-"}</p>
      </RuaCard>

      <Modal
        title={t("common.confirm")}
        open={deleteModalOpen}
        onClose={() => setDeleteModalOpen(false)}
        footer={
          <>
            <Btn variant="outline" onClick={() => setDeleteModalOpen(false)}>
              {t("common.cancel")}
            </Btn>
            <Btn
              variant="danger"
              onClick={() => {
                setDeleteModalOpen(false);
                onDelete(client);
              }}
            >
              {t("common.confirm")}
            </Btn>
          </>
        }
      >
        <p>{t("sharedMcp.deleteConfirm")}</p>
      </Modal>

      <Modal
        title={`${client.name} - Configuration`}
        open={jsonModalOpen}
        onClose={() => setJsonModalOpen(false)}
        footer={
          <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
            <Btn
              variant="outline"
              onClick={() => {
                if (isEditing && !enteredFromEdit) {
                  const jsonStr = JSON.stringify(client, null, 2);
                  setEditedJson(jsonStr);
                  setIsEditing(false);
                } else {
                  setJsonModalOpen(false);
                  setIsEditing(false);
                }
              }}
            >
              {isEditing ? t("common.cancel") : t("common.close")}
            </Btn>
            {isEditing ? (
              <Btn onClick={handleSaveJson}>{t("common.save")}</Btn>
            ) : (
              <Btn variant="outline" onClick={() => setIsEditing(true)}>
                {t("common.edit")}
              </Btn>
            )}
          </div>
        }
        width={700}
      >
        <div className={styles.maskedFieldHint}>
          {t("sharedMcp.maskedFieldHint")}
        </div>
        {isEditing ? (
          <TextArea
            value={editedJson}
            onChange={(e) => setEditedJson(e.target.value)}
            rows={20}
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: 13,
              lineHeight: 1.6,
              padding: 16,
              borderRadius: 8,
            }}
          />
        ) : (
          <pre
            style={{
              backgroundColor: isDark ? "#1f1f1f" : "#f5f5f5",
              color: isDark ? "rgba(255,255,255,0.85)" : "rgba(0,0,0,0.88)",
              padding: 16,
              borderRadius: 8,
              border: `1px solid ${isDark ? "#1f1f1f" : "#f5f5f5"}`,
              maxHeight: 500,
              overflow: "auto",
              fontFamily: "var(--font-mono)",
              fontSize: 13,
              lineHeight: 1.6,
            }}
          >
            {clientJson}
          </pre>
        )}
      </Modal>
    </>
  );
}
