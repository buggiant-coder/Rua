import { request } from "../request";
import type {
  ExecutionPosition,
  ResolvedExecutionDirectory,
} from "../types/chat";
import type { ProjectDirectoryListing } from "../types/project";

export const executionDirectoryApi = {
  browse: (deviceId: string, path: string) =>
    request<ProjectDirectoryListing>(
      `/devices/${encodeURIComponent(
        deviceId,
      )}/execution-directory?path=${encodeURIComponent(path)}`,
    ),
  update: (chatId: string, position: ExecutionPosition) =>
    request<ResolvedExecutionDirectory>(
      `/chats/${encodeURIComponent(chatId)}/execution-directory`,
      { method: "PATCH", body: JSON.stringify(position) },
    ),
};

export async function validateExecutionPosition(
  position: ExecutionPosition,
): Promise<ExecutionPosition> {
  if (position.path === undefined) return position;
  const listing = await executionDirectoryApi.browse(
    position.device_id,
    position.path.trim(),
  );
  return { device_id: position.device_id, path: listing.path };
}
