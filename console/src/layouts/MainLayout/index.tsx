import { useEffect, useState } from "react";
import { useLocation, useNavigate, Navigate, Outlet } from "react-router-dom";
import { Menu } from "lucide-react";
import Sidebar from "../Sidebar";
import MobileLayout from "../MobileLayout";
import Skeleton from "../../components/Skeleton";
import { isMobile } from "../../utils/isMobile";
import { devicesApi, syncApi } from "../../api/modules/sync";

import TerminalRuntimeHost from "../../pages/Terminal/TerminalRuntimeHost";
import { RouteCommitBoundary } from "../AppRouter";
import { navigateWithPageTransition } from "../pageTransition";
import { cancelPreparedChatTransition } from "../../pages/Chat/preparedChatTransition";
import { useAgentStore } from "../../stores/agentStore";
import styles from "../index.module.less";
import { useNavigationActivity } from "../useNavigationActivity";

export default function MainLayout() {
  useNavigationActivity();
  useEffect(() => {
    void useAgentStore
      .getState()
      .ensureCatalogLoaded()
      .catch((error) => console.error("Failed to preload agents:", error));
  }, []);

  if (isMobile()) {
    return <MobileLayout />;
  }

  return <DesktopMainLayout />;
}

function DesktopMainLayout() {
  const location = useLocation();
  const navigate = useNavigate();
  const currentPath = location.pathname;
  const [isRevoked, setIsRevoked] = useState(false);

  const [sidebarOpen, setSidebarOpen] = useState(() => {
    try {
      return localStorage.getItem("rua_sider_collapsed") !== "true";
    } catch {
      return true;
    }
  });

  const toggleSidebar = () => {
    const next = !sidebarOpen;
    setSidebarOpen(next);
    try {
      localStorage.setItem("rua_sider_collapsed", String(!next));
    } catch {}
  };

  const handleNavigate = (path: string) => {
    if (path === `${location.pathname}${location.search}`) return;
    cancelPreparedChatTransition();
    navigateWithPageTransition(navigate, path);
  };

  useEffect(() => {
    let cancelled = false;
    let interval: ReturnType<typeof setInterval> | null = null;
    const check = async () => {
      try {
        const status = await devicesApi.getDeviceStatus();
        if (!cancelled && status.is_revoked) {
          setIsRevoked(true);
        }
      } catch {}
    };
    const start = async () => {
      try {
        const status = await syncApi.getStatus();
        if (cancelled || !status.enabled) return;
        await check();
        if (!cancelled) {
          interval = setInterval(check, 60000);
        }
      } catch {}
    };
    start();
    return () => {
      cancelled = true;
      if (interval) clearInterval(interval);
    };
  }, []);

  useEffect(() => {
    document.documentElement.style.setProperty("--header-height", "0px");
  }, []);

  if (isRevoked && currentPath !== "/device-revoked") {
    return <Navigate to="/device-revoked" replace />;
  }
  return (
    <div
      className={`${styles.mainLayout} desktop-mobile-shell ${
        sidebarOpen ? styles.sidebarOpen : ""
      }`}
    >
      <button
        type="button"
        className={styles.desktopHamburger}
        onClick={toggleSidebar}
        aria-label="menu"
        aria-expanded={sidebarOpen}
        aria-controls="desktop-navigation"
      >
        <Menu size={22} />
      </button>

      <div className={styles.layoutBody}>
        <Sidebar
          location={location}
          onNavigate={handleNavigate}
          open={sidebarOpen}
        />
        <main className="page-container">
          <div className="page-content">
            <TerminalRuntimeHost active={currentPath === "/terminal"} />
            <RouteCommitBoundary
              fallback={<Skeleton variant="list" count={6} />}
            >
              <Outlet />
            </RouteCommitBoundary>
          </div>
        </main>
      </div>
    </div>
  );
}
