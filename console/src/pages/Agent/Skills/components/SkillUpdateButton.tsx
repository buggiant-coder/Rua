import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../../api";
import { RefreshCcw } from "lucide-react";
import { Btn, Modal, toast } from "@/ui";

type SkillUpdateButtonProps = {
  skillName: string;
  onUpdated?: () => void;
} & (
  | { scope: "pool"; agentId?: never }
  | { scope?: "workspace"; agentId: string }
);

export function SkillUpdateButton(props: SkillUpdateButtonProps) {
  const { skillName, onUpdated } = props;
  const { t } = useTranslation();
  const [checking, setChecking] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [checkResult, setCheckResult] = useState<{
    has_update: boolean;
    current_version: string;
    latest_version: string;
    local_modified: boolean;
  } | null>(null);

  const handleCheckUpdate = async () => {
    setChecking(true);
    setCheckResult(null);
    try {
      const result =
        props.scope === "pool"
          ? await api.checkPoolSkillUpdate(skillName)
          : await api.checkSkillUpdate(skillName, props.agentId);
      setCheckResult(result);
      if (!result.has_update) {
        toast.info(t("skills.alreadyLatest", "Already up to date"));
      }
    } catch {
      toast.error(t("skills.checkUpdateFailed", "Failed to check for updates"));
    } finally {
      setChecking(false);
    }
  };

  const handleUpdate = async (force: boolean) => {
    setUpdating(true);
    try {
      const result =
        props.scope === "pool"
          ? await api.updatePoolSkill(skillName, force)
          : await api.updateSkill(skillName, force, props.agentId);
      if (result.updated) {
        toast.success(
          t("skills.updateSuccess", "Updated from {{old}} to {{new}}", {
            old: result.old_version || "unknown",
            new: result.new_version || "unknown",
          }),
        );
        setCheckResult(null);
        onUpdated?.();
      }
    } catch {
      toast.error(t("skills.updateFailed", "Failed to update skill"));
    } finally {
      setUpdating(false);
    }
  };

  const handleUpdateClick = () => {
    if (checkResult?.local_modified) {
      Modal.confirm({
        title: t("skills.localModifiedTitle", "Local Modifications Detected"),
        content: t(
          "skills.localModifiedContent",
          "You have local modifications to this skill. Updating will overwrite them. Continue?",
        ),
        okText: t("skills.updateAnyway", "Update Anyway"),
        cancelText: t("common.cancel"),
        onOk: () => handleUpdate(true),
      });
    } else {
      handleUpdate(false);
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
      <Btn
        icon={<RefreshCcw size={14} className={checking ? "icon-spin" : ""} />}
        loading={checking}
        onClick={handleCheckUpdate}
        size="sm"
      >
        {t("skills.checkForUpdates", "Check for updates")}
      </Btn>

      {checkResult?.has_update && (
        <div
          style={{
            padding: "8px 12px",
            background: "var(--fill-2, #f5f5f5)",
            borderRadius: 6,
            fontSize: 13,
          }}
        >
          <div style={{ marginBottom: 4 }}>
            <span style={{ color: "var(--text-3)" }}>
              {checkResult.current_version || "–"}
            </span>
            {" → "}
            <span style={{ fontWeight: 500 }}>
              {checkResult.latest_version || "–"}
            </span>
          </div>
          {checkResult.local_modified && (
            <div
              style={{
                color: "var(--warning-6, #faad14)",
                fontSize: 12,
                marginBottom: 4,
              }}
            >
              {t(
                "skills.localModifiedWarning",
                "⚠ This skill has local modifications",
              )}
            </div>
          )}
          <Btn size="sm" loading={updating} onClick={handleUpdateClick}>
            {t("skills.update", "Update")}
          </Btn>
        </div>
      )}
    </div>
  );
}
