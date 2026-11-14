import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ToolInfo } from "@/api/modules/tools";
import {
  toolSelectionApi,
  type ToolIdentity,
  type ToolSelectionPolicy,
  type ToolSelectionPolicyPreview,
} from "@/api/modules/toolSelection";
import { Btn, EnableDisableSwitch, Modal, Select } from "@/ui";
import styles from "./toolSelectionPolicy.module.less";

const capabilities = ["phone_use", "browser_use", "computer_use"] as const;
const identity = (tool: ToolInfo): ToolIdentity =>
  tool.identity ?? { kind: "builtin", name: tool.name };

function candidateForMode(
  current: ToolSelectionPolicy,
  tools: ToolInfo[],
  mode: ToolSelectionPolicy["mode"],
): ToolSelectionPolicy {
  if (current.mode === mode) return current;
  const extra: Record<string, unknown> = { ...current };
  for (const key of [
    "mode",
    "allowed_tools",
    "disabled_tools",
    "disabled_capabilities",
  ]) {
    delete extra[key];
  }
  return mode === "restricted"
    ? {
        ...extra,
        mode,
        allowed_tools: tools.filter((tool) => tool.enabled).map(identity),
      }
    : {
        ...extra,
        mode,
        disabled_tools: [],
        disabled_capabilities: [],
      };
}

export function ToolSelectionPolicyPanel({
  agentId,
  tools,
  onSaved,
}: {
  agentId: string;
  tools: ToolInfo[];
  onSaved: () => Promise<void> | void;
}) {
  const { t } = useTranslation();
  const [current, setCurrent] = useState<ToolSelectionPolicy | null>(null);
  const [draft, setDraft] = useState<ToolSelectionPolicy | null>(null);
  const [preview, setPreview] = useState<ToolSelectionPolicyPreview | null>(
    null,
  );
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const agentRef = useRef(agentId);
  agentRef.current = agentId;

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setBusy(false);
    setError(null);
    setPreview(null);
    toolSelectionApi.get(agentId).then(
      ({ selection_policy: policy }) => {
        if (cancelled) return;
        setCurrent(policy);
        setDraft(policy);
        setLoading(false);
      },
      (reason: unknown) => {
        if (cancelled) return;
        setError(String(reason instanceof Error ? reason.message : reason));
        setLoading(false);
        setCurrent(null);
        setDraft(null);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [agentId, tools]);

  async function showPreview() {
    if (!draft) return;
    const requestedAgent = agentId;
    setBusy(true);
    setError(null);
    try {
      const result = await toolSelectionApi.preview(agentId, draft);
      if (agentRef.current === requestedAgent) setPreview(result);
    } catch (reason) {
      if (agentRef.current === requestedAgent)
        setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (agentRef.current === requestedAgent) setBusy(false);
    }
  }

  async function applyPreview() {
    if (!preview) return;
    const requestedAgent = agentId;
    setBusy(true);
    setError(null);
    try {
      const result = await toolSelectionApi.apply(agentId, preview);
      if (agentRef.current !== requestedAgent) return;
      setCurrent(result.selection_policy);
      setDraft(result.selection_policy);
      setPreview(null);
      await onSaved();
    } catch (reason) {
      if (agentRef.current === requestedAgent)
        setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (agentRef.current === requestedAgent) setBusy(false);
    }
  }

  return (
    <section
      className={styles.panel}
      aria-label={t("tools.selectionPolicy.title")}
    >
      <h2>{t("tools.selectionPolicy.title")}</h2>
      <p className={styles.hint}>{t("tools.selectionPolicy.chatCopyHint")}</p>
      {loading ? (
        <p>{t("common.loading")}</p>
      ) : (
        draft &&
        current && (
          <>
            <div className={styles.row}>
              <span id="tool-selection-mode">
                {t("tools.selectionPolicy.mode")}
              </span>
              <Select
                aria-labelledby="tool-selection-mode"
                value={draft.mode}
                disabled={busy}
                options={[
                  {
                    value: "default_enabled",
                    label: t("tools.selectionPolicy.defaultEnabled"),
                  },
                  {
                    value: "restricted",
                    label: t("tools.selectionPolicy.restricted"),
                  },
                ]}
                onChange={(mode) =>
                  setDraft(
                    candidateForMode(
                      current,
                      tools,
                      mode as ToolSelectionPolicy["mode"],
                    ),
                  )
                }
              />
            </div>
            <p className={styles.hint}>
              {t(
                `tools.selectionPolicy.${
                  draft.mode === "default_enabled"
                    ? "defaultHint"
                    : "restrictedHint"
                }`,
              )}
            </p>
            {draft.mode === "default_enabled" &&
              capabilities.map((capability) => (
                <div
                  className={styles.row}
                  role="group"
                  aria-label={t(`tools.selectionPolicy.${capability}`)}
                  key={capability}
                >
                  <span>{t(`tools.selectionPolicy.${capability}`)}</span>
                  <EnableDisableSwitch
                    disabled={busy}
                    enabled={!draft.disabled_capabilities.includes(capability)}
                    onChange={(enabled) =>
                      setDraft({
                        ...draft,
                        disabled_capabilities: enabled
                          ? draft.disabled_capabilities.filter(
                              (value) => value !== capability,
                            )
                          : [
                              ...new Set([
                                ...draft.disabled_capabilities,
                                capability,
                              ]),
                            ],
                      })
                    }
                  />
                </div>
              ))}
            <p className={styles.hint}>
              {t("tools.selectionPolicy.readinessHint")}
            </p>
            <Btn
              variant="outline"
              disabled={busy}
              loading={busy && !preview}
              onClick={() => void showPreview()}
            >
              {t("tools.selectionPolicy.preview")}
            </Btn>
          </>
        )
      )}
      {error && !preview && <p role="alert">{error}</p>}
      <Modal
        open={preview !== null}
        onClose={() => {
          if (!busy) {
            setPreview(null);
            setError(null);
          }
        }}
        title={t("tools.selectionPolicy.preview")}
        footer={
          <>
            <Btn
              variant="outline"
              disabled={busy}
              onClick={() => {
                setPreview(null);
                setError(null);
              }}
            >
              {t("common.cancel")}
            </Btn>
            <Btn loading={busy} onClick={() => void applyPreview()}>
              {t("tools.selectionPolicy.confirm")}
            </Btn>
          </>
        }
      >
        {preview && (
          <div className={styles.preview}>
            <p>{t("tools.selectionPolicy.chatCopyHint")}</p>
            <p>
              {t("tools.selectionPolicy.mode")}:{" "}
              <strong>
                {t(
                  `tools.selectionPolicy.${
                    preview.candidate.mode === "default_enabled"
                      ? "defaultEnabled"
                      : "restricted"
                  }`,
                )}
              </strong>
            </p>
            {preview.candidate.mode === "default_enabled" && (
              <p>{t("tools.selectionPolicy.defaultHint")}</p>
            )}
            <h4>{t("tools.selectionPolicy.currentPolicy")}</h4>
            <pre>{JSON.stringify(preview.current, null, 2)}</pre>
            <h4>{t("tools.selectionPolicy.proposedPolicy")}</h4>
            <pre>{JSON.stringify(preview.candidate, null, 2)}</pre>
            {error && <p role="alert">{error}</p>}
          </div>
        )}
      </Modal>
    </section>
  );
}
