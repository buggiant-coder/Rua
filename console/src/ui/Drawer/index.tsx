import { useState, useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import type { ReactNode } from "react";
import { useHistoryDismiss } from "@/hooks/useHistoryDismiss";
import { isMobile } from "@/utils/isMobile";
import "./drawer.css";

type Placement = "right" | "left" | "top" | "bottom";

interface DrawerProps {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  footer?: ReactNode;
  children?: ReactNode;
  placement?: Placement;
  width?: number | string;
  className?: string;
  maskClosable?: boolean;
  destroyOnClose?: boolean;
  closable?: boolean;
  rootClassName?: string;
  styles?: Record<string, unknown>;
  afterOpenChange?: (open: boolean) => void;
}

const FOCUSABLE =
  'a[href],button:not([disabled]),textarea:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';

function useFocusTrap(
  containerRef: React.RefObject<HTMLElement | null>,
  active: boolean,
) {
  useEffect(() => {
    if (!active) return;
    const el = containerRef.current;
    if (!el) return;

    const prev = document.activeElement as HTMLElement | null;
    const focusables = () =>
      Array.from(el.querySelectorAll<HTMLElement>(FOCUSABLE));
    const first = focusables()[0];
    first?.focus();

    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== "Tab") return;
      const items = focusables();
      if (items.length === 0) return;
      const firstItem = items[0];
      const lastItem = items[items.length - 1];
      if (e.shiftKey) {
        if (document.activeElement === firstItem) {
          e.preventDefault();
          lastItem.focus();
        }
      } else {
        if (document.activeElement === lastItem) {
          e.preventDefault();
          firstItem.focus();
        }
      }
    }

    el.addEventListener("keydown", onKeyDown);
    return () => {
      el.removeEventListener("keydown", onKeyDown);
      prev?.focus();
    };
  }, [active, containerRef]);
}

function Drawer({
  open,
  onClose,
  title,
  footer,
  children,
  placement = "right",
  width,
  className,
  maskClosable = true,
}: DrawerProps) {
  const [mounted, setMounted] = useState(open);
  const [visible, setVisible] = useState(false);
  const drawerRef = useRef<HTMLDivElement>(null);
  const historyDismissOpen = isMobile() && open;

  useEffect(() => {
    if (open) {
      setMounted(true);
      let secondFrame = 0;
      const firstFrame = requestAnimationFrame(() => {
        secondFrame = requestAnimationFrame(() => setVisible(true));
      });
      return () => {
        cancelAnimationFrame(firstFrame);
        cancelAnimationFrame(secondFrame);
      };
    } else {
      setVisible(false);
      const timer = setTimeout(() => setMounted(false), 200);
      return () => clearTimeout(timer);
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  useFocusTrap(drawerRef, open);
  useHistoryDismiss(historyDismissOpen, onClose);

  if (!mounted) return null;

  const isHorizontal = placement === "left" || placement === "right";
  const sizeStyle: React.CSSProperties = isHorizontal
    ? { width: width ?? 520 }
    : { height: width ?? 520 };

  const maskCls = [
    "rua-ui-drawer-mask",
    visible && "rua-ui-drawer-mask--visible",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  return createPortal(
    <div className={maskCls} onClick={maskClosable ? onClose : undefined}>
      <div
        ref={drawerRef}
        className={`rua-ui-drawer rua-ui-drawer--${placement} rua-drawer-content-wrapper`}
        style={sizeStyle}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        {title != null && (
          <div className="rua-ui-drawer-header">
            <h3 className="rua-ui-drawer-title">{title}</h3>
            <button
              className="rua-ui-drawer-close"
              onClick={onClose}
              aria-label="Close"
            >
              ×
            </button>
          </div>
        )}
        <div className="rua-ui-drawer-body">{children}</div>
        {footer != null && <div className="rua-ui-drawer-footer">{footer}</div>}
      </div>
    </div>,
    document.body,
  );
}

export default Drawer;
export type { DrawerProps, Placement as DrawerPlacement };
