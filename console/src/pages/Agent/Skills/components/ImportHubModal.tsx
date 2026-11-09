import { useState, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { skillMarkets, type SkillMarket } from "./index";
import styles from "./ImportHubModal.module.less";
import {
  CheckCircle,
  ChevronDown,
  FileCode,
  Link,
  Paperclip,
  Upload,
  X,
  XCircle,
} from "lucide-react";
import { Btn, Loading, Modal, TextInput } from "@/ui";
interface ImportHubModalProps {
  open: boolean;
  importing: boolean;
  onClose: () => void;
  onConfirm: (url: string, targetName?: string) => Promise<void>;
  cancelImport?: () => void;
  hint?: string;
}

type ValidationResult =
  | { ok: true; source: string }
  | { ok: false; messageKey: string };

function validateUrl(url: string): ValidationResult {
  const trimmed = url.trim();
  if (!trimmed) {
    return { ok: false, messageKey: "" };
  }

  try {
    new URL(trimmed);
  } catch {
    return { ok: false, messageKey: "skills.invalidUrl" };
  }

  const source = skillMarkets.find((m) =>
    trimmed.toLowerCase().startsWith(m.urlPrefix.toLowerCase()),
  );
  if (!source) {
    return { ok: false, messageKey: "skills.invalidSkillUrlSource" };
  }

  return { ok: true, source: source.name };
}

export function ImportHubModal({
  open,
  importing,
  onClose,
  onConfirm,
  cancelImport,
  hint,
}: ImportHubModalProps) {
  const { t } = useTranslation();
  const [importUrl, setImportUrl] = useState("");
  const [activeMarket, setActiveMarket] = useState<string | null>(
    skillMarkets[0]?.key || null,
  );

  const validation = useMemo(() => validateUrl(importUrl), [importUrl]);
  const canImport = validation.ok && !importing;

  const handleClose = () => {
    if (importing) return;
    setImportUrl("");
    setActiveMarket(skillMarkets[0]?.key || null);
    onClose();
  };

  const handleConfirm = async () => {
    if (importing || !validation.ok) return;
    await onConfirm(importUrl.trim());
  };

  const inputStateClass = validation.ok
    ? styles.valid
    : validation.messageKey
    ? styles.invalid
    : "";

  const activeMarketData = skillMarkets.find((m) => m.key === activeMarket);

  return (
    <Modal
      className={styles.importHubModal}
      title={t("skills.importHub")}
      open={open}
      onClose={handleClose}
      keyboard={!importing}
      closable={!importing}
      maskClosable={!importing}
      width={680}
      footer={
        <div className={styles.modalFooter}>
          <Btn
            className={styles.cancelButton}
            onClick={importing && cancelImport ? cancelImport : handleClose}
          >
            {t(
              importing && cancelImport
                ? "skills.cancelImport"
                : "common.cancel",
            )}
          </Btn>
          <Btn
            className={styles.importButton}
            onClick={handleConfirm}
            loading={importing}
            disabled={!canImport}
          >
            {t("skills.importHub")}
          </Btn>
        </div>
      }
    >
      {hint && <p className={styles.hintText}>{hint}</p>}

      <div className={styles.urlInputSection}>
        <div className={`${styles.inputWrapper} ${inputStateClass}`}>
          <Link className={styles.urlInputIcon} />
          <TextInput
            className={styles.urlInput}
            value={importUrl}
            onChange={(e) => setImportUrl(e.target.value)}
            placeholder={t("skills.enterSkillUrl")}
            disabled={importing}
            aria-label={t("skills.enterSkillUrl")}
            type="text"
          />
          {importUrl && (
            <Btn
              icon={<X />}
              size="sm"
              variant="outline"
              onClick={() => setImportUrl("")}
              title={t("common.clear")}
              aria-label={t("common.clear")}
            />
          )}
          <Btn
            icon={<FileCode />}
            size="sm"
            variant="outline"
            onClick={async () => {
              try {
                const text = await navigator.clipboard.readText();
                setImportUrl(text);
              } catch {}
            }}
            title={t("common.paste")}
            aria-label={t("common.paste")}
          />
        </div>

        <div className={styles.validationStatus}>
          {validation.ok ? (
            <span className={styles.valid}>
              <CheckCircle />
              {t("skills.urlValid", { source: validation.source })}
            </span>
          ) : validation.messageKey ? (
            <span className={styles.invalid}>
              <XCircle />
              {t(validation.messageKey)}
            </span>
          ) : importing ? (
            <span className={styles.validating}>
              <Loading size="small" />
              {t("common.loading")}
            </span>
          ) : null}
        </div>
      </div>

      <div className={styles.divider}>{t("skills.orChooseFromSources")}</div>

      <div className={styles.sourcesGrid}>
        {skillMarkets.map((market: SkillMarket) => (
          <div
            key={market.key}
            className={`${styles.sourceCard} ${
              activeMarket === market.key ? styles.active : ""
            } ${importing ? styles.disabled : ""}`}
            onClick={
              importing
                ? undefined
                : () =>
                    setActiveMarket((prev) =>
                      prev === market.key ? null : market.key,
                    )
            }
            role="button"
            tabIndex={importing ? -1 : 0}
            onKeyDown={(e) => {
              if (!importing && e.key === "Enter") {
                setActiveMarket((prev) =>
                  prev === market.key ? null : market.key,
                );
              }
            }}
            aria-expanded={activeMarket === market.key}
            aria-label={market.name}
          >
            <a
              href={market.homepage}
              target="_blank"
              rel="noopener noreferrer"
              className={styles.externalLink}
              onClick={(e) => e.stopPropagation()}
              title={market.homepage}
              aria-label={`${market.name} homepage`}
            >
              <Upload />
            </a>
            <div className={styles.sourceCardName}>{market.name}</div>
            <div className={styles.sourceCardMeta}>
              {market.examples.length > 0 && (
                <>
                  {market.examples.length} {t("skills.examples")}
                  <ChevronDown
                    className={`${styles.sourceCardArrow} ${
                      activeMarket === market.key ? styles.active : ""
                    }`}
                  />
                </>
              )}
            </div>
          </div>
        ))}
      </div>

      {activeMarketData && activeMarketData.examples.length > 0 && (
        <div className={styles.examplesPanel}>
          <div className={styles.examplesHeader}>
            <Paperclip />
            {t("skills.examplesFrom", { source: activeMarketData.name })}
          </div>
          <div className={styles.examplesList}>
            {activeMarketData.examples.map((example, idx) => (
              <button
                key={idx}
                className={styles.exampleItem}
                onClick={() => setImportUrl(example.url)}
                title={t("skills.clickToFill")}
                type="button"
              >
                <Link className={styles.exampleItemIcon} />
                <span className={styles.exampleUrl}>{example.url}</span>
                <span className={styles.exampleItemLabel}>{example.label}</span>
              </button>
            ))}
          </div>
        </div>
      )}
    </Modal>
  );
}
