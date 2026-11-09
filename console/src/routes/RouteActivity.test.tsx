import { useEffect } from "react";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { createMemoryRouter, Outlet, RouterProvider } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useConversationActivityWake } from "@/pages/Chat/owned/useConversationActivityWake";
import { useMultiDeviceFeatureAvailability } from "@/hooks/useMultiDeviceFeatureAvailability";
import { RouteActivityProvider, useRouteViewerVisible } from "./RouteActivity";

const deviceWatch = vi.hoisted(() => ({ close: vi.fn(), open: vi.fn() }));
vi.mock("@/api/config", () => ({ getApiUrl: (path: string) => `/api${path}` }));
vi.mock("@/api/authHeaders", () => ({ buildAuthHeaders: () => ({}) }));
vi.mock("@/api/modules/sync", () => ({
  devicesApi: {
    listDevices: async () => [{ is_self: true }, { is_self: false }],
    watchChanges: () => {
      deviceWatch.open();
      return deviceWatch.close;
    },
  },
  syncApi: { getStatus: async () => ({ enabled: true, paired: true }) },
}));

function Viewer({ visibility }: { visibility: (visible: boolean) => void }) {
  useConversationActivityWake(
    { scope: "agent", agentId: "default" },
    "old-chat",
  );
  useMultiDeviceFeatureAvailability();
  const visible = useRouteViewerVisible();
  useEffect(() => visibility(visible), [visibility, visible]);
  return <input aria-label="Draft" defaultValue="Keep this draft" />;
}

describe("route viewer lifecycle", () => {
  let signals: AbortSignal[];
  beforeEach(() => {
    signals = [];
    vi.clearAllMocks();
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn((_url, options: RequestInit) => {
        const signal = options.signal as AbortSignal;
        signals.push(signal);
        return new Promise((_resolve, reject) => {
          signal.addEventListener("abort", () =>
            reject(new DOMException("Aborted", "AbortError")),
          );
        });
      }),
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  function setup(
    loader: () => Promise<null>,
    initialPath = "/agents/default/chats/old-chat",
  ) {
    const visibility = vi.fn();
    const router = createMemoryRouter(
      [
        {
          element: (
            <RouteActivityProvider>
              <Outlet />
            </RouteActivityProvider>
          ),
          children: [
            { path: initialPath, element: <Viewer visibility={visibility} /> },
            {
              path: "/projects",
              loader,
              element: <div>Project destination</div>,
            },
          ],
        },
      ],
      { initialEntries: [initialPath] },
    );
    render(<RouterProvider router={router} useTransitions={false} />);
    return { router, visibility };
  }

  it.each([
    "/agents/default/chats/old-chat",
    "/projects/old-project/chats/old-chat",
  ])(
    "releases viewers before the destination loader can complete from %s",
    async (initialPath) => {
      let finishLoader: () => void = () => {};
      const { router, visibility } = setup(
        () =>
          new Promise<null>((resolve) => {
            // Model a saturated HTTP connection pool: loading cannot finish until
            // the current viewer gives up its connection.
            signals[0].addEventListener("abort", () => {
              finishLoader = () => resolve(null);
            });
          }),
        initialPath,
      );
      await waitFor(() => expect(deviceWatch.open).toHaveBeenCalledOnce());
      fireEvent.change(screen.getByLabelText("Draft"), {
        target: { value: "Unsaved input" },
      });
      let navigation: Promise<void>;
      act(() => {
        navigation = router.navigate("/projects");
      });
      await waitFor(() => expect(signals[0].aborted).toBe(true));
      expect(visibility).toHaveBeenLastCalledWith(false);
      expect(deviceWatch.close).toHaveBeenCalledOnce();
      expect(screen.getByLabelText("Draft")).toHaveValue("Unsaved input");
      expect(screen.queryByText("Project destination")).not.toBeInTheDocument();
      await act(async () => {
        finishLoader();
        await navigation;
      });
      expect(screen.getByText("Project destination")).toBeVisible();
      router.dispose();
    },
  );

  it("restores viewers and preserves the draft when navigation is cancelled", async () => {
    const { router, visibility } = setup(() => new Promise(() => {}));
    await waitFor(() => expect(deviceWatch.open).toHaveBeenCalledOnce());
    act(() => {
      void router.navigate("/projects");
    });
    await waitFor(() => expect(signals[0].aborted).toBe(true));
    await act(async () => {
      await router.navigate("/agents/default/chats/old-chat");
    });
    await waitFor(() => expect(signals).toHaveLength(2));
    expect(signals[1].aborted).toBe(false);
    expect(visibility).toHaveBeenLastCalledWith(true);
    expect(deviceWatch.open).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText("Draft")).toHaveValue("Keep this draft");
    router.dispose();
  });

  it("keeps the current viewer connected for query-only navigation", async () => {
    const { router } = setup(async () => null);
    await waitFor(() => expect(signals).toHaveLength(1));
    await act(async () => {
      await router.navigate("/agents/default/chats/old-chat?filter=all");
    });
    expect(signals).toHaveLength(1);
    expect(signals[0].aborted).toBe(false);
    router.dispose();
  });
});
