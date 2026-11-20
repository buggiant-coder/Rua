/* eslint-disable react-refresh/only-export-components */
import { useLoaderData, type LoaderFunctionArgs } from "react-router-dom";
import { chatApi } from "@/api/modules/chat";
import { projectsApi } from "@/api/modules/projects";
import type { ChatHistory } from "@/api/types";
import type { ProjectDescriptor } from "@/api/types/project";
import { AgentChatPage } from ".";
import ProjectChatPage from "../Projects/ProjectChat";
import { readDraftChatContext } from "./chatContext";
import { isDraftConversation } from "./conversationLifecycle";
import {
  INITIAL_CHAT_TURN_PAGE_SIZE,
  primeChatViewCache,
} from "./owned/useConversationViewRuntime";

export interface ChatRouteData {
  chatId: string;
  agentId: string;
  project?: ProjectDescriptor;
  projects?: ProjectDescriptor[];
  history: ChatHistory | null;
  unmaterialized: boolean;
}

export async function loader({
  params,
}: LoaderFunctionArgs): Promise<ChatRouteData> {
  const chatId = params.chatId;
  if (!chatId) throw new Response("Chat ID is required", { status: 400 });
  const draft = readDraftChatContext(chatId);
  const scope = draft?.projectId
    ? `project:${draft.projectId}`
    : draft?.agentId;
  const isDraft = draft && scope && isDraftConversation(scope, chatId);
  const history = isDraft
    ? null
    : await chatApi.getChat(chatId, { turnLimit: INITIAL_CHAT_TURN_PAGE_SIZE });
  const agentId = history ? history.agent_id : draft?.agentId;
  if (!agentId) throw new Error("Chat header is missing its selected Agent");
  const projectId = history ? history.project_id : draft?.projectId;
  const [project, projects] = projectId
    ? await Promise.all([
        projectsApi.get(projectId),
        projectsApi.list("active"),
      ])
    : [];
  if (history) primeChatViewCache("chat", chatId, history);
  return {
    chatId,
    agentId,
    project,
    projects,
    history,
    unmaterialized: !history,
  };
}

export function RouteComponent() {
  const data = useLoaderData() as ChatRouteData;
  return data.project ? (
    <ProjectChatPage
      key={data.chatId}
      initialData={{
        project: data.project,
        projects: data.projects,
        chatId: data.chatId,
        initialHistory: data.history,
        unmaterialized: data.unmaterialized,
      }}
    />
  ) : (
    <AgentChatPage
      key={data.chatId}
      agentId={data.agentId}
      chatId={data.chatId}
      initialHistory={data.history}
      unmaterialized={data.unmaterialized}
    />
  );
}
