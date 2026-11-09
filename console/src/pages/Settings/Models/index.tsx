/* eslint-disable react-refresh/only-export-components */
import { useCallback, useEffect, useMemo, useState } from "react";
import { useLoaderData } from "react-router-dom";

import { useProviders, type ProvidersInitialData } from "./useProviders";
import {
  LoadingState,
  ProviderCard,
  CustomProviderModal,
  ModelManageModal,
  ModelsSection,
  ProviderConfigModal,
} from "./components";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { useTranslation } from "react-i18next";
import type { ProviderInfo } from "../../../api/types/provider";
import { providerApi } from "../../../api/modules/provider";
import api from "../../../api";
import {
  isProviderConfigured,
  isRustLocalProvider,
  providerHasModels,
} from "../../../utils/providerReadiness";
import styles from "./index.module.less";
import { providerIcon } from "./components/providerIcon";
import { ArrowDownWideNarrow, ArrowUpNarrowWide, Plus } from "lucide-react";
import { Btn, Tag, Tooltip, TextInput } from "@/ui";

function isZeroConfigFreeProvider(provider: ProviderInfo): boolean {
  return (
    provider.is_free_tier === true &&
    provider.require_api_key === false &&
    !isRustLocalProvider(provider)
  );
}

function providerMatchesQuery(provider: ProviderInfo, query: string): boolean {
  if (!query) return true;
  const normalized = query.toLowerCase();
  return (
    provider.name.toLowerCase().includes(normalized) ||
    provider.id.toLowerCase().includes(normalized)
  );
}

interface FreeProviderCardProps {
  provider: ProviderInfo;
  activeModels: ReturnType<typeof useProviders>["activeModels"];
  onSaved: () => void;
}

