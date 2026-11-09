import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { projectsApi } from "@/api/modules/projects";
import type {
  ProjectDescriptor,
  ProjectLifecyclePreview,
} from "@/api/types/project";
import { Modal, InfoBox } from "@/ui";
import { extractFriendlyMessage } from "@/utils/error";
import { discardProjectLocalState } from "./projectChatNavigation";
import { invalidateConversationNavigation } from "@/layouts/conversationNavigationEvents";

export default function ProjectArchiveDialog({
  project,
  onClose,
  onArchived,
}: {
  project: ProjectDescriptor;
  onClose: () => void;
  onArchived: (project: ProjectDescriptor) => void;
}) {
  const { t } = useTranslation();
  const [preview, setPreview] = useState<ProjectLifecyclePreview | null>(null);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    let active = true;
    projectsApi
      .getLifecyclePreview(project.project_id)
      .then((value) => {
        if (active) setPreview(value);
      })
      .catch((error) => {
        if (active) setError(extractFriendlyMessage(error));
      });
    return () => {
      active = false;
    };
  }, [project.project_id]);
  const archive = async () => {
    if (!preview || saving) return;
    setSaving(true);
    setError("");
    try {
      const result = await projectsApi.archive(project.project_id);
      discardProjectLocalState(project.project_id);
      invalidateConversationNavigation();
      onArchived(result.project);
    } catch (error) {
      setError(extractFriendlyMessage(error));
    } finally {
      setSaving(false);
    }
  };
  return (
    <Modal
      open
      title={t("projects.archiveProject", "Archive Project")}
      onClose={onClose}
      onOk={archive}
      okText={t("projects.archiveProject", "Archive Project")}
      confirmLoading={saving}
      okButtonProps={{ disabled: !preview }}
      width={480}
    >
      <p>{t("projects.archiveConfirmDescription")}</p>
      {!preview && !error && <p>{t("common.loading")}</p>}
      {preview?.impact && (
        <p>
          {t("projects.archiveImpact", {
            chats: preview.impact.affected_chat_count,
            turns: preview.impact.active_turn_count,
            queued: preview.impact.queued_input_count,
            tasks: preview.impact.background_task_count,
            approvals: preview.impact.pending_approval_count,
            prompts: preview.impact.pending_prompt_count,
            acp: preview.impact.active_acp_count,
          })}
        </p>
      )}
      {preview?.resource_device_revoked && (
        <InfoBox type="warning">{t("projects.archiveRevoked")}</InfoBox>
      )}
      {error && <InfoBox type="error">{error}</InfoBox>}
    </Modal>
  );
}
