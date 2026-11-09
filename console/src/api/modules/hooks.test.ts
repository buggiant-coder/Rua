import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { hooksApi } from "./hooks";

describe("hooksApi", () => {
  afterEach(() => vi.clearAllMocks());

  it("keeps registration, defaults, Chat adoption, and Runs on distinct routes", async () => {
    await hooksApi.listDefinitions("agent/one");
    await hooksApi.updateDefault("agent/one", "owner:hook/value", true);
    await hooksApi.getChatConfiguration("agent/one", "chat/two");
    await hooksApi.listRuns("agent/one", "chat/two");
    await hooksApi.listContributions("agent/one", "chat/two", "turn/three");
    await hooksApi.preview("agent/one", { agent: [] });
    await hooksApi.listProjectDefinitions("project/one");
    await hooksApi.updateProjectDefault(
      "project/one",
      "project:project/one/audit",
      true,
    );

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent%2Fone/hooks/definitions",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2Fone/hooks/defaults",
      {
        method: "PUT",
        body: JSON.stringify({
          definition_id: "owner:hook/value",
          enabled: true,
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent%2Fone/hooks/chats/chat%2Ftwo/configuration",
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agents/agent%2Fone/hooks/runs?chat_id=chat%2Ftwo",
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/agents/agent%2Fone/hooks/contributions?chat_id=chat%2Ftwo&turn_id=turn%2Fthree",
    );
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/agents/agent%2Fone/hooks/effective-preview",
      { method: "POST", body: JSON.stringify({ agent: [] }) },
    );
    expect(request).toHaveBeenNthCalledWith(
      7,
      "/projects/project%2Fone/hooks/definitions",
    );
    expect(request).toHaveBeenNthCalledWith(
      8,
      "/projects/project%2Fone/hooks/defaults",
      {
        method: "PUT",
        body: JSON.stringify({
          definition_id: "project:project/one/audit",
          enabled: true,
        }),
      },
    );
  });
});