function FreeProviderCard({
  provider,
  activeModels,
  onSaved,
}: FreeProviderCardProps) {
  const { t } = useTranslation();
  const [modelManageOpen, setModelManageOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const models = [...(provider.models ?? []), ...(provider.extra_models ?? [])];
  const isActive =
    activeModels?.default_llm?.provider_id === provider.id &&
    models.some((model) => model.id === activeModels.default_llm?.model);

  return (
    <div
      className={[
        styles.freeProviderCard,
        isActive ? styles.freeProviderCardActive : "",
      ].join(" ")}
    >
      <div className={styles.freeProviderHeader}>
        <div className={styles.freeProviderIdentity}>
          <img
            src={providerIcon(provider.id)}
            alt={provider.name}
            className={styles.freeProviderIcon}
          />
          <div className={styles.freeProviderTitleBlock}>
            <span className={styles.freeProviderName}>{provider.name}</span>
            <span className={styles.freeProviderId}>{provider.id}</span>
          </div>
        </div>
        <div className={styles.freeProviderBadges}>
          <Tag color="success" className={styles.freeTag}>
            {t("models.free")}
          </Tag>
        </div>
      </div>

      <div className={styles.freeProviderMeta}>
        <div className={styles.freeProviderMetaRow}>
          <span>{t("models.cardIdLabel")}</span>
          <code title={provider.id}>{provider.id}</code>
        </div>
        <div className={styles.freeProviderMetaRow}>
          <span>{t("models.cardBaseUrlLabel")}</span>
          <code title={provider.base_url}>{provider.base_url}</code>
        </div>
        <div className={styles.freeProviderMetaRow}>
          <span>{t("models.cardApiKeyLabel")}</span>
          <strong>{provider.api_key || t("models.noApiKeyRequired")}</strong>
        </div>
        <div className={styles.freeProviderMetaRow}>
          <span>{t("models.cardModelLabel")}</span>
          <strong>{t("models.modelsCount", { count: models.length })}</strong>
        </div>
      </div>

      <div className={styles.freeProviderActions}>
        <Btn variant="outline" onClick={() => setModelManageOpen(true)}>
          {t("models.models")}
        </Btn>
        <Btn variant="outline" onClick={() => setSettingsOpen(true)}>
          {t("models.settings")}
        </Btn>
      </div>

      <ModelManageModal
        provider={provider}
        open={modelManageOpen}
        onClose={() => setModelManageOpen(false)}
        onSaved={onSaved}
      />
      <ProviderConfigModal
        provider={provider}
        activeModels={activeModels}
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        onSaved={onSaved}
      />
    </div>
  );
}

interface ProviderTemplateTileProps {
  template: ProviderInfo;
  onConfigure: (templateId: string) => void;
}

function ProviderTemplateTile({
  template,
  onConfigure,
}: ProviderTemplateTileProps) {
  const { t } = useTranslation();
  const displayName = t(`models.providerTemplateNames.${template.id}`, {
    defaultValue: template.name,
  });
  return (
    <button
      type="button"
      className={styles.providerTemplateTile}
      onClick={() => onConfigure(template.id)}
    >
      <img
        src={providerIcon(template.id)}
        alt={displayName}
        className={styles.providerTemplateIcon}
      />
      <span className={styles.providerTemplateLabel}>
        <span className={styles.providerTemplateName}>{displayName}</span>
        {template.is_free_tier && (
          <Tag color="success" className={styles.freeTag}>
            {t("models.free")}
          </Tag>
        )}
      </span>
      <span className={styles.providerTemplateAction}>
        {template.supports_oauth
          ? t("models.oauthAuthorize")
          : t("models.configure")}
      </span>
    </button>
  );
}

interface ModelsInitialData extends ProvidersInitialData {
  templates: ProviderInfo[];
}

interface ModelsPageProps {
  initialData?: ModelsInitialData;
}

export async function loader(): Promise<ModelsInitialData> {
  const [providers, activeModels, templates] = await Promise.all([
    api.listProviders(),
    api.getActiveModels({ scope: "global" }),
    providerApi.listProviderTemplates(),
  ]);
  if (!Array.isArray(providers)) {
    throw new Error("Unexpected provider response");
  }
  return {
    providers,
    activeModels: activeModels ?? null,
    templates,
  };
}

export function RouteComponent() {
  return <ModelsPage initialData={useLoaderData() as ModelsInitialData} />;
}

function ModelsPage({ initialData }: ModelsPageProps) {
  const { t } = useTranslation();
  const { providers, activeModels, loading, error, fetchAll } = useProviders(
    initialData
      ? {
          providers: initialData.providers,
          activeModels: initialData.activeModels,
        }
      : undefined,
  );
  const [templates, setTemplates] = useState<ProviderInfo[]>(
    initialData?.templates ?? [],
  );
  const [hoveredCard, setHoveredCard] = useState<string | null>(null);
  const [addProviderOpen, setAddProviderOpen] = useState(false);
  const [initialTemplateId, setInitialTemplateId] = useState<
    string | undefined
  >(undefined);
  const [searchQuery, setSearchQuery] = useState("");
  const [sortOrder, setSortOrder] = useState<"asc" | "desc">("asc");

  const refreshProvidersSilently = useCallback(() => {
    void fetchAll(false);
  }, [fetchAll]);

  useEffect(() => {
    if (initialData) {
      setTemplates(initialData.templates);
      return;
    }
    providerApi
      .listProviderTemplates()
      .then(setTemplates)
      .catch(() => setTemplates([]));
  }, [initialData]);

  const {
    localProviders,
    freeReadyProviders,
    configuredRemoteProviders,
    templateProviders,
  } = useMemo(() => {
    const query = searchQuery.trim().toLowerCase();
    const filtered = providers.filter((p) => providerMatchesQuery(p, query));
    const nameCmp = (a: ProviderInfo, b: ProviderInfo) => {
      const r = a.name.localeCompare(b.name);
      return sortOrder === "asc" ? r : -r;
    };

    const sortPriority = (provider: ProviderInfo): number => {
      const isConfigured = isProviderConfigured(provider);
      const hasModels = providerHasModels(provider);
      const isAvailable = isConfigured && hasModels;

      if (isAvailable && provider.is_custom) return 0;
      if (isAvailable) return 1;
      if (provider.is_custom) return 2;
      if (isConfigured) return 3;
      return 4;
    };

    const local: ProviderInfo[] = [];
    const freeReady: ProviderInfo[] = [];
    const configuredRemote: ProviderInfo[] = [];
    for (const p of filtered) {
      if (isRustLocalProvider(p)) local.push(p);
      else if (isZeroConfigFreeProvider(p)) freeReady.push(p);
      else configuredRemote.push(p);
    }
    local.sort(nameCmp);
    freeReady.sort(nameCmp);
    configuredRemote.sort(
      (a, b) => sortPriority(a) - sortPriority(b) || nameCmp(a, b),
    );

    const existingProviderIds = new Set(providers.map((p) => p.id));
    const templateList = templates
      .filter((template) => !existingProviderIds.has(template.id))
      .filter((template) => !isRustLocalProvider(template))
      .filter((template) => !isZeroConfigFreeProvider(template))
      .filter((template) => providerMatchesQuery(template, query))
      .sort(nameCmp);

    return {
      localProviders: local,
      freeReadyProviders: freeReady,
      configuredRemoteProviders: configuredRemote,
      templateProviders: templateList,
    };
  }, [providers, templates, searchQuery, sortOrder]);

  const handleMouseEnter = (providerId: string) => {
    setHoveredCard(providerId);
  };

  const handleMouseLeave = () => {
    setHoveredCard(null);
  };

  const renderProviderCards = (list: ProviderInfo[]) =>
    list.map((provider) => (
      <ProviderCard
        key={provider.id}
        provider={provider}
        activeModels={activeModels}
        onSaved={refreshProvidersSilently}
        isHover={hoveredCard === provider.id}
        onMouseEnter={() => handleMouseEnter(provider.id)}
        onMouseLeave={handleMouseLeave}
      />
    ));

  const openProviderTemplate = (templateId?: string) => {
    setInitialTemplateId(templateId);
    setAddProviderOpen(true);
  };

  return (
    <Page>
      {loading ? (
        <LoadingState message={t("models.loading")} />
      ) : error ? (
        <LoadingState message={error} error onRetry={fetchAll} />
      ) : (
        <>
          {}
          <PageHeader
            current={t("models.llmTitle")}
            onRefresh={fetchAll}
            refreshing={loading}
          />
          {}
          <PageBody>
            <ModelsSection
              providers={providers}
              activeModels={activeModels}
              onSaved={fetchAll}
            />
            {}
            <div className={styles.providersBlock}>
              <div className={styles.sectionHeaderRow}>
                <PageHeader
                  current={t("models.providersTitle")}
                  className={styles.providersPageHeader}
                  showMoreMenu={false}
                />
                <div className={styles.headerRight}>
                  {}
                  <div className={styles.searchInput}>
                    <TextInput
                      placeholder={t("models.searchPlaceholder")}
                      value={searchQuery}
                      onChange={(e) => setSearchQuery(e.target.value)}
                      style={{ width: "100%" }}
                    />
                  </div>
                  <Tooltip title={t("models.sortByName")}>
                    <Btn
                      variant="outline"
                      icon={
                        sortOrder === "desc" ? (
                          <ArrowDownWideNarrow />
                        ) : (
                          <ArrowUpNarrowWide />
                        )
                      }
                      onClick={() =>
                        setSortOrder((prev) =>
                          prev === "asc" ? "desc" : "asc",
                        )
                      }
                    />
                  </Tooltip>
                  <Tooltip title={t("models.addProvider")}>
                    <Btn
                      icon={<Plus />}
                      onClick={() => openProviderTemplate()}
                      className={styles.addProviderBtn}
                    />
                  </Tooltip>
                </div>
              </div>

              {freeReadyProviders.length > 0 && (
                <div className={styles.providerGroup}>
                  <div className={styles.providerGroupHeading}>
                    <div>
                      <h4 className={styles.providerGroupTitle}>
                        {t("models.zeroConfigFreeProviders")}
                      </h4>
                      <p className={styles.providerGroupDescription}>
                        {t("models.zeroConfigFreeProvidersDescription")}
                      </p>
                    </div>
                    <Tag color="success" className={styles.freeTag}>
                      {t("models.free")}
                    </Tag>
                  </div>
                  <div className={styles.freeProviderCards}>
                    {freeReadyProviders.map((provider) => (
                      <FreeProviderCard
                        key={provider.id}
                        provider={provider}
                        activeModels={activeModels}
                        onSaved={refreshProvidersSilently}
                      />
                    ))}
                  </div>
                </div>
              )}

              {configuredRemoteProviders.length > 0 && (
                <div className={styles.providerGroup}>
                  <h4 className={styles.providerGroupTitle}>
                    {t("models.configuredProviders")}
                  </h4>
                  <div className={styles.providerCards}>
                    {renderProviderCards(configuredRemoteProviders)}
                  </div>
                </div>
              )}

              {templateProviders.length > 0 && (
                <div className={styles.providerGroup}>
                  <div className={styles.providerGroupHeading}>
                    <div>
                      <h4 className={styles.providerGroupTitle}>
                        {t("models.moreProviders")}
                      </h4>
                      <p className={styles.providerGroupDescription}>
                        {t("models.moreProvidersDescription")}
                      </p>
                    </div>
                  </div>
                  <div className={styles.providerTemplateGrid}>
                    {templateProviders.map((template) => (
                      <ProviderTemplateTile
                        key={template.id}
                        template={template}
                        onConfigure={openProviderTemplate}
                      />
                    ))}
                  </div>
                </div>
              )}

              {localProviders.length > 0 && (
                <div className={styles.providerGroup}>
                  <h4 className={styles.providerGroupTitle}>
                    {t("models.localProviders")}
                  </h4>
                  <div className={styles.providerCards}>
                    {renderProviderCards(localProviders)}
                  </div>
                </div>
              )}
            </div>

            <CustomProviderModal
              open={addProviderOpen}
              initialTemplateId={initialTemplateId}
              onClose={() => setAddProviderOpen(false)}
              onSaved={fetchAll}
            />
          </PageBody>
        </>
      )}
    </Page>
  );
}

export default ModelsPage;
