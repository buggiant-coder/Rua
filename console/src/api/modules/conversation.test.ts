import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));
vi.mock("../consoleTransport", () => ({ consoleFetch: vi.fn() }));
vi.mock("../authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer fixture" }),
}));
vi.mock("../config", () => ({ getApiUrl: (path: string) => `/api${path}` }));

import { consoleFetch } from "../consoleTransport";
import { request } from "../request";
import { conversationApi, type ConversationApiTarget } from "./conversation";

describe("conversationApi", () => {
  afterEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
  });

  it.each<ConversationApiTarget>([
    { agentId: "agent/one" },
    { projectId: "project/one", agentId: "agent/two" },
  ])(
    "uses the same saved Chat for history, configuration, controls and live viewing",
    async (context) => {
      const id = "chat/one";
      const path = "/chats/chat%2Fone";
      const model = { provider_id: "fixture", model: "chosen" };
      const stop = { turn_id: "turn/one", authority_epoch: "epoch-1" };
      const turn = { before_turn_id: "turn/one" };
      const sendNow = { command_id: "command-1", authority_epoch: "epoch-1" };

      await conversationApi.getHistory(id, { turnLimit: 20 });
      await conversationApi.previewConfiguration(context, id);
      await conversationApi.applyConfiguration(context, id, {
        current: { epoch: "old" },
        candidate: { epoch: "new" },
        applied: false,
      });
      await conversationApi.updateModel(context, id, model);
      await conversationApi.getRunningConfig(context, id);
      await conversationApi.stopChat(context, id, stop);
      await conversationApi.selectVariant(
        context,
        id,
        "turn/one",
        "variant/one",
      );
      await conversationApi.rollback(context, id, turn);
      await conversationApi.branch(context, id, turn);
      await conversationApi.sendNowQueuedInput(
        context,
        id,
        "input/one",
        sendNow,
      );
      await conversationApi.getApprovalSnapshot(id);
      await conversationApi.getAcpPermissionSnapshot(id);

      expect(vi.mocked(request).mock.calls).toEqual([
        [`${path}?turn_limit=20`],
        [`${path}/configuration`],
        [
          `${path}/configuration`,
          {
            method: "PUT",
            body: JSON.stringify({
              expected_epoch: "old",
              candidate_epoch: "new",
            }),
          },
        ],
        [path, { method: "PUT", body: JSON.stringify({ model }) }],
        [`${path}/running-config`],
        [
          `${path}/console/stop`,
          { method: "POST", body: JSON.stringify(stop) },
        ],
        [
          path,
          {
            method: "PUT",
            body: JSON.stringify({
              turn_id: "turn/one",
              selected_variant_id: "variant/one",
            }),
          },
        ],
        [`${path}/rollback`, { method: "POST", body: JSON.stringify(turn) }],
        [`${path}/branch`, { method: "POST", body: JSON.stringify(turn) }],
        [
          `${path}/queue/input%2Fone/send-now`,
          { method: "POST", body: JSON.stringify(sendNow) },
        ],
        [`${path}/approvals/snapshot`],
        [`${path}/acp/permissions`],
      ]);
      const signal = new AbortController().signal;
      await conversationApi.openLive(id, { viewer_id: "viewer" }, signal);
      expect(consoleFetch).toHaveBeenCalledWith(
        `${path}/console/live`,
        expect.objectContaining({
          method: "POST",
          signal,
          body: JSON.stringify({ viewer_id: "viewer" }),
        }),
      );
    },
  );

  it("sends explicit draft context on first submit and addresses later input by ID", async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(null));
    vi.stubGlobal("fetch", fetch);
    for (const body of [
      {
        submission_id: "first",
        agent_id: "agent/one",
        project_id: "project/two",
      },
      { submission_id: "second" },
    ]) {
      await conversationApi.submit("chat/one", body);
      expect(fetch).toHaveBeenLastCalledWith(
        "/api/chats/chat%2Fone/console/submit",
        expect.objectContaining({ method: "POST", body: JSON.stringify(body) }),
      );
    }
  });
});
