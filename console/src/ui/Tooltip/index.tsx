import {
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type TouchEvent as ReactTouchEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { isMobile } from "@/utils/isMobile";
import "./tooltip.css";

type Placement =
  | "top"
  | "topLeft"
  | "topRight"
  | "top-start"
  | "top-end"
  | "bottom"
  | "bottomLeft"
  | "bottomRight"
  | "bottom-start"
  | "bottom-end"
  | "left"
  | "leftTop"
  | "leftBottom"
  | "left-start"
  | "left-end"
  | "right"
  | "rightTop"
  | "rightBottom"
  | "right-start"
  | "right-end";

interface TooltipProps {
  title?: ReactNode;
  placement?: Placement;
  mouseEnterDelay?: number;
  open?: boolean;
  children: ReactNode;
}

const GAP = 8;

function normalizePlacement(p: Placement): Placement {
  const map: Record<string, Placement> = {
    "top-start": "topLeft",
    "top-end": "topRight",
    "bottom-start": "bottomLeft",
    "bottom-end": "bottomRight",
    "left-start": "leftTop",
    "left-end": "leftBottom",
    "right-start": "rightTop",
    "right-end": "rightBottom",
  };
  return map[p] ?? p;
}

function getSide(p: Placement) {
  const n = normalizePlacement(p);
  if (n.startsWith("top")) return "top";
  if (n.startsWith("bottom")) return "bottom";
  if (n.startsWith("left")) return "left";
  return "right";
}

function calcPosition(trigger: DOMRect, tip: DOMRect, placementRaw: Placement) {
  const placement = normalizePlacement(placementRaw);
  const side = getSide(placement);
  const cx = trigger.left + trigger.width / 2;
  const cy = trigger.top + trigger.height / 2;

  let top = 0;
  let left = 0;

  if (side === "top") top = trigger.top - tip.height - GAP;
  else if (side === "bottom") top = trigger.bottom + GAP;
  else if (side === "left") left = trigger.left - tip.width - GAP;
  else left = trigger.right + GAP;

  if (side === "top" || side === "bottom") {
    if (placement === "top" || placement === "bottom")
      left = cx - tip.width / 2;
    else if (placement === "topLeft" || placement === "bottomLeft")
      left = trigger.left;
    else left = trigger.right - tip.width;
  } else {
    if (placement === "left" || placement === "right")
      top = cy - tip.height / 2;
    else if (placement === "leftTop" || placement === "rightTop")
      top = trigger.top;
    else top = trigger.bottom - tip.height;
  }

  let arrowTop: number;
  let arrowLeft: number;

  if (side === "top" || side === "bottom") {
    arrowLeft = Math.max(8, Math.min(tip.width - 8, cx - left)) - 4;
    arrowTop = side === "top" ? tip.height - 4 : -4;
  } else {
    arrowTop = Math.max(8, Math.min(tip.height - 8, cy - top)) - 4;
    arrowLeft = side === "left" ? tip.width - 5 : -3;
  }

  return { top, left, arrowTop, arrowLeft, side };
}

type Pos = ReturnType<typeof calcPosition>;

export default function Tooltip({
  title,
  placement = "top",
  mouseEnterDelay = 0,
  open,
  children,
}: TooltipProps) {
  const [hover, setHover] = useState(false);
  const [pos, setPos] = useState<Pos | null>(null);
  const triggerRef = useRef<HTMLSpanElement>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const showTimerRef = useRef<ReturnType<typeof setTimeout>>();
  const hideTimerRef = useRef<ReturnType<typeof setTimeout>>();
  const touchStartRef = useRef<{ x: number; y: number } | null>(null);
  const longPressTriggeredRef = useRef(false);

  const visible = open !== undefined ? open && !!title : hover && !!title;
  const mobile = isMobile();

  const updatePosition = useCallback(() => {
    const el = triggerRef.current;
    const tip = tooltipRef.current;
    if (!el || !tip) return;
    const triggerRect = el.getBoundingClientRect();
    const tipRect = tip.getBoundingClientRect();
    let pos = calcPosition(triggerRect, tipRect, placement);

    if (pos.top < 0 && getSide(placement) === "top") {
      pos = calcPosition(triggerRect, tipRect, "bottom");
    } else if (
      pos.top + tipRect.height > window.innerHeight &&
      getSide(placement) === "bottom"
    ) {
      pos = calcPosition(triggerRect, tipRect, "top");
    } else if (pos.left < 0 && getSide(placement) === "left") {
      pos = calcPosition(triggerRect, tipRect, "right");
    } else if (
      pos.left + tipRect.width > window.innerWidth &&
      getSide(placement) === "right"
    ) {
      pos = calcPosition(triggerRect, tipRect, "left");
    }
    setPos(pos);
  }, [placement]);

  useEffect(() => {
    if (!visible) {
      setPos(null);
      return;
    }
    updatePosition();
    window.addEventListener("scroll", updatePosition, true);
    window.addEventListener("resize", updatePosition);
    return () => {
      window.removeEventListener("scroll", updatePosition, true);
      window.removeEventListener("resize", updatePosition);
    };
  }, [visible, updatePosition]);

  const handleEnter = useCallback(() => {
    clearTimeout(showTimerRef.current);
    if (mouseEnterDelay > 0) {
      showTimerRef.current = setTimeout(
        () => setHover(true),
        mouseEnterDelay * 1000,
      );
    } else {
      setHover(true);
    }
  }, [mouseEnterDelay]);

  const handleLeave = useCallback(() => {
    clearTimeout(showTimerRef.current);
    clearTimeout(hideTimerRef.current);
    setHover(false);
  }, []);

  const handleTouchStart = useCallback(
    (event: ReactTouchEvent<HTMLSpanElement>) => {
      if (!mobile || open !== undefined || !title) return;
      const touch = event.touches[0];
      if (!touch) return;
      clearTimeout(showTimerRef.current);
      clearTimeout(hideTimerRef.current);
      touchStartRef.current = { x: touch.clientX, y: touch.clientY };
      longPressTriggeredRef.current = false;
      showTimerRef.current = setTimeout(() => {
        longPressTriggeredRef.current = true;
        setHover(true);
        hideTimerRef.current = setTimeout(() => setHover(false), 2500);
      }, 500);
    },
    [mobile, open, title],
  );

  const handleTouchMove = useCallback(
    (event: ReactTouchEvent<HTMLSpanElement>) => {
      const start = touchStartRef.current;
      const touch = event.touches[0];
      if (
        !start ||
        !touch ||
        Math.hypot(touch.clientX - start.x, touch.clientY - start.y) < 8
      ) {
        return;
      }
      clearTimeout(showTimerRef.current);
      touchStartRef.current = null;
    },
    [],
  );

  const handleTouchEnd = useCallback(() => {
    clearTimeout(showTimerRef.current);
    touchStartRef.current = null;
    if (!longPressTriggeredRef.current) {
      setHover(false);
    }
  }, []);

  const handleTouchCancel = useCallback(() => {
    clearTimeout(showTimerRef.current);
    clearTimeout(hideTimerRef.current);
    touchStartRef.current = null;
    longPressTriggeredRef.current = false;
    setHover(false);
  }, []);

  const handleClickCapture = useCallback(
    (event: ReactMouseEvent<HTMLSpanElement>) => {
      if (!mobile || !longPressTriggeredRef.current) return;
      event.preventDefault();
      event.stopPropagation();
      longPressTriggeredRef.current = false;
    },
    [mobile],
  );

  const handleContextMenu = useCallback(
    (event: ReactMouseEvent<HTMLSpanElement>) => {
      if (mobile && touchStartRef.current) {
        event.preventDefault();
      }
    },
    [mobile],
  );

  useEffect(
    () => () => {
      clearTimeout(showTimerRef.current);
      clearTimeout(hideTimerRef.current);
    },
    [],
  );

  return (
    <>
      <span
        ref={triggerRef}
        className="rua-tooltip-wrap"
        onMouseEnter={mobile ? undefined : handleEnter}
        onMouseLeave={mobile ? undefined : handleLeave}
        onTouchStart={mobile ? handleTouchStart : undefined}
        onTouchMove={mobile ? handleTouchMove : undefined}
        onTouchEnd={mobile ? handleTouchEnd : undefined}
        onTouchCancel={mobile ? handleTouchCancel : undefined}
        onClickCapture={mobile ? handleClickCapture : undefined}
        onContextMenu={mobile ? handleContextMenu : undefined}
        onClick={mobile ? undefined : handleLeave}
      >
        {children}
      </span>
      {visible &&
        createPortal(
          <div
            ref={tooltipRef}
            className={`rua-tooltip-popup${
              pos ? " rua-tooltip-popup--visible" : ""
            }`}
            style={
              pos
                ? { top: pos.top, left: pos.left }
                : { top: -9999, left: -9999 }
            }
            data-side={pos?.side}
            role="tooltip"
          >
            <div className="rua-tooltip-popup-content">{title}</div>
            <div
              className="rua-tooltip-popup-arrow"
              style={
                pos ? { top: pos.arrowTop, left: pos.arrowLeft } : undefined
              }
            />
          </div>,
          document.body,
        )}
    </>
  );
}
