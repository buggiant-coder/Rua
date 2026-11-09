import { useEffect, useRef, useState } from "react";
import { ArrowLeft, MoreHorizontal } from "lucide-react";
import { useTranslation } from "react-i18next";

import LanguageSwitcher from "../components/LanguageSwitcher";
import SyncNowButton from "../components/SyncNowButton";
import ThemeToggleButton from "../components/ThemeToggleButton";
import { NAV_GROUPS, type NavItem } from "./navGroups";
import ConversationNavigation from "./ConversationNavigation";
import { resolveAgentChatPath } from "@/pages/Chat/conversationAddress";
import { useAgentStore } from "@/stores/agentStore";
import styles from "./NavigationPanel.module.less";

interface NavigationLocation {
  pathname: string;
  search: string;
}
interface NavigationPanelProps {
  location: NavigationLocation;
  onNavigate: (path: string) => void;
}

const isConversationPath = (path: string) =>
  /^\/(agents|projects)\/[^/]+\/chats\/[^/]+$/.test(path);
const quickKeys = new Set(["terminal", "airdrop-transfer"]);
const quickItems = NAV_GROUPS.flatMap((group) => group.items).filter((item) =>
  quickKeys.has(item.key),
);
const moreGroups = NAV_GROUPS.map((group) => ({
  ...group,
  items: group.items.filter(
    (item) =>
      item.key !== "chat" &&
      item.key !== "projects" &&
      !quickKeys.has(item.key),
  ),
})).filter((group) => group.items.length > 0);

function isNavItemActive(
  path: string | null,
  location: NavigationLocation,
  key: string,
) {
  if (!path) return false;
  const [pathname, search = ""] = path.split("?");
  const tab = new URLSearchParams(search).get("tab");
  return (
    (location.pathname === pathname ||
      (key !== "agents" && location.pathname.startsWith(pathname + "/"))) &&
    (!tab || new URLSearchParams(location.search).get("tab") === tab)
  );
}

export default function NavigationPanel({
  location,
  onNavigate,
}: NavigationPanelProps) {
  const { t } = useTranslation();
  const selectedAgent = useAgentStore((state) => state.selectedAgent);
  const [mode, setMode] = useState<"chats" | "more">(() => {
    try {
      return sessionStorage.getItem("rua_navigation_mode") === "more"
        ? "more"
        : "chats";
    } catch {
      return "chats";
    }
  });
  const lastChatPath = useRef<string | null>(null);
  if (lastChatPath.current === null) {
    try {
      const saved = sessionStorage.getItem("rua_navigation_last_chat");
      if (saved && isConversationPath(saved)) lastChatPath.current = saved;
    } catch {
      // Navigation preferences are optional when storage is unavailable.
    }
  }
  const previousPath = useRef(location.pathname);

  useEffect(() => {
    if (isConversationPath(location.pathname)) {
      lastChatPath.current = location.pathname;
      try {
        sessionStorage.setItem("rua_navigation_last_chat", location.pathname);
      } catch {
        // Navigation preferences are optional when storage is unavailable.
      }
      if (previousPath.current !== location.pathname) setMode("chats");
    }
    previousPath.current = location.pathname;
  }, [location.pathname]);

  useEffect(() => {
    try {
      sessionStorage.setItem("rua_navigation_mode", mode);
    } catch {
      // Navigation preferences are optional when storage is unavailable.
    }
  }, [mode]);

  const renderItem = (item: NavItem) => (
    <button
      type="button"
      key={item.key}
      className={`${styles.navItem} ${
        isNavItemActive(item.path, location, item.key)
          ? styles.navItemActive
          : ""
      }`}
      onClick={() => item.path && onNavigate(item.path)}
    >
      <span className={styles.navIcon}>{item.icon}</span>
      <span className={styles.navLabel}>{t(item.labelKey)}</span>
    </button>
  );

  return (
    <div className={styles.panel}>
      <div className={styles.header}>
        <span className={styles.brand}>Rua</span>
        <div className={styles.actions}>
          <SyncNowButton />
          <LanguageSwitcher />
          <ThemeToggleButton />
        </div>
      </div>
      <div className={styles.conversations} hidden={mode !== "chats"}>
        <ConversationNavigation location={location} onNavigate={onNavigate} />
      </div>
      <div className={styles.moreNavigation} hidden={mode !== "more"}>
        <nav
          className={styles.navMenu}
          aria-label={t("navigation.allFunctions")}
        >
          {moreGroups.map((group) => (
            <div key={group.key} className={styles.navGroup}>
              {group.labelKey && (
                <div className={styles.groupLabel}>{t(group.labelKey)}</div>
              )}
              {group.items.map(renderItem)}
            </div>
          ))}
        </nav>
      </div>
      <div className={styles.footer}>
        {mode === "chats" && quickItems.map(renderItem)}
        <button
          type="button"
          className={styles.navItem}
          aria-expanded={mode === "more"}
          onClick={() => {
            if (mode === "chats") {
              setMode("more");
              return;
            }
            setMode("chats");
            if (!isConversationPath(location.pathname)) {
              onNavigate(
                lastChatPath.current ?? resolveAgentChatPath(selectedAgent),
              );
            }
          }}
        >
          {mode === "more" ? (
            <ArrowLeft size={18} />
          ) : (
            <MoreHorizontal size={18} />
          )}
          <span>
            {t(
              mode === "more"
                ? "navigation.backToConversations"
                : "common.more",
            )}
          </span>
        </button>
      </div>
    </div>
  );
}
