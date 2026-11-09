import { Minus, Plus, RotateCcw, X } from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type WheelEvent,
} from "react";
import { createPortal } from "react-dom";
import { useRenderProtection } from "../ChatRenderBlock/useRenderState";
import { useImageSource } from "./useImageSource";
import "./image-viewer.css";

const MIN_SCALE = 0.25;
const MAX_SCALE = 5;
const SCALE_STEP = 0.25;

type ImageViewerProps = {
  src: string;
  alt: string;
  imageClassName?: string;
  triggerClassName?: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  hideTrigger?: boolean;
};

type Point = {
  x: number;
  y: number;
};

function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

export default function ImageViewer({
  src,
  alt,
  imageClassName,
  triggerClassName,
  open: controlledOpen,
  onOpenChange,
  hideTrigger = false,
}: ImageViewerProps) {
  const imageSource = useImageSource(src);
  const [internalOpen, setInternalOpen] = useState(false);
  const open = controlledOpen ?? internalOpen;
  useRenderProtection(open);
  const setOpen = useCallback(
    (nextOpen: boolean) => {
      setInternalOpen(nextOpen);
      onOpenChange?.(nextOpen);
    },
    [onOpenChange],
  );
  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState<Point>({ x: 0, y: 0 });
  const dragRef = useRef<{
    pointerId: number;
    start: Point;
    offset: Point;
  } | null>(null);

  const reset = useCallback(() => {
    setScale(1);
    setOffset({ x: 0, y: 0 });
  }, []);

  const close = useCallback(() => {
    setOpen(false);
    reset();
  }, [reset]);

  const changeScale = useCallback((nextScale: number) => {
    const clamped = clampScale(nextScale);
    setScale(clamped);
    if (clamped <= 1) setOffset({ x: 0, y: 0 });
  }, []);

  useEffect(() => {
    if (!open) return;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
      if (event.key === "+" || event.key === "=") {
        changeScale(scale + SCALE_STEP);
      }
      if (event.key === "-") changeScale(scale - SCALE_STEP);
      if (event.key === "0") reset();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.body.style.overflow = previousOverflow;
    };
  }, [changeScale, close, open, reset, scale]);

  const handleWheel = (event: WheelEvent<HTMLDivElement>) => {
    event.preventDefault();
    changeScale(scale + (event.deltaY < 0 ? SCALE_STEP : -SCALE_STEP));
  };

  const handlePointerDown = (event: ReactPointerEvent<HTMLImageElement>) => {
    if (scale <= 1) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      start: { x: event.clientX, y: event.clientY },
      offset,
    };
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLImageElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    setOffset({
      x: drag.offset.x + event.clientX - drag.start.x,
      y: drag.offset.y + event.clientY - drag.start.y,
    });
  };

  const stopDragging = (event: ReactPointerEvent<HTMLImageElement>) => {
    if (dragRef.current?.pointerId === event.pointerId) {
      dragRef.current = null;
    }
  };

  return (
    <>
      {!hideTrigger && (
        <button
          type="button"
          className={["rua-image-viewer-trigger", triggerClassName]
            .filter(Boolean)
            .join(" ")}
          aria-label={`Open ${alt || "image"}`}
          onClick={() => setOpen(true)}
        >
          <img
            src={imageSource}
            alt={alt}
            className={imageClassName}
            draggable={false}
          />
        </button>
      )}
      {open &&
        createPortal(
          <div
            className="rua-image-viewer"
            role="dialog"
            aria-modal="true"
            aria-label={alt || "Image preview"}
            onClick={close}
            onWheel={handleWheel}
          >
            <div
              className="rua-image-viewer-toolbar"
              onClick={(event) => event.stopPropagation()}
            >
              <button
                type="button"
                aria-label="Zoom out"
                onClick={() => changeScale(scale - SCALE_STEP)}
              >
                <Minus aria-hidden="true" />
              </button>
              <button type="button" aria-label="Reset zoom" onClick={reset}>
                <RotateCcw aria-hidden="true" />
                <span>{Math.round(scale * 100)}%</span>
              </button>
              <button
                type="button"
                aria-label="Zoom in"
                onClick={() => changeScale(scale + SCALE_STEP)}
              >
                <Plus aria-hidden="true" />
              </button>
              <button type="button" aria-label="Close image" onClick={close}>
                <X aria-hidden="true" />
              </button>
            </div>
            <div className="rua-image-viewer-stage">
              <img
                src={imageSource}
                alt={alt}
                draggable={false}
                onClick={(event) => event.stopPropagation()}
                onDoubleClick={() => changeScale(scale === 1 ? 2 : 1)}
                onPointerDown={handlePointerDown}
                onPointerMove={handlePointerMove}
                onPointerUp={stopDragging}
                onPointerCancel={stopDragging}
                style={{
                  transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})`,
                  cursor: scale > 1 ? "grab" : "zoom-in",
                }}
              />
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
