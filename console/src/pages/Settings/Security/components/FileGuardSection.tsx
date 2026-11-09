import { useState, useEffect, useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import api from "../../../../api";
import { isMobile } from "../../../../utils/isMobile";
import { RuaCard } from "@/components/RuaCard";
import { EmptyState } from "@/components/EmptyState";
import { InputWithExample } from "@/components/InputWithExample";
import styles from "../index.module.less";
import { File, Folder, PlusCircle, Trash2 } from "lucide-react";
import { Btn, Popconfirm, Table, Tag, Toggle, toast } from "@/ui";
interface FileGuardSectionProps {
  onSave?: (handlers: {
    save: () => Promise<void>;
    reset: () => void;
    saving: boolean;
  }) => void;
}

interface TableRow {
  key: string;
  path: string;
  builtin: boolean;
  enabled: boolean;
}

export function FileGuardSection({ onSave }: FileGuardSectionProps = {}) {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(true);
  const [paths, setPaths] = useState<string[]>([]);
  const [builtinPaths, setBuiltinPaths] = useState<string[]>([]);
  const [disabledPatterns, setDisabledPatterns] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [newPath, setNewPath] = useState("");
  const isMobileMode = useMemo(() => isMobile(), []);

  const fetchData = useCallback(async () => {
    try {
      setLoading(true);
      const data = await api.getFileGuard();
      setEnabled(data?.enabled ?? true);
      setPaths(data?.paths ?? []);
      setBuiltinPaths(data?.builtin_paths ?? []);
      setDisabledPatterns(data?.disabled_patterns ?? []);
    } catch {
      toast.error(t("security.fileGuard.loadFailed"));
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const handleToggle = useCallback(
    async (checked: boolean) => {
      setEnabled(checked);
      try {
        await api.updateFileGuard({ enabled: checked });
        toast.success(t("security.fileGuard.saveSuccess"));
      } catch {
        setEnabled(!checked);
        toast.error(t("security.fileGuard.saveFailed"));
      }
    },
    [t],
  );

  const handleAdd = useCallback(async () => {
    const trimmed = newPath.trim();
    if (!trimmed) return;
    if (paths.includes(trimmed)) {
      toast.warning(t("security.fileGuard.duplicate"));
      return;
    }
    const newPaths = [...paths, trimmed];
    setPaths(newPaths);
    setNewPath("");
    try {
      await api.updateFileGuard({
        enabled,
        paths: newPaths,
        disabled_patterns: disabledPatterns,
      });
      toast.success(t("security.fileGuard.saveSuccess"));
    } catch {
      setPaths(paths);
      setNewPath(trimmed);
      toast.error(t("security.fileGuard.saveFailed"));
    }
  }, [newPath, paths, enabled, disabledPatterns, t]);

  const handlePathToggle = useCallback(
    async (path: string, checked: boolean) => {
      const newDisabled = checked
        ? disabledPatterns.filter((p) => p !== path)
        : [...disabledPatterns, path];
      setDisabledPatterns(newDisabled);
      try {
        await api.updateFileGuard({
          enabled,
          paths,
          disabled_patterns: newDisabled,
        });
        toast.success(t("security.fileGuard.saveSuccess"));
      } catch {
        setDisabledPatterns(disabledPatterns);
        toast.error(t("security.fileGuard.saveFailed"));
      }
    },
    [disabledPatterns, enabled, paths, t],
  );

  const handleRemove = useCallback(
    async (path: string) => {
      const newPaths = paths.filter((p) => p !== path);
      setPaths(newPaths);
      try {
        await api.updateFileGuard({
          enabled,
          paths: newPaths,
          disabled_patterns: disabledPatterns,
        });
        toast.success(t("security.fileGuard.saveSuccess"));
      } catch {
        setPaths(paths);
        toast.error(t("security.fileGuard.saveFailed"));
      }
    },
    [paths, enabled, disabledPatterns, t],
  );

  const handleSave = useCallback(async () => {
    try {
      setSaving(true);
      await api.updateFileGuard({
        enabled,
        paths,
        disabled_patterns: disabledPatterns,
      });
      toast.success(t("security.fileGuard.saveSuccess"));
    } catch {
      toast.error(t("security.fileGuard.saveFailed"));
    } finally {
      setSaving(false);
    }
  }, [enabled, paths, disabledPatterns, t]);

  const handleReset = useCallback(() => {
    fetchData();
  }, [fetchData]);

  useEffect(() => {
    onSave?.({ save: handleSave, reset: handleReset, saving });
  }, [handleSave, handleReset, saving, onSave]);

  const dataSource: TableRow[] = useMemo(() => {
    const builtinRows: TableRow[] = builtinPaths.map((path) => ({
      key: `builtin:${path}`,
      path,
      builtin: true,
      enabled: !disabledPatterns.includes(path),
    }));
    const customRows: TableRow[] = paths.map((path) => ({
      key: `custom:${path}`,
      path,
      builtin: false,
      enabled: !disabledPatterns.includes(path),
    }));
    return [...builtinRows, ...customRows];
  }, [builtinPaths, paths, disabledPatterns]);

  const columns = [
    {
      title: t("security.fileGuard.path"),
      dataIndex: "path",
      key: "path",
      stackLine: 1,
      render: (_: string, record: TableRow) => {
        const isDir =
          record.path.endsWith("/") ||
          record.path.endsWith("\\") ||
          record.path.endsWith("**");
        return (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              flexWrap: "wrap",
              gap: "var(--space-2)",
              minWidth: 0,
            }}
          >
            {isDir ? (
              <Folder style={{ color: "var(--warning-fg)" }} />
            ) : (
              <File style={{ color: "var(--primary)" }} />
            )}
            <code style={{ overflowWrap: "anywhere" }}>{record.path}</code>
            {isDir && !record.path.includes("*") && (
              <Tag color="warning">{t("security.fileGuard.directory")}</Tag>
            )}
          </div>
        );
      },
    },
    {
      title: t("security.fileGuard.type"),
      key: "type",
      width: 100,
      stackLine: 1,
      render: (_: unknown, record: TableRow) =>
        record.builtin ? (
          <Tag color="primary">{t("security.fileGuard.builtin")}</Tag>
        ) : (
          <Tag color="success">{t("security.fileGuard.custom")}</Tag>
        ),
    },
    {
      title: t("security.fileGuard.actions"),
      key: "actions",
      width: 120,
      stackLine: 1,
      render: (_: unknown, record: TableRow) => (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            flexWrap: "wrap",
            gap: "var(--space-2)",
          }}
        >
          <Toggle
            checked={record.enabled}
            onChange={(e) => handlePathToggle(record.path, e.target.checked)}
          />
          {!record.builtin && (
            <Popconfirm
              title={t("security.fileGuard.removeConfirm")}
              onConfirm={() => handleRemove(record.path)}
              okText={t("common.delete")}
              cancelText={t("common.cancel")}
            >
              <Btn variant="danger" icon={<Trash2 />} size="sm" />
            </Popconfirm>
          )}
        </div>
      ),
    },
  ];

  return (
    <>
      <RuaCard className={styles.formCard}>
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            marginBottom: 16,
          }}
        >
          <span style={{ fontWeight: 500 }}>
            {t("security.fileGuard.enableLabel")}
          </span>
          <Toggle
            checked={enabled}
            onChange={(e) => handleToggle(e.target.checked)}
          />
        </div>

        <div
          style={{ display: "flex", gap: 8 }}
          className={styles.fileGuardInputRow}
        >
          <InputWithExample
            style={{ flex: 1 }}
            value={newPath}
            onChange={(e) => setNewPath(e.target.value)}
            placeholder={t("security.fileGuard.inputPlaceholder")}
            example="~/.ssh/"
            onPressEnter={handleAdd}
            disabled={!enabled}
          />
          <Btn
            icon={<PlusCircle />}
            onClick={handleAdd}
            disabled={!newPath.trim() || !enabled}
          >
            {t("security.fileGuard.add")}
          </Btn>
        </div>
      </RuaCard>

      <RuaCard className={styles.tableCard} bare>
        {isMobileMode ? (
          dataSource.length === 0 ? (
            <div className={styles.listEmpty}>
              <EmptyState description={t("security.fileGuard.empty")} />
            </div>
          ) : (
            <div className={styles.list} role="list">
              {dataSource.map((item) => {
                const isDir =
                  item.path.endsWith("/") ||
                  item.path.endsWith("\\") ||
                  item.path.endsWith("**");
                return (
                  <div
                    key={item.key}
                    role="listitem"
                    className={styles.listItem}
                  >
                    <div className={styles.listItemActions}>
                      <Toggle
                        checked={item.enabled}
                        onChange={(e) =>
                          handlePathToggle(item.path, e.target.checked)
                        }
                      />
                      {!item.builtin && (
                        <Popconfirm
                          title={t("security.fileGuard.removeConfirm")}
                          onConfirm={() => handleRemove(item.path)}
                          okText={t("common.delete")}
                          cancelText={t("common.cancel")}
                        >
                          <Btn variant="danger" icon={<Trash2 />} />
                        </Popconfirm>
                      )}
                    </div>

                    <div className={styles.listItemHeader}>
                      {isDir ? (
                        <Folder style={{ color: "var(--warning-fg)" }} />
                      ) : (
                        <File style={{ color: "var(--primary)" }} />
                      )}
                      <code className={styles.listItemCode}>{item.path}</code>
                      {item.builtin ? (
                        <Tag color="primary" style={{ margin: 0 }}>
                          {t("security.fileGuard.builtin")}
                        </Tag>
                      ) : (
                        <Tag color="success" style={{ margin: 0 }}>
                          {t("security.fileGuard.custom")}
                        </Tag>
                      )}
                      {isDir && !item.path.includes("*") && (
                        <Tag color="warning" style={{ margin: 0 }}>
                          {t("security.fileGuard.directory")}
                        </Tag>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )
        ) : (
          <Table
            columns={columns}
            dataSource={dataSource}
            loading={loading}
            pagination={false}
            responsive="stack"
            size="middle"
            locale={{
              emptyText: (
                <EmptyState description={t("security.fileGuard.empty")} />
              ),
            }}
          />
        )}
      </RuaCard>
    </>
  );
}
