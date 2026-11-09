import { useState, useEffect, useMemo } from "react";
import type { ModelSlotRequest } from "../../../../../api/types";
import api from "../../../../../api";
import { useTranslation } from "react-i18next";
import { isProviderReadyForModelSelection } from "../../../../../utils/providerReadiness";
import styles from "../../index.module.less";
import { Save } from "lucide-react";
import { Btn, Select, toast } from "@/ui";

interface ModelsSectionProps {
  providers: Array<{
    id: string;
    name: string;
    models?: Array<{ id: string; name: string }>;
    extra_models?: Array<{ id: string; name: string }>;
    base_url?: string;
    api_key?: string;
    is_custom: boolean;
    is_local?: boolean;
    require_api_key?: boolean;
  }>;
  activeModels: {
    default_llm?: {
      provider_id?: string;
      model?: string;
    };
  } | null;
  onSaved: () => void;
}

export function ModelsSection({
  providers,
  activeModels,
  onSaved,
}: ModelsSectionProps) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);
  const [selectedProviderId, setSelectedProviderId] = useState<
    string | undefined
  >(undefined);
  const [selectedModel, setSelectedModel] = useState<string | undefined>(
    undefined,
  );
  const [dirty, setDirty] = useState(false);
  const currentSlot = activeModels?.default_llm;

  const eligible = useMemo(
    () => providers.filter(isProviderReadyForModelSelection),
    [providers],
  );

  useEffect(() => {
    if (currentSlot) {
      setSelectedProviderId(currentSlot.provider_id || undefined);
      setSelectedModel(currentSlot.model || undefined);
    }
    setDirty(false);
  }, [currentSlot?.provider_id, currentSlot?.model]);

  const chosenProvider = providers.find((p) => p.id === selectedProviderId);
  const modelOptions = [
    ...(chosenProvider?.models ?? []),
    ...(chosenProvider?.extra_models ?? []),
  ];
  const hasModels = modelOptions.length > 0;

  const handleProviderChange = (pid: string) => {
    setSelectedProviderId(pid);
    setSelectedModel(undefined);
    setDirty(true);
  };

  const handleModelChange = (model: string) => {
    setSelectedModel(model);
    setDirty(true);
  };

  const handleSave = async () => {
    if (!selectedProviderId || !selectedModel) return;

    const body: ModelSlotRequest = {
      provider_id: selectedProviderId,
      model: selectedModel,
      scope: "global",
    };

    setSaving(true);
    try {
      await api.setActiveLlm(body);
      toast.success(t("models.llmModelUpdated"));
      setDirty(false);
      onSaved();
    } catch (error) {
      const errMsg =
        error instanceof Error ? error.message : t("models.failedToSave");
      toast.error(errMsg);
    } finally {
      setSaving(false);
    }
  };

  const isActive =
    currentSlot &&
    currentSlot.provider_id === selectedProviderId &&
    currentSlot.model === selectedModel;
  const canSave = dirty && !!selectedProviderId && !!selectedModel;

  return (
    <div className={styles.slotSection}>
      <div className={styles.slotForm}>
        <div className={styles.slotField}>
          <label className={styles.slotLabel}>{t("models.provider")}</label>
          <Select
            style={{ width: "100%" }}
            placeholder={t("models.selectProvider")}
            value={selectedProviderId}
            onChange={handleProviderChange}
            options={eligible.map((p) => ({
              value: p.id,
              label: p.name,
            }))}
          />
        </div>

        <div className={styles.slotField}>
          <label className={styles.slotLabel}>{t("models.model")}</label>
          <Select
            style={{ width: "100%" }}
            placeholder={
              hasModels ? t("models.selectModel") : t("models.addModelFirst")
            }
            disabled={!hasModels}
            showSearch
            optionFilterProp="label"
            value={selectedModel}
            onChange={handleModelChange}
            options={modelOptions.map((m) => ({
              value: m.id,
              label: `${m.name} (${m.id})`,
            }))}
          />
        </div>

        <div
          className={styles.slotField}
          style={{ flex: "0 0 auto", minWidth: "120px" }}
        >
          <label className={styles.slotLabel} style={{ visibility: "hidden" }}>
            {t("models.actions")}
          </label>
          <Btn
            loading={saving}
            disabled={!canSave}
            onClick={handleSave}
            style={{ width: "100%" }}
            icon={<Save />}
          >
            {isActive ? t("models.saved") : t("models.save")}
          </Btn>
        </div>
      </div>
      <p className={styles.slotDescription}>{t("models.llmDescription")}</p>
    </div>
  );
}
