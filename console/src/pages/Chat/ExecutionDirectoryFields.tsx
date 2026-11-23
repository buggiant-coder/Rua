import { useTranslation } from "react-i18next";
import type { ExecutionPosition } from "@/api/types/chat";
import { executionDirectoryApi } from "@/api/modules/executionDirectory";
import type { DeviceDto } from "@/api/modules/sync";
import DirectoryPicker from "@/components/DirectoryPicker";
import { FormField, Select } from "@/ui";

export default function ExecutionDirectoryFields({
  position,
  devices,
  allowServiceDefault,
  lockDevice,
  hideDevice,
  disabled,
  onChange,
  onError,
}: {
  position: ExecutionPosition | null;
  devices: DeviceDto[];
  allowServiceDefault?: boolean;
  lockDevice?: boolean;
  hideDevice?: boolean;
  disabled?: boolean;
  onChange: (position: ExecutionPosition | null) => void;
  onError: (error: unknown) => void;
}) {
  const { t } = useTranslation();
  const deviceId = position?.device_id;
  return (
    <>
      {!hideDevice && (
        <FormField label={t("executionLocation.device")}>
          <Select
            aria-label={t("executionLocation.device")}
            disabled={disabled || lockDevice}
            value={deviceId ?? ""}
            onChange={(id) => onChange(id ? { device_id: id } : null)}
          >
            {allowServiceDefault && (
              <Select.Option value="">
                {t("executionLocation.connectedService")}
              </Select.Option>
            )}
            {deviceId && !devices.some((d) => d.device_id === deviceId) && (
              <Select.Option value={deviceId}>{deviceId}</Select.Option>
            )}
            {devices.map((d) => (
              <Select.Option key={d.device_id} value={d.device_id}>
                {d.friendly_name || d.label || d.device_id}
              </Select.Option>
            ))}
          </Select>
        </FormField>
      )}
      {position && (
        <>
          <FormField label={t("executionLocation.directory")}>
            <Select
              aria-label={t("executionLocation.directory")}
              value={position.path === undefined ? "managed" : "explicit"}
              disabled={disabled}
              onChange={(mode) =>
                onChange(
                  mode === "managed"
                    ? { device_id: position.device_id }
                    : { device_id: position.device_id, path: "" },
                )
              }
            >
              <Select.Option value="managed">
                {t("executionLocation.managed")}
              </Select.Option>
              <Select.Option value="explicit">
                {t("executionLocation.explicit")}
              </Select.Option>
            </Select>
          </FormField>
          {position.path !== undefined && (
            <DirectoryPicker
              key={position.device_id}
              ariaLabel={t("executionLocation.path")}
              value={position.path}
              disabled={disabled}
              onChange={(path) => onChange({ ...position, path })}
              onError={onError}
              openLabel={t("projects.browse")}
              closeLabel={t("projects.closeBrowser")}
              emptyLabel={t("projects.noDirectories")}
              placeholder="~"
              browse={async (path) => {
                const result = await executionDirectoryApi.browse(
                  position.device_id,
                  path,
                );
                return {
                  path: result.path,
                  parent: result.parent_path,
                  roots: result.roots,
                  directories: result.entries.filter(
                    (e) => e.kind === "directory",
                  ),
                };
              }}
            />
          )}
        </>
      )}
    </>
  );
}
