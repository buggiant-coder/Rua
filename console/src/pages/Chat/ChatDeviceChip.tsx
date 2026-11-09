import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Monitor, Smartphone } from "lucide-react";
import type { DeviceDto } from "@/api/modules/sync";
import { deviceConnectivityLabel } from "@/components/DeviceConnectivity";
import { Spinner } from "@/ui";
import "./chat-device-chip.css";

interface ChatDeviceChipProps {
  currentDeviceId: string | null;

  devices: DeviceDto[];

  onSelect?: (deviceId: string) => void;

  readOnly?: boolean;
  disabled?: boolean;
  loading?: boolean;
}

function displayName(device: DeviceDto): string {
  return (
    device.friendly_name?.trim() ||
    device.label?.trim() ||
    device.device_id.slice(0, 8)
  );
}

export default function ChatDeviceChip({
  currentDeviceId,
  devices,
  onSelect,
  readOnly = false,
  disabled,
  loading = false,
}: ChatDeviceChipProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onClick = (e: MouseEvent) => {
      if (!containerRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  const current = currentDeviceId
    ? devices.find((d) => d.device_id === currentDeviceId)
    : null;
  const chipLabel = currentDeviceId
    ? current
      ? current.is_self
        ? t("chat.deviceChip.localShort", "本机")
        : displayName(current)
      : t("chat.deviceChip.unknown", "未知设备")
    : t("chat.deviceChip.unavailable", "不可用");
  const connectivityState = current?.connectivity_state ?? "not_connected";
  const connectivityLabel = current?.is_self
    ? null
    : deviceConnectivityLabel(t, connectivityState);
  const tooltipLabel = connectivityLabel
    ? `${chipLabel} · ${connectivityLabel}`
    : chipLabel;
  const Icon = currentDeviceId ? Smartphone : Monitor;
  const ariaLabel = loading
    ? t("chat.deviceChip.switching", "正在切换执行设备")
    : t("chat.deviceChip.tooltip", {
        defaultValue: "Execution device: {{name}}",
        name: tooltipLabel,
      });

  return (
    <div className="rua-chat-device-chip-wrap" ref={containerRef}>
      <button
        type="button"
        className={
          currentDeviceId
            ? current?.is_self
              ? "rua-chat-device-chip is-local"
              : `rua-chat-device-chip is-${connectivityState.replace(
                  "_",
                  "-",
                )}`
            : "rua-chat-device-chip is-unavailable"
        }
        onClick={readOnly ? undefined : () => setOpen((v) => !v)}
        disabled={disabled}
        aria-disabled={readOnly || undefined}
        aria-label={ariaLabel}
        title={ariaLabel}
      >
        {loading ? <Spinner /> : <Icon size={12} aria-hidden="true" />}
        <span className="rua-chat-device-chip-name">{chipLabel}</span>
      </button>
      {open && !readOnly && onSelect && (
        <div className="rua-chat-device-chip-popover" role="dialog">
          <div className="rua-chat-device-chip-popover-title">
            {t("chat.deviceChip.pickerTitle", "指定执行设备")}
          </div>
          {devices.map((d) => {
            const isLocal = d.is_self;
            const isActive = d.device_id === currentDeviceId;
            return (
              <button
                type="button"
                key={d.device_id}
                className={
                  isActive
                    ? "rua-chat-device-chip-row is-active"
                    : "rua-chat-device-chip-row"
                }
                onClick={() => {
                  onSelect(d.device_id);
                  setOpen(false);
                }}
              >
                <Smartphone size={14} aria-hidden="true" />
                <span className="rua-chat-device-chip-row-info">
                  <span className="rua-chat-device-chip-row-name">
                    {isLocal
                      ? t("chat.deviceChip.local", {
                          defaultValue: "本机 · {{name}}",
                          name: displayName(d),
                        })
                      : displayName(d)}
                    {isActive && (
                      <span className="rua-chat-device-chip-row-check">
                        ✓
                      </span>
                    )}
                  </span>
                  {d.device_id && (
                    <span className="rua-chat-device-chip-row-sub">
                      {!isLocal && (
                        <>
                          {deviceConnectivityLabel(t, d.connectivity_state)} ·{" "}
                        </>
                      )}
                      {d.device_id.slice(0, 8)}
                    </span>
                  )}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
