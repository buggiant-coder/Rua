import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Braces, ChevronDown, FolderKanban, Inbox, Plus } from "lucide-react";

import {
  hooksApi,
  type HookDefinitionView,
  type HookRunView,
} from "@/api/modules/hooks";
import { Page, PageBody } from "@/components/Page";
import { PageHeader } from "@/components/PageHeader";
import ProjectSelector from "@/components/ProjectSelector";
import { lastProjectId } from "@/pages/Projects/projectSelection";
import {
  Btn,
  Drawer,
  FormField,
  Select,
  Tabs,
  Tag,
  TextArea,
  TextInput,
  toast,
  Tooltip,
} from "@/ui";
import type { HooksInitialData } from "./index";
import styles from "./index.module.less";

type ScopeTab = "agent" | "project";
type HookEvent =
  | "turn.before_completion"
  | "turn.terminal"
  | "tool.before"
  | "tool.after"
  | "tool.failed"
  | "permission.request";

const runColor = (status: HookRunView["status"]) =>
  status === "completed"
    ? "success"
    : status === "failed"
    ? "danger"
    : "warning";

export default function ScopedHooksControlPlane({
  initialData,
}: {
  initialData: HooksInitialData;
}) {
  const { t } = useTranslation();
  const [scope, setScope] = useState<ScopeTab>("agent");
  const [agentDefinitions, setAgentDefinitions] = useState(
    initialData.definitions,
  );
  const [projectDefinitions, setProjectDefinitions] = useState<
    HookDefinitionView[]
  >([]);
  const [project, setProject] = useState(() => {
    const remembered = lastProjectId();
    return (
      initialData.projects.find((item) => item.project_id === remembered) ??
      initialData.projects[0]
    );
  });
  const [loadingProject, setLoadingProject] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [createOpen, setCreateOpen] = useState(false);
  const [logicalId, setLogicalId] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [program, setProgram] = useState("");
  const [argsText, setArgsText] = useState("");
  const [workingDirectory, setWorkingDirectory] = useState("");
  const [timeoutText, setTimeoutText] = useState("30");
  const [event, setEvent] = useState<HookEvent>("turn.terminal");
  const projectLoadFailed = t("hooksControl.toast.projectLoadFailed");

  useEffect(() => {
    if (scope !== "project" || !project) return;
    let active = true;
    setLoadingProject(true);
    hooksApi
      .listProjectDefinitions(project.project_id)
      .then((items) => active && setProjectDefinitions(items))
      .catch(
        (error) =>
          active &&
          toast.error(
            error instanceof Error ? error.message : projectLoadFailed,
          ),
      )
      .finally(() => active && setLoadingProject(false));
    return () => {
      active = false;
    };
  }, [project, projectLoadFailed, scope]);

  const definitions = scope === "agent" ? agentDefinitions : projectDefinitions;
  const visibleDefinitions = useMemo(() => {
    const query = search.trim().toLowerCase();
    return query
      ? definitions.filter((item) =>
          [item.display_name, item.logical_id, item.event, item.source]
            .join(" ")
            .toLowerCase()
            .includes(query),
        )
      : definitions;
  }, [definitions, search]);

  const toggle = async (definition: HookDefinitionView) => {
    setBusy(definition.id);
    try {
      if (scope === "agent") {
        setAgentDefinitions(
          await hooksApi.updateDefault(
            initialData.agentId,
            definition.id,
            !definition.active,
          ),
        );
      } else if (project) {
        setProjectDefinitions(
          await hooksApi.updateProjectDefault(
            project.project_id,
            definition.id,
            !definition.active,
          ),
        );
      }
      toast.success(
        t(
          definition.active
            ? "hooksControl.toast.deactivated"
            : "hooksControl.toast.activated",
        ),
      );
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("hooksControl.toast.updateFailed"),
      );
    } finally {
      setBusy(null);
    }
  };

  const createDefinition = async () => {
    setBusy("create");
    try {
      const created = await hooksApi.createDefinition(initialData.agentId, {
        logical_id: logicalId.trim(),
        display_name: displayName.trim(),
        event,
        capabilities: ["identity"],
        handler: {
          kind: "command",
          program: program.trim(),
          args: argsText
            .split("\n")
            .map((argument) => argument.trim())
            .filter(Boolean),
          working_directory: workingDirectory.trim(),
          timeout_seconds: Number(timeoutText),
          available: true,
        },
      });
      setAgentDefinitions((items) => [...items, created]);
      closeCreate();
      toast.success(t("hooksControl.toast.created"));
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("hooksControl.toast.createFailed"),
      );
    } finally {
      setBusy(null);
    }
  };

  const closeCreate = () => {
    setCreateOpen(false);
    setLogicalId("");
    setDisplayName("");
    setProgram("");
    setArgsText("");
    setWorkingDirectory("");
    setTimeoutText("30");
    setEvent("turn.terminal");
  };

  const logicalIdError =
    logicalId.length > 0 && !/^[A-Za-z0-9_-]+$/.test(logicalId)
      ? t("hooksControl.create.logicalIdInvalid")
      : undefined;
  const isAbsolutePath = (value: string) =>
    /^(?:\/|[A-Za-z]:[\\/])/.test(value.trim());
  const programError =
    program.length > 0 && !isAbsolutePath(program)
      ? t("hooksControl.create.programAbsolute")
      : undefined;
  const workingDirectoryError =
    workingDirectory.length > 0 && !isAbsolutePath(workingDirectory)
      ? t("hooksControl.create.workingDirectoryAbsolute")
      : undefined;
  const timeout = Number(timeoutText);
  const timeoutError =
    timeoutText.length > 0 && (!Number.isInteger(timeout) || timeout <= 0)
      ? t("hooksControl.create.timeoutInvalid")
      : undefined;
  const canCreate =
    displayName.trim().length > 0 &&
    logicalId.trim().length > 0 &&
    program.trim().length > 0 &&
    workingDirectory.trim().length > 0 &&
    timeoutText.length > 0 &&
    !logicalIdError &&
    !programError &&
    !workingDirectoryError &&
    !timeoutError;

  const activeCount = definitions.filter((item) => item.active).length;
  const attentionCount = definitions.filter(
    (item) =>
      item.revision_changed ||
      item.availability.status === "unavailable" ||
      item.validation.status === "invalid",
  ).length;
  const stats = [
    { label: t("hooksControl.overview.registered"), value: definitions.length },
    { label: t("hooksControl.overview.active"), value: activeCount },
    { label: t("hooksControl.overview.attention"), value: attentionCount },
    ...(scope === "agent"
      ? [
          {
            label: t("hooksControl.overview.failed"),
            value: initialData.runs.filter((run) => run.status === "failed")
              .length,
          },
        ]
      : []),
  ];

  const scopeContent = (
    <div className={styles.scopeContent}>
      <section
        className={styles.stats}
        aria-label={t("hooksControl.overview.summary")}
      >
        {stats.map((item) => (
          <Stat key={item.label} label={item.label} value={item.value} />
        ))}
      </section>

      {scope === "project" && !project ? (
        <Empty
          icon={<FolderKanban size={30} />}
          title={t("hooksControl.scope.noProjects")}
          description={t("hooksControl.scope.noProjectsHint")}
        />
      ) : (
        <section className={styles.contentPanel}>
          <div className={styles.toolbar}>
            <TextInput
              className={styles.search}
              value={search}
              allowClear
              aria-label={t("hooksControl.definitions.search")}
              placeholder={t("hooksControl.definitions.search")}
              onChange={(event) => setSearch(event.target.value)}
              onClear={() => setSearch("")}
            />
            {scope === "project" && (
              <span className={styles.manifestHint}>
                <FolderKanban size={15} aria-hidden="true" />
                {t("hooksControl.scope.manifestHint")}
              </span>
            )}
          </div>
          {loadingProject ? (
            <Empty
              icon={<FolderKanban size={30} />}
              title={t("common.loading")}
            />
          ) : visibleDefinitions.length === 0 ? (
            <Empty
              icon={
                scope === "agent" ? (
                  <Inbox size={30} />
                ) : (
                  <FolderKanban size={30} />
                )
              }
              title={t(
                scope === "agent"
                  ? "hooksControl.definitions.noResults"
                  : "hooksControl.scope.noProjectDefinitions",
              )}
              description={t(
                scope === "agent"
                  ? "hooksControl.definitions.noResultsHint"
                  : "hooksControl.scope.noProjectDefinitionsHint",
              )}
            />
          ) : (
            <div className={styles.definitionList}>
              {visibleDefinitions.map((definition) => (
                <article className={styles.definitionCard} key={definition.id}>
                  <div className={styles.cardHeader}>
                    <div>
                      <div className={styles.cardTitleRow}>
                        <Braces
                          className={styles.cardIcon}
                          size={20}
                          aria-hidden="true"
                        />
                        <h3>{definition.display_name}</h3>
                      </div>
                      <div className={styles.tags}>
                        <Tag color={definition.active ? "success" : "default"}>
                          {t(
                            definition.active
                              ? "hooksControl.status.enabled"
                              : "hooksControl.status.disabled",
                          )}
                        </Tag>
                        <Tag>
                          {t(
                            definition.source === "native"
                              ? "hooksControl.source.system"
                              : `hooksControl.source.${definition.source}`,
                          )}
                        </Tag>
                        <Tag
                          color={
                            definition.availability.status === "available"
                              ? "success"
                              : "danger"
                          }
                        >
                          {t(
                            `hooksControl.status.${definition.availability.status}`,
                          )}
                        </Tag>
                        <Tag
                          color={
                            definition.validation.status === "valid"
                              ? "default"
                              : "danger"
                          }
                        >
                          {t(
                            `hooksControl.status.${definition.validation.status}`,
                          )}
                        </Tag>
                      </div>
                      {definition.source === "native" && (
                        <p className={styles.definitionDescription}>
                          {t(
                            "hooksControl.native.turnTerminalAuditDescription",
                          )}
                        </p>
                      )}
                    </div>
                    <Btn
                      variant={definition.active ? "outline" : "filled"}
                      loading={busy === definition.id}
                      onClick={() => void toggle(definition)}
                    >
                      {t(
                        definition.active
                          ? "hooksControl.actions.disable"
                          : "hooksControl.actions.enable",
                      )}
                    </Btn>
                  </div>
                  <details className={styles.technicalDetails}>
                    <summary>
                      {t("hooksControl.labels.technicalDetails")}
                      <ChevronDown size={15} aria-hidden="true" />
                    </summary>
                    <dl>
                      <div>
                        <dt>{t("hooksControl.labels.event")}</dt>
                        <dd>{definition.event}</dd>
                      </div>
                      <div>
                        <dt>{t("hooksControl.labels.handler")}</dt>
                        <dd>{definition.handler_kind}</dd>
                      </div>
                      <div>
                        <dt>{t("hooksControl.labels.revisionLabel")}</dt>
                        <dd>{definition.latest_revision}</dd>
                      </div>
                      <div>
                        <dt>{t("hooksControl.labels.definitionId")}</dt>
                        <dd>{definition.id}</dd>
                      </div>
                    </dl>
                  </details>
                </article>
              ))}
            </div>
          )}
        </section>
      )}

      {scope === "agent" && initialData.runs.length > 0 && (
        <section className={styles.recentRuns}>
          <h2>{t("hooksControl.activity.title")}</h2>
          {initialData.runs.slice(0, 5).map((run) => (
            <div className={styles.runSummary} key={run.invocation_id}>
              <span>{run.event}</span>
              <span>{run.definition_revision}</span>
              <Tag color={runColor(run.status)}>
                {t(`hooksControl.status.${run.status}`)}
              </Tag>
            </div>
          ))}
        </section>
      )}
    </div>
  );

  return (
    <Page className={styles.hubPage}>
      <PageHeader
        agentScoped={scope === "agent"}
        items={[]}
        afterBreadcrumb={
          scope === "project" ? (
            project ? (
              <ProjectSelector
                currentProject={project}
                projects={initialData.projects}
                onSelect={setProject}
              />
            ) : undefined
          ) : undefined
        }
        extra={
          scope === "agent" ? (
            <Tooltip title={t("hooksControl.actions.newCommandHook")}>
              <Btn
                icon={<Plus />}
                aria-label={t("hooksControl.actions.newCommandHook")}
                title={t("hooksControl.actions.newCommandHook")}
                onClick={() => setCreateOpen(true)}
              />
            </Tooltip>
          ) : undefined
        }
      />
      <PageBody className={styles.pageBody}>
        <Tabs
          className={styles.mainTabs}
          activeKey={scope}
          onChange={(key) => {
            setScope(key as ScopeTab);
            setSearch("");
          }}
          items={(
            [
              ["agent", t("hooksControl.scope.agent")],
              ["project", t("hooksControl.scope.project")],
            ] as const
          ).map(([key, label]) => ({
            key,
            label,
            children:
              scope === key ? (
                <div className={styles.tabPane}>{scopeContent}</div>
              ) : null,
          }))}
        />
      </PageBody>

      <Drawer
        open={createOpen}
        onClose={closeCreate}
        title={t("hooksControl.create.title")}
        width={520}
        footer={
          <>
            <Btn variant="ghost" onClick={closeCreate}>
              {t("common.cancel")}
            </Btn>
            <Btn
              loading={busy === "create"}
              disabled={!canCreate}
              onClick={() => void createDefinition()}
            >
              {t("hooksControl.create.submit")}
            </Btn>
          </>
        }
      >
        <div className={styles.drawerBody}>
          <p className={styles.drawerIntro}>
            {t("hooksControl.create.description")}
          </p>

          <section className={styles.formSection}>
            <h3>{t("hooksControl.create.triggerSection")}</h3>
            <FormField label={t("hooksControl.create.event")} required>
              <Select
                value={event}
                onChange={(value) => setEvent(value as HookEvent)}
                options={(
                  [
                    "turn.before_completion",
                    "turn.terminal",
                    "tool.before",
                    "tool.after",
                    "tool.failed",
                    "permission.request",
                  ] as HookEvent[]
                ).map((value) => ({
                  value,
                  label: t(`hooksControl.create.events.${value}`),
                }))}
              />
            </FormField>
            <div className={styles.eventSummary}>
              <strong>{t(`hooksControl.create.events.${event}`)}</strong>
              <p>{t(`hooksControl.create.eventDescriptions.${event}`)}</p>
              <small>{t("hooksControl.create.identityOnly")}</small>
            </div>
          </section>

          <section className={styles.formSection}>
            <h3>{t("hooksControl.create.identitySection")}</h3>
            <FormField label={t("hooksControl.create.displayName")} required>
              <TextInput
                aria-label={t("hooksControl.create.displayName")}
                value={displayName}
                placeholder={t("hooksControl.create.displayNamePlaceholder")}
                onChange={(input) => setDisplayName(input.target.value)}
              />
            </FormField>
            <FormField
              label={t("hooksControl.create.logicalId")}
              error={logicalIdError}
              required
            >
              <TextInput
                aria-label={t("hooksControl.create.logicalId")}
                value={logicalId}
                placeholder="audit-terminal-turn"
                onChange={(input) => setLogicalId(input.target.value)}
              />
              <p className={styles.fieldHelp}>
                {t("hooksControl.create.logicalIdHelp")}
              </p>
            </FormField>
          </section>

          <section className={styles.formSection}>
            <h3>{t("hooksControl.create.commandSection")}</h3>
            <FormField
              label={t("hooksControl.create.program")}
              error={programError}
              required
            >
              <TextInput
                aria-label={t("hooksControl.create.program")}
                value={program}
                placeholder="/usr/bin/python3"
                onChange={(input) => setProgram(input.target.value)}
              />
              <p className={styles.fieldHelp}>
                {t("hooksControl.create.programHelp")}
              </p>
            </FormField>
            <FormField label={t("hooksControl.create.arguments")}>
              <TextArea
                aria-label={t("hooksControl.create.arguments")}
                value={argsText}
                rows={4}
                placeholder={"./hooks/audit.py\n--strict"}
                onChange={(input) => setArgsText(input.target.value)}
              />
              <p className={styles.fieldHelp}>
                {t("hooksControl.create.argumentsHelp")}
              </p>
            </FormField>
            <FormField
              label={t("hooksControl.create.workingDirectory")}
              error={workingDirectoryError}
              required
            >
              <TextInput
                aria-label={t("hooksControl.create.workingDirectory")}
                value={workingDirectory}
                placeholder="/workspace/project"
                onChange={(input) => setWorkingDirectory(input.target.value)}
              />
              <p className={styles.fieldHelp}>
                {t("hooksControl.create.workingDirectoryHelp")}
              </p>
            </FormField>
            <FormField
              label={t("hooksControl.create.timeout")}
              error={timeoutError}
              required
            >
              <TextInput
                aria-label={t("hooksControl.create.timeout")}
                type="number"
                min={1}
                step={1}
                value={timeoutText}
                onChange={(input) => setTimeoutText(input.target.value)}
              />
            </FormField>
          </section>

          <div className={styles.createNotice}>
            <strong>{t("hooksControl.create.disabledAfterCreate")}</strong>
            <p>{t("hooksControl.create.disabledAfterCreateDescription")}</p>
          </div>
        </div>
      </Drawer>
    </Page>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className={styles.statCard}>
      <span className={styles.statLabel}>{label}</span>
      <strong className={styles.statValue}>{value}</strong>
    </div>
  );
}

function Empty({
  icon,
  title,
  description,
}: {
  icon: React.ReactNode;
  title: string;
  description?: string;
}) {
  return (
    <div className={styles.emptyPanel}>
      <span className={styles.emptyIcon}>{icon}</span>
      <strong>{title}</strong>
      {description && <p>{description}</p>}
    </div>
  );
}
