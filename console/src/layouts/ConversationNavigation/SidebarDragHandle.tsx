import { useEffect, useRef, type ButtonHTMLAttributes } from "react";
import { GripVertical } from "lucide-react";
import styles from "./index.module.less";

export type SidebarDropTarget = { area: string; projectId: string | null };
type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  onDropTarget: (target: SidebarDropTarget) => void;
  dragDisabled?: boolean;
  kind?: "project" | "section";
};

/** The title itself is draggable. Short touch swipes retain native scrolling. */
export default function SidebarDragHandle({
  onDropTarget,
  dragDisabled,
  kind = "project",
  children,
  className,
  onClick,
  ...props
}: Props) {
  const ref = useRef<HTMLButtonElement>(null);
  const drop = useRef(onDropTarget);
  const suppressClickUntil = useRef(0);
  drop.current = onDropTarget;
  useEffect(() => {
    const button = ref.current;
    const navigation = button?.closest<HTMLElement>("nav");
    if (!button || !navigation || dragDisabled) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let start: { x: number; y: number; identifier: number } | null = null;
    let active = false;
    let target: HTMLElement | null = null;
    let destination: SidebarDropTarget | null = null;
    let preview: HTMLElement | null = null;
    let dragImage: HTMLCanvasElement | null = null;
    const source = button.closest<HTMLElement>(
      kind === "project"
        ? "[data-project-drop-id]"
        : "[data-project-drop-area]",
    );
    const clearTarget = () => {
      target?.removeAttribute("data-drop-position");
      target = null;
      destination = null;
    };
    const reset = () => {
      clearTimeout(timer);
      if (active) suppressClickUntil.current = Date.now() + 350;
      start = null;
      active = false;
      delete button.dataset.touchDragging;
      source?.removeAttribute("data-drag-source");
      delete navigation.dataset.sidebarDragging;
      clearTarget();
      preview?.remove();
      preview = null;
      dragImage?.remove();
      dragImage = null;
    };
    const movePreview = (x: number, y: number) => {
      if (!preview) return;
      const beside = navigation.getBoundingClientRect().right + 12;
      const fitsBeside = beside + preview.offsetWidth + 8 <= innerWidth;
      const left = fitsBeside ? beside : x + 16;
      const top = fitsBeside
        ? y - preview.offsetHeight / 2
        : y - preview.offsetHeight - 48;
      preview.style.left = `${Math.max(
        8,
        Math.min(left, innerWidth - preview.offsetWidth - 8),
      )}px`;
      preview.style.top = `${Math.max(
        8,
        Math.min(top, innerHeight - preview.offsetHeight - 8),
      )}px`;
    };
    const begin = (x: number, y: number) => {
      active = true;
      navigation.dataset.sidebarDragging = "true";
      preview = document.createElement("div");
      preview.className = styles.dragPreview;
      preview.setAttribute("aria-hidden", "true");
      preview.dataset.sidebarDragPreview = "true";
      for (const child of button.childNodes)
        preview.append(child.cloneNode(true));
      document.body.append(preview);
      source?.setAttribute("data-drag-source", "true");
      movePreview(x, y);
    };
    const locate = (element: Element | null, x: number, y: number) => {
      clearTarget();
      movePreview(x, y);
      const area = element?.closest<HTMLElement>("[data-project-drop-area]");
      if (!area || !navigation.contains(area)) return;
      const row = element?.closest<HTMLElement>("[data-project-drop-id]");
      if (kind === "section") {
        if (
          !area.dataset.projectDropArea ||
          area.dataset.projectDropArea === "pinned" ||
          area === source
        )
          return;
        target = area;
        destination = { area: area.dataset.projectDropArea, projectId: null };
        target.dataset.dropPosition = "before";
      } else if (row) {
        if (row === source) return;
        const header = row.querySelector<HTMLElement>("[aria-expanded]") ?? row;
        const bounds = header.getBoundingClientRect();
        const after = y > bounds.top + bounds.height / 2;
        const next = row.nextElementSibling as HTMLElement | null;
        target = row;
        destination = {
          area: area.dataset.projectDropArea ?? "",
          projectId: after
            ? next?.dataset.projectDropId ?? null
            : row.dataset.projectDropId ?? null,
        };
        target.dataset.dropPosition = after ? "after" : "before";
      } else {
        target = area;
        destination = {
          area: area.dataset.projectDropArea ?? "",
          projectId: null,
        };
        target.dataset.dropPosition = "after";
      }
    };
    const scrollAtEdge = (y: number) => {
      const scroll = button.closest<HTMLElement>("[data-sidebar-scroll]");
      if (!scroll) return;
      const bounds = scroll.getBoundingClientRect();
      if (y < bounds.top + 36) scroll.scrollTop -= 16;
      else if (y > bounds.bottom - 36) scroll.scrollTop += 16;
    };
    const finish = () => {
      const selected = destination;
      reset();
      if (selected) drop.current(selected);
    };
    const dragStart = (event: DragEvent) => {
      if (!event.dataTransfer) return;
      reset();
      event.dataTransfer.setData("text/plain", button.textContent ?? "");
      event.dataTransfer.effectAllowed = "move";
      // Use one readable in-page preview instead of the browser's translucent button ghost.
      // WebKit cancels the drag if its canvas image is detached from the document.
      dragImage = document.createElement("canvas");
      dragImage.width = dragImage.height = 1;
      dragImage.style.cssText = "position:fixed;left:-10px;pointer-events:none";
      dragImage.setAttribute("aria-hidden", "true");
      document.body.append(dragImage);
      event.dataTransfer.setDragImage(dragImage, 0, 0);
      begin(event.clientX, event.clientY);
    };
    const dragOver = (event: DragEvent) => {
      if (!active) return;
      locate(
        event.target instanceof Element ? event.target : null,
        event.clientX,
        event.clientY,
      );
      if (destination) {
        event.preventDefault();
        if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
        scrollAtEdge(event.clientY);
      }
    };
    const dragDrop = (event: DragEvent) => {
      if (!active) return;
      locate(
        event.target instanceof Element ? event.target : null,
        event.clientX,
        event.clientY,
      );
      if (destination) event.preventDefault();
      finish();
    };
    const dragLeave = (event: DragEvent) => {
      if (active && !event.relatedTarget) clearTarget();
    };
    const cancelOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") reset();
    };
    const touchStart = (event: TouchEvent) => {
      reset();
      if (event.touches.length !== 1) return;
      const touch = event.touches[0];
      start = {
        x: touch.clientX,
        y: touch.clientY,
        identifier: touch.identifier,
      };
      timer = setTimeout(() => {
        if (!start) return;
        begin(start.x, start.y);
        button.dataset.touchDragging = "true";
      }, 400);
    };
    const touchMove = (event: TouchEvent) => {
      if (!start) return;
      if (event.touches.length !== 1) {
        reset();
        return;
      }
      const touch = event.touches[0];
      if (!active) {
        if (Math.hypot(touch.clientX - start.x, touch.clientY - start.y) > 8)
          reset();
        return;
      }
      event.preventDefault();
      locate(
        document.elementFromPoint(touch.clientX, touch.clientY),
        touch.clientX,
        touch.clientY,
      );
      scrollAtEdge(touch.clientY);
    };
    const touchEnd = (event: TouchEvent) => {
      if (active && start) {
        event.preventDefault();
        const touch = Array.from(event.changedTouches).find(
          (t) => t.identifier === start?.identifier,
        );
        if (touch)
          locate(
            document.elementFromPoint(touch.clientX, touch.clientY),
            touch.clientX,
            touch.clientY,
          );
        finish();
      } else reset();
    };
    button.addEventListener("dragstart", dragStart);
    button.addEventListener("dragend", reset);
    document.addEventListener("dragenter", dragOver);
    document.addEventListener("dragover", dragOver);
    document.addEventListener("drop", dragDrop);
    document.addEventListener("dragleave", dragLeave);
    document.addEventListener("keydown", cancelOnEscape);
    window.addEventListener("blur", reset);
    button.addEventListener("touchstart", touchStart, { passive: true });
    button.addEventListener("touchmove", touchMove, { passive: false });
    button.addEventListener("touchend", touchEnd, { passive: false });
    button.addEventListener("touchcancel", reset);
    return () => {
      reset();
      button.removeEventListener("dragstart", dragStart);
      button.removeEventListener("dragend", reset);
      document.removeEventListener("dragenter", dragOver);
      document.removeEventListener("dragover", dragOver);
      document.removeEventListener("drop", dragDrop);
      document.removeEventListener("dragleave", dragLeave);
      document.removeEventListener("keydown", cancelOnEscape);
      window.removeEventListener("blur", reset);
      button.removeEventListener("touchstart", touchStart);
      button.removeEventListener("touchmove", touchMove);
      button.removeEventListener("touchend", touchEnd);
      button.removeEventListener("touchcancel", reset);
    };
  }, [dragDisabled, kind]);
  return (
    <button
      {...props}
      ref={ref}
      type="button"
      draggable={!dragDisabled}
      className={`${className ?? styles.iconButton} ${styles.dragHandle}`}
      onClick={(event) => {
        if (Date.now() < suppressClickUntil.current) {
          event.preventDefault();
          event.stopPropagation();
        } else onClick?.(event);
      }}
      onContextMenu={(event) => event.preventDefault()}
    >
      {children ?? <GripVertical size={14} />}
    </button>
  );
}
