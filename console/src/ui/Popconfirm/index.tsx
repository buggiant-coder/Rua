import { useState, type ReactNode } from "react";
import {
  useFloating,
  useClick,
  useDismiss,
  useInteractions,
  useTransitionStyles,
  autoUpdate,
  offset,
  flip,
  shift,
  FloatingPortal,
  type Placement,
} from "@floating-ui/react";
import Btn from "../Btn";
import "./popconfirm.css";

interface PopconfirmProps {
  title: ReactNode;
  description?: ReactNode;
  onConfirm?: () => void | Promise<void>;
  onCancel?: () => void;
  okText?: string;
  cancelText?: string;
  danger?: boolean;
  placement?: Placement;
  children: ReactNode;
  disabled?: boolean;
}

export default function Popconfirm({
  title,
  onConfirm,
  onCancel,
  okText = "确定",
  cancelText = "取消",
  danger = false,
  placement = "top",
  children,
}: PopconfirmProps) {
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);

  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange: setOpen,
    placement,
    middleware: [offset(8), flip(), shift({ padding: 8 })],
    whileElementsMounted: autoUpdate,
  });

  const click = useClick(context);
  const dismiss = useDismiss(context);
  const { getReferenceProps, getFloatingProps } = useInteractions([
    click,
    dismiss,
  ]);
  const { isMounted, styles: transitionStyles } = useTransitionStyles(context, {
    duration: 150,
    initial: { opacity: 0 },
  });

  const handleConfirm = async () => {
    if (onConfirm) {
      setLoading(true);
      try {
        await onConfirm();
      } finally {
        setLoading(false);
      }
    }
    setOpen(false);
  };

  const handleCancel = () => {
    onCancel?.();
    setOpen(false);
  };

  return (
    <>
      <span
        ref={refs.setReference}
        className="rua-ui-popconfirm-trigger"
        {...getReferenceProps()}
      >
        {children}
      </span>
      {isMounted && (
        <FloatingPortal>
          <div
            ref={refs.setFloating}
            style={{ ...floatingStyles, ...transitionStyles }}
            className="rua-ui-popconfirm"
            {...getFloatingProps()}
          >
            <div className="rua-ui-popconfirm-message">{title}</div>
            <div className="rua-ui-popconfirm-actions">
              <Btn variant="outline" size="sm" onClick={handleCancel}>
                {cancelText}
              </Btn>
              <Btn
                variant={danger ? "danger" : "filled"}
                size="sm"
                onClick={handleConfirm}
                loading={loading}
              >
                {okText}
              </Btn>
            </div>
          </div>
        </FloatingPortal>
      )}
    </>
  );
}
