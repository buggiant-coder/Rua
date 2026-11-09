import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import Modal from ".";

describe("Modal", () => {
  it("does not close when a pointer drag starts inside and ends on the mask", () => {
    const onClose = vi.fn();
    render(
      <Modal open onClose={onClose}>
        <input aria-label="Title" />
      </Modal>,
    );

    const input = screen.getByRole("textbox", { name: "Title" });
    const mask = document.querySelector(".rua-ui-modal-mask");
    expect(mask).not.toBeNull();

    fireEvent.pointerDown(input);
    fireEvent.pointerUp(mask as Element);
    fireEvent.click(mask as Element);

    expect(onClose).not.toHaveBeenCalled();
  });

  it("closes when the pointer press and click both happen on the mask", () => {
    const onClose = vi.fn();
    render(<Modal open onClose={onClose} />);
    const mask = document.querySelector(".rua-ui-modal-mask") as Element;

    fireEvent.pointerDown(mask);
    fireEvent.click(mask);

    expect(onClose).toHaveBeenCalledOnce();
  });
});
