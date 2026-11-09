/* eslint-disable react-refresh/only-export-components */
import type { ComponentType } from "react";
import {
  isRouteErrorResponse,
  redirect,
  useRevalidator,
  useRouteError,
  type LoaderFunction,
  type RouteObject,
} from "react-router-dom";
import { EmptyState } from "@/components/EmptyState";
import { Page, PageBody } from "@/components/Page";
import { requireRouteAccess } from "./routeAuth";
import { landingLoader } from "./landingRoute";

function RouteLoadError() {
  const error = useRouteError();
  const revalidator = useRevalidator();
  const description = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
    ? error.message
    : String(error);

  return (
    <Page>
      <PageBody>
        <EmptyState
          variant="error"
          description={description}
          onRetry={() => void revalidator.revalidate()}
        />
      </PageBody>
    </Page>
  );
}

function lazyPage(
  load: () => Promise<{ default: ComponentType }>,
): RouteObject["lazy"] {
  return async () => {
    const module = await load();
    return { Component: module.default };
  };
}

function lazyDataPage(
  load: () => Promise<{
    RouteComponent: ComponentType;
    loader: LoaderFunction;
  }>,
): RouteObject["lazy"] {
  return async () => {
    const module = await load();
    return {
      Component: module.RouteComponent,
      loader: async (args) => {
        await requireRouteAccess(args.request);
        return module.loader(args);
      },
      ErrorBoundary: RouteLoadError,
    };
  };
}

function redirectTo(to: string): RouteObject["loader"] {
  return () => redirect(to);
}

export const appRoutes: RouteObject[] = [
  { index: true, loader: landingLoader },
  {
    path: "agents/:agentId/chats/:chatId",
    lazy: lazyPage(() => import("../pages/Chat")),
  },
  {
    path: "projects/:projectId/chats/:chatId",
    lazy: lazyDataPage(() => import("../pages/Projects/ProjectChat")),
  },
  {
    path: "projects",
    lazy: lazyDataPage(() => import("../pages/Projects")),
  },
  {
    path: "storage",
    lazy: lazyPage(() => import("../pages/Storage")),
  },
  {
    path: "channels",
    lazy: lazyDataPage(() => import("../pages/Control/Channels")),
  },
  {
    path: "sessions",
    lazy: lazyDataPage(() => import("../pages/Control/Sessions")),
  },
  {
    path: "cron-jobs",
    lazy: lazyDataPage(() => import("../pages/Control/CronJobs")),
  },
  {
    path: "heartbeat",
    lazy: lazyDataPage(() => import("../pages/Control/Heartbeat")),
  },
  {
    path: "prompts",
    lazy: lazyPage(() => import("../pages/Agent/PromptsHub")),
  },
  { path: "workspace", loader: redirectTo("/prompts") },
  {
    path: "prompt-library",
    loader: redirectTo("/prompts?tab=library"),
  },
  {
    path: "skills",
    lazy: lazyPage(() => import("../pages/Agent/SkillsHub")),
  },
  { path: "skill-pool", loader: redirectTo("/skills?tab=pool") },
  { path: "market", loader: redirectTo("/skills?tab=market") },
  { path: "shared-mcp", loader: redirectTo("/mcp?tab=shared") },
  {
    path: "tools",
    lazy: lazyDataPage(() => import("../pages/Agent/Tools")),
  },
  {
    path: "desktop-tools",
    lazy: lazyPage(() => import("../pages/DesktopTools")),
  },
  {
    path: "hooks",
    lazy: lazyDataPage(() => import("../pages/Agent/Hooks")),
  },
  {
    path: "memory",
    lazy: lazyDataPage(() => import("../pages/Agent/Memory")),
  },
  {
    path: "mcp",
    lazy: lazyPage(() => import("../pages/Agent/MCPHub")),
  },
  {
    path: "acp",
    lazy: lazyPage(() => import("../pages/Agent/ACP")),
  },
  { path: "terminal", element: null },
  {
    path: "agents",
    lazy: lazyDataPage(() => import("../pages/Settings/Agents")),
  },
  { path: "agent-templates", loader: redirectTo("/agents?tab=templates") },
  {
    path: "models",
    lazy: lazyDataPage(() => import("../pages/Settings/Models")),
  },
  {
    path: "environments",
    lazy: lazyDataPage(() => import("../pages/Settings/Environments")),
  },
  {
    path: "agent-config",
    lazy: lazyDataPage(() => import("../pages/Agent/Config")),
  },
  {
    path: "security",
    lazy: lazyDataPage(() => import("../pages/Settings/Security")),
  },
  {
    path: "token-usage",
    lazy: lazyDataPage(() => import("../pages/Settings/TokenUsage")),
  },
  {
    path: "voice-transcription",
    lazy: lazyDataPage(() => import("../pages/Settings/VoiceTranscription")),
  },
  {
    path: "plugins",
    lazy: lazyDataPage(() => import("../pages/Settings/Plugins")),
  },
  {
    path: "debug",
    lazy: lazyPage(() => import("../pages/Debug")),
  },
  {
    path: "settings/appearance",
    lazy: lazyPage(() => import("../pages/Settings/Appearance")),
  },
  { path: "settings/desktop", loader: redirectTo("/desktop-tools") },
  {
    path: "about",
    lazy: lazyPage(() => import("../pages/About")),
  },
  {
    path: "settings/device-sync",
    lazy: lazyPage(() => import("../pages/Settings/DeviceSync")),
  },
  {
    path: "settings/airdrop-transfer",
    lazy: lazyPage(() => import("../pages/Settings/AirdropTransfer")),
  },
  { path: "settings/devices", loader: redirectTo("/settings/device-sync") },
  { path: "settings/media", loader: redirectTo("/settings/device-sync") },
  { path: "settings/sync", loader: redirectTo("/settings/device-sync") },
  {
    path: "settings/sync/conflicts",
    lazy: lazyPage(() => import("../pages/Settings/SyncConflicts")),
  },
  {
    path: "security/master-export",
    loader: redirectTo("/settings/device-sync"),
  },
  { path: "devices/add", loader: redirectTo("/settings/device-sync") },
  {
    path: "join",
    lazy: lazyPage(() => import("../pages/Onboarding/JoinDevices")),
  },
  {
    path: "device-revoked",
    lazy: lazyPage(() => import("../pages/DeviceRevoked")),
  },
  {
    path: "*",
    lazy: lazyPage(() => import("../pages/NotFound")),
  },
];
