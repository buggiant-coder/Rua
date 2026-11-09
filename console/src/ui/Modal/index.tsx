import {
  createContext,
  useContext,
  useState,
  useEffect,
  useRef,
  useCallback,
} from "react";
import { createPortal } from "react-dom";
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { useHistoryDismiss } from "@/hooks/useHistoryDismiss";
import { isMobile } from "@/utils/isMobile";
import Btn from "../Btn";
import "./modal.css";

interface ModalProps {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  footer?: ReactNode | null;
  children?: ReactNode;
  className?: string;
  maskClosable?: boolean;
  width?: number | string;
  destroyOnClose?: boolean;
  onOk?: () => unknown;
  okText?: string;
  cancelText?: string;
  confirmLoading?: boolean;
  okButtonProps?: Record<string, unknown>;
  closable?: boolean;
  keyboard?: boolean;
  zIndex?: number;
  styles?: Record<string, unknown>;
  loading?: boolean;
  [key: string]: unknown;
}

interface ConfirmConfig {
  title?: ReactNode;
  content?: ReactNode;
  okText?: string;
  cancelText?: string;
  onOk?: () => unknown;
  onCancel?: () => void;
  danger?: boolean;
  hideCancel?: boolean;
  okType?: string;
  [key: string]: unknown;
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

function Modal({
  open,
  onClose,
  title,
  footer,
  children,
  className,
  maskClosable = true,
  width,
  destroyOnClose,
  onOk,
  okText,
  cancelText,
  confirmLoading,
  okButtonProps,
}: ModalProps) {
  const [mounted, setMounted] = useState(open);
  const [visible, setVisible] = useState(false);
  const [asyncLoading, setAsyncLoading] = useState(false);
  const modalRef = useRef<HTMLDivElement>(null);
  const maskPointerDownRef = useRef(false);
  const historyDismissOpen = isMobile() && open;

  const handleOk = async () => {
    if (!onOk) return;
    const result = onOk();
    if (result && typeof (result as Promise<unknown>).then === "function") {
      setAsyncLoading(true);
      try {
        await result;
      } finally {
        setAsyncLoading(false);
      }
    }
  };

  useEffect(() => {
    if (open) {
      setMounted(true);
      requestAnimationFrame(() =>
        requestAnimationFrame(() => setVisible(true)),
      );
    } else {
      setVisible(false);
      const timer = setTimeout(() => setMounted(false), 200);
      return () => clearTimeout(timer);
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      e.stopImmediatePropagation();
      onClose();
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  useFocusTrap(modalRef, open);
  useHistoryDismiss(historyDismissOpen, onClose);

  if (!mounted && destroyOnClose) return null;
  if (!mounted) return null;

  const maskCls = [
    "rua-ui-modal-mask",
    visible && "rua-ui-modal-mask--visible",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  const handleMaskPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    maskPointerDownRef.current = event.target === event.currentTarget;
  };

  const handleMaskClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const isMaskClick =
      maskPointerDownRef.current && event.target === event.currentTarget;
    maskPointerDownRef.current = false;
    if (maskClosable && isMaskClick) onClose();
  };

  return createPortal(
    <div
      className={maskCls}
      onPointerDown={handleMaskPointerDown}
      onClick={handleMaskClick}
    >
      <div
        ref={modalRef}
        className="rua-ui-modal"
        style={width ? { width } : undefined}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        {title != null && (
          <div className="rua-ui-modal-header">
            <h3 className="rua-ui-modal-title">{title}</h3>
            <button
              className="rua-ui-modal-close"
              onClick={onClose}
              aria-label="Close"
            >
              ×
            </button>
          </div>
        )}
        <div className="rua-ui-modal-body">{children}</div>
        {footer !== null &&
          (footer !== undefined ? (
            <div className="rua-ui-modal-footer">{footer}</div>
          ) : onOk ? (
            <div className="rua-ui-modal-footer">
              <Btn variant="outline" onClick={onClose}>
                {cancelText || "取消"}
              </Btn>
              <Btn
                onClick={handleOk}
                loading={confirmLoading || asyncLoading}
                {...okButtonProps}
              >
                {okText || "确定"}
              </Btn>
            </div>
          ) : null)}
      </div>
    </div>,
    document.body,
  );
}

type ShowConfirm = (config: ConfirmConfig) => Promise<boolean>;

const ConfirmContext = createContext<ShowConfirm | null>(null);

interface ConfirmEntry {
  id: number;
  config: ConfirmConfig;
  resolve: (ok: boolean) => void;
}

let nextId = 0;

function ModalProvider({ children }: { children: ReactNode }) {
  const [entries, setEntries] = useState<ConfirmEntry[]>([]);

  const showConfirm = useCallback<ShowConfirm>((config) => {
    return new Promise<boolean>((resolve) => {
      setEntries((prev) => [...prev, { id: nextId++, config, resolve }]);
    });
  }, []);

  const close = useCallback((entry: ConfirmEntry, ok: boolean) => {
    entry.resolve(ok);
    setEntries((prev) => prev.filter((e) => e.id !== entry.id));
  }, []);

  return (
    <ConfirmContext.Provider value={showConfirm}>
      {children}
      {entries.map((entry) => (
        <ConfirmDialog key={entry.id} entry={entry} onClose={close} />
      ))}
    </ConfirmContext.Provider>
  );
}

function ConfirmDialog({
  entry,
  onClose,
}: {
  entry: ConfirmEntry;
  onClose: (e: ConfirmEntry, ok: boolean) => void;
}) {
  const { config } = entry;
  const [loading, setLoading] = useState(false);

  const handleOk = async () => {
    if (config.onOk) {
      setLoading(true);
      try {
        await config.onOk();
      } finally {
        setLoading(false);
      }
    }
    onClose(entry, true);
  };

  const handleCancel = () => {
    config.onCancel?.();
    onClose(entry, false);
  };

  const handleContentKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      handleCancel();
      return;
    }
    if (event.key !== "Enter" || loading || event.nativeEvent.isComposing) {
      return;
    }
    const target = event.target as HTMLElement | null;
    if (target?.tagName === "TEXTAREA") return;
    event.preventDefault();
    event.stopPropagation();
    void handleOk();
  };

  return (
    <Modal
      open
      onClose={handleCancel}
      title={config.title}
      footer={
        <>
          {!config.hideCancel && (
            <Btn variant="outline" onClick={handleCancel}>
              {config.cancelText || "取消"}
            </Btn>
          )}
          <Btn
            variant={config.danger ? "danger" : "filled"}
            onClick={handleOk}
            loading={loading}
          >
            {config.okText || "确定"}
          </Btn>
        </>
      }
    >
      <div onKeyDownCapture={handleContentKeyDown}>{config.content}</div>
    </Modal>
  );
}

let _showConfirm: ShowConfirm | null = null;

function useConfirmRegistration() {
  const ctx = useContext(ConfirmContext);
  useEffect(() => {
    _showConfirm = ctx;
    return () => {
      _showConfirm = null;
    };
  }, [ctx]);
}

function ConfirmRegistrar() {
  useConfirmRegistration();
  return null;
}

function ModalProviderWithRegistrar({ children }: { children: ReactNode }) {
  return (
    <ModalProvider>
      <ConfirmRegistrar />
      {children}
    </ModalProvider>
  );
}

function confirm(config: ConfirmConfig): Promise<boolean> {
  if (!_showConfirm) {
    throw new Error(
      "Modal.confirm requires <ModalProvider> in the component tree",
    );
  }
  return _showConfirm(config);
}

function error(config: ConfirmConfig): Promise<boolean> {
  return confirm({ ...config, danger: true });
}

function warning(config: ConfirmConfig): Promise<boolean> {
  return confirm({
    ...config,
    hideCancel: true,
    okText: config.okText || "知道了",
  });
}

type ModalType = typeof Modal & {
  confirm: typeof confirm;
  error: typeof error;
  warning: typeof warning;
  Provider: typeof ModalProviderWithRegistrar;
};

const ModalExport = Modal as ModalType;
ModalExport.confirm = confirm;
ModalExport.error = error;
ModalExport.warning = warning;
ModalExport.Provider = ModalProviderWithRegistrar;

export default ModalExport;
export { ModalProviderWithRegistrar as ModalProvider };
export type { ModalProps, ConfirmConfig };
