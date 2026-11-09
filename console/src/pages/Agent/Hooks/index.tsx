/* eslint-disable react-refresh/only-export-components */
import { useLoaderData } from "react-router-dom";

import {
  hooksApi,
  type EffectiveHookPreview,
  type HookDefinitionView,
  type HookRunView,
} from "@/api/modules/hooks";
import { routeAgentId } from "@/routes/routeAgent";
import { projectsApi } from "@/api/modules/projects";
import type { ProjectDescriptor } from "@/api/types/project";
import ScopedHooksControlPlane from "./ScopedHooksControlPlane";

export interface HooksInitialData {
  agentId: string;
  definitions: HookDefinitionView[];
  runs: HookRunView[];
  preview: EffectiveHookPreview;
  projects: ProjectDescriptor[];
}

export async function loader(): Promise<HooksInitialData> {
  const agentId = routeAgentId();
  const [definitions, runs, projects] = await Promise.all([
    hooksApi.listDefinitions(agentId),
    hooksApi.listRuns(agentId),
    projectsApi.list("all"),
  ]);
  const preview = await hooksApi.preview(agentId, {
    agent: definitions
      .filter((definition) => definition.active)
      .map((definition, configured_order) => ({
        definition_revision: `${definition.id}@${definition.latest_revision}`,
        event: definition.event,
        source: "agent" as const,
        configured_order,
      })),
  });
  return { agentId, definitions, runs, preview, projects };
}

export function RouteComponent() {
  return <HooksPage initialData={useLoaderData() as HooksInitialData} />;
}

export default function HooksPage({
  initialData,
}: {
  initialData: HooksInitialData;
}) {
  return <ScopedHooksControlPlane initialData={initialData} />;
}
