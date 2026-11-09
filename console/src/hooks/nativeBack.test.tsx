import { useState } from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import Drawer from "@/ui/Drawer";
import Modal from "@/ui/Modal";
import { __resetMobileDetectionForTests } from "@/utils/isMobile";

afterEach(() => {
  vi.restoreAllMocks();
  document.documentElement.classList.remove("mobile-mode");
  __resetMobileDetectionForTests();
});

it("native Back closes a modal above a drawer, then the drawer, without route changes", () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Android Mobile/0.1.0",
  );
  __resetMobileDetectionForTests();
  const pushSpy = vi.spyOn(window.history, "pushState");
  const backSpy = vi.spyOn(window.history, "back");
  const route = window.location.href;
  function Layers() {
    const [drawerOpen, setDrawerOpen] = useState(true);
    const [modalOpen, setModalOpen] = useState(false);
    return (
      <>
        <output data-testid="layers">{`${drawerOpen}/${modalOpen}`}</output>
        <Drawer open={drawerOpen} onClose={() => setDrawerOpen(false)}>
          <button onClick={() => setModalOpen(true)}>Open modal</button>
        </Drawer>
        <Modal open={modalOpen} onClose={() => setModalOpen(false)}>
          Modal content
        </Modal>
      </>
    );
  }
  render(<Layers />);
  fireEvent.click(screen.getByText("Open modal"));
  expect(screen.getByTestId("layers")).toHaveTextContent("true/true");
  act(() => window.dispatchEvent(new Event("rua:back")));
  expect(screen.getByTestId("layers")).toHaveTextContent("true/false");
  act(() => window.dispatchEvent(new Event("rua:back")));
  expect(screen.getByTestId("layers")).toHaveTextContent("false/false");
  act(() => window.dispatchEvent(new Event("rua:back")));
  expect(screen.getByTestId("layers")).toHaveTextContent("false/false");
  expect(window.location.href).toBe(route);
  expect(pushSpy).not.toHaveBeenCalled();
  expect(backSpy).not.toHaveBeenCalled();
});
