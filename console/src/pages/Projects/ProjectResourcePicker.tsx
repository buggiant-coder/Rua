import { useEffect, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import DirectoryPicker from "@/components/DirectoryPicker";
import { projectsApi } from "@/api/modules/projects";
import type { DeviceDto } from "@/api/modules/sync";
import type {
  ProjectDirectoryListing,
  ProjectResourceInput,
} from "@/api/types/project";
import { Btn, FormField, InfoBox, Select } from "@/ui";
import {
  needsLocalStoragePermission,
  isAndroidExternalPath,
  StoragePermissionRequired,
} from "@/utils/androidStorage";
import { extractFriendlyMessage } from "@/utils/error";

export function projectResourceDeviceLabel(
  deviceId: string,
  devices: DeviceDto[],
) {
  const device = devices.find((item) => item.device_id === deviceId);
  return device?.friendly_name || device?.label || deviceId;
}

export default function ProjectResourcePicker({
  devices,
  previous,
  formRef,
  onStatusChange,
  onSelect,
}: {
  devices: DeviceDto[];
  previous?: ProjectResourceInput;
  formRef: RefObject<HTMLFormElement>;
  onStatusChange: (status: { ready: boolean; saving: boolean }) => void;
  onSelect: (directory: ProjectResourceInput) => void;
}) {
  const { t } = useTranslation();
  const connected = devices.filter(
    (d) => d.is_self || d.connectivity_state === "connected",
  );
  const preferredDevice =
    connected.find((d) => d.device_id === previous?.device_id)?.device_id ??
    connected.find((d) => d.is_self)?.device_id ??
    connected[0]?.device_id ??
    "";
  const [deviceId, setDeviceId] = useState(preferredDevice);
  useEffect(() => {
    if (!deviceId && preferredDevice) setDeviceId(preferredDevice);
  }, [deviceId, preferredDevice]);
  const [path, setPath] = useState(
    deviceId === previous?.device_id ? previous.resource_root : "",
  );
  const [browseRequest, setBrowseRequest] = useState(() => ({
    path: path || "~",
    id: 1,
  }));
  const [error, setError] = useState("");
  const [storageAccess, setStorageAccess] = useState<{
    path: string;
    local: boolean;
  } | null>(null);
  const [browsing, setBrowsing] = useState(false);
  const [saving, setSaving] = useState(false);
  const isSelf = devices.some((d) => d.device_id === deviceId && d.is_self);
  const available = devices.some((d) => d.device_id === deviceId);
  const ready = available && !!path.trim() && !saving && !browsing;
  useEffect(() => {
    onStatusChange({ ready, saving });
  }, [onStatusChange, ready, saving]);

  useEffect(() => {
    const resume = () => {
      if (
        !storageAccess?.local ||
        needsLocalStoragePermission(storageAccess.path, isSelf)
      )
        return;
      setBrowseRequest({ path: storageAccess.path, id: Date.now() });
      setStorageAccess(null);
    };
    window.addEventListener("rua:foreground", resume);
    return () => window.removeEventListener("rua:foreground", resume);
  }, [isSelf, storageAccess]);

  async function withStorageAccess(
    destination: string,
    operation: () => Promise<ProjectDirectoryListing>,
  ) {
    if (needsLocalStoragePermission(destination, isSelf)) {
      setStorageAccess({ path: destination, local: true });
      throw new StoragePermissionRequired();
    }
    try {
      const result = await operation();
      setStorageAccess(null);
      return result;
    } catch (cause) {
      if (
        isAndroidExternalPath(destination) &&
        extractFriendlyMessage(cause).includes("os error 13")
      ) {
        setStorageAccess({
          path: destination,
          local: isSelf && Boolean(window.MobileStorage),
        });
        throw new StoragePermissionRequired();
      }
      throw cause;
    }
  }
  const browse = (destination: string) =>
    withStorageAccess(destination, () =>
      projectsApi.browseDirectory(deviceId, destination),
    );
  const listing = (value: ProjectDirectoryListing) => ({
    path: value.path,
    parent: value.parent_path,
    roots: value.roots.map((root) => ({
      ...root,
      name:
        root.name === "Home"
          ? t("projects.ruaRoot")
          : root.name === "Shared storage"
          ? t("projects.phoneStorage")
          : root.name,
    })),
    directories: value.entries.filter((entry) => entry.kind === "directory"),
  });
  async function confirm() {
    if (!path.trim() || !available || saving || browsing) return;
    setSaving(true);
    setError("");
    try {
      const result = await browse(path.trim());
      const sameLocation =
        previous?.device_id === deviceId &&
        previous.resource_root === result.path;
      onSelect(
        sameLocation
          ? previous
          : {
              ...(previous?.display_name
                ? { display_name: previous.display_name }
                : {}),
              device_id: deviceId,
              resource_root: result.path,
            },
      );
    } catch (cause) {
      if (!(cause instanceof StoragePermissionRequired))
        setError(extractFriendlyMessage(cause));
    } finally {
      setSaving(false);
    }
  }
  return (
    <form
      ref={formRef}
      onSubmit={(event) => {
        event.preventDefault();
        void confirm();
      }}
    >
      <FormField label={t("projects.resourceDevice")}>
        <Select
          aria-label={t("projects.resourceDevice")}
          value={deviceId || undefined}
          disabled={saving}
          onChange={(value) => {
            setDeviceId(value ?? "");
            setPath("");
            setError("");
            setStorageAccess(null);
            setBrowseRequest({ path: "~", id: Date.now() });
          }}
        >
          {devices.map((d) => (
            <Select.Option key={d.device_id} value={d.device_id}>
              {projectResourceDeviceLabel(d.device_id, devices)}
              {!d.is_self &&
                d.connectivity_state !== "connected" &&
                ` · ${t("projects.offline")}`}
            </Select.Option>
          ))}
        </Select>
      </FormField>
      {available && (
        <DirectoryPicker
          key={deviceId}
          value={path}
          onChange={(value) => {
            setPath(value);
            setError("");
          }}
          ariaLabel={t("projects.folderPath")}
          placeholder="~"
          browseRequest={browseRequest}
          onLoadingChange={setBrowsing}
          browse={async (destination) => {
            try {
              return listing(await browse(destination));
            } catch (cause) {
              if (
                cause instanceof StoragePermissionRequired ||
                !previous ||
                destination === "~" ||
                destination !== browseRequest.path
              )
                throw cause;
              return listing(await browse("~"));
            }
          }}
          createDirectory={async (parent, name) =>
            listing(
              await withStorageAccess(parent, () =>
                projectsApi.createDirectory(deviceId, parent, name),
              ),
            )
          }
          rootsLabel={t("projects.storageLocations")}
          createLabel={t("projects.newFolder")}
          folderNameLabel={t("projects.folderName")}
          cancelLabel={t("common.cancel")}
          disabled={saving}
          openLabel={t("projects.browse")}
          closeLabel={t("projects.closeBrowser")}
          emptyLabel={t("projects.noDirectories")}
          onError={(cause) => {
            if (!(cause instanceof StoragePermissionRequired))
              setError(extractFriendlyMessage(cause));
          }}
        />
      )}
      {storageAccess && (
        <InfoBox type="warning" title={t("projects.storagePermissionTitle")}>
          <p>
            {t(
              storageAccess.local
                ? "projects.storagePermissionLocal"
                : "projects.storagePermissionRemote",
            )}
          </p>
          {storageAccess.local && (
            <Btn
              type="button"
              onClick={() => window.MobileStorage?.requestAllFilesAccess()}
            >
              {t("projects.grantStoragePermission")}
            </Btn>
          )}
          <Btn
            type="button"
            onClick={() =>
              setBrowseRequest({ path: storageAccess.path, id: Date.now() })
            }
          >
            {t("projects.retryDirectory")}
          </Btn>
        </InfoBox>
      )}
      {error && <div role="alert">{error}</div>}
    </form>
  );
}
