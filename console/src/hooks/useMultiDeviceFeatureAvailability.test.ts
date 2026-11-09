import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DeviceDto } from "../api/modules/sync";
import {
  isMultiDeviceFeatureAvailable,
  useMultiDeviceFeatureAvailability,
} from "./useMultiDeviceFeatureAvailability";

const apiMocks = vi.hoisted(() => ({
  getStatus: vi.fn(),
  listDevices: vi.fn(),
  watchChanges: vi.fn(),
}));

vi.mock("../api/modules/sync", () => ({
  syncApi: { getStatus: apiMocks.getStatus },
  devicesApi: {
    listDevices: apiMocks.listDevices,
    watchChanges: apiMocks.watchChanges,
  },
}));

function device(deviceId: string, isSelf: boolean): DeviceDto {
  return {
    device_id: deviceId,
    public_key: `pk-${deviceId}`,
    friendly_name: deviceId,
    label: deviceId,
    added_at_ms: 1,
    last_seen_ms: 2,
    is_self: isSelf,
    connectivity_state: isSelf ? "connected" : "not_connected",
    session_path: null,
    session_rtt_ms: null,
  };
}

const pairedDevices = [device("local", true), device("remote", false)];

beforeEach(() => {
  apiMocks.getStatus.mockReset();
  apiMocks.listDevices.mockReset();
  apiMocks.watchChanges.mockReset();
  apiMocks.watchChanges.mockReturnValue(() => {});
  Object.defineProperty(document, "hidden", {
    configurable: true,
    value: false,
  });
});

describe("isMultiDeviceFeatureAvailable", () => {
  it("requires sync enabled, completed pairing, and at least two devices", () => {
    expect(
      isMultiDeviceFeatureAvailable(
        { enabled: true, paired: true },
        pairedDevices,
      ),
    ).toBe(true);
    expect(
      isMultiDeviceFeatureAvailable(
        { enabled: false, paired: true },
        pairedDevices,
      ),
    ).toBe(false);
    expect(
      isMultiDeviceFeatureAvailable(
        { enabled: true, paired: false },
        pairedDevices,
      ),
    ).toBe(false);
    expect(
      isMultiDeviceFeatureAvailable(
        { enabled: true, paired: true },
        pairedDevices.slice(0, 1),
      ),
    ).toBe(false);
  });

  it("publishes the local Device list without waiting for slow sync status", async () => {
    let resolveStatus:
      | ((status: { enabled: boolean; paired: boolean }) => void)
      | undefined;
    apiMocks.getStatus.mockReturnValue(
      new Promise((resolve) => {
        resolveStatus = resolve;
      }),
    );
    apiMocks.listDevices.mockResolvedValue(pairedDevices);

    const { result, unmount } = renderHook(() =>
      useMultiDeviceFeatureAvailability(),
    );

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.devices).toEqual(pairedDevices);
    expect(result.current.available).toBe(false);

    resolveStatus?.({ enabled: true, paired: true });
    await waitFor(() => expect(result.current.available).toBe(true));
    unmount();
  });

  it("reloads once when the backend reports a Device Session change", async () => {
    let onChange: (() => void) | undefined;
    apiMocks.watchChanges.mockImplementation((listener: () => void) => {
      onChange = listener;
      return () => {};
    });
    const connectedDevices = [
      pairedDevices[0],
      {
        ...pairedDevices[1],
        connectivity_state: "connected" as const,
        session_path: "direct" as const,
        session_rtt_ms: 18,
      },
    ];
    apiMocks.getStatus.mockResolvedValue({ enabled: true, paired: true });
    apiMocks.listDevices
      .mockResolvedValueOnce(pairedDevices)
      .mockResolvedValueOnce(connectedDevices);

    const { result, unmount } = renderHook(() =>
      useMultiDeviceFeatureAvailability(),
    );
    await waitFor(() => expect(result.current.devices).toEqual(pairedDevices));

    act(() => onChange?.());

    await waitFor(() =>
      expect(result.current.devices).toEqual(connectedDevices),
    );
    expect(apiMocks.listDevices).toHaveBeenCalledTimes(2);
    unmount();
  });

  it("coalesces a burst of Device changes into one follow-up read", async () => {
    let onChange: (() => void) | undefined;
    apiMocks.watchChanges.mockImplementation((listener: () => void) => {
      onChange = listener;
      return () => {};
    });
    apiMocks.getStatus.mockResolvedValue({ enabled: true, paired: true });
    apiMocks.listDevices.mockResolvedValue(pairedDevices);
    const { result, unmount } = renderHook(() =>
      useMultiDeviceFeatureAvailability(),
    );
    try {
      await waitFor(() => expect(result.current.available).toBe(true));
      let resolveFirst!: (devices: DeviceDto[]) => void;
      apiMocks.listDevices.mockReturnValueOnce(
        new Promise<DeviceDto[]>((resolve) => {
          resolveFirst = resolve;
        }),
      );
      act(() => {
        for (let i = 0; i < 20; i += 1) onChange?.();
      });
      expect(apiMocks.listDevices).toHaveBeenCalledTimes(2);
      const newest = [pairedDevices[0]];
      apiMocks.listDevices.mockResolvedValueOnce(newest);
      await act(async () => resolveFirst(pairedDevices));
      await waitFor(() =>
        expect(apiMocks.listDevices).toHaveBeenCalledTimes(3),
      );
      await waitFor(() => expect(result.current.devices).toEqual(newest));
      expect(result.current.available).toBe(false);
    } finally {
      unmount();
    }
  });

  it("does not publish a refresh that completed after the page became hidden", async () => {
    apiMocks.getStatus.mockResolvedValue({ enabled: true, paired: true });
    let resolveDevices!: (devices: DeviceDto[]) => void;
    apiMocks.listDevices.mockReturnValueOnce(
      new Promise<DeviceDto[]>((resolve) => {
        resolveDevices = resolve;
      }),
    );
    const { result, unmount } = renderHook(() =>
      useMultiDeviceFeatureAvailability(),
    );
    try {
      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: true,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      await act(async () => resolveDevices(pairedDevices));
      expect(result.current.loaded).toBe(false);
      expect(apiMocks.watchChanges).not.toHaveBeenCalled();
      apiMocks.listDevices.mockResolvedValue(pairedDevices);
      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: false,
      });
      act(() => document.dispatchEvent(new Event("visibilitychange")));
      await waitFor(() => expect(result.current.available).toBe(true));
      expect(apiMocks.listDevices).toHaveBeenCalledTimes(2);
    } finally {
      unmount();
    }
  });

  it("stops watching while hidden and refreshes before reconnecting", async () => {
    const stopWatching = vi.fn();
    apiMocks.watchChanges.mockReturnValue(stopWatching);
    apiMocks.getStatus.mockResolvedValue({ enabled: true, paired: true });
    apiMocks.listDevices.mockResolvedValue(pairedDevices);

    const { unmount } = renderHook(() => useMultiDeviceFeatureAvailability());
    await waitFor(() => expect(apiMocks.watchChanges).toHaveBeenCalledTimes(1));

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    });
    act(() => document.dispatchEvent(new Event("visibilitychange")));

    expect(stopWatching).toHaveBeenCalledTimes(1);

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
    act(() => document.dispatchEvent(new Event("visibilitychange")));

    await waitFor(() => expect(apiMocks.listDevices).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(apiMocks.watchChanges).toHaveBeenCalledTimes(2));
    unmount();
  });
});
