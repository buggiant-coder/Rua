import { Monitor, Moon, Sun } from "lucide-react";
import { useTheme, type ThemeMode } from "../../contexts/ThemeContext";
import { useTranslation } from "react-i18next";
import type { ReactNode } from "react";
import styles from "./index.module.less";
import { Btn, Dropdown, type MenuProps } from "@/ui";
const ICONS: Record<ThemeMode, ReactNode> = {
  light: <Sun />,
  dark: <Moon />,
  system: <Monitor />,
};

export default function ThemeToggleButton() {
  const { themeMode, isDark, setThemeMode } = useTheme();
  const { t } = useTranslation();

  const items: MenuProps["items"] = [
    {
      key: "light",
      label: t("theme.light"),
      onClick: () => setThemeMode("light"),
    },
    {
      key: "dark",
      label: t("theme.dark"),
      onClick: () => setThemeMode("dark"),
    },
    {
      key: "system",
      label: t("theme.system"),
      onClick: () => setThemeMode("system"),
    },
  ];

  const icon =
    themeMode === "system" ? ICONS.system : ICONS[isDark ? "dark" : "light"];

  return (
    <Dropdown
      menu={{ items, selectedKeys: [themeMode] }}
      placement="bottom-end"
      overlayClassName={styles.themeDropdown}
    >
      <Btn className={styles.toggleBtn} variant="ghost" icon={icon} size="sm" />
    </Dropdown>
  );
}
