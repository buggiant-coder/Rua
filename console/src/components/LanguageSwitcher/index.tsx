import { useTranslation } from "react-i18next";
import { languageApi } from "../../api/modules/language";
import styles from "./index.module.less";
import { Btn, Dropdown, type MenuProps } from "@/ui";

const LANGUAGE_MARKS: Record<string, string> = {
  en: "En",
  zh: "中",
  ja: "日",
  ru: "Ру",
};

function LanguageGlyphIcon({ lang }: { lang: string }) {
  const mark = LANGUAGE_MARKS[lang] ?? LANGUAGE_MARKS.en;
  const isWide = mark.length > 1;

  if (lang === "zh") {
    return (
      <svg
        width="1em"
        height="1em"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <rect x="5.5" y="8" width="13" height="8" rx="2.4" />
        <path d="M12 4.5v15" />
      </svg>
    );
  }

  if (lang === "ja") {
    return (
      <svg
        width="1em"
        height="1em"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <rect x="6" y="5.5" width="12" height="13" rx="2.8" />
        <path d="M6 12h12" />
      </svg>
    );
  }

  return (
    <svg
      width="1em"
      height="1em"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <text
        x="12"
        y={isWide ? "17" : "17.7"}
        textAnchor="middle"
        fill="currentColor"
        fontFamily="var(--font-sans)"
        fontSize={isWide ? "12.5" : "15.5"}
        fontWeight="750"
        letterSpacing="0"
      >
        {mark}
      </text>
    </svg>
  );
}

export default function LanguageSwitcher() {
  const { i18n } = useTranslation();

  const currentLanguage = i18n.resolvedLanguage || i18n.language;
  const currentLangKey = currentLanguage.split("-")[0];
  const activeLangKey =
    currentLangKey === "zh" ||
    currentLangKey === "ja" ||
    currentLangKey === "ru"
      ? currentLangKey
      : "en";

  const changeLanguage = (lang: string) => {
    i18n.changeLanguage(lang);
    localStorage.setItem("language", lang);
    languageApi
      .updateLanguage(lang)
      .catch((err) =>
        console.error("Failed to save language preference:", err),
      );
  };

  const items: MenuProps["items"] = [
    {
      key: "en",
      label: "English",
      onClick: () => changeLanguage("en"),
    },
    {
      key: "zh",
      label: "简体中文",
      onClick: () => changeLanguage("zh"),
    },
    {
      key: "ja",
      label: "日本語",
      onClick: () => changeLanguage("ja"),
    },
    {
      key: "ru",
      label: "Русский",
      onClick: () => changeLanguage("ru"),
    },
  ];

  return (
    <Dropdown
      menu={{ items, selectedKeys: [currentLangKey] }}
      placement="bottom-end"
      overlayClassName={styles.languageDropdown}
    >
      <Btn
        className={styles.languageBtn}
        icon={<LanguageGlyphIcon lang={activeLangKey} />}
        variant="ghost"
        size="sm"
      />
    </Dropdown>
  );
}
