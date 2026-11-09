import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { useTheme } from "../../../contexts/ThemeContext";
import {
  COLOR_SCHEME_NAMES,
  SKIN_NAMES,
  colorSchemeMeta,
  skinConfigs,
  skinMeta,
  type ColorSchemeName,
  type SkinName,
} from "../../../styles/skin-configs";
import styles from "./index.module.less";
import { Check } from "lucide-react";

export default function AppearancePage() {
  const { t } = useTranslation();
  const { colorScheme, setColorScheme, skin, setSkin } = useTheme();

  return (
    <Page>
      <PageHeader current={t("appearance.title")} />
      <PageBody>
        <section className={styles.section}>
          <div className={styles.sectionHeader}>
            <h2>{t("appearance.colorSchemeTitle")}</h2>
            <p>{t("appearance.colorSchemeDesc")}</p>
          </div>
          <div className={styles.grid}>
            {COLOR_SCHEME_NAMES.map((name: ColorSchemeName) => {
              const meta = colorSchemeMeta[name];
              const active = colorScheme === name;
              return (
                <button
                  key={name}
                  data-color-scheme={name}
                  className={`${styles.card} ${
                    active ? styles.cardActive : ""
                  }`}
                  aria-pressed={active}
                  onClick={() => setColorScheme(name)}
                >
                  <div
                    className={styles.colorPreview}
                    style={{ background: meta.preview.surface }}
                  >
                    <span
                      className={styles.colorSwatchMain}
                      style={{ background: meta.preview.primary }}
                    />
                    <span
                      className={styles.colorSwatchAccent}
                      style={{ background: meta.preview.accent }}
                    />
                  </div>
                  <div className={styles.info}>
                    <span className={styles.name}>{t(meta.nameKey)}</span>
                    <span className={styles.desc}>{t(meta.descKey)}</span>
                  </div>
                  {active && (
                    <span className={styles.check}>
                      <Check />
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </section>

        <section className={styles.section}>
          <div className={styles.sectionHeader}>
            <h2>{t("appearance.shapeSkinTitle")}</h2>
            <p>{t("appearance.shapeSkinDesc")}</p>
          </div>
          <div className={styles.grid}>
            {SKIN_NAMES.map((name: SkinName) => {
              const meta = skinMeta[name];
              const active = skin === name;
              return (
                <button
                  key={name}
                  data-skin={name}
                  className={`${styles.card} ${
                    active ? styles.cardActive : ""
                  }`}
                  aria-pressed={active}
                  onClick={() => setSkin(name)}
                >
                  <div
                    className={styles.preview}
                    style={{ background: meta.preview }}
                  >
                    <div
                      className={styles.swatch}
                      style={{
                        borderRadius: skinConfigs[name].borderRadius,
                      }}
                    />
                    <div
                      className={styles.swatchSmall}
                      style={{
                        borderRadius: skinConfigs[name].borderRadiusSM,
                      }}
                    />
                  </div>
                  <div className={styles.info}>
                    <span className={styles.name}>{t(meta.nameKey)}</span>
                    <span className={styles.desc}>{t(meta.descKey)}</span>
                  </div>
                  {active && (
                    <span className={styles.check}>
                      <Check />
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </section>
      </PageBody>
    </Page>
  );
}
