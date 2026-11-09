import { useState } from "react";
import { useTranslation } from "react-i18next";
import { projectsApi } from "@/api/modules/projects";
import type { ProjectDescriptor } from "@/api/types/project";
import DirectoryPicker from "@/components/DirectoryPicker";
import { Btn, FormField, InfoBox, Modal, TextInput, toast } from "@/ui";
import { Folder, FolderPen, Plus, X } from "lucide-react";
import styles from "./ProjectEditDialog.module.less";
import { extractFriendlyMessage } from "@/utils/error";

export default function ProjectEditDialog({
  project,
  onClose,
  onSaved,
}: {
  project: ProjectDescriptor;
  onClose: () => void;
  onSaved: (project: ProjectDescriptor) => void;
}) {
  const { t } = useTranslation();
  const [current, setCurrent] = useState(project);
  const [name, setName] = useState(project.display_name);
  const [primary, setPrimary] = useState(
    project.resources?.primary_directory_id ?? "initial",
  );
  const [paths, setPaths] = useState(
    project.resources?.directories.map(
      (directory) => directory.display_path,
    ) ?? [project.display_path],
  );
  const [addedPath, setAddedPath] = useState("");
  const [directoryIds, setDirectoryIds] = useState<Record<string, string>>(
    Object.fromEntries(
      project.resources?.directories.map((directory) => [
        directory.display_path,
        directory.directory_id,
      ]) ?? [],
    ),
  );
  const [replacingPath, setReplacingPath] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [validating, setValidating] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  const [browseRequest, setBrowseRequest] = useState<{
    path: string;
    id: number;
  }>();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const save = async () => {
    if (!name.trim() || saving || adding) return;
    setSaving(true);
    setError("");
    try {
      let updated = current;
      const currentPaths = current.resources?.directories.map(
        (directory) => directory.display_path,
      ) ?? [current.display_path];
      const selectedPath =
        Object.keys(directoryIds).find(
          (path) => directoryIds[path] === primary,
        ) ?? primary;
      if (JSON.stringify(paths) !== JSON.stringify(currentPaths)) {
        if (
          !current.resources ||
          paths.length === 0 ||
          !paths.includes(selectedPath)
        ) {
          throw new Error(t("projects.selectRemainingPrimary"));
        }
        const outcome = await projectsApi.editResources(
          current.project_id,
          current.resources.revision,
          paths,
          paths.indexOf(selectedPath),
          paths.map((path) => directoryIds[path] ?? null),
        );
        updated = outcome.project;
        setCurrent(updated);
        setDirectoryIds(
          Object.fromEntries(
            updated.resources?.directories.map((directory) => [
              directory.display_path,
              directory.directory_id,
            ]) ?? [],
          ),
        );
        setPaths(
          updated.resources?.directories.map(
            (directory) => directory.display_path,
          ) ?? [updated.display_path],
        );
        setPrimary(updated.resources?.primary_directory_id ?? "initial");
        if (outcome.overlap_warnings.length)
          toast.warning(t("projects.overlapCreated"));
      } else if (
        current.resources &&
        primary !== current.resources.primary_directory_id
      ) {
        updated = await projectsApi.setPrimaryDirectory(
          current.project_id,
          current.resources.revision,
          primary,
        );
        setCurrent(updated);
        setPaths(
          updated.resources?.directories.map(
            (directory) => directory.display_path,
          ) ?? [updated.display_path],
        );
        setPrimary(updated.resources?.primary_directory_id ?? "initial");
      }
      if (name.trim() !== updated.display_name) {
        updated = await projectsApi.rename(updated.project_id, name.trim());
      }
      onSaved(updated);
    } catch (error) {
      setError(extractFriendlyMessage(error));
    } finally {
      setSaving(false);
    }
  };
  const selectedPath =
    Object.keys(directoryIds).find((path) => directoryIds[path] === primary) ??
    primary;
  const busy = saving || validating;
  const close = () => {
    if (busy) return;
    if (adding) {
      setAdding(false);
      setAddedPath("");
      setReplacingPath(null);
      setError("");
    } else onClose();
  };
  const addFolder = async () => {
    if (!addedPath.trim() || busy || browsing) return;
    setValidating(true);
    setError("");
    try {
      const listing = await projectsApi.browseDirectory(
        current.resource_device_id,
        addedPath.trim(),
      );
      if (paths.includes(listing.path) && listing.path !== replacingPath) {
        setError(t("projects.folderAlreadyAdded"));
        return;
      }
      if (replacingPath !== null) {
        setPaths((previous) =>
          previous.map((path) =>
            path === replacingPath ? listing.path : path,
          ),
        );
        const directoryId = directoryIds[replacingPath];
        if (directoryId)
          setDirectoryIds((previous) => {
            const next = { ...previous };
            delete next[replacingPath];
            next[listing.path] = directoryId;
            return next;
          });
        if (primary === replacingPath) setPrimary(listing.path);
      } else setPaths((previous) => [...previous, listing.path]);
      setAdding(false);
      setAddedPath("");
      setReplacingPath(null);
    } catch (error) {
      setError(extractFriendlyMessage(error));
    } finally {
      setValidating(false);
    }
  };
  return (
    <Modal
      open
      title={t(
        adding
          ? replacingPath !== null
            ? "projects.changeFolder"
            : "projects.addFolder"
          : "projects.editProject",
      )}
      onClose={close}
      onOk={adding ? addFolder : save}
      okText={t(
        adding
          ? replacingPath !== null
            ? "projects.changeFolder"
            : "projects.addFolder"
          : "common.save",
      )}
      cancelText={t("common.cancel")}
      okButtonProps={{
        disabled: adding
          ? busy || browsing || !addedPath.trim()
          : saving || !name.trim(),
      }}
      confirmLoading={busy}
      maskClosable={!busy}
      width={560}
    >
      <div className={styles.content}>
        {adding ? (
          <DirectoryPicker
            value={addedPath}
            onChange={setAddedPath}
            browseRequest={browseRequest}
            onLoadingChange={setBrowsing}
            disabled={busy}
            ariaLabel={t("projects.folderPath")}
            placeholder={t("projects.folderPath")}
            rootsLabel={t("projects.storageLocations")}
            openLabel={t("projects.browse")}
            closeLabel={t("projects.closeBrowser")}
            emptyLabel={t("projects.noDirectories")}
            browse={async (path) => {
              let listing;
              try {
                listing = await projectsApi.browseDirectory(
                  current.resource_device_id,
                  path,
                );
              } catch (error) {
                if (path !== browseRequest?.path || path === "~") throw error;
                listing = await projectsApi.browseDirectory(
                  current.resource_device_id,
                  "~",
                );
              }
              return {
                path: listing.path,
                parent: listing.parent_path,
                roots: listing.roots.map((root) => ({
                  ...root,
                  name:
                    root.name === "Home"
                      ? t("projects.homeDirectory")
                      : root.name === "Shared storage"
                      ? t("projects.phoneStorage")
                      : root.name,
                })),
                directories: listing.entries.filter(
                  (entry) => entry.kind === "directory",
                ),
              };
            }}
            onError={(error) => setError(extractFriendlyMessage(error))}
          />
        ) : (
          <>
            <FormField label={t("projects.displayName")} required>
              <TextInput
                aria-label={t("projects.displayName")}
                value={name}
                disabled={saving}
                onChange={(event) => setName(event.target.value)}
                autoFocus
              />
            </FormField>
            <section aria-label={t("projects.folders")}>
              <div className={styles.sectionHeader}>
                <h3>{t("projects.folders")}</h3>
                {current.lifecycle === "active" && current.resources && (
                  <Btn
                    variant="ghost"
                    size="sm"
                    disabled={saving}
                    onClick={() => {
                      setError("");
                      setReplacingPath(null);
                      setAdding(true);
                      setBrowsing(true);
                      const parent =
                        selectedPath.slice(0, selectedPath.lastIndexOf("/")) ||
                        "/";
                      setBrowseRequest({ path: parent, id: Date.now() });
                    }}
                  >
                    <Plus size={14} aria-hidden="true" />
                    {t("projects.addFolder")}
                  </Btn>
                )}
              </div>
              <div
                className={styles.folderList}
                role="radiogroup"
                aria-label={t("projects.primaryFolder")}
              >
                {paths.map((path) => (
                  <div
                    className={styles.folderRow}
                    key={path}
                    data-primary={selectedPath === path || undefined}
                  >
                    <label
                      className={styles.folderChoice}
                      title={t("projects.primaryFolderHint")}
                    >
                      <input
                        type="radio"
                        name="project-primary-directory"
                        aria-label={t("projects.useAsPrimary", { path })}
                        checked={selectedPath === path}
                        disabled={
                          saving ||
                          current.lifecycle !== "active" ||
                          !current.resources
                        }
                        onChange={() => setPrimary(directoryIds[path] ?? path)}
                      />
                      <Folder size={18} aria-hidden="true" />
                      <span className={styles.folderDetails}>
                        <span className={styles.folderName}>
                          {path.split("/").filter(Boolean).pop() || path}
                          {selectedPath === path && (
                            <span className={styles.primaryBadge}>
                              {t("projects.defaultFolder")}
                            </span>
                          )}
                        </span>
                        <span className={styles.folderPath} title={path}>
                          {path}
                        </span>
                      </span>
                    </label>
                    <Btn
                      variant="ghost"
                      size="sm"
                      disabled={
                        saving ||
                        current.lifecycle !== "active" ||
                        !current.resources
                      }
                      aria-label={t("projects.changeResourceFolder", { path })}
                      title={t("projects.changeResourceFolder", { path })}
                      onClick={() => {
                        setError("");
                        setReplacingPath(path);
                        setAddedPath("");
                        setAdding(true);
                        setBrowsing(true);
                        setBrowseRequest({
                          path: path.slice(0, path.lastIndexOf("/")) || "/",
                          id: Date.now(),
                        });
                      }}
                    >
                      <FolderPen size={16} aria-hidden="true" />
                    </Btn>
                    <button
                      type="button"
                      className={styles.removeFolder}
                      disabled={
                        saving ||
                        paths.length <= 1 ||
                        current.lifecycle !== "active" ||
                        !current.resources
                      }
                      aria-label={t("projects.removeResourceFolder", { path })}
                      title={t("projects.removeResourceFolder", { path })}
                      onClick={() => {
                        setPaths(paths.filter((item) => item !== path));
                        setError("");
                      }}
                    >
                      <X size={16} aria-hidden="true" />
                    </button>
                  </div>
                ))}
              </div>
            </section>
          </>
        )}
        {replacingPath !== null && (
          <InfoBox type="info">{t("projects.changeFolderHint")}</InfoBox>
        )}
        {error && <InfoBox type="error">{error}</InfoBox>}
      </div>
    </Modal>
  );
}
