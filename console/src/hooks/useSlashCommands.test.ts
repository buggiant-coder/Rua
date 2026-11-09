import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSlashCommands } from "./useSlashCommands";

const apiConfigMock = vi.hoisted(() => ({
  token: "test-token",
}));

vi.mock("../api/config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
  getApiToken: () => apiConfigMock.token,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      ({
        "chat.commands.status.description": "Daemon status",
        "chat.commands.clear.description": "Clear translated",
      })[key] ??
      options?.defaultValue ??
      key,
  }),
}));

describe("useSlashCommands", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiConfigMock.token = "test-token";
  });

  it("loads command suggestions from Rust command discovery", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify([
          {
            name: "status",
            scope: "daemon",
            help: "Show daemon status",
            agent_required: false,
            subsystem: "daemon",
          },
          {
            name: "clear",
            scope: "conversation",
            help: "Clear current conversation",
            agent_required: false,
          },
          {
            name: "plugin_only",
            scope: "plugin",
            help: "Plugin fallback help",
            agent_required: false,
          },
        ]),
        {
          status: 200,
          headers: { "Content-Type": "application/json" },
        },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => useSlashCommands());

    await waitFor(() =>
      expect(result.current).toEqual([
        {
          command: "/status",
          value: "status",
          description: "Daemon status",
          subsystem: "daemon",
        },
        {
          command: "/clear",
          value: "clear",
          description: "Clear translated",
          subsystem: undefined,
        },
        {
          command: "/plugin_only",
          value: "plugin_only",
          description: "Plugin fallback help",
          subsystem: undefined,
        },
      ]),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/commands",
      expect.objectContaining({
        headers: {
          "Content-Type": "application/json",
          Authorization: "Bearer test-token",
        },
      }),
    );
  });

  it("omits auth when no console token exists", async () => {
    apiConfigMock.token = "";
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify([]), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    renderHook(() => useSlashCommands());

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(fetchMock.mock.calls[0][1].headers).toEqual({
      "Content-Type": "application/json",
    });
  });

  it("keeps an empty list when discovery fails", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response("nope", { status: 500 })),
    );

    const { result } = renderHook(() => useSlashCommands());

    await waitFor(() => expect(fetch).toHaveBeenCalled());
    expect(result.current).toEqual([]);
  });
});
