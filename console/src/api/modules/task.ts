import { request } from "../request";
import type { TaskListItem, TaskRecord, TaskState } from "../types/task";

function taskPath(chatId: string): string {
  return `/chats/${encodeURIComponent(chatId)}/tasks`;
}

export const taskApi = {
  listTasks: (
    chatId: string,
    options?: { state?: TaskState; limit?: number },
  ) => {
    const search = new URLSearchParams();
    if (options?.state) search.set("state", options.state);
    if (options?.limit != null) search.set("limit", String(options.limit));
    const query = search.toString();
    return request<TaskListItem[]>(
      `${taskPath(chatId)}${query ? `?${query}` : ""}`,
    );
  },

  getTask: (chatId: string, taskId: string) =>
    request<TaskRecord>(`${taskPath(chatId)}/${encodeURIComponent(taskId)}`),

  cancelTask: (chatId: string, taskId: string) =>
    request<TaskRecord>(
      `${taskPath(chatId)}/${encodeURIComponent(taskId)}/cancel`,
      { method: "POST" },
    ),

  watchTask: (chatId: string, taskId: string) =>
    request(`${taskPath(chatId)}/${encodeURIComponent(taskId)}/watch`, {
      method: "POST",
    }),

  unwatchTask: (chatId: string, taskId: string) =>
    request(`${taskPath(chatId)}/${encodeURIComponent(taskId)}/unwatch`, {
      method: "POST",
    }),

  sendInput: (
    chatId: string,
    taskId: string,
    input: { type: "text"; text: string } | { type: "close_stdin" },
  ) =>
    request(`${taskPath(chatId)}/${encodeURIComponent(taskId)}/input`, {
      method: "POST",
      body: JSON.stringify({
        submission_id: crypto.randomUUID(),
        input,
      }),
    }),

  retryTask: (chatId: string, taskId: string, authorizationAgentId: string) =>
    request<TaskRecord>(
      `${taskPath(chatId)}/${encodeURIComponent(taskId)}/retry`,
      {
        method: "POST",
        body: JSON.stringify({
          authorization_agent_id: authorizationAgentId,
        }),
      },
    ),

  getFullResult: (chatId: string, taskId: string) =>
    request<unknown>(
      `${taskPath(chatId)}/${encodeURIComponent(taskId)}/result`,
    ),

  activityPath: (chatId: string) => `${taskPath(chatId)}/activity`,
};
