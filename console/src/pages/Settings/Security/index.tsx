/* eslint-disable react-refresh/only-export-components */
import { useState, useCallback, useRef, useEffect } from "react";
import { useLoaderData } from "react-router-dom";
import type { RuleFormValues } from "./components/RuleModal";
import { useTranslation } from "react-i18next";
import api from "../../../api";
import {
  useToolGuard,
  type MergedRule,
  type ToolGuardInitialData,
} from "./useToolGuard";
import {
  RuleTable,
  RuleModal,
  PreviewModal,
  SkillScannerSection,
  FileGuardSection,
  ApprovalModeSection,
} from "./components";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { RuaCard } from "@/components/RuaCard";
import Skeleton from "@/components/Skeleton";
import { useAgentStore } from "../../../stores/agentStore";
import styles from "./index.module.less";
import { Edit2, Plus, RefreshCw, Save, Trash2, Undo2 } from "lucide-react";
import {
  Btn,
  FormField,
  Modal,
  Tabs,
  TextInput,
  Tooltip,
  Toggle,
  toast,
} from "@/ui";
import type { ApprovalMode } from "../../../api/modules/security";
import type {
  ApprovalReuseMatcher,
  ApprovalRule,
  PermissionRuleEffect,
} from "../../../api/modules/console";
import { routeAgentId } from "@/routes/routeAgent";

type PermissionRuleDraft = {
  id?: string;
  effect: PermissionRuleEffect;
  enabled: boolean;
  label: string;
  matcherType: "tool_name" | "bash_command_pattern" | "bash_command_family";
  toolPattern: string;
  bashPattern: string;
  program: string;
  subcommand: string;
};

const emptyPermissionDraft = (
  effect: PermissionRuleEffect,
): PermissionRuleDraft => ({
  effect,
  enabled: true,
  label: "",
  matcherType: "tool_name",
  toolPattern: "",
  bashPattern: "",
  program: "git",
  subcommand: "status",
});

function matcherToText(matcher: ApprovalReuseMatcher): string {
  if (matcher.type === "bash_command_pattern") {
    return `Bash(${matcher.pattern})`;
  }
  if (matcher.type === "bash_command_family") {
    return `Bash(program=${matcher.program}, subcommand=${matcher.subcommand})`;
  }
  return matcher.pattern;
}

function draftFromRule(rule: ApprovalRule): PermissionRuleDraft {
  if (rule.matcher.type === "bash_command_pattern") {
    return {
      id: rule.id,
      effect: rule.effect,
      enabled: rule.enabled,
      label: rule.label,
      matcherType: "bash_command_pattern",
      toolPattern: "",
      bashPattern: rule.matcher.pattern,
      program: "git",
      subcommand: "status",
    };
  }
  if (rule.matcher.type === "bash_command_family") {
    return {
      id: rule.id,
      effect: rule.effect,
      enabled: rule.enabled,
      label: rule.label,
      matcherType: "bash_command_family",
      toolPattern: "",
      bashPattern: "",
      program: rule.matcher.program,
      subcommand: rule.matcher.subcommand,
    };
  }
  return {
    id: rule.id,
    effect: rule.effect,
    enabled: rule.enabled,
    label: rule.label,
    matcherType: "tool_name",
    toolPattern: rule.matcher.pattern,
    bashPattern: "",
    program: "git",
    subcommand: "status",
  };
}

function matcherFromDraft(draft: PermissionRuleDraft): ApprovalReuseMatcher {
  if (draft.matcherType === "bash_command_pattern") {
    return {
      type: "bash_command_pattern",
      pattern: draft.bashPattern.trim(),
    };
  }
  if (draft.matcherType === "bash_command_family") {
    return {
      type: "bash_command_family",
      program: draft.program.trim(),
      subcommand: draft.subcommand.trim(),
    };
  }
  return { type: "tool_name", pattern: draft.toolPattern.trim() };
}

interface SecurityPageProps {
  initialData?: ToolGuardInitialData;
}

export async function loader(): Promise<ToolGuardInitialData> {
  const agentId = routeAgentId();
  const [config, builtinRules] = await Promise.all([
    api.getAgentToolGuard(agentId),
    api.getAgentBuiltinRules(agentId),
  ]);
  return { agentId, config, builtinRules };
}

