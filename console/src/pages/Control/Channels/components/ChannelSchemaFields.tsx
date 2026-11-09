import { Controller, type Control } from "react-hook-form";
import { useTranslation } from "react-i18next";
import type { ChannelSchemaField } from "../../../../api/types/channel";
import {
  FormField,
  NumberInput,
  Select,
  TextArea,
  TextInput,
  Toggle,
} from "@/ui";

interface Props {
  fields: ChannelSchemaField[];
  control: Control<Record<string, any>>;
  enabled: boolean;
}

export function ChannelSchemaFields({ fields, control, enabled }: Props) {
  const { t, i18n } = useTranslation();
  const language = i18n.language?.startsWith("zh") ? "zh" : "en";
  return fields.map((spec) => {
    const labelKey: Record<string, string> = {
      bot_prefix: "botPrefix",
      dm_policy: "dmPolicy",
      group_policy: "groupPolicy",
      require_mention: "requireMention",
      allow_from: "allowFrom",
      access_control_dm: "accessControlDm",
      access_control_group: "accessControlGroup",
      show_thinking: "filterThinking",
    };
    const label = labelKey[spec.name]
      ? t("channels." + labelKey[spec.name], {
          defaultValue: spec.label[language],
        })
      : spec.label[language];
    const json =
      spec.default !== null &&
      typeof spec.default === "object" &&
      !Array.isArray(spec.default);
    return (
      <Controller
        key={spec.name}
        name={spec.name}
        control={control}
        rules={{
          validate: (value) => {
            if (
              enabled &&
              spec.required &&
              (value == null || String(value).trim() === "")
            ) {
              return t("common.required", { defaultValue: "Required" });
            }
            if (
              typeof value === "number" &&
              ((spec.ge !== undefined && value < spec.ge) ||
                (spec.gt !== undefined && value <= spec.gt) ||
                (spec.le !== undefined && value > spec.le) ||
                (spec.lt !== undefined && value >= spec.lt))
            )
              return t("common.invalidValue", {
                defaultValue: "Invalid value",
              });
            if (json && typeof value === "string") {
              try {
                const parsed = JSON.parse(value);
                if (
                  parsed === null ||
                  typeof parsed !== "object" ||
                  Array.isArray(parsed)
                )
                  throw new Error();
              } catch {
                return t("channels.invalidJson", {
                  defaultValue: "Enter a JSON object",
                });
              }
            }
            return true;
          },
        }}
        render={({ field, fieldState }) => (
          <FormField
            label={label}
            required={enabled && spec.required}
            error={fieldState.error?.message}
          >
            {spec.kind === "toggle" ? (
              <Toggle
                aria-label={label}
                checked={field.value ?? spec.default ?? true}
                onChange={field.onChange}
              />
            ) : spec.kind === "number" ? (
              <NumberInput
                {...field}
                aria-label={label}
                value={field.value ?? null}
                min={spec.ge}
                max={spec.le}
              />
            ) : spec.kind === "select" ? (
              <Select
                aria-label={label}
                value={field.value == null ? undefined : String(field.value)}
                options={spec.options}
                onChange={(value) =>
                  field.onChange(
                    typeof spec.default === "number" ? Number(value) : value,
                  )
                }
              />
            ) : spec.kind === "tags" ? (
              <Select
                mode="tags"
                aria-label={label}
                value={field.value ?? []}
                onChange={field.onChange}
              />
            ) : spec.kind === "textarea" || json ? (
              <TextArea
                {...field}
                aria-label={label}
                rows={4}
                value={
                  json && typeof field.value !== "string"
                    ? JSON.stringify(field.value ?? {}, null, 2)
                    : field.value ?? ""
                }
              />
            ) : (
              <TextInput
                {...field}
                aria-label={label}
                value={field.value ?? ""}
                type={
                  spec.secret || spec.kind === "password" ? "password" : "text"
                }
                autoComplete={spec.secret ? "new-password" : undefined}
              />
            )}
          </FormField>
        )}
      />
    );
  });
}
