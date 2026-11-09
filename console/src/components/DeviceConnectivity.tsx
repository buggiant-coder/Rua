import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";

import type { DeviceConnectivityState } from "@/api/modules/sync";
import { Tag } from "@/ui";

export function deviceConnectivityLabel(
  t: TFunction,
  state: DeviceConnectivityState | undefined,
): string {
  switch (state ?? "not_connected") {
    case "connected":
      return t("deviceConnectivity.connected", "Connected");
    case "connecting":
      return t("deviceConnectivity.connecting", "Connecting");
    case "not_connected":
      return t("deviceConnectivity.notConnected", "Not connected");
  }
}

export function deviceConnectivityColor(
  state: DeviceConnectivityState | undefined,
): "success" | "warning" | "danger" | "default" {
  switch (state ?? "not_connected") {
    case "connected":
      return "success";
    case "connecting":
      return "warning";
    case "not_connected":
      return "default";
  }
}

export function DeviceConnectivityTag({
  state,
  className,
}: {
  state: DeviceConnectivityState | undefined;
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    <Tag color={deviceConnectivityColor(state)} className={className}>
      {deviceConnectivityLabel(t, state)}
    </Tag>
  );
}
