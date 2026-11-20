import type { ExecutionPosition } from "@/api/types/chat";
import ExecutionDirectoryFields from "../Chat/ExecutionDirectoryFields";
import { validateExecutionPosition } from "@/api/modules/executionDirectory";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderPen, Plus, X } from "lucide-react";
import { projectsApi } from "@/api/modules/projects";
import type { DeviceDto } from "@/api/modules/sync";
import type {
  ProjectCreationOutcome,
  ProjectDescriptor,
  ProjectDuplicateResponse,
  ProjectResourceInput,
} from "@/api/types/project";
import { Btn, FormField, InfoBox, Modal, TextInput, toast } from "@/ui";
import { extractFriendlyMessage } from "@/utils/error";
import { useHistoryDismiss } from "@/hooks/useHistoryDismiss";
import { isMobile } from "@/utils/isMobile";
import ProjectResourcePicker, {
  projectResourceDeviceLabel,
} from "./ProjectResourcePicker";
import styles from "./ProjectEditDialog.module.less";

type ProjectFormProps = {
  devices: DeviceDto[];
  onClose: () => void;
} & (
  | {
      project: ProjectDescriptor;
      onSaved: (project: ProjectDescriptor) => void;
    }
  | {
      project?: undefined;
      onCreated: (outcome: ProjectCreationOutcome) => void;
      onViewDuplicate: (project: ProjectDescriptor) => void;
    }
);

