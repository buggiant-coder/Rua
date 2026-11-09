import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SidebarDragHandle from "./SidebarDragHandle";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});
function setup() {
  vi.useFakeTimers();
  const drop = vi.fn(),
    click = vi.fn();
  render(
    <nav>
      <section data-project-drop-area="work">
        <div data-project-drop-id="first">
          <SidebarDragHandle
            onDropTarget={drop}
            onClick={click}
            aria-expanded="true"
          >
            Project first
          </SidebarDragHandle>
        </div>
        <div data-testid="target" data-project-drop-id="second">
          <button aria-expanded="true">Project second</button>
          <div>Expanded conversation</div>
        </div>
        <div data-project-drop-id="third">Project third</div>
      </section>
      <section data-testid="empty" data-project-drop-area="empty" />
    </nav>,
  );
  const button = screen.getByRole("button", { name: "Project first" });
  const target = screen.getByTestId("target");
  vi.spyOn(
    target.querySelector("button")!,
    "getBoundingClientRect",
  ).mockReturnValue({ top: 20, height: 40 } as DOMRect);
  Object.defineProperty(document, "elementFromPoint", {
    configurable: true,
    value: vi.fn(() => target),
  });
  return { button, target, drop, click };
}
const touch = (x: number, y: number) => ({
  identifier: 1,
  clientX: x,
  clientY: y,
});
function drag(element: Element, type: string, y = 30) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.assign(event, {
    clientX: 10,
    clientY: y,
    dataTransfer: {
      setData: vi.fn(),
      setDragImage: vi.fn(),
      effectAllowed: "",
      dropEffect: "",
    },
  });
  fireEvent(element, event);
  return event;
}
it("keeps a short swipe scrollable and ordinary clicking toggles the title", () => {
  const { button, drop, click } = setup();
  fireEvent.touchStart(button, { touches: [touch(10, 10)] });
  expect(fireEvent.touchMove(button, { touches: [touch(10, 40)] })).toBe(true);
  act(() => vi.advanceTimersByTime(500));
  fireEvent.touchEnd(button, { changedTouches: [touch(10, 40)] });
  fireEvent.click(button);
  expect(drop).not.toHaveBeenCalled();
  expect(click).toHaveBeenCalledOnce();
  expect(document.querySelector("[data-sidebar-drag-preview]")).toBeNull();
});
it("long pressing the title shows a preview and insertion line without toggling after drop", () => {
  const { button, target, drop, click } = setup();
  fireEvent.touchStart(button, { touches: [touch(10, 10)] });
  act(() => vi.advanceTimersByTime(400));
  expect(button).toHaveAttribute("data-touch-dragging", "true");
  expect(
    document.querySelector("[data-sidebar-drag-preview]"),
  ).toHaveTextContent("Project first");
  expect(fireEvent.touchMove(button, { touches: [touch(10, 30)] })).toBe(false);
  expect(target).toHaveAttribute("data-drop-position", "before");
  fireEvent.touchEnd(button, { changedTouches: [touch(10, 30)] });
  expect(drop).toHaveBeenCalledWith({ area: "work", projectId: "second" });
  fireEvent.click(button);
  expect(click).not.toHaveBeenCalled();
  expect(target).not.toHaveAttribute("data-drop-position");
  expect(document.querySelector("[data-sidebar-drag-preview]")).toBeNull();
  act(() => vi.advanceTimersByTime(400));
  fireEvent.click(button);
  expect(click).toHaveBeenCalledOnce();
});
it.each([
  ["dragover", 30, "before", "second"],
  ["dragover", 60, "after", "third"],
  ["dragenter", 30, "before", "second"],
  ["dragenter", 60, "after", "third"],
])(
  "desktop %s at y=%s previews and commits the same %s position",
  (event, y, edge, before) => {
    const { button, target, drop } = setup();
    drag(button, "dragstart");
    expect(drag(target, event as string, y as number).defaultPrevented).toBe(
      true,
    );
    expect(target).toHaveAttribute("data-drop-position", edge as string);
    drag(target, "drop", y as number);
    expect(drop).toHaveBeenCalledWith({ area: "work", projectId: before });
    expect(target).not.toHaveAttribute("data-drop-position");
  },
);
it("dropping over expanded conversations inserts after the entire project", () => {
  const { button, target, drop } = setup();
  drag(button, "dragstart");
  drag(screen.getByText("Expanded conversation"), "dragover", 120);
  expect(target).toHaveAttribute("data-drop-position", "after");
  drag(screen.getByText("Expanded conversation"), "drop", 120);
  expect(drop).toHaveBeenCalledWith({ area: "work", projectId: "third" });
});
it("supports an empty destination and clears indicators outside the sidebar", () => {
  const { button, drop } = setup();
  drag(button, "dragstart");
  const empty = screen.getByTestId("empty");
  drag(empty, "dragover");
  expect(empty).toHaveAttribute("data-drop-position", "after");
  drag(document.body, "dragover");
  expect(empty).not.toHaveAttribute("data-drop-position");
  drag(empty, "drop");
  expect(drop).toHaveBeenCalledWith({ area: "empty", projectId: null });
});
it("Escape, touch cancellation and unmount discard the gesture", () => {
  const { button, target, drop } = setup();
  drag(button, "dragstart");
  drag(target, "dragover");
  fireEvent.keyDown(document, { key: "Escape" });
  expect(target).not.toHaveAttribute("data-drop-position");
  drag(target, "drop");
  fireEvent.touchStart(button, { touches: [touch(10, 10)] });
  act(() => vi.advanceTimersByTime(400));
  fireEvent.touchCancel(button);
  fireEvent.touchEnd(button, { changedTouches: [touch(10, 30)] });
  fireEvent.touchStart(button, { touches: [touch(10, 10)] });
  cleanup();
  act(() => vi.advanceTimersByTime(500));
  expect(drop).not.toHaveBeenCalled();
  expect(document.querySelector("[data-sidebar-drag-preview]")).toBeNull();
});
