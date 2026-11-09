export * from "./types";

export { request } from "./request";

export { getApiUrl, getApiToken } from "./config";

import { rootApi } from "./modules/root";
import { channelApi } from "./modules/channel";
import { heartbeatApi } from "./modules/heartbeat";
import { cronJobApi } from "./modules/cronjob";
import { chatApi } from "./modules/chat";
import { consoleApi } from "./modules/console";
import { envApi } from "./modules/env";
import { providerApi } from "./modules/provider";
import { skillApi } from "./modules/skill";
import { agentApi } from "./modules/agent";
import { agentStatsApi } from "./modules/agentStats";
import { agentsApi } from "./modules/agents";
import { workspaceApi } from "./modules/workspace";
import { localModelApi } from "./modules/localModel";
import { mcpApi } from "./modules/mcp";
import { acpApi } from "./modules/acp";
import { tokenUsageApi } from "./modules/tokenUsage";
import { securityApi } from "./modules/security";
import { userTimezoneApi } from "./modules/userTimezone";
import { languageApi } from "./modules/language";
import { memoryApi } from "./modules/memory";
import { accessControlApi } from "./modules/accessControl";
import { terminalApi } from "./modules/terminal";
import { syncTransferApi } from "./modules/syncTransfer";
import { taskApi } from "./modules/task";
import { storageApi } from "./modules/storage";

export const api = {
  ...rootApi,

  ...channelApi,
  ...accessControlApi,

  ...heartbeatApi,

  ...cronJobApi,

  ...chatApi,
  ...consoleApi,

  ...envApi,

  ...providerApi,

  ...agentApi,

  ...agentStatsApi,

  ...skillApi,

  ...workspaceApi,

  ...localModelApi,

  ...mcpApi,

  ...acpApi,

  ...tokenUsageApi,

  ...memoryApi,

  ...securityApi,

  ...userTimezoneApi,

  ...languageApi,

  listTerminalSessions: terminalApi.listSessions,
  openTerminalSession: terminalApi.openSession,
  attachTerminalSession: terminalApi.attachSession,
  takeTerminalInputLease: terminalApi.takeInputLease,
  sendTerminalInput: terminalApi.sendInput,
  killTerminalSession: terminalApi.killSession,

  ...syncTransferApi,

  ...taskApi,
  ...storageApi,
};

export default api;

export { agentsApi };
export { sharedMcpApi } from "./modules/sharedMcp";
export { terminalApi } from "./modules/terminal";
export { syncTransferApi } from "./modules/syncTransfer";
export { taskApi } from "./modules/task";
export { userInboxApi } from "./modules/userInbox";
