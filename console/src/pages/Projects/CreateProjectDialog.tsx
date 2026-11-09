import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { WifiOff } from "lucide-react";

import { projectsApi } from "@/api/modules/projects";
import type { DeviceDto } from "@/api/modules/sync";
import type {
  ProjectCreationOutcome,
  ProjectDescriptor,
  ProjectDuplicateResponse,
} from "@/api/types/project";
import DirectoryPicker from "@/components/DirectoryPicker";
import { extractFriendlyMessage } from "@/utils/error";
import {
  needsLocalStoragePermission,
  isAndroidExternalPath,
  StoragePermissionRequired,
} from "@/utils/androidStorage";
import { Btn, FormField, InfoBox, Modal, Select, TextInput } from "@/ui";

import styles from "./index.module.less";

interface CreateProjectDialogProps {
  open: boolean;
  devices: DeviceDto[];
  onClose: () => void;
  onCreated: (outcome: ProjectCreationOutcome) => void;
  onViewDuplicate: (project: ProjectDescriptor) => void;
}

export default function CreateProjectDialog({
  open,
  devices,
  onClose,
  onCreated,
  onViewDuplicate,
}: CreateProjectDialogProps) {
  const { t } = useTranslation();
  const selectableDevices = useMemo(
    () => devices.filter((device) => device.connectivity_state === "connected"),
    [devices],
  );
  const [deviceId, setDeviceId] = useState("");
  const [path, setPath] = useState("");
  const [attachedPaths, setAttachedPaths] = useState<string[]>([]);
  const [primaryPath, setPrimaryPath] = useState("");
  const resourcePaths = [
    ...new Set([...attachedPaths, ...(path.trim() ? [path.trim()] : [])]),
  ];
  const primaryIndex = Math.max(0, resourcePaths.indexOf(primaryPath));
  const [displayName, setDisplayName] = useState("");
  const [nameEdited, setNameEdited] = useState(false);
  const [creating, setCreating] = useState(false);
  const [storageAccess, setStorageAccess] = useState<{
    path: string;
    local: boolean;
  } | null>(null);
  const [browseRequest, setBrowseRequest] = useState<{
    path: string;
    id: number;
  }>();
  const isSelf = devices.some(
    (device) => device.device_id === deviceId && device.is_self,
  );

  const requireStorageAccess = useCallback(
    (nextPath: string) => {
      if (needsLocalStoragePermission(nextPath, isSelf)) {
        setPathError("");
        setStorageAccess({ path: nextPath, local: true });
        throw new StoragePermissionRequired();
      }
    },
    [isSelf],
  );

  useEffect(() => {
    setStorageAccess(null);
    setBrowseRequest(undefined);
    setAttachedPaths([]);
    setPrimaryPath("");
    setPath("");
  }, [deviceId, open]);

  useEffect(() => {
    const retry = () => {
      if (
        !open ||
        !storageAccess?.local ||
        needsLocalStoragePermission(storageAccess.path, isSelf)
      )
        return;
      setBrowseRequest({ path: storageAccess.path, id: Date.now() });
      setStorageAccess(null);
    };
    window.addEventListener("rua:foreground", retry);
    return () => window.removeEventListener("rua:foreground", retry);
  }, [open, storageAccess, isSelf]);
  const [pathError, setPathError] = useState("");
  const [formError, setFormError] = useState("");
  const [duplicate, setDuplicate] = useState<ProjectDuplicateResponse | null>(
    null,
  );

  useEffect(() => {
    if (!open) return;
    setDeviceId(selectableDevices[0]?.device_id ?? "");
    setPath("");
    setDisplayName("");
    setNameEdited(false);
    setPathError("");
    setFormError("");
    setDuplicate(null);
  }, [open, selectableDevices]);

  const browse = useCallback(
    async (nextPath: string) => {
      requireStorageAccess(nextPath);
      const result = await projectsApi
        .browseDirectory(deviceId, nextPath.trim() || "~")
        .catch((error: unknown) => {
          if (
            isAndroidExternalPath(nextPath) &&
            extractFriendlyMessage(error).includes("os error 13")
          ) {
            setStorageAccess({
              path: nextPath,
              local: isSelf && !!window.MobileStorage,
            });
            setPathError("");
            throw new StoragePermissionRequired();
          }
          throw error;
        });
      return {
        path: result.path,
        parent: result.parent_path,
        roots: result.roots.map((root) => ({
          ...root,
          name:
            root.name === "Home"
              ? t("projects.ruaRoot", "Rua root directory")
              : root.name === "Shared storage"
              ? t("projects.phoneStorage", "Phone storage")
              : root.name,
        })),
        directories: result.entries
          .filter((entry) => entry.kind === "directory")
          .map((entry) => ({ name: entry.name, path: entry.path })),
      };
    },
    [deviceId, requireStorageAccess, isSelf, t],
  );

  const create = async () => {
    if (!deviceId || resourcePaths.length === 0) return;
    setCreating(true);
    setFormError("");
    setPathError("");
    setDuplicate(null);
    try {
      const outcome = await projectsApi.create(
        deviceId,
        resourcePaths.length === 1 ? resourcePaths[0] : resourcePaths,
        displayName.trim() || undefined,
        primaryIndex,
      );
      onCreated(outcome);
    } catch (error) {
      const parsed = parseDuplicateResponse(error);
      if (parsed) {
        setDuplicate(parsed);
      } else {
        setFormError(extractFriendlyMessage(error));
      }
    } finally {
      setCreating(false);
    }
  };

  const footer = (
    <>
      <Btn variant="outline" onClick={onClose}>
        {t("common.cancel", "Cancel")}
      </Btn>
      <Btn
        onClick={() => void create()}
        loading={creating}
        disabled={!deviceId || resourcePaths.length === 0}
      >
        {t("projects.create", "Create Project")}
      </Btn>
    </>
  );

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("projects.newProject", "New Project")}
      footer={footer}
      width={620}
      destroyOnClose
    >
      <div className={styles.createForm}>
        <FormField
          label={t("projects.resourceDevice", "Resource Device")}
          required
        >
          <Select
            value={deviceId || undefined}
            onChange={(value) => {
              setDeviceId(value ?? "");
              setPathError("");
              setDuplicate(null);
            }}
            placeholder={t("projects.selectDevice", "Select a Device")}
          >
            {devices.map((device) => {
              const connected = device.connectivity_state === "connected";
              return (
                <Select.Option
                  key={device.device_id}
                  value={device.device_id}
                  disabled={!connected}
                >
                  <span className={styles.deviceOption}>
                    <span
                      className={`${styles.statusDot} ${
                        connected ? styles.statusOnline : styles.statusOffline
                      }`}
                    />
                    <span>{device.friendly_name || device.label}</span>
                    {!connected ? (
                      <span className={styles.deviceOffline}>
                        <WifiOff size={12} />
                        {t("projects.offline", "Offline")}
                      </span>
                    ) : null}
                  </span>
                </Select.Option>
              );
            })}
          </Select>
        </FormField>

        <FormField label={t("projects.displayName", "Display name")}>
          <TextInput
            value={displayName}
            onChange={(event) => {
              setDisplayName(event.target.value);
              setNameEdited(true);
              setDuplicate(null);
            }}
          />
        </FormField>

        <FormField
          label={t("projects.directory", "Directory")}
          error={pathError}
          required
        >
          <DirectoryPicker
            value={path}
            onChange={(nextPath) => {
              setPath(nextPath);
              if (!nameEdited) setDisplayName(proposeNameFromInput(nextPath));
              setPathError("");
              setDuplicate(null);
            }}
            browse={browse}
            browseRequest={browseRequest}
            onListingChange={() => {
              setPathError("");
              setStorageAccess(null);
            }}
            key={deviceId}
            createDirectory={async (parent, name) => {
              requireStorageAccess(parent);
              const result = await projectsApi.createDirectory(
                deviceId,
                parent,
                name,
              );
              return {
                path: result.path,
                parent: result.parent_path,
                roots: result.roots.map((root) => ({
                  ...root,
                  name:
                    root.name === "Home"
                      ? t("projects.ruaRoot", "Rua root directory")
                      : root.name === "Shared storage"
                      ? t("projects.phoneStorage", "Phone storage")
                      : root.name,
                })),
                directories: result.entries.filter(
                  (entry) => entry.kind === "directory",
                ),
              };
            }}
            rootsLabel={t("projects.storageLocations", "Storage locations")}
            createLabel={t("projects.newFolder", "New folder")}
            folderNameLabel={t("projects.folderName", "Folder name")}
            cancelLabel={t("common.cancel", "Cancel")}
            placeholder="~"
            disabled={!deviceId}
            required={attachedPaths.length === 0}
            openLabel={t("projects.browse", "Browse directory")}
            closeLabel={t("projects.closeBrowser", "Close directory browser")}
            emptyLabel={t("projects.noDirectories", "No directories")}
            onError={(error) => {
              if (error instanceof StoragePermissionRequired) return;
              const message = extractFriendlyMessage(error);
              if (
                isAndroidExternalPath(path) &&
                message.includes("os error 13")
              ) {
                setStorageAccess({
                  path,
                  local: isSelf && !!window.MobileStorage,
                });
                setPathError("");
              } else setPathError(message);
            }}
          />
          <Btn
            type="button"
            disabled={!path.trim() || creating}
            onClick={() => {
              setAttachedPaths(resourcePaths);
              if (!primaryPath) setPrimaryPath(resourcePaths[0]);
              setNameEdited(Boolean(displayName.trim()));
              setPath("");
            }}
          >
            {t("projects.addResourceFolder", "Add another folder")}
          </Btn>
          {resourcePaths.length > 0 && (
            <fieldset>
              <legend>
                {t(
                  "projects.resourceFolders",
                  "Resource folders · choose the primary folder",
                )}
              </legend>
              {resourcePaths.map((resourcePath, index) => (
                <div key={resourcePath} className={styles.resourceFolderRow}>
                  <label>
                    <input
                      type="radio"
                      name="project-primary-folder"
                      checked={index === primaryIndex}
                      onChange={() => setPrimaryPath(resourcePath)}
                      disabled={creating}
                    />
                    <span>{resourcePath}</span>
                  </label>
                  <Btn
                    type="button"
                    variant="ghost"
                    disabled={creating}
                    aria-label={t(
                      "projects.removeResourceFolder",
                      "Remove {{path}}",
                      { path: resourcePath },
                    )}
                    onClick={() => {
                      setAttachedPaths((paths) =>
                        paths.filter((value) => value !== resourcePath),
                      );
                      if (path.trim() === resourcePath) setPath("");
                      if (primaryPath === resourcePath) setPrimaryPath("");
                    }}
                  >
                    {t("common.remove", "Remove")}
                  </Btn>
                </div>
              ))}
            </fieldset>
          )}
          {storageAccess && (
            <InfoBox
              type="warning"
              title={t(
                "projects.storagePermissionTitle",
                "Storage permission required",
              )}
            >
              <p>
                {storageAccess.local
                  ? t(
                      "projects.storagePermissionLocal",
                      "Allow Rua to access files in Android settings. Return here after granting access; the directory will open automatically.",
                    )
                  : t(
                      "projects.storagePermissionRemote",
                      "Enable All files access for Rua on the selected resource device, then retry browsing.",
                    )}
              </p>
              {storageAccess.local && (
                <Btn
                  type="button"
                  onClick={() => window.MobileStorage?.requestAllFilesAccess()}
                >
                  {t("projects.grantStoragePermission", "Grant access")}
                </Btn>
              )}
              <Btn
                type="button"
                variant="outline"
                onClick={() =>
                  setBrowseRequest({ path: storageAccess.path, id: Date.now() })
                }
              >
                {t("projects.retryDirectory", "Retry browsing")}
              </Btn>
            </InfoBox>
          )}
        </FormField>

        {duplicate ? (
          <InfoBox
            type="warning"
            title={t("projects.duplicateTitle", "Project already exists")}
          >
            <div className={styles.duplicateBody}>
              <div>
                <strong>{duplicate.project.display_name}</strong>
                <code>{duplicate.project.display_path}</code>
              </div>
              <Btn
                size="sm"
                variant="outline"
                onClick={() => onViewDuplicate(duplicate.project)}
              >
                {t("projects.view", "View")}
              </Btn>
            </div>
          </InfoBox>
        ) : null}

        {formError ? <InfoBox type="error">{formError}</InfoBox> : null}
      </div>
    </Modal>
  );
}

function parseDuplicateResponse(
  error: unknown,
): ProjectDuplicateResponse | null {
  if (!(error instanceof Error)) return null;
  const marker = error.message.indexOf(" - ");
  if (marker < 0) return null;
  try {
    const payload = JSON.parse(error.message.slice(marker + 3)) as
      | ProjectDuplicateResponse
      | undefined;
    return payload?.code === "project_binding_duplicate" ? payload : null;
  } catch {
    return null;
  }
}

function proposeNameFromInput(path: string): string {
  const trimmed = path.trim().replace(/[\\/]+$/, "");
  if (!trimmed) return "";
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] ?? "";
}
