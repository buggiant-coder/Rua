import { describe, expect, it, vi } from "vitest";
import { showErrorModal } from "./error";

describe("showErrorModal", () => {
  it("renders friendly message and pretty raw JSON details", () => {
    const modal = { error: vi.fn() };

    showErrorModal(modal, {
      error: new Error('save failed - {"detail":{"field":"name"}}'),
      detailLabel: "Debug details",
    });

    expect(modal.error).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "save failed",
        width: 480,
        centered: true,
        content: expect.objectContaining({
          type: "div",
          props: expect.objectContaining({
            children: expect.arrayContaining([
              expect.objectContaining({
                type: "div",
                props: expect.objectContaining({
                  children: "save failed",
                }),
              }),
              expect.objectContaining({
                type: "details",
              }),
            ]),
          }),
        }),
      }),
    );
  });

  it("allows callers to override title, message, and detail", () => {
    const modal = { error: vi.fn() };

    showErrorModal(modal, {
      title: "Cannot save",
      message: "Name is invalid",
      detail: "raw backend payload",
    });

    const config = modal.error.mock.calls[0][0];
    expect(config.title).toBe("Cannot save");
    expect(config.content.props.children[0].props.children).toBe(
      "Name is invalid",
    );
    expect(
      config.content.props.children[1].props.children[1].props.children,
    ).toBe("raw backend payload");
  });

  it("uses a stable fallback title when no friendly message exists", () => {
    const modal = { error: vi.fn() };

    showErrorModal(modal, {});

    expect(modal.error).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Error",
      }),
    );
  });
});
