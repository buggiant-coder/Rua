import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  conversationApi,
  type ChatConfigurationPreview,
  type ConversationApiTarget,
} from "@/api/modules/conversation";
import { Btn, Modal } from "@/ui";
import { configurationChanges } from "./configurationDiff";
import styles from "./ChatConfigurationUpdateButton.module.less";

export function ChatConfigurationUpdateButton({
  target,
  chatId,
  disabled = false,
  onApplied,
}: {
  target: ConversationApiTarget;
  chatId: string;
  disabled?: boolean;
  onApplied?: () => void;
}) {
  const { t } = useTranslation();
  const [preview, setPreview] = useState<ChatConfigurationPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const address = JSON.stringify([target, chatId]);
  const addressRef = useRef(address);
  addressRef.current = address;
  useEffect(() => {
    setPreview(null);
    setError(null);
    setLoading(false);
  }, [address]);

  async function loadPreview() {
    const requestedAddress = address;
    setLoading(true);
    setError(null);
    try {
      const result = await conversationApi.previewConfiguration(target, chatId);
      if (addressRef.current === requestedAddress) setPreview(result);
    } catch (reason) {
      if (addressRef.current === requestedAddress)
        setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (addressRef.current === requestedAddress) setLoading(false);
    }
  }

  async function applyPreview() {
    if (!preview) return;
    const requestedAddress = address;
    setLoading(true);
    setError(null);
    try {
      await conversationApi.applyConfiguration(target, chatId, preview);
      if (addressRef.current !== requestedAddress) return;
      setPreview(null);
      onApplied?.();
    } catch (reason) {
      if (addressRef.current === requestedAddress)
        setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (addressRef.current === requestedAddress) setLoading(false);
    }
  }

  const changes = preview
    ? configurationChanges(preview.current, preview.candidate).filter(
        (change) => change.field !== "epoch",
      )
    : [];

  return (
    <>
      <Btn
        variant="outline"
        disabled={disabled || !chatId}
        loading={loading && !preview}
        onClick={() => void loadPreview()}
      >
        {t("tools.selectionPolicy.updateChat")}
      </Btn>
      {error && !preview && <span role="alert">{error}</span>}
      <Modal
        open={preview !== null}
        title={t("tools.selectionPolicy.updateChat")}
        onClose={() => {
          if (!loading) {
            setPreview(null);
            setError(null);
          }
        }}
        width={720}
        footer={
          <>
            <Btn
              variant="outline"
              disabled={loading}
              onClick={() => {
                setPreview(null);
                setError(null);
              }}
            >
              {t("common.cancel")}
            </Btn>
            <Btn
              loading={loading}
              disabled={changes.length === 0}
              onClick={() => void applyPreview()}
            >
              {t("tools.selectionPolicy.applyChat")}
            </Btn>
          </>
        }
      >
        <p>{t("tools.selectionPolicy.updateChatHint")}</p>
        <dl className={styles.changes}>
          {changes.map((change) => (
            <div key={change.field}>
              <dt>
                <code>{change.field}</code>
              </dt>
              <dd>
                <del>{JSON.stringify(change.before) ?? "—"}</del>
                {" → "}
                <ins>{JSON.stringify(change.after) ?? "—"}</ins>
              </dd>
            </div>
          ))}
        </dl>
        {changes.length === 0 && <p>{t("tools.selectionPolicy.noChanges")}</p>}
        {error && preview && <p role="alert">{error}</p>}
      </Modal>
    </>
  );
}