export default function ProjectEditDialog(props: ProjectFormProps) {
  const { t } = useTranslation();
  const { devices, onClose } = props;
  const [current, setCurrent] = useState(props.project);
  const [name, setName] = useState(current?.display_name ?? "");
  const [directories, setDirectories] = useState<ProjectResourceInput[]>(
    current?.resources.directories ?? [],
  );
  const [executionDefault, setExecutionDefault] =
    useState<ExecutionPosition | null>(current?.execution_default ?? null);
  const [picker, setPicker] = useState<number | "add" | null>(null);
  const pickerForm = useRef<HTMLFormElement>(null);
  const [pickerStatus, setPickerStatus] = useState({
    ready: false,
    saving: false,
  });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [duplicate, setDuplicate] = useState<ProjectDuplicateResponse | null>(
    null,
  );
  const [conflict, setConflict] = useState(false);

  const pickerLabel =
    picker === "add" ? "projects.addFolder" : "projects.changeFolder";
  useHistoryDismiss(isMobile() && picker !== null, () => {
    if (!pickerStatus.saving) setPicker(null);
  });

  function openPicker(index: number | "add") {
    setPickerStatus({ ready: false, saving: false });
    setPicker(index);
  }

  const savedDefault = current?.execution_default;
  const defaultChanged =
    savedDefault?.device_id !== executionDefault?.device_id ||
    savedDefault?.path !== executionDefault?.path;

  async function save() {
    if (saving || !name.trim()) return;
    setError("");
    setDuplicate(null);
    setConflict(false);
    setSaving(true);
    try {
      const nextDefault =
        defaultChanged && executionDefault
          ? await validateExecutionPosition(executionDefault)
          : executionDefault;
      if (!props.project) {
        props.onCreated(
          await projectsApi.create(name.trim(), directories, nextDefault),
        );
        return;
      }
      if (!current) return;
      let updated = current;
      if (
        JSON.stringify(current.resources.directories) !==
        JSON.stringify(directories)
      ) {
        const outcome = await projectsApi.editResources(
          current.project_id,
          current.resources.revision,
          directories,
        );
        updated = outcome.project;
        setCurrent(updated);
        setDirectories(updated.resources.directories);
        if (!defaultChanged)
          setExecutionDefault(updated.execution_default ?? null);
        if (outcome.overlap_warnings.length)
          toast.warning(t("projects.overlapCreated"));
      }
      if (defaultChanged) {
        updated = await projectsApi.setExecutionDefault(
          updated.project_id,
          nextDefault,
        );
        setCurrent(updated);
        setExecutionDefault(updated.execution_default ?? null);
      }
      if (name.trim() !== updated.display_name)
        updated = await projectsApi.rename(updated.project_id, name.trim());
      props.onSaved(updated);
    } catch (cause) {
      const existing = parseDuplicateResponse(cause);
      if (existing && !props.project) setDuplicate(existing);
      else {
        setError(extractFriendlyMessage(cause));
        setConflict(
          Boolean(current) && /revision|stale|conflict/i.test(String(cause)),
        );
      }
    } finally {
      setSaving(false);
    }
  }
  async function reload() {
    if (!current || saving) return;
    setSaving(true);
    try {
      const updated = await projectsApi.get(current.project_id);
      if (name === current.display_name) setName(updated.display_name);
      if (!defaultChanged)
        setExecutionDefault(updated.execution_default ?? null);
      setCurrent(updated);
      setDirectories(updated.resources.directories);
      setError("");
      setConflict(false);
    } catch (cause) {
      setError(extractFriendlyMessage(cause));
    } finally {
      setSaving(false);
    }
  }
  return (
    <Modal
      open
      title={t(
        picker !== null
          ? pickerLabel
          : current
          ? "projects.editProject"
          : "projects.newProject",
      )}
      onClose={() => {
        if (picker !== null) {
          if (!pickerStatus.saving) setPicker(null);
        } else if (!saving) onClose();
      }}
      onOk={picker !== null ? () => pickerForm.current?.requestSubmit() : save}
      okText={t(
        picker !== null
          ? pickerLabel
          : current
          ? "common.save"
          : "projects.create",
      )}
      cancelText={t("common.cancel")}
      okButtonProps={{
        disabled:
          picker !== null
            ? !pickerStatus.ready
            : !name.trim() ||
              saving ||
              conflict ||
              executionDefault?.path?.trim() === "",
      }}
      confirmLoading={picker !== null ? pickerStatus.saving : saving}
      width={620}
    >
      {picker !== null ? (
        <ProjectResourcePicker
          devices={devices}
          previous={
            typeof picker === "number" ? directories[picker] : undefined
          }
          formRef={pickerForm}
          onStatusChange={setPickerStatus}
          onSelect={(directory) => {
            if (
              directories.some(
                (row, i) =>
                  i !== picker &&
                  row.device_id === directory.device_id &&
                  row.resource_root === directory.resource_root,
              )
            )
              throw new Error(t("projects.folderAlreadyAdded"));
            setDirectories(
              picker === "add"
                ? [...directories, directory]
                : directories.map((row, i) => (i === picker ? directory : row)),
            );
            if (!name.trim())
              setName(
                directory.resource_root.split(/[\\/]/).filter(Boolean).pop() ||
                  directory.resource_root,
              );
            setPicker(null);
            setError("");
            setDuplicate(null);
          }}
        />
      ) : (
        <div className={styles.content}>
          <FormField label={t("projects.displayName")} required>
            <TextInput
              aria-label={t("projects.displayName")}
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={saving}
            />
          </FormField>
          <section aria-label={t("executionLocation.projectDefault")}>
            <h3>{t("executionLocation.projectDefault")}</h3>
            <ExecutionDirectoryFields
              position={executionDefault}
              devices={devices}
              allowServiceDefault
              disabled={saving}
              onChange={(next) => {
                setExecutionDefault(next);
                setError("");
              }}
              onError={(cause) => setError(extractFriendlyMessage(cause))}
            />
          </section>
          <section aria-label={t("projects.resourceFolders")}>
            <div className={styles.sectionHeader}>
              <h3>{t("projects.resourceFolders")}</h3>
              <Btn
                variant="outline"
                disabled={saving}
                onClick={() => openPicker("add")}
              >
                <Plus size={15} />
                {t("projects.addFolder")}
              </Btn>
            </div>
            {!directories.length && <p>{t("projects.noResourceFolders")}</p>}
            <div className={styles.folderList}>
              {directories.map((row, i) => (
                <div
                  key={
                    row.directory_id ??
                    JSON.stringify([row.device_id, row.resource_root])
                  }
                  className={styles.folderRow}
                >
                  <div className={styles.folderDetails}>
                    <TextInput
                      aria-label={t("projects.resourceName")}
                      placeholder={
                        row.resource_root
                          .split(/[\\/]/)
                          .filter(Boolean)
                          .pop() || row.resource_root
                      }
                      disabled={saving}
                      value={row.display_name ?? ""}
                      onChange={(e) => {
                        const value = e.target.value;
                        setDirectories((previous) =>
                          previous.map((item, n) =>
                            n === i
                              ? { ...item, display_name: value || undefined }
                              : item,
                          ),
                        );
                      }}
                    />
                    <span>
                      {projectResourceDeviceLabel(row.device_id, devices)}
                    </span>
                    <code
                      className={styles.folderPath}
                      title={row.resource_root}
                    >
                      {row.resource_root}
                    </code>
                  </div>
                  <Btn
                    variant="ghost"
                    title={t("projects.changeFolder")}
                    aria-label={t("projects.changeFolder")}
                    disabled={saving}
                    onClick={() => openPicker(i)}
                  >
                    <FolderPen size={16} />
                  </Btn>
                  <button
                    type="button"
                    className={styles.removeFolder}
                    title={t("projects.detachFolder")}
                    aria-label={t("projects.detachFolder")}
                    disabled={saving}
                    onClick={() =>
                      setDirectories(directories.filter((_, n) => n !== i))
                    }
                  >
                    <X size={16} />
                  </button>
                </div>
              ))}
            </div>
          </section>
          {error && <div role="alert">{error}</div>}
          {conflict && (
            <Btn disabled={saving} variant="outline" onClick={reload}>
              {t("projects.reloadResources")}
            </Btn>
          )}
          {duplicate && !props.project && (
            <InfoBox type="warning" title={t("projects.duplicateTitle")}>
              <strong>{duplicate.project.display_name}</strong>
              <Btn onClick={() => props.onViewDuplicate(duplicate.project)}>
                {t("projects.view")}
              </Btn>
            </InfoBox>
          )}
        </div>
      )}
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
