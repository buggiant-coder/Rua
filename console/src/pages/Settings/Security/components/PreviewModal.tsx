import type { TagProps } from "@/ui/Tag";
import { useTranslation } from "react-i18next";
import type {
  ToolGuardRule,
  ArgMatcher,
  PathMatcher,
} from "../../../../api/modules/security";
import { Modal, Tag } from "@/ui";

const SEVERITY_COLORS: Record<string, TagProps["color"]> = {
  CRITICAL: "danger",
  HIGH: "warning",
  MEDIUM: "warning",
  LOW: "primary",
  INFO: "default",
};

interface PreviewModalProps {
  rule: ToolGuardRule | null;
  onClose: () => void;
}

export function PreviewModal({ rule, onClose }: PreviewModalProps) {
  const { t } = useTranslation();

  if (!rule) return null;

  const isAst = rule.rule_type === "ast";

  const preStyle: React.CSSProperties = {
    background: "var(--surface-2)",
    color: "var(--text)",
    padding: 12,
    borderRadius: "var(--radius-sm)",
    fontSize: 13,
    border: "1px solid var(--border-soft)",
    whiteSpace: "pre-wrap",
    wordBreak: "break-all",
  };

  return (
    <Modal
      className="modal-fullscreen-mobile"
      title={t("security.rules.previewTitle")}
      open={!!rule}
      onClose={onClose}
      footer={null}
      width={640}
    >
      <div style={{ marginTop: 16, lineHeight: 2 }}>
        <p>
          <strong>{t("security.rules.ruleId")}:</strong> {rule.id}
          {isAst && (
            <Tag color="primary" style={{ marginLeft: 8 }}>
              AST
            </Tag>
          )}
        </p>
        <p>
          <strong>{t("security.rules.severityLabel")}:</strong>{" "}
          <Tag color={SEVERITY_COLORS[rule.severity] ?? "default"}>
            {rule.severity}
          </Tag>
        </p>
        <p>
          <strong>{t("security.rules.tools")}:</strong>{" "}
          {rule.tools.length > 0
            ? rule.tools.join(", ")
            : t("security.rules.allTools")}
        </p>
        {!isAst && (
          <p>
            <strong>{t("security.rules.params")}:</strong>{" "}
            {(rule.params?.length ?? 0) > 0
              ? rule.params.join(", ")
              : t("security.rules.allParams")}
          </p>
        )}
        <p>
          <strong>{t("security.rules.actionLabel")}:</strong>{" "}
          <Tag color="warning">
            {isAst && rule.action
              ? rule.action
              : t("security.rules.actionApproval")}
          </Tag>
        </p>
        <p style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
          <strong>{t("security.rules.descriptionLabel")}:</strong>{" "}
          {t(`security.rules.descriptions.${rule.id}`, {
            defaultValue: "",
          }) || rule.description}
        </p>
        {isAst ? (
          <>
            {rule.command_set && rule.command_set.length > 0 && (
              <>
                <p>
                  <strong>
                    {t("security.rules.commandSet", "Command Set")}:
                  </strong>
                </p>
                <pre style={preStyle}>{rule.command_set.join(", ")}</pre>
              </>
            )}
            {rule.structural && rule.structural.length > 0 && (
              <>
                <p>
                  <strong>
                    {t("security.rules.structuralFlags", "Structural Flags")}:
                  </strong>
                </p>
                <div style={preStyle}>
                  {rule.structural.map((s, i) => (
                    <div key={s} style={i > 0 ? { marginTop: 8 } : undefined}>
                      <strong>
                        {t(`security.rules.structuralValues.${s}`, s)}
                      </strong>
                      <div style={{ fontSize: 12, opacity: 0.8, marginTop: 2 }}>
                        {t(`security.rules.structuralExplain.${s}`, "")}
                      </div>
                    </div>
                  ))}
                </div>
              </>
            )}
            {rule.arg_matchers && rule.arg_matchers.length > 0 && (
              <>
                <p>
                  <strong>
                    {t("security.rules.argMatchers", "Arg Matchers")}:
                  </strong>
                </p>
                <pre style={preStyle}>
                  {rule.arg_matchers
                    .map((m: ArgMatcher) => {
                      const parts: string[] = [];
                      if (m.command) parts.push(`command: ${m.command}`);
                      if (m.flag) parts.push(`flag: ${m.flag}`);
                      if (m.value_contains)
                        parts.push(`value_contains: ${m.value_contains}`);
                      if (m.value_in && m.value_in.length > 0)
                        parts.push(`value_in: [${m.value_in.join(", ")}]`);
                      return parts.join(", ");
                    })
                    .join("\n")}
                </pre>
              </>
            )}
            {rule.path_matchers && rule.path_matchers.length > 0 && (
              <>
                <p>
                  <strong>
                    {t("security.rules.pathMatchers", "Path Matchers")}:
                  </strong>
                </p>
                <pre style={preStyle}>
                  {rule.path_matchers
                    .map((m: PathMatcher) => {
                      const parts: string[] = [];
                      if (m.contains && m.contains.length > 0)
                        parts.push(`contains: [${m.contains.join(", ")}]`);
                      if (m.prefix && m.prefix.length > 0)
                        parts.push(`prefix: [${m.prefix.join(", ")}]`);
                      if (m.glob) parts.push(`glob: ${m.glob}`);
                      if (m.on_redirect) parts.push("on_redirect");
                      return parts.join(", ");
                    })
                    .join("\n")}
                </pre>
              </>
            )}
            {rule.lexical && rule.lexical.length > 0 && (
              <>
                <p>
                  <strong>
                    {t("security.rules.lexicalChecks", "Lexical Checks")}:
                  </strong>
                </p>
                <div style={preStyle}>
                  {rule.lexical.map((l, i) => (
                    <div key={l} style={i > 0 ? { marginTop: 8 } : undefined}>
                      <strong>
                        {t(`security.rules.lexicalValues.${l}`, l)}
                      </strong>
                      <div style={{ fontSize: 12, opacity: 0.8, marginTop: 2 }}>
                        {t(`security.rules.lexicalExplain.${l}`, "")}
                      </div>
                    </div>
                  ))}
                </div>
              </>
            )}
          </>
        ) : (
          <>
            {rule.patterns && rule.patterns.length > 0 && (
              <>
                <p>
                  <strong>{t("security.rules.patterns")}:</strong>
                </p>
                <pre style={preStyle}>{rule.patterns.join("\n")}</pre>
              </>
            )}
            {rule.exclude_patterns && rule.exclude_patterns.length > 0 && (
              <>
                <p>
                  <strong>{t("security.rules.excludePatterns")}:</strong>
                </p>
                <pre style={preStyle}>{rule.exclude_patterns.join("\n")}</pre>
              </>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
