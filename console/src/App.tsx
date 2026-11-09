import {
  Navigate,
  Outlet,
  RouterProvider,
  createBrowserRouter,
} from "react-router-dom";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useTranslation } from "react-i18next";
import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";
import "dayjs/locale/zh-cn";
import "dayjs/locale/ja";
import "dayjs/locale/ru";
dayjs.extend(relativeTime);
import MainLayout from "./layouts/MainLayout";
import { RouteActivityProvider } from "./routes/RouteActivity";
import { ThemeProvider } from "./contexts/ThemeContext";
import LoginPage from "./pages/Login";
import { languageApi } from "./api/modules/language";
import "./styles/theme.css";
import "./styles/layout.css";
import "./styles/form-override.css";
import "./styles/mobile.css";
import "./styles/skin-industrial.css";
import "./styles/skin-standard.css";
import "./styles/skin-cute.css";
import "./styles/skin-pill.css";
import { useAppLifecycle } from "./hooks/useAppLifecycle";
import { useSyncConflictToast } from "./hooks/useSyncConflictToast";
import { useUserNotifications } from "./hooks/useUserNotifications";
import { ModalProvider, ToastProvider } from "./ui";
import { appRoutes } from "./routes/appRoutes";
import { verifyRouteAccess } from "./routes/routeAuth";
import { isMobile } from "./utils/isMobile";
import { syncDesktopLocale } from "./desktop/runtime";

const dayjsLocaleMap: Record<string, string> = {
  zh: "zh-cn",
  en: "en",
  ja: "ja",
  ru: "ru",
};

function AuthGuard() {
  const [status, setStatus] = useState<"loading" | "auth-required" | "ok">(
    "loading",
  );

  useEffect(() => {
    let cancelled = false;
    let retryTimer: number | undefined;
    const verify = () => {
      void verifyRouteAccess()
        .then((access) => {
          if (!cancelled) setStatus(access);
        })
        .catch(() => {
          if (!cancelled) retryTimer = window.setTimeout(verify, 500);
        });
    };
    verify();
    return () => {
      cancelled = true;
      if (retryTimer !== undefined) window.clearTimeout(retryTimer);
    };
  }, []);

  if (status === "loading") return null;
  if (status === "auth-required")
    return (
      <Navigate
        to={`/login?redirect=${encodeURIComponent(window.location.pathname)}`}
        replace
      />
    );
  return <Outlet />;
}

function SyncConflictToastMount() {
  useSyncConflictToast();
  return null;
}

function AppRouteShell() {
  return (
    <>
      <SyncConflictToastMount />
      <RouteActivityProvider>
        <Outlet />
      </RouteActivityProvider>
    </>
  );
}

function AppBootFallback() {
  return (
    <div
      aria-hidden="true"
      style={{ width: "100%", height: "100%", background: "var(--bg)" }}
    />
  );
}

function InitializedRouter({
  router,
}: {
  router: ReturnType<typeof createBrowserRouter>;
}) {
  const subscribe = useCallback(
    (onStoreChange: () => void) =>
      router.subscribe(() => {
        onStoreChange();
      }),
    [router],
  );
  const initialized = useSyncExternalStore(
    subscribe,
    () => router.state.initialized,
  );

  if (!initialized) return <AppBootFallback />;
  return <RouterProvider router={router} useTransitions={false} />;
}

function getRouterBasename(pathname: string): string | undefined {
  return /^\/console(?:\/|$)/.test(pathname) ? "/console" : undefined;
}

function AppInner() {
  useAppLifecycle();
  useUserNotifications();
  isMobile();
  const basename = getRouterBasename(window.location.pathname);
  const routerRef = useRef<ReturnType<typeof createBrowserRouter> | null>(null);
  const { i18n } = useTranslation();
  const lang = i18n.resolvedLanguage || i18n.language || "zh";

  useEffect(() => {
    if (!localStorage.getItem("language")) {
      languageApi
        .getLanguage()
        .then(({ language }) => {
          if (language && language !== i18n.language) {
            i18n.changeLanguage(language);
            localStorage.setItem("language", language);
          }
        })
        .catch((err) =>
          console.error("Failed to fetch language preference:", err),
        );
    }
  }, [i18n]);

  useEffect(() => {
    const handleLanguageChanged = (lng: string) => {
      const shortLng = lng.split("-")[0];
      dayjs.locale(dayjsLocaleMap[shortLng] ?? "en");
      void syncDesktopLocale(lng).catch((error) =>
        console.error("Failed to sync Desktop Shell language:", error),
      );
    };

    dayjs.locale(dayjsLocaleMap[lang.split("-")[0]] ?? "en");
    void syncDesktopLocale(lang).catch((error) =>
      console.error("Failed to sync Desktop Shell language:", error),
    );

    i18n.on("languageChanged", handleLanguageChanged);
    return () => {
      i18n.off("languageChanged", handleLanguageChanged);
    };
  }, [i18n, lang]);

  if (routerRef.current === null) {
    routerRef.current = createBrowserRouter(
      [
        {
          element: <AppRouteShell />,
          children: [
            { path: "/login", element: <LoginPage /> },
            {
              element: <AuthGuard />,
              children: [{ element: <MainLayout />, children: appRoutes }],
            },
          ],
        },
      ],
      { basename },
    );
  }

  return (
    <ModalProvider>
      <InitializedRouter router={routerRef.current} />
    </ModalProvider>
  );
}

function App() {
  return (
    <ThemeProvider>
      <ToastProvider>
        <AppInner />
      </ToastProvider>
    </ThemeProvider>
  );
}

export default App;
