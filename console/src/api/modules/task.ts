import { request } from "../request";
import type { TaskRecord, TaskState } from "../types/task";
import type { ConversationApiTarget } from "./conversation";

function taskScope(_target: ConversationApiTarget, chatId: string): string {
  return `/chats/${encodeURIComponent(chatId)}/tasks`;
}

export const taskApi = {
  listTasks: (
    target: ConversationApiTarget,
    chatId: string,
    options?: { state?: TaskState; limit?: number },
  ) => {
    const search = new URLSearchParams();
    if (options?.state) search.set("state", options.state);
    if (options?.limit != null) search.set("limit", String(options.limit));
    const query = search.toString();
    return request<TaskRecord[]>(
      `${taskScope(target, chatId)}${query ? `?${query}` : ""}`,
    );
  },

  getTask: (target: ConversationApiTarget, chatId: string, taskId: string) =>
    request<TaskRecord>(
      `${taskScope(target, chatId)}/${encodeURIComponent(taskId)}`,
    ),

  cancelTask: (target: ConversationApiTarget, chatId: string, taskId: string) =>
    request<TaskRecord>(
      `${taskScope(target, chatId)}/${encodeURIComponent(taskId)}/cancel`,
      { method: "POST" },
    ),

  watchTask: (target: ConversationApiTarget, chatId: string, taskId: string) =>
    request(
      `${taskScope(target, chatId)}/${encodeURIComponent(taskId)}/watch`,
      { method: "POST" },
    ),

  unwatchTask: (
    target: ConversationApiTarget,
    chatId: string,
    taskId: string,
  ) =>
    request(
      `${taskScope(target, chatId)}/${encodeURIComponent(taskId)}/unwatch`,
      { method: "POST" },
    ),

  sendInput: (
    target: ConversationApiTarget,
    chatId: string,
    taskId: string,
    input: { type: "text"; text: string } | { type: "close_stdin" },
  ) =>
    request(
      `${taskScope(target, chatId)}/${encodeURIComponent(taskId)}/input`,
      {
        method: "POST",
        body: JSON.stringify({
          submission_id: crypto.randomUUID(),
          input,
        }),
      },
    ),

  retryTask: (
    target: ConversationApiTarget,
    chatId: string,
    taskId: string,
    authorizationAgentId: string,
  ) =>
    request<TaskRecord>(
      `${taskScope(target, chatId)}/${encodeURIComponent(taskId)}/retry`,
      {
        method: "POST",
        body: JSON.stringify({
          authorization_agent_id: authorizationAgentId,
        }),
      },
    ),

  getFullResult: (
    target: ConversationApiTarget,
    chatId: string,
    taskId: string,
  ) =>
    request<unknown>(
      `${taskScope(target, chatId)}/${encodeURIComponent(taskId)}/result`,
    ),

  activityPath: (target: ConversationApiTarget, chatId: string) =>
    `${taskScope(target, chatId)}/activity`,
};
