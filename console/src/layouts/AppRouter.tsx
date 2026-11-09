import {
  Suspense,
  useEffect,
  type PropsWithChildren,
  type ReactNode,
} from "react";
import { BrowserRouter, useLocation } from "react-router-dom";
import { rememberChatRoute } from "@/routes/lastChatRoute";
import { useAgentStore } from "@/stores/agentStore";
import ContentTransitionFrame from "./ContentTransitionFrame";

interface AppRouterProps extends PropsWithChildren {
  basename?: string;
}

export default function AppRouter({ basename, children }: AppRouterProps) {
  return <BrowserRouter basename={basename}>{children}</BrowserRouter>;
}

interface RouteCommitBoundaryProps extends PropsWithChildren {
  fallback: ReactNode;
}

export function RouteCommitBoundary({
  children,
  fallback,
}: RouteCommitBoundaryProps) {
  const location = useLocation();
  const selectedAgent = useAgentStore((state) => state.selectedAgent);

  useEffect(() => {
    rememberChatRoute(location.pathname);
  }, [location.pathname]);

  const routeKey = /^\/agents\/[^/]+\/chats\/[^/]+$/.test(location.pathname)
    ? "/agents/:agentId/chats/:chatId"
    : location.pathname === "/projects" ||
      location.pathname.startsWith("/projects/")
    ? "/projects"
    : location.pathname;
  const transitionKey =
    routeKey === "/agents/:agentId/chats/:chatId" || routeKey === "/projects"
      ? routeKey
      : `${routeKey}:${selectedAgent}`;

  return (
    <Suspense fallback={fallback}>
      <ContentTransitionFrame transitionKey={transitionKey}>
        {children}
      </ContentTransitionFrame>
    </Suspense>
  );
}
