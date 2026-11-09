import { beforeEach, describe, expect, it } from "vitest";
import {
  clearPendingSelectedVariant,
  getPendingSelectedVariant,
  setPendingSelectedVariant,
} from "../api";

describe("owned chat action api", () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it("stores, reads, and clears pending selected variants", () => {
    expect(getPendingSelectedVariant("chat-1")).toBeNull();

    setPendingSelectedVariant("chat/1", {
      turn_id: "turn-1",
      selected_variant_id: "variant-2",
    });
    expect(getPendingSelectedVariant("chat/1")).toEqual({
      turn_id: "turn-1",
      selected_variant_id: "variant-2",
    });

    clearPendingSelectedVariant("chat/1");
    expect(getPendingSelectedVariant("chat/1")).toBeNull();
  });

  it("ignores malformed pending selected variant storage", () => {
    sessionStorage.setItem("rua_selected_variant_chat-1", "{bad json");
    expect(getPendingSelectedVariant("chat-1")).toBeNull();

    sessionStorage.setItem(
      "rua_selected_variant_chat-1",
      JSON.stringify({ turn_id: "turn-1" }),
    );
    expect(getPendingSelectedVariant("chat-1")).toBeNull();
  });
});
