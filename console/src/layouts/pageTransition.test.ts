import { afterEach, describe, expect, it, vi } from "vitest";
import {
  navigateWithPageTransition,
  runWithPageTransition,
} from "./pageTransition";

const { startTransition } = vi.hoisted(() => ({
  startTransition: vi.fn((update: () => void) => update()),
}));

vi.mock("react", async (importOriginal) => {
  const actual = await importOriginal<typeof import("react")>();
  return { ...actual, startTransition };
});

describe("navigateWithPageTransition", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    startTransition.mockClear();
    delete (document as Document & { startViewTransition?: unknown })
      .startViewTransition;
  });

  it("navigates without starting a document snapshot transition", () => {
    const navigate = vi.fn();
    const startViewTransition = vi.fn();
    Object.defineProperty(document, "startViewTransition", {
      configurable: true,
      value: startViewTransition,
    });

    navigateWithPageTransition(navigate, "/skills");

    expect(startViewTransition).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledWith("/skills");
  });

  it("preserves navigation options", () => {
    const navigate = vi.fn();

    navigateWithPageTransition(navigate, "/chat/chat-1", { replace: true });

    expect(navigate).toHaveBeenCalledWith("/chat/chat-1", {
      replace: true,
    });
  });

  it("runs non-route state updates without a document snapshot", () => {
    const update = vi.fn();
    const startViewTransition = vi.fn();
    Object.defineProperty(document, "startViewTransition", {
      configurable: true,
      value: startViewTransition,
    });

    runWithPageTransition(update);

    expect(startViewTransition).not.toHaveBeenCalled();
    expect(startTransition).not.toHaveBeenCalled();
    expect(update).toHaveBeenCalledOnce();
  });
});
