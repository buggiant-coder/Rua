import { useEffect, useState } from "react";
import { Outlet, useLocation, useNavigate } from "react-router-dom";

import Skeleton from "../../components/Skeleton";

import { Menu } from "lucide-react";

import TerminalRuntimeHost from "../../pages/Terminal/TerminalRuntimeHost";

import NavigationPanel from "../NavigationPanel";
import { navigateWithPageTransition } from "../pageTransition";
import { RouteCommitBoundary } from "../AppRouter";
import { cancelPreparedChatTransition } from "../../pages/Chat/preparedChatTransition";

import styles from "./index.module.less";
import { Drawer } from "@/ui";

export default function MobileLayout() {
  const location = useLocation();
  const navigate = useNavigate();
  const [drawerLocationKey, setDrawerLocationKey] = useState<string | null>(
    null,
  );
  const drawerOpen = drawerLocationKey === location.key;

  useEffect(() => {
    document.documentElement.classList.add("mobile-mode");
  }, []);

  useEffect(() => {
    const root = document.documentElement;
    const viewport = window.visualViewport;
    const syncVisualViewport = () => {
      const height = viewport?.height ?? window.innerHeight;
      if (Number.isFinite(height) && height > 0) {
        root.style.setProperty(
          "--rua-mobile-visual-height",
          `${Math.round(height)}px`,
        );
      }
    };

    syncVisualViewport();
    viewport?.addEventListener("resize", syncVisualViewport);
    viewport?.addEventListener("scroll", syncVisualViewport);
    window.addEventListener("resize", syncVisualViewport);
    return () => {
      viewport?.removeEventListener("resize", syncVisualViewport);
      viewport?.removeEventListener("scroll", syncVisualViewport);
      window.removeEventListener("resize", syncVisualViewport);
      root.style.removeProperty("--rua-mobile-visual-height");
    };
  }, []);

  const handleNav = (path: string) => {
    cancelPreparedChatTransition();
    if (path === `${location.pathname}${location.search}`) {
      setDrawerLocationKey(null);
      return;
    }
    navigateWithPageTransition(navigate, path);
  };

  return (
    <div className={styles.root}>
      {}
      <button
        type="button"
        className={styles.floatingHamburger}
        onClick={() => setDrawerLocationKey(location.key)}
        aria-label="menu"
      >
        <Menu size={22} />
      </button>

      <Drawer
        placement="left"
        width="min(288px, calc(100vw - 32px))"
        open={drawerOpen}
        onClose={() => setDrawerLocationKey(null)}
        closable={false}
        className={`${styles.drawerRoot} rua-nav-drawer`}
        styles={{ body: { padding: 0 } }}
      >
        <NavigationPanel location={location} onNavigate={handleNav} />
      </Drawer>

      <main className={styles.content}>
        <TerminalRuntimeHost
          active={location.pathname === "/terminal"}
          mobile
        />
        <RouteCommitBoundary
          fallback={
            <div className={styles.suspenseFallback}>
              <Skeleton variant="list" count={5} />
            </div>
          }
        >
          <Outlet />
        </RouteCommitBoundary>
      </main>
    </div>
  );
}
