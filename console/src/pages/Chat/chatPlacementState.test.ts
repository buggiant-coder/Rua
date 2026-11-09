import { describe, expect, it } from "vitest";
import type { ACPChatStatus, ChatHistory } from "@/api/types";
import {
  acpStatusFromConversationControl,
  acpStatusMatchesAuthorityPlacement,
  chatAcpBindingFromMeta,
  chatAuthorityPlacementFromMeta,
  removeAcpBindingFromHistory,
  resolveChatPlacement,
} from "./chatPlacementState";

function inactiveStatus(targetDeviceId: string | null = null): ACPChatStatus {
  return {
    active: false,
    runner: null,
    session_id: null,
    cwd: null,
    chat_id: "chat-1",
    workspace_dir: "/workspace",
    target_device_id: targetDeviceId,
  };
}

describe("chatPlacementState", () => {
  it("parses backend authority placement and ACP binding metadata", () => {
    const meta = {
      authority_placement: { device_id: "device-b" },
      acp_binding: {
        kind: "acp_passthrough",
        device_id: "device-b",
        runner: "codex",
        cwd: "/workspace",
        runner_session_id: "runner-session-1",
      },
    };

    expect(chatAuthorityPlacementFromMeta(meta)).toBe("device-b");
    expect(chatAcpBindingFromMeta(meta)).toEqual({
      deviceId: "device-b",
      runner: "codex",
      cwd: "/workspace",
      runnerSessionId: "runner-session-1",
    });
  });

  it("uses backend metadata as the authority placement", () => {
    expect(
      resolveChatPlacement({
        chatId: "chat-1",
        meta: {
          authority_placement: { device_id: "device-b" },
        },
        polledAcpStatus: inactiveStatus("device-b"),
      }),
    ).toMatchObject({
      authorityPlacementDeviceId: "device-b",
      authorityPlacementLocked: false,
    });
  });

  it("reads ACP status only when the control binding matches authority", () => {
    expect(
      acpStatusFromConversationControl(
        {
          authority_device_id: "device-b",
          placement: {
            authority_device_id: "device-b",
            switchable: false,
            lock_reason: "acp_passthrough",
          },
          active_turn: null,
          queue: { entries: [] },
          acp: {
            active: true,
            binding: {
              device_id: "device-b",
              runner: "codex",
              cwd: "/workspace",
              runner_session_id: "runner-session-1",
            },
          },
          allowed_controls: {
            stop: false,
            queue: true,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: false,
            enter_acp: false,
            exit_acp: true,
          },
        },
        "chat-1",
      ),
    ).toMatchObject({
      active: true,
      runner: "codex",
      target_device_id: "device-b",
    });
  });

  it("ignores an ACP control binding for a non-authority device", () => {
    expect(
      acpStatusFromConversationControl(
        {
          authority_device_id: "device-b",
          placement: {
            authority_device_id: "device-b",
            switchable: false,
            lock_reason: "acp_passthrough",
          },
          active_turn: null,
          queue: { entries: [] },
          acp: {
            active: true,
            binding: {
              device_id: "device-c",
              runner: "codex",
            },
          },
          allowed_controls: {
            stop: false,
            queue: true,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: false,
            enter_acp: false,
            exit_acp: true,
          },
        },
        "chat-1",
      ),
    ).toBeNull();
  });

  it("reports authority unavailable when backend placement is missing", () => {
    expect(
      resolveChatPlacement({
        chatId: "chat-1",
        meta: {},
        polledAcpStatus: inactiveStatus(null),
      }),
    ).toMatchObject({
      authorityPlacementDeviceId: null,
      authorityPlacementLocked: false,
    });
  });

  it("does not let an ACP binding establish a missing authority placement", () => {
    expect(
      resolveChatPlacement({
        chatId: "chat-1",
        meta: {
          acp_binding: {
            kind: "acp_passthrough",
            device_id: "device-b",
            runner: "codex",
            cwd: "/workspace",
            runner_session_id: "runner-session-1",
          },
        },
        polledAcpStatus: inactiveStatus("device-b"),
      }),
    ).toMatchObject({
      authorityPlacementDeviceId: null,
      acpActive: false,
      authorityPlacementLocked: false,
      lockReason: null,
      acpStatus: null,
    });
  });

  it("does not lock authority placement while checking ACP status for the target", () => {
    const state = resolveChatPlacement({
      chatId: "chat-1",
      meta: {
        authority_placement: { device_id: "device-b" },
      },
      polledAcpStatus: null,
      currentDeviceId: "device-a",
    });

    expect(state.acpStatusUnknown).toBe(true);
    expect(state.authorityPlacementLocked).toBe(false);
    expect(state.lockReason).toBeNull();
  });

  it("ignores ACP polling status for another target without locking placement", () => {
    const state = resolveChatPlacement({
      chatId: "chat-1",
      meta: {
        authority_placement: { device_id: "device-b" },
      },
      polledAcpStatus: {
        active: true,
        runner: "codex",
        session_id: "runner-session-1",
        cwd: "/workspace",
        chat_id: "chat-1",
        workspace_dir: "/workspace",
        target_device_id: "device-c",
      },
      currentDeviceId: "device-a",
    });

    expect(state.acpStatus).toBeNull();
    expect(state.acpStatusUnknown).toBe(true);
    expect(state.authorityPlacementLocked).toBe(false);
    expect(state.lockReason).toBeNull();
  });

  it("matches local ACP status to the explicit current-device authority", () => {
    expect(
      acpStatusMatchesAuthorityPlacement(
        {
          active: true,
          runner: "codex",
          session_id: "runner-session-1",
          cwd: "/workspace",
          chat_id: "chat-1",
          workspace_dir: "/workspace",
          target_device_id: null,
        },
        "device-a",
        "device-a",
      ),
    ).toBe(true);
  });

  it("preserves authority placement when clearing ACP binding locally", () => {
    const history: ChatHistory = {
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-a" },
        acp_binding: {
          kind: "acp_passthrough",
          device_id: "device-a",
          runner: "codex",
        },
      },
    };

    expect(removeAcpBindingFromHistory(history)?.meta).toEqual({
      authority_placement: { device_id: "device-a" },
    });
  });

  it("keeps authority placement when clearing ACP binding locally", () => {
    const history: ChatHistory = {
      status: "idle",
      messages: [],
      meta: {
        authority_placement: { device_id: "device-b" },
        acp_binding: {
          kind: "acp_passthrough",
          device_id: "device-b",
          runner: "codex",
        },
      },
    };

    expect(removeAcpBindingFromHistory(history)?.meta).toEqual({
      authority_placement: { device_id: "device-b" },
    });
  });
});
