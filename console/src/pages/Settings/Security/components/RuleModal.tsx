import { useEffect } from "react";
import { useForm, useFieldArray, Controller } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { InputWithExample } from "../../../../components/InputWithExample";
import type { ToolGuardRule } from "../../../../api/modules/security";
import { MinusCircle, Plus } from "lucide-react";
import { Btn, FormField, Modal, Select, TextInput, Toggle } from "@/ui";

const SEVERITY_OPTIONS = ["CRITICAL", "HIGH", "MEDIUM", "LOW", "INFO"];
const CATEGORY_OPTIONS = [
  "command_injection",
  "data_exfiltration",
  "path_traversal",
  "sensitive_file_access",
  "network_abuse",
  "credential_exposure",
  "resource_abuse",
  "code_execution",
  "command_obfuscation",
  "privilege_escalation",
];
const BUILTIN_TOOLS = [
  "Bash",
  "Read",
  "Write",
  "Edit",
  "Grep",
  "Glob",
  "append_file",
  "execute_python_code",
  "conversation_recall",
  "conversation_read_span",
  "list_tools",
  "view_image",
  "view_video",
  "send_file_to_user",
  "get_current_time",
  "set_user_timezone",
  "get_token_usage",
  "web_fetch",
  "web_search",
  "ask_user",
  "task_create",
  "task_cancel",
  "task_retry",
  "task_get",
  "task_list",
  "task_await",
  "memory_search",
  "recall_tool_result",
  "screenshot_capability",
  "browser_capability",
];
const LEXICAL_OPTIONS = [
  "ControlChars",
  "UnicodeWhitespace",
  "IfsReference",
  "AnsiCQuoting",
  "BackslashWhitespace",
  "BackslashOperator",
  "EmbeddedNewline",
  "CommentQuoteDesync",
  "QuotedNewline",
];
const STRUCTURAL_OPTIONS = [
  "PipeToShell",
  "CommandSubstitution",
  "SensitiveRedirect",
  "ForkBomb",
  "Base64ToShell",
];
const ACTION_OPTIONS = ["NeedsApproval", "Deny", "Audit"];

export interface RuleFormValues {
  id: string;
  tools: string[];
  params: string[];
  severity: string;
  category?: string;
  description?: string;
  remediation?: string;
  command_set: string[];
  arg_matchers: {
    command?: string;
    flag?: string;
    value_contains?: string;
    value_in?: string[];
  }[];
  path_matchers: {
    contains?: string[];
    prefix?: string[];
    glob?: string;
    on_redirect?: boolean;
  }[];
  lexical: string[];
  structural: string[];
  action?: string;
}

interface RuleModalProps {
  open: boolean;
  editingRule: ToolGuardRule | null;
  existingRuleIds: string[];
  onSubmit: (values: RuleFormValues) => void;
  onClose: () => void;
}

