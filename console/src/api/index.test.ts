import { describe, expect, it } from "vitest";
import api, { agentsApi, sharedMcpApi } from "./index";
import { rootApi } from "./modules/root";
import { agentApi } from "./modules/agent";
import { agentStatsApi } from "./modules/agentStats";
import { heartbeatApi } from "./modules/heartbeat";
import { chatApi } from "./modules/chat";
import { skillApi } from "./modules/skill";
import { securityApi } from "./modules/security";
import { languageApi } from "./modules/language";
import { terminalApi } from "./modules/terminal";

describe("api aggregate", () => {
  it("exposes root, agent, agent-stats, chat, skill, security, language, and terminal APIs", () => {
    expect(api.getVersion).toBe(rootApi.getVersion);
    expect(api.getTTSConfig).toBe(agentApi.getTTSConfig);
    expect(api.getAgentStats).toBe(agentStatsApi.getAgentStats);
    expect(api.getHeartbeatConfig).toBe(heartbeatApi.getHeartbeatConfig);
    expect(api.runHeartbeatNow).toBe(heartbeatApi.runHeartbeatNow);
    expect(api.listChats).toBe(chatApi.listChats);
    expect(api.listSkills).toBe(skillApi.listSkills);
    expect(api.getSecurityConfig).toBe(securityApi.getSecurityConfig);
    expect(api.getLanguage).toBe(languageApi.getLanguage);
    expect(api.listTerminalSessions).toBe(terminalApi.listSessions);
  });

  it("keeps direct named API exports for multi-agent and shared MCP callers", () => {
    expect(agentsApi.listAgents).toEqual(expect.any(Function));
    expect(sharedMcpApi.list).toEqual(expect.any(Function));
    expect(api).not.toHaveProperty("listAgents");
    expect(api).not.toHaveProperty("sharedMcpApi");
  });
});
