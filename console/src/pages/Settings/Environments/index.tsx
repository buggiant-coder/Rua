/* eslint-disable react-refresh/only-export-components */
import { useState, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { useLoaderData } from "react-router-dom";

import api from "../../../api";
import { useEnvVars, type EnvVarsInitialData } from "./useEnvVars";
import { EnvDrawer } from "./EnvDrawer";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { EmptyState } from "@/components/EmptyState";
import type { EnvVar } from "../../../api/types";
import styles from "./index.module.less";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { Btn, Modal, Table, Tooltip, toast } from "@/ui";
import type { ColumnDef } from "@/ui/Table";

interface EnvironmentsPageProps {
  initialData?: EnvVarsInitialData;
}

export async function loader(): Promise<EnvVarsInitialData> {
  return { envVars: (await api.listEnvs()) ?? [] };
}

export function RouteComponent() {
  return (
    <EnvironmentsPage initialData={useLoaderData() as EnvVarsInitialData} />
  );
}

export default function EnvironmentsPage({
  initialData,
}: EnvironmentsPageProps) {
  const { t } = useTranslation();
  const { envVars, loading, fetchAll } = useEnvVars(initialData);

  const [drawerOpen, setDrawerOpen] = useState(false);
  const [editingEnv, setEditingEnv] = useState<EnvVar | null>(null);
  const [saving, setSaving] = useState(false);

  const handleCreate = () => {
    setEditingEnv(null);
    setDrawerOpen(true);
  };

  const handleEdit = (env: EnvVar) => {
    setEditingEnv(env);
    setDrawerOpen(true);
  };

  const handleDelete = (env: EnvVar) => {
    Modal.confirm({
      title: t("environments.deleteVariable"),
      content: t("environments.deleteConfirm", { name: env.key }),
      okText: t("common.delete"),
      danger: true,
      cancelText: t("common.cancel"),
      onOk: async () => {
        try {
          await api.deleteEnv(env.key);
          toast.success(t("environments.deleteSuccess", { name: env.key }));
          fetchAll();
        } catch (err) {
          const errMsg =
            err instanceof Error ? err.message : t("environments.deleteFailed");
          toast.error(errMsg);
        }
      },
    });
  };

  const handleSubmit = useCallback(
    async ({ key, value }: { key: string; value: string }) => {
      const dict: Record<string, string> = {};
      for (const e of envVars) {
        if (editingEnv && e.key === editingEnv.key) continue;
        dict[e.key] = e.value ?? "";
      }
      dict[key] = value ?? "";
      setSaving(true);
      try {
        await api.saveEnvs(dict);
        toast.success(
          editingEnv
            ? t("environments.updateSuccess", { name: key })
            : t("environments.createSuccess", { name: key }),
        );
        setDrawerOpen(false);
        fetchAll();
      } catch (err) {
        const errMsg =
          err instanceof Error ? err.message : t("environments.saveFailed");
        toast.error(errMsg);
      } finally {
        setSaving(false);
      }
    },
    [envVars, editingEnv, fetchAll, t],
  );

  const columns: ColumnDef<EnvVar>[] = [
    {
      title: t("environments.key"),
      dataIndex: "key",
      key: "key",
      width: 260,
      render: (k: string) => (
        <span
          className={styles.idCell}
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: 12,
            cursor: "pointer",
          }}
          onClick={() => navigator.clipboard.writeText(k)}
        >
          <span className={styles.idCellText}>{k}</span>
        </span>
      ),
    },
    {
      title: t("environments.value"),
      dataIndex: "value",
      key: "value",
      ellipsis: true,
      render: (v: string) => (
        <span
          className={styles.valueCell}
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: 12,
          }}
        >
          {v}
        </span>
      ),
    },
    {
      title: t("common.actions"),
      key: "actions",
      width: 110,
      align: "right" as const,
      render: (_: unknown, record: EnvVar) => (
        <div
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: "var(--space-1)",
          }}
        >
          <Tooltip title={t("common.edit")}>
            <Btn
              aria-label={t("common.edit")}
              variant="outline"
              size="sm"
              icon={<Pencil />}
              onClick={() => handleEdit(record)}
            />
          </Tooltip>
          <Tooltip title={t("common.delete")}>
            <Btn
              aria-label={t("common.delete")}
              variant="danger"
              size="sm"
              icon={<Trash2 />}
              onClick={() => handleDelete(record)}
            />
          </Tooltip>
        </div>
      ),
    },
  ];

  return (
    <Page className={styles.environmentsPage}>
      <PageHeader
        items={[{ title: t("environments.environments") }]}
        onRefresh={fetchAll}
        refreshing={loading}
        extra={
          <div style={{ display: "flex", gap: "var(--space-2)" }}>
            <Tooltip title={t("environments.createVariable")}>
              <Btn
                aria-label={t("environments.createVariable")}
                onClick={handleCreate}
                icon={<Plus />}
              />
            </Tooltip>
          </div>
        }
      />

      <PageBody>
        <Table
          dataSource={envVars}
          columns={columns}
          rowKey="key"
          pagination={false}
          size="small"
          loading={loading}
          className={styles.envTable}
          responsive="stack"
          emptyText={<EmptyState description={t("environments.noVariables")} />}
        />
      </PageBody>

      <EnvDrawer
        open={drawerOpen}
        editingEnv={editingEnv}
        existingKeys={envVars.map((e) => e.key)}
        saving={saving}
        onClose={() => setDrawerOpen(false)}
        onSubmit={handleSubmit}
      />
    </Page>
  );
}
