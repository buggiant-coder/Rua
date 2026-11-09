import {
  useLayoutEffect,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import {
  useFloating,
  useClick,
  useHover,
  useDismiss,
  useInteractions,
  useTransitionStyles,
  autoUpdate,
  offset,
  flip,
  shift,
  safePolygon,
  FloatingPortal,
  type Placement,
} from "@floating-ui/react";
import "./popover.css";

type PopoverTrigger = "click" | "hover";

function isPopoverTriggerArray(
  value: ReactNode | PopoverTrigger | PopoverTrigger[] | undefined,
): value is PopoverTrigger[] {
  return (
    Array.isArray(value) &&
    value.every((item) => item === "click" || item === "hover")
  );
}

function resolvePopoverTriggers(
  value: ReactNode | PopoverTrigger | PopoverTrigger[] | undefined,
): PopoverTrigger[] {
  if (value === "click") return ["click"];
  if (value === "hover") return ["hover"];
  if (!Array.isArray(value)) return ["click"];
  return value.filter(
    (item): item is PopoverTrigger => item === "click" || item === "hover",
  );
}

interface PopoverProps {
  trigger?: ReactNode | PopoverTrigger | PopoverTrigger[];
  content?: ReactNode;
  placement?: Placement;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  children?: ReactNode;
  overlayClassName?: string;
  surface?: "single" | "multi";
  positionReferenceRef?: RefObject<Element | null>;
  positionReferencePoint?: "bottom-start";
  offsetDistance?: number;
}

export default function Popover({
  trigger,
  content,
  placement = "bottom",
  open: controlledOpen,
  onOpenChange,
  children,
  overlayClassName,
  surface = "multi",
  positionReferenceRef,
  positionReferencePoint,
  offsetDistance = 8,
}: PopoverProps) {
  const [uncontrolledOpen, setUncontrolledOpen] = useState(false);
  const isControlled = controlledOpen !== undefined;
  const isOpen = isControlled ? controlledOpen : uncontrolledOpen;
  const triggerIsMode =
    trigger === "click" ||
    trigger === "hover" ||
    isPopoverTriggerArray(trigger);
  const triggerModes = resolvePopoverTriggers(trigger);
  const triggerElement = triggerIsMode ? children : children ?? trigger;

  const handleOpenChange = (next: boolean) => {
    if (!isControlled) setUncontrolledOpen(next);
    onOpenChange?.(next);
  };

  const { refs, floatingStyles, context } = useFloating({
    open: isOpen,
    onOpenChange: handleOpenChange,
    placement,
    middleware: [offset(offsetDistance), flip(), shift({ padding: 8 })],
    whileElementsMounted: autoUpdate,
  });

  useLayoutEffect(() => {
    const element = positionReferenceRef?.current;
    if (!element || positionReferencePoint !== "bottom-start") return;
    refs.setPositionReference({
      contextElement: element,
      getBoundingClientRect: () => {
        const rect = element.getBoundingClientRect();
        return DOMRect.fromRect({ x: rect.left, y: rect.bottom });
      },
    });
  }, [positionReferencePoint, positionReferenceRef, refs]);

  const click = useClick(context, {
    enabled: triggerModes.includes("click"),
  });
  const hover = useHover(context, {
    enabled: triggerModes.includes("hover"),
    delay: { open: 120, close: 80 },
    handleClose: safePolygon(),
    move: false,
  });
  const dismiss = useDismiss(context);
  const { getReferenceProps, getFloatingProps } = useInteractions([
    click,
    hover,
    dismiss,
  ]);
  const { isMounted, styles: transitionStyles } = useTransitionStyles(context, {
    duration: 150,
    initial: { opacity: 0 },
  });

  return (
    <>
      <span
        ref={refs.setReference}
        className="rua-ui-popover-trigger"
        {...getReferenceProps()}
      >
        {triggerElement}
      </span>
      {isMounted && (
        <FloatingPortal>
          <div
            ref={refs.setFloating}
            style={{ ...floatingStyles, ...transitionStyles }}
            className={[
              "rua-ui-popover",
              surface === "single" && "rua-ui-popover--single",
              overlayClassName,
            ]
              .filter(Boolean)
              .join(" ")}
            {...getFloatingProps()}
          >
            {content}
          </div>
        </FloatingPortal>
      )}
    </>
  );
}