export function RuleModal({
  open,
  editingRule,
  existingRuleIds,
  onSubmit,
  onClose,
}: RuleModalProps) {
  const { t } = useTranslation();

  const {
    register,
    handleSubmit,
    reset,
    control,
    formState: { errors },
  } = useForm<RuleFormValues>({
    defaultValues: {
      id: "",
      tools: [],
      params: [],
      severity: "HIGH",
      command_set: [],
      arg_matchers: [],
      path_matchers: [],
      lexical: [],
      structural: [],
    },
  });

  const {
    fields: argMatcherFields,
    append: appendArgMatcher,
    remove: removeArgMatcher,
  } = useFieldArray({ control, name: "arg_matchers" });

  const {
    fields: pathMatcherFields,
    append: appendPathMatcher,
    remove: removePathMatcher,
  } = useFieldArray({ control, name: "path_matchers" });

  useEffect(() => {
    if (open) {
      if (editingRule) {
        reset({
          id: editingRule.id,
          tools: editingRule.tools,
          params: editingRule.params,
          severity: editingRule.severity,
          category: editingRule.category,
          description: editingRule.description,
          remediation: editingRule.remediation,
          command_set: editingRule.command_set ?? [],
          arg_matchers: editingRule.arg_matchers ?? [],
          path_matchers: editingRule.path_matchers ?? [],
          lexical: editingRule.lexical ?? [],
          structural: editingRule.structural ?? [],
          action: editingRule.action,
        });
      } else {
        reset({
          id: "",
          tools: [],
          params: [],
          severity: "HIGH",
          command_set: [],
          arg_matchers: [],
          path_matchers: [],
          lexical: [],
          structural: [],
        });
      }
    }
  }, [open, editingRule, reset]);

  const toolOptions = BUILTIN_TOOLS.map((name) => ({
    label: name,
    value: name,
  }));

  return (
    <Modal
      className="modal-fullscreen-mobile"
      title={
        editingRule
          ? t("security.rules.editTitle")
          : t("security.rules.addTitle")
      }
      open={open}
      onOk={handleSubmit(onSubmit)}
      onClose={onClose}
      okText={t("common.confirm")}
      cancelText={t("common.cancel")}
      width={720}
      destroyOnClose
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 16,
          marginTop: 16,
        }}
      >
        <FormField
          label={t("security.rules.ruleId")}
          error={errors.id?.message}
          required
        >
          <InputWithExample
            {...register("id", {
              required: t("security.rules.ruleIdRequired"),
              validate: (value) => {
                if (!value || editingRule) return true;
                if (existingRuleIds.includes(value)) {
                  return t("security.rules.duplicateId");
                }
                return true;
              },
            })}
            placeholder={t("security.rules.ruleIdPlaceholder")}
            disabled={!!editingRule}
          />
        </FormField>

        <Controller
          name="tools"
          control={control}
          render={({ field }) => (
            <FormField label={t("security.rules.tools")}>
              <Select
                mode="tags"
                value={field.value}
                onChange={field.onChange}
                options={toolOptions}
                placeholder={t("security.rules.toolsPlaceholder")}
                allowClear
              />
            </FormField>
          )}
        />

        <Controller
          name="params"
          control={control}
          render={({ field }) => (
            <FormField label={t("security.rules.params")}>
              <Select
                mode="tags"
                value={field.value}
                onChange={field.onChange}
                placeholder={t("security.rules.paramsPlaceholder")}
                allowClear
              />
            </FormField>
          )}
        />

        <Controller
          name="severity"
          control={control}
          render={({ field }) => (
            <FormField label={t("security.rules.severityLabel")}>
              <Select
                value={field.value}
                onChange={field.onChange}
                options={SEVERITY_OPTIONS.map((s) => ({ label: s, value: s }))}
              />
            </FormField>
          )}
        />

        <Controller
          name="category"
          control={control}
          render={({ field }) => (
            <FormField label={t("security.rules.categoryLabel")}>
              <Select
                showSearch
                value={field.value}
                onChange={(v) => field.onChange(v as string)}
                options={CATEGORY_OPTIONS.map((c) => ({
                  label: t(`security.rules.categories.${c}`, c),
                  value: c,
                }))}
                filterOption={(inputValue, option) =>
                  String(option.label)
                    .toLowerCase()
                    .includes(inputValue.toLowerCase()) ||
                  String(option.value)
                    .toLowerCase()
                    .includes(inputValue.toLowerCase())
                }
              />
            </FormField>
          )}
        />

        <div
          className="rua-divider-text rua-divider-text--left"
          style={{ margin: "16px 0 12px" }}
        >
          {t("security.rules.matchers")}
        </div>

        <Controller
          name="command_set"
          control={control}
          render={({ field }) => (
            <FormField label={t("security.rules.commandSetLabel")}>
              <Select
                mode="tags"
                value={field.value}
                onChange={field.onChange}
                placeholder={t("security.rules.commandSetPlaceholder")}
                allowClear
              />
            </FormField>
          )}
        />

        <FormField label={t("security.rules.argMatchersLabel")}>
          {argMatcherFields.map((field, index) => (
            <div
              key={field.id}
              style={{
                display: "flex",
                gap: 6,
                marginBottom: 8,
                alignItems: "center",
              }}
            >
              <TextInput
                {...register(`arg_matchers.${index}.command`)}
                placeholder={t("security.rules.argCommand")}
                style={{ flex: 1 }}
              />
              <TextInput
                {...register(`arg_matchers.${index}.flag`)}
                placeholder={t("security.rules.argFlag")}
                style={{ flex: 1 }}
              />
              <TextInput
                {...register(`arg_matchers.${index}.value_contains`)}
                placeholder={t("security.rules.argValueContains")}
                style={{ flex: 1 }}
              />
              <Controller
                name={`arg_matchers.${index}.value_in`}
                control={control}
                render={({ field: valField }) => (
                  <Select
                    mode="tags"
                    value={valField.value}
                    onChange={valField.onChange}
                    placeholder={t("security.rules.argValueIn")}
                    size="sm"
                    style={{ flex: 1 }}
                  />
                )}
              />
              <MinusCircle
                onClick={() => removeArgMatcher(index)}
                style={{ color: "var(--text-mute)", cursor: "pointer" }}
              />
            </div>
          ))}
          <Btn
            variant="outline"
            onClick={() => appendArgMatcher({})}
            style={{ width: "100%" }}
            icon={<Plus />}
            size="sm"
          >
            {t("security.rules.addArgMatcher")}
          </Btn>
        </FormField>

        <FormField label={t("security.rules.pathMatchersLabel")}>
          {pathMatcherFields.map((field, index) => (
            <div
              key={field.id}
              style={{
                border: "1px solid var(--border)",
                borderRadius: "var(--radius-sm)",
                padding: "8px 10px",
                marginBottom: 8,
                position: "relative",
              }}
            >
              <MinusCircle
                onClick={() => removePathMatcher(index)}
                style={{
                  position: "absolute",
                  top: 8,
                  right: 8,
                  color: "var(--text-mute)",
                  cursor: "pointer",
                }}
              />
              <div style={{ display: "flex", gap: 8, marginBottom: 6 }}>
                <Controller
                  name={`path_matchers.${index}.contains`}
                  control={control}
                  render={({ field: cField }) => (
                    <FormField
                      label={t("security.rules.pathContains")}
                      className={undefined}
                    >
                      <Select
                        mode="tags"
                        value={cField.value}
                        onChange={cField.onChange}
                        placeholder={t(
                          "security.rules.pathContainsPlaceholder",
                        )}
                        size="sm"
                        style={{ width: "100%" }}
                      />
                    </FormField>
                  )}
                />
                <Controller
                  name={`path_matchers.${index}.prefix`}
                  control={control}
                  render={({ field: pField }) => (
                    <FormField
                      label={t("security.rules.pathPrefix")}
                      className={undefined}
                    >
                      <Select
                        mode="tags"
                        value={pField.value}
                        onChange={pField.onChange}
                        placeholder={t("security.rules.pathPrefixPlaceholder")}
                        size="sm"
                        style={{ width: "100%" }}
                      />
                    </FormField>
                  )}
                />
              </div>
              <div style={{ display: "flex", gap: 8, alignItems: "flex-end" }}>
                <FormField
                  label={t("security.rules.pathGlob")}
                  className={undefined}
                >
                  <TextInput
                    {...register(`path_matchers.${index}.glob`)}
                    placeholder={t("security.rules.pathGlobPlaceholder")}
                  />
                </FormField>
                <Controller
                  name={`path_matchers.${index}.on_redirect`}
                  control={control}
                  render={({ field: orField }) => (
                    <FormField
                      label={t("security.rules.pathOnRedirect")}
                      className={undefined}
                    >
                      <Toggle
                        checked={orField.value ?? false}
                        onChange={(e) => orField.onChange(e.target.checked)}
                      />
                    </FormField>
                  )}
                />
              </div>
            </div>
          ))}
          <Btn
            variant="outline"
            onClick={() => appendPathMatcher({})}
            style={{ width: "100%" }}
            icon={<Plus />}
            size="sm"
          >
            {t("security.rules.addPathMatcher")}
          </Btn>
        </FormField>

        <Controller
          name="lexical"
          control={control}
          render={({ field }) => (
            <FormField label={t("security.rules.lexicalLabel")}>
              <Select
                mode="multiple"
                value={field.value}
                onChange={field.onChange}
                options={LEXICAL_OPTIONS.map((l) => ({
                  label: t(`security.rules.lexicalValues.${l}`, l),
                  value: l,
                }))}
                placeholder={t("security.rules.lexicalPlaceholder")}
                allowClear
              />
            </FormField>
          )}
        />

        <Controller
          name="structural"
          control={control}
          render={({ field }) => (
            <FormField label={t("security.rules.structuralLabel")}>
              <Select
                mode="multiple"
                value={field.value}
                onChange={field.onChange}
                options={STRUCTURAL_OPTIONS.map((s) => ({
                  label: t(`security.rules.structuralValues.${s}`, s),
                  value: s,
                }))}
                placeholder={t("security.rules.structuralPlaceholder")}
                allowClear
              />
            </FormField>
          )}
        />

        <div
          className="rua-divider-text rua-divider-text--left"
          style={{ margin: "16px 0 12px" }}
        >
          {t("security.rules.actionDetails")}
        </div>

        <Controller
          name="action"
          control={control}
          render={({ field }) => (
            <FormField label={t("security.rules.actionFieldLabel")}>
              <Select
                value={field.value}
                onChange={field.onChange}
                options={ACTION_OPTIONS.map((a) => ({
                  label: t(`security.rules.actionValues.${a}`, a),
                  value: a,
                }))}
                placeholder={t("security.rules.actionPlaceholder")}
                allowClear
              />
            </FormField>
          )}
        />

        <FormField label={t("security.rules.descriptionLabel")}>
          <TextInput
            {...register("description")}
            placeholder={t("security.rules.descriptionPlaceholder")}
          />
        </FormField>

        <FormField label={t("security.rules.remediationLabel")}>
          <TextInput
            {...register("remediation")}
            placeholder={t("security.rules.remediationPlaceholder")}
          />
        </FormField>
      </div>
    </Modal>
  );
}
