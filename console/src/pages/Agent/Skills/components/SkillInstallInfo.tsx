import dayjs from "dayjs";
import { useTranslation } from "react-i18next";
import type { InstallMeta } from "../../../../api/types";

interface SkillInstallInfoProps {
  meta: InstallMeta;
}

function optionalText(value: unknown): string | null {
  if (typeof value === "string" && value.trim()) return value;
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  return null;
}

export function SkillInstallInfo({ meta }: SkillInstallInfoProps) {
  const { t } = useTranslation();
  const marketName = optionalText(meta.market_meta?.name);
  const marketSource = optionalText(meta.market_meta?.source);
  const marketSlug = optionalText(meta.market_meta?.slug);
  const marketAuthor = optionalText(meta.market_meta?.author);
  const isMarketInstall = Boolean(meta.market_meta);

  return (
    <div>
      <div style={{ fontWeight: 500, marginBottom: 8 }}>
        {t("skills.installInfo")}
      </div>
      <div
        style={{
          fontSize: 13,
          color: "var(--text-2)",
          display: "flex",
          flexDirection: "column",
          gap: 4,
        }}
      >
        {isMarketInstall && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.installChannel")}:{" "}
            </span>
            {t("skills.installChannelMarket")}
          </div>
        )}
        <div>
          <span style={{ color: "var(--text-3)" }}>
            {t("skills.installFrom")}:{" "}
          </span>
          {meta.installed_from}
        </div>
        {marketSource && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.marketProvider")}:{" "}
            </span>
            {marketSource}
          </div>
        )}
        {marketName && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.marketName")}:{" "}
            </span>
            {marketName}
          </div>
        )}
        {marketSlug && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.marketSlug")}:{" "}
            </span>
            {marketSlug}
          </div>
        )}
        {meta.installed_version && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.installVersion")}:{" "}
            </span>
            {meta.installed_version}
          </div>
        )}
        {meta.installed_at && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.installedAt")}:{" "}
            </span>
            {dayjs(meta.installed_at).format("YYYY-MM-DD HH:mm:ss")}
          </div>
        )}
        {meta.updated_at && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.updatedAt")}:{" "}
            </span>
            {dayjs(meta.updated_at).format("YYYY-MM-DD HH:mm:ss")}
          </div>
        )}
        {marketAuthor && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.author")}:{" "}
            </span>
            {marketAuthor}
          </div>
        )}
        {meta.source_url && (
          <div>
            <span style={{ color: "var(--text-3)" }}>
              {t("skills.sourceUrl")}:{" "}
            </span>
            <a href={meta.source_url} rel="noreferrer" target="_blank">
              {meta.source_url}
            </a>
          </div>
        )}
      </div>
    </div>
  );
}
