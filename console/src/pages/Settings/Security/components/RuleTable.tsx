import { useMemo, useState, useCallback } from "react";
import type { TagProps } from "@/ui/Tag";
import {
  ArrowUpNarrowWide,
  ChevronDown,
  ChevronRight,
  Pencil,
  Plus,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { MergedRule } from "../useToolGuard";
import { useTheme } from "../../../../contexts/ThemeContext";
import { isMobile } from "../../../../utils/isMobile";
import { EmptyState } from "@/components/EmptyState";
import styles from "../index.module.less";
import { Btn, Dropdown, Table, Tag, TextInput, Toggle } from "@/ui";
type SortField = "id" | "enabled";
type SortOrder = "asc" | "desc";

const SORT_FIELD_KEY = "rua.security.rules.sortField";
const SORT_ORDER_KEY = "rua.security.rules.sortOrder";
const VALID_SORT_FIELDS: ReadonlyArray<SortField> = ["id", "enabled"];

const CATEGORY_ORDER = [
  "command_injection",
  "resource_abuse",
  "code_execution",
  "network_abuse",
  "sensitive_file_access",
  "credential_exposure",
  "data_exfiltration",
  "path_traversal",
  "command_obfuscation",
];

const loadSortField = (): SortField => {
  try {
    const v = localStorage.getItem(SORT_FIELD_KEY);
    if (v && (VALID_SORT_FIELDS as ReadonlyArray<string>).includes(v)) {
      return v as SortField;
    }
  } catch {}
  return "id";
};

const loadSortOrder = (): SortOrder => {
  try {
    const v = localStorage.getItem(SORT_ORDER_KEY);
    if (v === "asc" || v === "desc") return v;
  } catch {}
  return "asc";
};

const SEVERITY_COLORS: Record<string, TagProps["color"]> = {
  CRITICAL: "danger",
  HIGH: "warning",
  MEDIUM: "warning",
  LOW: "primary",
  INFO: "default",
};

interface CategoryGroup {
  category: string;
  rules: MergedRule[];
  enabledCount: number;
  totalCount: number;
}

interface RuleTableProps {
  rules: MergedRule[];
  enabled: boolean;
  onToggleRule: (ruleId: string, currentlyDisabled: boolean) => void;
  onPreviewRule: (rule: MergedRule) => void;
  onEditRule: (rule: MergedRule) => void;
  onDeleteRule: (ruleId: string) => void;
  onAddRule?: () => void;
  addRuleDisabled?: boolean;
}

export function RuleTable({
  rules,
  enabled,
  onToggleRule,
  onPreviewRule,
  onEditRule,
  onDeleteRule,
  onAddRule,
  addRuleDisabled = false,
}: RuleTableProps) {
  const { t } = useTranslation();
  const { isDark } = useTheme();
  const darkBtnStyle = isDark ? { color: "var(--text-2)" } : undefined;
  const isMobileMode = useMemo(() => isMobile(), []);

  const [searchTerm, setSearchTerm] = useState<string>("");
  const [sortField, setSortFieldState] = useState<SortField>(loadSortField);
  const [sortOrder, setSortOrderState] = useState<SortOrder>(loadSortOrder);
  const [collapsedCategories, setCollapsedCategories] = useState<Set<string>>(
    new Set(),
  );

  const setSortField = (value: SortField) => {
    setSortFieldState(value);
    try {
      localStorage.setItem(SORT_FIELD_KEY, value);
    } catch {}
  };

  const setSortOrder = (value: SortOrder) => {
    setSortOrderState(value);
    try {
      localStorage.setItem(SORT_ORDER_KEY, value);
    } catch {}
  };

  const toggleCategory = useCallback((cat: string) => {
    setCollapsedCategories((prev) => {
      const next = new Set(prev);
      if (next.has(cat)) {
        next.delete(cat);
      } else {
        next.add(cat);
      }
      return next;
    });
  }, []);

  const displayedRules = useMemo(() => {
    const q = searchTerm.trim().toLowerCase();
    const filtered = q
      ? rules.filter((r) => {
          const i18nKey = `security.rules.descriptions.${r.id}`;
          const translated = t(i18nKey, { defaultValue: "" });
          const desc = (translated || r.description || "").toLowerCase();
          const toolsStr = (r.tools || []).join(",").toLowerCase();
          return (
            r.id.toLowerCase().includes(q) ||
            desc.includes(q) ||
            toolsStr.includes(q)
          );
        })
      : rules;

    const compareStrings = (a: string, b: string) =>
      a.localeCompare(b, undefined, { sensitivity: "base", numeric: true });

    const sorted = [...filtered].sort((a, b) => {
      let cmp = 0;
      if (sortField === "enabled") {
        cmp = Number(a.disabled) - Number(b.disabled);
        if (cmp === 0) cmp = compareStrings(a.id, b.id);
      } else {
        cmp = compareStrings(a.id, b.id);
      }
      return sortOrder === "asc" ? cmp : -cmp;
    });
    return sorted;
  }, [rules, searchTerm, sortField, sortOrder]);

  const categoryGroups = useMemo((): CategoryGroup[] => {
    const groupMap = new Map<string, MergedRule[]>();
    for (const rule of displayedRules) {
      const cat = rule.category || "command_injection";
      if (!groupMap.has(cat)) groupMap.set(cat, []);
      groupMap.get(cat)!.push(rule);
    }

    const orderedCategories = [
      ...CATEGORY_ORDER.filter((c) => groupMap.has(c)),
      ...[...groupMap.keys()].filter((c) => !CATEGORY_ORDER.includes(c)),
    ];

    return orderedCategories.map((cat) => {
      const catRules = groupMap.get(cat)!;
      return {
        category: cat,
        rules: catRules,
        enabledCount: catRules.filter((r) => !r.disabled).length,
        totalCount: catRules.length,
      };
    });
  }, [displayedRules]);

  const toolbar = (
    <div className={styles.filterRow}>
      <div className={styles.filterBar}>
        <div className={styles.searchInput}>
          <TextInput
            style={{ width: "100%" }}
            placeholder={t("security.rules.searchRule")}
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
          />
        </div>
        <Dropdown
          menu={{
            items: [
              { key: "id-asc", label: t("security.rules.sortById") + " ↑" },
              { key: "id-desc", label: t("security.rules.sortById") + " ↓" },
              {
                key: "enabled-desc",
                label: t("security.rules.sortByEnabled") + " ↓",
              },
              {
                key: "enabled-asc",
                label: t("security.rules.sortByEnabled") + " ↑",
              },
            ],
            selectedKeys: [`${sortField}-${sortOrder}`],
            onClick: ({ key }: { key: string }) => {
              const idx = key.lastIndexOf("-");
              setSortField(key.slice(0, idx) as SortField);
              setSortOrder(key.slice(idx + 1) as SortOrder);
            },
          }}
          trigger={["click"]}
        >
          <Btn icon={<ArrowUpNarrowWide />} />
        </Dropdown>
        <div className={styles.filterActions}>
          {onAddRule ? (
            <Btn
              icon={<Plus />}
              onClick={onAddRule}
              disabled={addRuleDisabled}
            />
          ) : null}
        </div>
      </div>
    </div>
  );

  const categoryHeaderStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    gap: 8,
    padding: "10px 16px",
    cursor: "pointer",
    userSelect: "none",
    borderBottom: "1px solid var(--border-soft)",
    background: "var(--surface-2)",
    fontSize: 13,
    fontWeight: 600,
    color: "var(--text)",
  };

  const countBadgeStyle: React.CSSProperties = {
    fontSize: 12,
    fontWeight: 400,
    color: "var(--text-3)",
    marginLeft: "auto",
  };

  if (isMobileMode) {
    if (displayedRules.length === 0) {
      return (
        <>
          {toolbar}
          <div className={styles.listEmpty}>
            <EmptyState description={t("security.rules.emptyText")} />
          </div>
        </>
      );
    }

    return (
      <>
        {toolbar}
        <div className={styles.list} role="list">
          {categoryGroups.map((group) => {
            const isCollapsed = collapsedCategories.has(group.category);
            return (
              <div key={group.category}>
                <div
                  style={categoryHeaderStyle}
                  onClick={() => toggleCategory(group.category)}
                >
                  {isCollapsed ? (
                    <ChevronRight size={12} />
                  ) : (
                    <ChevronDown size={12} />
                  )}
                  <span>
                    {t(
                      `security.rules.categories.${group.category}`,
                      group.category,
                    )}
                  </span>
                  <span style={countBadgeStyle}>
                    {group.enabledCount}/{group.totalCount}
                  </span>
                </div>
                {!isCollapsed &&
                  group.rules.map((record) => {
                    const i18nKey = `security.rules.descriptions.${record.id}`;
                    const translated = t(i18nKey, { defaultValue: "" });
                    const display = translated || record.description;
                    return (
                      <div
                        key={record.id}
                        role="listitem"
                        className={`${styles.listItem} ${styles.ruleListItem} ${
                          record.disabled ? styles.listItemDisabled : ""
                        }`}
                        onClick={() => onPreviewRule(record)}
                      >
                        <div className={styles.ruleListItemHeader}>
                          <span
                            className={`${styles.listItemId} ${styles.idCell} ${styles.ruleListItemId}`}
                          >
                            <span className={styles.idCellText}>
                              {record.id}
                            </span>
                          </span>
                          <span
                            className={styles.ruleListItemControls}
                            onClick={(event) => event.stopPropagation()}
                          >
                            <Tag
                              color={
                                SEVERITY_COLORS[record.severity] ?? "default"
                              }
                              className={styles.miniTag}
                            >
                              {record.severity}
                            </Tag>
                            <Tag
                              color={
                                record.source === "builtin"
                                  ? "default"
                                  : "success"
                              }
                              className={styles.miniTag}
                            >
                              {record.source === "builtin"
                                ? t("security.rules.builtin")
                                : t("security.rules.custom")}
                            </Tag>
                            <Toggle
                              checked={!record.disabled}
                              onChange={() =>
                                onToggleRule(record.id, record.disabled)
                              }
                              disabled={!enabled}
                            />
                            {record.source === "custom" && (
                              <>
                                <Btn
                                  variant="outline"
                                  icon={<Pencil size={14} />}
                                  onClick={() => onEditRule(record)}
                                  disabled={!enabled}
                                  style={darkBtnStyle}
                                />
                                <Btn
                                  variant="danger"
                                  icon={<Trash2 size={14} />}
                                  onClick={() => onDeleteRule(record.id)}
                                  disabled={!enabled}
                                />
                              </>
                            )}
                          </span>
                        </div>

                        <div className={styles.listItemSummary}>{display}</div>
                      </div>
                    );
                  })}
              </div>
            );
          })}
        </div>
      </>
    );
  }

  const columns = [
    {
      title: t("security.rules.id"),
      dataIndex: "id",
      key: "id",
      width: "28%",
      stackLine: 1,
      render: (id: string, record: MergedRule) => (
        <span
          className={styles.idCell}
          style={{
            opacity: record.disabled ? 0.4 : 1,
            fontFamily: "var(--font-mono)",
            fontSize: 12,
          }}
        >
          <span className={styles.idCellText}>{id}</span>
        </span>
      ),
    },
    {
      title: t("security.rules.severity"),
      dataIndex: "severity",
      key: "severity",
      width: "10%",
      stackLine: 1,
      render: (sev: string, record: MergedRule) => (
        <Tag
          color={SEVERITY_COLORS[sev] ?? "default"}
          className={styles.miniTag}
          style={{ opacity: record.disabled ? 0.4 : 1 }}
        >
          {sev}
        </Tag>
      ),
    },
    {
      title: t("security.rules.descriptionCol"),
      dataIndex: "description",
      key: "description",
      stackLine: 2,
      render: (_text: string, record: MergedRule) => {
        const i18nKey = `security.rules.descriptions.${record.id}`;
        const translated = t(i18nKey, { defaultValue: "" });
        const display = translated || record.description;
        return (
          <span
            style={{
              opacity: record.disabled ? 0.4 : 1,
              display: "block",
              overflowWrap: "anywhere",
            }}
          >
            {display}
          </span>
        );
      },
    },
    {
      title: t("security.rules.source"),
      dataIndex: "source",
      key: "source",
      width: 44,
      align: "right" as const,
      stackLine: 2,
      render: (source: string, record: MergedRule) => (
        <Tag
          color={source === "builtin" ? "default" : "success"}
          className={styles.miniTag}
          style={{ opacity: record.disabled ? 0.4 : 1 }}
        >
          {source === "builtin"
            ? t("security.rules.builtin")
            : t("security.rules.custom")}
        </Tag>
      ),
    },
    {
      title: t("security.rules.actions"),
      key: "actions",
      width: 38,
      align: "right" as const,
      stackLine: 2,
      render: (_: unknown, record: MergedRule) => (
        <div
          style={{
            display: "flex",
            width: "100%",
            alignItems: "center",
            justifyContent: "flex-end",
            flexWrap: "wrap",
            gap: "var(--space-1)",
          }}
          onClick={(event) => event.stopPropagation()}
        >
          <Toggle
            checked={!record.disabled}
            onChange={() => onToggleRule(record.id, record.disabled)}
            disabled={!enabled}
          />
          {record.source === "custom" && (
            <>
              <Btn
                variant="outline"
                size="sm"
                icon={<Pencil size={14} />}
                onClick={() => onEditRule(record)}
                disabled={!enabled}
                style={darkBtnStyle}
              />
              <Btn
                variant="danger"
                size="sm"
                icon={<Trash2 size={14} />}
                onClick={() => onDeleteRule(record.id)}
                disabled={!enabled}
              />
            </>
          )}
        </div>
      ),
    },
  ];

  if (displayedRules.length === 0) {
    return (
      <>
        {toolbar}
        <div style={{ padding: "40px 0" }}>
          <EmptyState description={t("security.rules.emptyText")} />
        </div>
      </>
    );
  }

  return (
    <>
      {toolbar}
      {categoryGroups.map((group) => {
        const isCollapsed = collapsedCategories.has(group.category);
        return (
          <div key={group.category}>
            <div
              style={categoryHeaderStyle}
              onClick={() => toggleCategory(group.category)}
            >
              {isCollapsed ? (
                <ChevronRight size={12} />
              ) : (
                <ChevronDown size={12} />
              )}
              <span>
                {t(
                  `security.rules.categories.${group.category}`,
                  group.category,
                )}
              </span>
              <span style={countBadgeStyle}>
                {group.enabledCount}/{group.totalCount}
              </span>
            </div>
            {!isCollapsed && (
              <Table
                dataSource={group.rules}
                columns={columns}
                rowKey="id"
                pagination={false}
                size="sm"
                className={styles.ruleTable}
                showHeader={false}
                responsive="stack"
                onRow={(record) => ({
                  onClick: () => onPreviewRule(record),
                })}
              />
            )}
          </div>
        );
      })}
    </>
  );
}
