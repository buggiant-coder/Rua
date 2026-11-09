import { useEffect, useState } from "react";
import { useRouteActivity } from "@/routes/RouteActivity";
import {
  devicesApi,
  syncApi,
  type DeviceDto,
  type SyncStatusResponse,
} from "../api/modules/sync";

interface MultiDeviceFeatureState {
  devices: DeviceDto[];
  loaded: boolean;
  available: boolean;
}

export const MULTI_DEVICE_FEATURES_CHANGED_EVENT =
  "rua-multi-device-features-changed";

export function notifyMultiDeviceFeaturesChanged() {
  window.dispatchEvent(new Event(MULTI_DEVICE_FEATURES_CHANGED_EVENT));
}

export function isMultiDeviceFeatureAvailable(
  status: Pick<SyncStatusResponse, "enabled" | "paired"> | null,
  devices: readonly DeviceDto[],
) {
  return (
    status?.enabled === true && status.paired === true && devices.length >= 2
  );
}

export function useMultiDeviceFeatureAvailability(
  active = true,
): MultiDeviceFeatureState {
  const routeActive = useRouteActivity();
  const watching = active && routeActive;
  const [state, setState] = useState<MultiDeviceFeatureState>({
    devices: [],
    loaded: false,
    available: false,
  });

  useEffect(() => {
    if (!watching) return;
    let cancelled = false;
    let latestRequest = 0;
    let running = false;
    let dirty = false;
    let stopWatching: (() => void) | null = null;
    const handleChange = () => void load();
    const updateWatcher = (watch: boolean) => {
      if (watch && !document.hidden && !stopWatching) {
        stopWatching = devicesApi.watchChanges?.(handleChange) ?? (() => {});
      } else if (!watch && stopWatching) {
        stopWatching();
        stopWatching = null;
      }
    };
    const load = async () => {
      if (cancelled || document.hidden) return;
      dirty = true;
      if (running) return;
      running = true;
      try {
        while (dirty && !cancelled && !document.hidden) {
          dirty = false;
          const request = ++latestRequest;
          const statusPromise = syncApi.getStatus().catch(() => null);
          const devices = await devicesApi.listDevices().catch(() => []);
          if (!cancelled && !document.hidden && request === latestRequest) {
            setState((current) => ({
              devices,
              loaded: true,
              available:
                current.loaded && current.available
                  ? devices.length >= 2
                  : false,
            }));
          }
          const status = await statusPromise;
          if (cancelled || document.hidden || request !== latestRequest)
            continue;
          updateWatcher(status?.enabled === true && status.paired === true);
          setState({
            devices,
            loaded: true,
            available: isMultiDeviceFeatureAvailable(status, devices),
          });
        }
      } finally {
        running = false;
      }
    };
    const handleVisibility = () => {
      if (document.hidden) {
        latestRequest += 1;
        dirty = false;
        updateWatcher(false);
      } else {
        void load();
      }
    };
    window.addEventListener(MULTI_DEVICE_FEATURES_CHANGED_EVENT, handleChange);
    document.addEventListener("visibilitychange", handleVisibility);
    void load();
    return () => {
      cancelled = true;
      stopWatching?.();
      window.removeEventListener(
        MULTI_DEVICE_FEATURES_CHANGED_EVENT,
        handleChange,
      );
      document.removeEventListener("visibilitychange", handleVisibility);
    };
  }, [watching]);

  return state;
}
