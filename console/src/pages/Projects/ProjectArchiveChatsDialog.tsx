import { useState } from "react";
import { useTranslation } from "react-i18next";
import { projectsApi } from "@/api/modules/projects";
import type {
  ProjectBulkArchiveResult,
  ProjectDescriptor,
} from "@/api/types/project";
import { Modal, InfoBox, Btn } from "@/ui";
import { extractFriendlyMessage } from "@/utils/error";
import { invalidateConversationNavigation } from "@/layouts/conversationNavigationEvents";
export default function ProjectArchiveChatsDialog({
  project,
  onClose,
}: {
  project: ProjectDescriptor;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [result, setResult] = useState<ProjectBulkArchiveResult | null>(null);
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const archive = async (ids?: string[]) => {
    if (pending) return;
    setPending(true);
    setError("");
    try {
      const next = await projectsApi.archiveIdleChats(project.project_id, ids);
      setResult((current) =>
        !ids || !current
          ? next
          : {
              items: current.items.map(
                (item) =>
                  next.items.find(
                    (updated) => updated.chat_id === item.chat_id,
                  ) ?? item,
              ),
            },
      );
      invalidateConversationNavigation();
    } catch (error) {
      setError(extractFriendlyMessage(error));
    } finally {
      setPending(false);
    }
  };
  const failed =
    result?.items
      .filter((item) => item.status === "failed")
      .map((item) => item.chat_id) ?? [];
  return (
    <Modal
      open
      title={t("projects.archiveChats")}
      onClose={onClose}
      onOk={() => (result ? onClose() : void archive())}
      okText={t(result ? "common.close" : "projects.archiveChats")}
      confirmLoading={pending}
      width={560}
    >
      <p>{t("projects.archiveChatsDescription")}</p>
      {error && <InfoBox type="error">{error}</InfoBox>}
      {result && (
        <>
          <p>
            {t("projects.archiveChatsSummary", {
              archived: result.items.filter(
                (item) => item.status === "archived",
              ).length,
              skipped: result.items.filter((item) => item.status === "skipped")
                .length,
              failed: failed.length,
            })}
          </p>
          <ul>
            {result.items
              .filter((item) => item.status !== "archived")
              .map((item) => (
                <li key={item.chat_id}>
                  <code>{item.chat_id}</code> —{" "}
                  {t(`projects.bulkArchiveStatus.${item.status}`)}
                  {item.reason &&
                    `: ${item.reason
                      .split(",")
                      .map((reason) =>
                        t(`projects.bulkArchiveReason.${reason}`, reason),
                      )
                      .join(", ")}`}
                </li>
              ))}
          </ul>
          {failed.length > 0 && (
            <Btn disabled={pending} onClick={() => void archive(failed)}>
              {t("projects.retryFailedArchives")}
            </Btn>
          )}
        </>
      )}
    </Modal>
  );
}