export function RouteComponent() {
  return <SecurityPage initialData={useLoaderData() as ToolGuardInitialData} />;
}

function SecurityPage({ initialData }: SecurityPageProps) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);
  const [approvalMode, setApprovalMode] = useState<ApprovalMode>("hands_off");
  const [approvalModeLoading, setApprovalModeLoading] = useState(false);
  const [approvalRules, setApprovalRules] = useState<ApprovalRule[]>([]);
  const [approvalRulesLoading, setApprovalRulesLoading] = useState(false);
  const [permissionRuleDraft, setPermissionRuleDraft] =
    useState<PermissionRuleDraft | null>(null);
  const [permissionRuleSaving, setPermissionRuleSaving] = useState(false);
  const [activeTab, setActiveTab] = useState("approvalMode");

  const currentAgentId = useAgentStore((s) => s.selectedAgent);

  const approvalModeDirtyRef = useRef(false);

  const [fileGuardHandlers, setFileGuardHandlers] = useState<{
    save: () => Promise<void>;
    reset: () => void;
    saving: boolean;
  } | null>(null);

  const onFileGuardHandlersReady = useCallback(
    (handlers: {
      save: () => Promise<void>;
      reset: () => void;
      saving: boolean;
    }) => {
      setFileGuardHandlers(handlers);
    },
    [],
  );

  const {
    config,
    customRules,
    builtinRules,
    mergedRules,
    loading,
    error,
    fetchAll,
    toggleRule,
    deleteCustomRule,
    addCustomRule,
    updateCustomRule,
    buildSaveBody,
    resetToBuiltin,
  } = useToolGuard(currentAgentId, initialData);

  useEffect(() => {
    if (!currentAgentId) return;
    let cancelled = false;
    setApprovalModeLoading(true);
    api
      .getAgentApprovalMode(currentAgentId)
      .then((cfg) => {
        if (!cancelled && !approvalModeDirtyRef.current) {
          setApprovalMode(
            cfg.approval_mode === "balanced"
              ? "hands_off"
              : cfg.approval_mode ?? "hands_off",
          );
        }
      })
      .catch((err) => {
        const errMsg =
          err instanceof Error
            ? err.message
            : t("security.approvalMode.saveFailed");
        toast.error(errMsg);
      })
      .finally(() => {
        if (!cancelled) setApprovalModeLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [currentAgentId, t]);

  useEffect(() => {
    if (!currentAgentId) return;
    let cancelled = false;
    setApprovalRulesLoading(true);
    api
      .listApprovalRules(currentAgentId)
      .then((rules) => {
        if (!cancelled) setApprovalRules(rules);
      })
      .catch((err) => {
        const errMsg =
          err instanceof Error
            ? err.message
            : t("security.permissionManagement.allowRulesLoadFailed");
        toast.error(errMsg);
      })
      .finally(() => {
        if (!cancelled) setApprovalRulesLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [currentAgentId, t]);

  const allowRules = approvalRules.filter((rule) => rule.effect === "allow");
  const denyRules = approvalRules.filter((rule) => rule.effect === "deny");

  const [editModal, setEditModal] = useState(false);
  const [editingRule, setEditingRule] = useState<MergedRule | null>(null);
  const [previewRule, setPreviewRule] = useState<MergedRule | null>(null);
  const [resetConfirmOpen, setResetConfirmOpen] = useState(false);
  const [resetting, setResetting] = useState(false);
  const persistCustomRules = useCallback(
    async (nextRules: typeof customRules) => {
      if (!currentAgentId) return;
      try {
        const body = { ...buildSaveBody(), custom_rules: nextRules };
        await api.updateAgentToolGuard(currentAgentId, body);
        toast.success(t("security.saveSuccess"));
      } catch (err) {
        const errMsg =
          err instanceof Error ? err.message : t("security.saveFailed");
        toast.error(errMsg);
      }
    },
    [buildSaveBody, t, currentAgentId],
  );

  const handleDeleteCustomRule = useCallback(
    (ruleId: string) => {
      deleteCustomRule(ruleId);
      const next = customRules.filter((r) => r.id !== ruleId);
      void persistCustomRules(next);
    },
    [deleteCustomRule, customRules, persistCustomRules],
  );

  const handleSave = useCallback(async () => {
    if (!currentAgentId) return;
    try {
      setSaving(true);
      await api.updateAgentToolGuard(currentAgentId, buildSaveBody());
      toast.success(t("security.saveSuccess"));
    } catch (err) {
      const errMsg =
        err instanceof Error ? err.message : t("security.saveFailed");
      toast.error(errMsg);
    } finally {
      setSaving(false);
    }
  }, [currentAgentId, buildSaveBody, t]);

  const handleSavePermissionManagement = useCallback(async () => {
    if (!currentAgentId) return;
    try {
      setApprovalModeLoading(true);
      await api.updateAgentApprovalMode(currentAgentId, {
        approval_mode: approvalMode,
      });
      approvalModeDirtyRef.current = false;
      toast.success(t("security.approvalMode.saveSuccess"));
    } catch (err) {
      const errMsg =
        err instanceof Error
          ? err.message
          : t("security.approvalMode.saveFailed");
      toast.error(errMsg);
    } finally {
      setApprovalModeLoading(false);
    }
  }, [approvalMode, currentAgentId, t]);

  const handleDeleteApprovalRule = useCallback(
    async (ruleId: string) => {
      if (!currentAgentId) return;
      await api.deleteApprovalRule(currentAgentId, ruleId);
      setApprovalRules((rules) => rules.filter((rule) => rule.id !== ruleId));
    },
    [currentAgentId],
  );

  const handleTogglePermissionRule = useCallback(
    async (rule: ApprovalRule) => {
      if (!currentAgentId) return;
      const updated = await api.updatePermissionRule(currentAgentId, rule.id, {
        effect: rule.effect,
        enabled: !rule.enabled,
        label: rule.label,
        matcher: rule.matcher,
      });
      setApprovalRules((rules) =>
        rules.map((item) => (item.id === updated.id ? updated : item)),
      );
    },
    [currentAgentId],
  );

  const handleSavePermissionRule = useCallback(async () => {
    if (!currentAgentId || !permissionRuleDraft) return;
    const matcher = matcherFromDraft(permissionRuleDraft);
    if (!permissionRuleDraft.label.trim()) {
      toast.error(t("security.permissionManagement.ruleLabelRequired"));
      return;
    }
    if (
      (matcher.type === "tool_name" && !matcher.pattern) ||
      (matcher.type === "bash_command_pattern" && !matcher.pattern) ||
      (matcher.type === "bash_command_family" &&
        (!matcher.program || !matcher.subcommand))
    ) {
      toast.error(t("security.permissionManagement.ruleMatcherRequired"));
      return;
    }
    setPermissionRuleSaving(true);
    try {
      const body = {
        effect: permissionRuleDraft.effect,
        enabled: permissionRuleDraft.enabled,
        label: permissionRuleDraft.label.trim(),
        matcher,
      };
      const saved = permissionRuleDraft.id
        ? await api.updatePermissionRule(
            currentAgentId,
            permissionRuleDraft.id,
            body,
          )
        : await api.createPermissionRule(currentAgentId, body);
      setApprovalRules((rules) => {
        if (permissionRuleDraft.id) {
          return rules.map((rule) => (rule.id === saved.id ? saved : rule));
        }
        return [...rules, saved];
      });
      setPermissionRuleDraft(null);
    } catch (err) {
      const errMsg =
        err instanceof Error ? err.message : t("security.saveFailed");
      toast.error(errMsg);
    } finally {
      setPermissionRuleSaving(false);
    }
  }, [currentAgentId, permissionRuleDraft, t]);

  const handleResetToBuiltin = useCallback(async () => {
    try {
      setResetting(true);
      await resetToBuiltin();
      toast.success(t("security.resetToBuiltinSuccess"));
    } catch (err) {
      const errMsg =
        err instanceof Error ? err.message : t("security.saveFailed");
      toast.error(errMsg);
    } finally {
      setResetting(false);
      setResetConfirmOpen(false);
    }
  }, [resetToBuiltin, t]);

  const openAddRule = useCallback(() => {
    setEditingRule(null);
    setEditModal(true);
  }, []);

  const openEditRule = useCallback((rule: MergedRule) => {
    setEditingRule(rule);
    setEditModal(true);
  }, []);

  const handleEditSave = useCallback(
    (values: RuleFormValues) => {
      const argMatchers = (values.arg_matchers ?? [])
        .map((m) => {
          const cleaned: Record<string, unknown> = {};
          if (m.command) cleaned.command = m.command;
          if (m.flag) cleaned.flag = m.flag;
          if (m.value_contains) cleaned.value_contains = m.value_contains;
          if (m.value_in?.length) cleaned.value_in = m.value_in;
          return cleaned;
        })
        .filter((m) => Object.keys(m).length > 0);

      const pathMatchers = (values.path_matchers ?? [])
        .map((m) => {
          const cleaned: Record<string, unknown> = {};
          if (m.contains?.length) cleaned.contains = m.contains;
          if (m.prefix?.length) cleaned.prefix = m.prefix;
          if (m.glob) cleaned.glob = m.glob;
          if (m.on_redirect) cleaned.on_redirect = m.on_redirect;
          return cleaned;
        })
        .filter((m) => Object.keys(m).length > 0);

      const commandSet: string[] = values.command_set ?? [];
      const lexical: string[] = values.lexical ?? [];
      const structural: string[] = values.structural ?? [];

      if (
        commandSet.length === 0 &&
        argMatchers.length === 0 &&
        pathMatchers.length === 0 &&
        lexical.length === 0 &&
        structural.length === 0
      ) {
        toast.error(t("security.rules.matcherRequired"));
        return;
      }

      const rule = {
        id: values.id,
        tools: values.tools ?? [],
        params: values.params ?? [],
        category: values.category || "",
        severity: values.severity,
        patterns: [] as string[],
        exclude_patterns: [] as string[],
        description: values.description || "",
        remediation: values.remediation || "",
        command_set: commandSet.length > 0 ? commandSet : undefined,
        arg_matchers: argMatchers.length > 0 ? argMatchers : undefined,
        path_matchers: pathMatchers.length > 0 ? pathMatchers : undefined,
        lexical: lexical.length > 0 ? lexical : undefined,
        structural: structural.length > 0 ? structural : undefined,
        action: values.action || undefined,
      };

      let nextRules = customRules;
      if (editingRule) {
        updateCustomRule(editingRule.id, rule);
        nextRules = customRules.map((r) =>
          r.id === editingRule.id ? rule : r,
        );
      } else {
        const allIds = [
          ...builtinRules.map((r) => r.id),
          ...customRules.map((r) => r.id),
        ];
        if (allIds.includes(rule.id)) {
          toast.error(t("security.rules.duplicateId"));
          return;
        }
        addCustomRule(rule);
        nextRules = [...customRules, rule];
      }
      setEditModal(false);
      void persistCustomRules(nextRules);
    },
    [
      editingRule,
      builtinRules,
      customRules,
      updateCustomRule,
      addCustomRule,
      persistCustomRules,
      t,
    ],
  );

  const renderPermissionRuleList = (
    rules: ApprovalRule[],
    effect: PermissionRuleEffect,
  ) => (
    <div className={styles.permissionRuleList}>
      <div className={styles.permissionRuleToolbar}>
        <Btn
          size="sm"
          icon={<Plus />}
          aria-label={t("security.permissionManagement.addRule")}
          title={t("security.permissionManagement.addRule")}
          onClick={() => setPermissionRuleDraft(emptyPermissionDraft(effect))}
        />
      </div>
      {approvalRulesLoading ? (
        <Skeleton variant="detail" count={1} />
      ) : rules.length === 0 ? (
        <span className={styles.emptyText}>
          {effect === "allow"
            ? t("security.permissionManagement.allowRulesEmpty")
            : t("security.permissionManagement.denyRulesEmpty")}
        </span>
      ) : (
        rules.map((rule) => (
          <div className={styles.permissionRuleItem} key={rule.id}>
            <div>
              <div className={styles.permissionRuleLabel}>{rule.label}</div>
              <div className={styles.permissionRuleMatcher}>
                {matcherToText(rule.matcher)}
              </div>
              <div className={styles.permissionRuleMeta}>
                {rule.source} ·{" "}
                {rule.enabled
                  ? t("security.permissionManagement.ruleEnabled")
                  : t("security.permissionManagement.ruleDisabled")}{" "}
                ·{" "}
                {t("security.permissionManagement.hitCount", {
                  count: rule.hit_count,
                })}
              </div>
            </div>
            <div className={styles.permissionRuleActions}>
              <Toggle
                checked={rule.enabled}
                onChange={() => void handleTogglePermissionRule(rule)}
                aria-label={t("security.permissionManagement.toggleRule")}
              />
              <Btn
                aria-label={t("common.edit")}
                icon={<Edit2 />}
                onClick={() => setPermissionRuleDraft(draftFromRule(rule))}
              />
              <Btn
                aria-label={t("common.delete")}
                icon={<Trash2 />}
                onClick={() => void handleDeleteApprovalRule(rule.id)}
              />
            </div>
          </div>
        ))
      )}
    </div>
  );

  if (loading && !config) {
    return (
      <Page className={styles.securityPage}>
        <Skeleton variant="detail" count={2} />
      </Page>
    );
  }

  if (error) {
    return (
      <Page className={styles.securityPage}>
        <div className={styles.centerState}>
          <span className={styles.stateTextError}>{error}</span>
          <Btn size="sm" onClick={fetchAll} style={{ marginTop: 12 }}>
            {t("environments.retry")}
          </Btn>
        </div>
      </Page>
    );
  }

  return (
    <Page className={styles.securityPage}>
      <PageHeader
        current={t("security.security")}
        onRefresh={fetchAll}
        moreMenuItems={[
          ...(activeTab === "toolGuard"
            ? [
                {
                  key: "resetRules",
                  label: t("security.resetToBuiltinTooltip"),
                  icon: <Undo2 size={14} />,
                  onClick: () => setResetConfirmOpen(true),
                },
              ]
            : []),
          ...(activeTab === "fileGuard" && fileGuardHandlers
            ? [
                {
                  key: "resetFileGuard",
                  label: t("common.reset"),
                  icon: <RefreshCw size={14} />,
                  onClick: () => fileGuardHandlers.reset(),
                },
              ]
            : []),
        ]}
        extra={
          <div style={{ display: "flex", gap: "var(--space-2)" }}>
            {(activeTab === "approvalMode" ||
              activeTab === "toolGuard" ||
              (activeTab === "fileGuard" && fileGuardHandlers)) && (
              <Tooltip title={t("common.save")}>
                <Btn
                  aria-label={t("common.save")}
                  onClick={
                    activeTab === "approvalMode"
                      ? handleSavePermissionManagement
                      : activeTab === "fileGuard"
                      ? fileGuardHandlers?.save
                      : handleSave
                  }
                  loading={
                    activeTab === "approvalMode"
                      ? approvalModeLoading
                      : activeTab === "fileGuard"
                      ? fileGuardHandlers?.saving
                      : saving
                  }
                  icon={<Save />}
                />
              </Tooltip>
            )}
          </div>
        }
      />

      <PageBody className={styles.pageBodyOverride}>
        <Tabs
          className={styles.mainTabs}
          activeKey={activeTab}
          onChange={setActiveTab}
          items={[
            {
              key: "approvalMode",
              label: (
                <span className={styles.tabLabel}>
                  {t("security.permissionManagement.title")}
                </span>
              ),
              children: (
                <div className={styles.tabContent}>
                  <div
                    style={{
                      textAlign: "center",
                      fontSize: 12,
                      color: "var(--text-3)",
                      padding: "4px 16px 0",
                      marginBottom: 16,
                    }}
                  >
                    {t("security.perAgentNotice")}
                  </div>
                  <div className={styles.sectionConfigureContainer}>
                    <p className={styles.tabDescription}>
                      {t("security.permissionManagement.description")}
                    </p>
                    <RuaCard className={styles.formCard}>
                      <ApprovalModeSection
                        value={approvalMode}
                        onChange={(mode) => {
                          approvalModeDirtyRef.current = true;
                          setApprovalMode(mode);
                        }}
                      />
                    </RuaCard>
                    <RuaCard className={styles.formCard}>
                      <div className={styles.permissionCardContent}>
                        <div>
                          <h2 className={styles.sectionTitle}>
                            {t("security.permissionManagement.allowRulesTitle")}
                          </h2>
                          <p className={styles.permissionRuleDescription}>
                            {t(
                              "security.permissionManagement.allowRulesDescription",
                            )}
                          </p>
                        </div>
                        {renderPermissionRuleList(allowRules, "allow")}
                      </div>
                    </RuaCard>
                    <RuaCard className={styles.formCard}>
                      <div className={styles.permissionCardContent}>
                        <div>
                          <h2 className={styles.sectionTitle}>
                            {t("security.permissionManagement.denyRulesTitle")}
                          </h2>
                          <p className={styles.permissionRuleDescription}>
                            {t(
                              "security.permissionManagement.denyRulesDescription",
                            )}
                          </p>
                        </div>
                        {renderPermissionRuleList(denyRules, "deny")}
                        <div className={styles.permissionPrecedence}>
                          {t("security.permissionManagement.precedence")}
                        </div>
                      </div>
                    </RuaCard>
                  </div>
                </div>
              ),
            },
            {
              key: "toolGuard",
              label: (
                <span className={styles.tabLabel}>
                  {t("security.safetyRules.title")}
                </span>
              ),
              children: (
                <div className={styles.tabContent}>
                  <div
                    style={{
                      textAlign: "center",
                      fontSize: 12,
                      color: "var(--text-3)",
                      padding: "4px 16px 0",
                      marginBottom: 16,
                    }}
                  >
                    {t("security.perAgentNotice")}
                  </div>
                  <div className={styles.sectionConfigureContainer}>
                    <p className={styles.tabDescription}>
                      {t("security.safetyRules.description")}
                    </p>
                  </div>

                  <div className={styles.sectionContainer}>
                    <div className={styles.sectionHeader}>
                      <h2 className={styles.sectionTitle}>
                        {t("security.rules.title")}
                      </h2>
                    </div>
                    <p
                      style={{
                        color: "var(--text-secondary)",
                        fontSize: 13,
                        margin: "0 0 12px",
                        paddingLeft: 16,
                      }}
                    >
                      {t("security.rules.modeHint")}
                    </p>

                    <RuaCard className={styles.tableCard} bare>
                      <RuleTable
                        rules={mergedRules}
                        enabled
                        onToggleRule={toggleRule}
                        onPreviewRule={setPreviewRule}
                        onEditRule={openEditRule}
                        onDeleteRule={handleDeleteCustomRule}
                        onAddRule={openAddRule}
                      />
                    </RuaCard>
                  </div>
                </div>
              ),
            },
            {
              key: "fileGuard",
              label: (
                <span className={styles.tabLabel}>
                  {t("security.fileGuard.title")}
                </span>
              ),
              children: (
                <div className={styles.tabContent}>
                  <div
                    style={{
                      textAlign: "center",
                      fontSize: 12,
                      color: "var(--text-3)",
                      padding: "4px 16px 0",
                      marginBottom: 16,
                    }}
                  >
                    {t("security.processWideNotice")}
                  </div>
                  <div className={styles.sectionFileGuardContainer}>
                    <p className={styles.tabDescription}>
                      {t("security.fileGuard.description")}
                    </p>
                    <FileGuardSection onSave={onFileGuardHandlersReady} />
                  </div>
                </div>
              ),
            },
            {
              key: "skillScanner",
              label: (
                <span className={styles.tabLabel}>
                  {t("security.skillScanner.title")}
                </span>
              ),
              children: (
                <div
                  className={`${styles.tabContent} ${styles.skillScannerTabContent}`}
                >
                  <div
                    style={{
                      textAlign: "center",
                      fontSize: 12,
                      color: "var(--text-3)",
                      padding: "4px 16px 0",
                      marginBottom: 16,
                    }}
                  >
                    {t("security.processWideNotice")}
                  </div>
                  <div className={styles.sectionSkillScannerContainer}>
                    <p className={styles.tabDescription}>
                      {t("security.skillScanner.description")}
                    </p>
                    <SkillScannerSection />
                  </div>
                </div>
              ),
            },
          ]}
        />
      </PageBody>

      <RuleModal
        open={editModal}
        editingRule={editingRule}
        existingRuleIds={[
          ...builtinRules.map((r) => r.id),
          ...customRules.map((r) => r.id),
        ]}
        onSubmit={handleEditSave}
        onClose={() => setEditModal(false)}
      />

      <PreviewModal rule={previewRule} onClose={() => setPreviewRule(null)} />

      <Modal
        open={!!permissionRuleDraft}
        title={
          permissionRuleDraft?.id
            ? t("security.permissionManagement.editRule")
            : t("security.permissionManagement.addRule")
        }
        onOk={handleSavePermissionRule}
        onClose={() => setPermissionRuleDraft(null)}
        confirmLoading={permissionRuleSaving}
        okText={t("common.save")}
        cancelText={t("common.cancel")}
      >
        {permissionRuleDraft && (
          <div className={styles.permissionRuleForm}>
            <FormField label={t("security.permissionManagement.ruleLabel")}>
              <TextInput
                aria-label={t("security.permissionManagement.ruleLabel")}
                value={permissionRuleDraft.label}
                onChange={(e) =>
                  setPermissionRuleDraft({
                    ...permissionRuleDraft,
                    label: e.target.value,
                  })
                }
              />
            </FormField>
            <FormField label={t("security.permissionManagement.ruleEffect")}>
              <select
                aria-label={t("security.permissionManagement.ruleEffect")}
                className={styles.permissionRuleNativeSelect}
                value={permissionRuleDraft.effect}
                onChange={(e) =>
                  setPermissionRuleDraft({
                    ...permissionRuleDraft,
                    effect: e.target.value as PermissionRuleEffect,
                  })
                }
              >
                <option value="allow">
                  {t("security.permissionManagement.allowRulesTitle")}
                </option>
                <option value="deny">
                  {t("security.permissionManagement.denyRulesTitle")}
                </option>
              </select>
            </FormField>
            <FormField label={t("security.permissionManagement.ruleMatcher")}>
              <select
                aria-label={t("security.permissionManagement.ruleMatcher")}
                className={styles.permissionRuleNativeSelect}
                value={permissionRuleDraft.matcherType}
                onChange={(e) =>
                  setPermissionRuleDraft({
                    ...permissionRuleDraft,
                    matcherType: e.target
                      .value as PermissionRuleDraft["matcherType"],
                  })
                }
              >
                <option value="tool_name">ToolName</option>
                <option value="bash_command_pattern">BashCommandPattern</option>
                <option value="bash_command_family">BashCommandFamily</option>
              </select>
            </FormField>
            {permissionRuleDraft.matcherType === "tool_name" ? (
              <FormField label={t("security.permissionManagement.toolPattern")}>
                <TextInput
                  aria-label={t("security.permissionManagement.toolPattern")}
                  value={permissionRuleDraft.toolPattern}
                  placeholder="WebSearch"
                  onChange={(e) =>
                    setPermissionRuleDraft({
                      ...permissionRuleDraft,
                      toolPattern: e.target.value,
                    })
                  }
                />
              </FormField>
            ) : permissionRuleDraft.matcherType === "bash_command_pattern" ? (
              <FormField label="Bash pattern">
                <TextInput
                  aria-label="Bash pattern"
                  value={permissionRuleDraft.bashPattern}
                  placeholder="du -sh *"
                  onChange={(e) =>
                    setPermissionRuleDraft({
                      ...permissionRuleDraft,
                      bashPattern: e.target.value,
                    })
                  }
                />
              </FormField>
            ) : (
              <div className={styles.permissionRuleFormGrid}>
                <FormField label="program">
                  <TextInput
                    aria-label="program"
                    value={permissionRuleDraft.program}
                    onChange={(e) =>
                      setPermissionRuleDraft({
                        ...permissionRuleDraft,
                        program: e.target.value,
                      })
                    }
                  />
                </FormField>
                <FormField label="subcommand">
                  <TextInput
                    aria-label="subcommand"
                    value={permissionRuleDraft.subcommand}
                    onChange={(e) =>
                      setPermissionRuleDraft({
                        ...permissionRuleDraft,
                        subcommand: e.target.value,
                      })
                    }
                  />
                </FormField>
              </div>
            )}
            <label className={styles.permissionRuleEnabledRow}>
              <Toggle
                checked={permissionRuleDraft.enabled}
                onChange={(e) =>
                  setPermissionRuleDraft({
                    ...permissionRuleDraft,
                    enabled: e.target.checked,
                  })
                }
              />
              <span>{t("security.permissionManagement.ruleEnabled")}</span>
            </label>
          </div>
        )}
      </Modal>

      <Modal
        open={resetConfirmOpen}
        title={t("security.resetToBuiltinTitle")}
        onOk={handleResetToBuiltin}
        onClose={() => setResetConfirmOpen(false)}
        confirmLoading={resetting}
        okText={t("security.resetToBuiltinConfirm")}
        cancelText={t("common.cancel")}
      >
        <p>{t("security.resetToBuiltinDescription")}</p>
      </Modal>
    </Page>
  );
}

export default SecurityPage;
