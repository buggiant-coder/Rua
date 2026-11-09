import { beforeEach, describe, expect, it, vi } from "vitest";

import { activateRuaUri, consolePathFromRuaUri } from "./ruaUri";

describe("Rua URI activation", () => {
  beforeEach(() => {
    history.replaceState({}, "", "/console/settings");
  });

  it("maps an opaque Rua URI to the matching Console route", () => {
    expect(
      consolePathFromRuaUri(
        "rua://agents/researcher/chats/chat-42?turn=7#result",
      ),
    ).toBe("/console/agents/researcher/chats/chat-42?turn=7#result");
  });

  it("rejects malformed and non-Rua URIs without navigation", () => {
    for (const uri of [
      "",
      " rua://agents/default/chats/a",
      "https://rua.example",
      "rua:/missing-host",
      "rua://user:secret@agents/default",
    ]) {
      expect(consolePathFromRuaUri(uri)).toBeNull();
    }
  });

  it("focuses and navigates on a valid activation", () => {
    const focus = vi.spyOn(window, "focus").mockImplementation(() => {});
    const popstate = vi.fn();
    window.addEventListener("popstate", popstate, { once: true });

    expect(activateRuaUri("rua://projects/project-1/chats/chat-2")).toBe(
      true,
    );

    expect(focus).toHaveBeenCalledTimes(1);
    expect(window.location.pathname).toBe(
      "/console/projects/project-1/chats/chat-2",
    );
    expect(popstate).toHaveBeenCalledTimes(1);
  });
});
