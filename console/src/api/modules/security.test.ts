import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import {
  securityApi,
  type SkillScannerConfig,
  type ToolGuardConfig,
} from "./security";

describe("securityApi per-agent tool guard", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust agent-scoped tool-guard routes", async () => {
    const config: ToolGuardConfig = {
      enabled: true,
      custom_rules: [],
      disabled_rules: [],
    };

    await securityApi.getAgentToolGuard("agent-1");
    await securityApi.updateAgentToolGuard("agent-1", config);
    await securityApi.getAgentBuiltinRules("agent-1");
    await securityApi.resetAgentToolGuardToBuiltin("agent-1");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent-1/security/tool-guard",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent-1/security/tool-guard",
      {
        method: "PUT",
        body: JSON.stringify(config),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent-1/security/tool-guard/builtin-rules",
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agents/agent-1/security/tool-guard/reset-to-builtin",
      { method: "POST" },
    );
  });

  it("uses a separate Rust agent-scoped approval-mode route", async () => {
    await securityApi.getAgentApprovalMode("agent-1");
    await securityApi.updateAgentApprovalMode("agent-1", {
      approval_mode: "hands_off",
    });

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent-1/security/approval-mode",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent-1/security/approval-mode",
      {
        method: "PUT",
        body: JSON.stringify({ approval_mode: "hands_off" }),
      },
    );
  });

  it("keeps the legacy process-wide update route explicit", async () => {
    const config: ToolGuardConfig = {
      enabled: false,
      custom_rules: [],
      disabled_rules: ["dangerous"],
    };

    await securityApi.updateToolGuard(config);

    expect(request).toHaveBeenCalledWith("/config/security/tool-guard", {
      method: "PUT",
      body: JSON.stringify(config),
    });
  });
});

describe("securityApi file guard and skill scanner", () => {
  afterEach(() => vi.clearAllMocks());

  it("round-trips file guard through Rust config security routes", async () => {
    const body = { enabled: true, paths: ["/secret"] };

    await securityApi.getFileGuard();
    await securityApi.updateFileGuard(body);

    expect(request).toHaveBeenNthCalledWith(1, "/config/security/file-guard");
    expect(request).toHaveBeenNthCalledWith(2, "/config/security/file-guard", {
      method: "PUT",
      body: JSON.stringify(body),
    });
  });

  it("round-trips skill scanner config and history routes", async () => {
    const config: SkillScannerConfig = {
      mode: "block",
      timeout: 30,
      whitelist: [],
    };

    await securityApi.getSkillScanner();
    await securityApi.updateSkillScanner(config);
    await securityApi.getBlockedHistory();
    await securityApi.clearBlockedHistory();
    await securityApi.removeBlockedEntry(2);

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/config/security/skill-scanner",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/config/security/skill-scanner",
      {
        method: "PUT",
        body: JSON.stringify(config),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/config/security/skill-scanner/blocked-history",
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/config/security/skill-scanner/blocked-history",
      { method: "DELETE" },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/config/security/skill-scanner/blocked-history/2",
      { method: "DELETE" },
    );
  });

  it("encodes whitelist skill names and triggers the Rust scanner route", async () => {
    await securityApi.addToWhitelist("my/skill", "abc123");
    await securityApi.removeFromWhitelist("my/skill");
    await securityApi.triggerScan();

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/config/security/skill-scanner/whitelist",
      {
        method: "POST",
        body: JSON.stringify({
          skill_name: "my/skill",
          content_hash: "abc123",
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/config/security/skill-scanner/whitelist/my%2Fskill",
      { method: "DELETE" },
    );
    expect(request).toHaveBeenNthCalledWith(3, "/skills/scanner/scan", {
      method: "POST",
    });
  });

  it("covers NetworkSecret backup helper routes owned by Rust sync security", async () => {
    await securityApi.exportNetworkSecretBackup("pass");
    await securityApi.renderNetworkSecretBackupQr("armored");

    expect(request).toHaveBeenNthCalledWith(1, "/security/master-key-export", {
      method: "POST",
      body: JSON.stringify({ passphrase: "pass" }),
    });
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/security/master-key-export/qr",
      {
        method: "POST",
        body: JSON.stringify({ text: "armored" }),
      },
    );
  });
});
