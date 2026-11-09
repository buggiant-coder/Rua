import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
} from "react";
import { createPortal } from "react-dom";
import type { ReactNode } from "react";
import "./toast.css";

type ToastType = "success" | "error" | "info" | "warning";

interface ToastEntry {
  id: number;
  type: ToastType;
  message: ReactNode;
}

type ShowToast = (
  type: ToastType,
  message: ReactNode,
  duration?: number,
) => void;

const ToastContext = createContext<ShowToast | null>(null);

let nextId = 0;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastEntry[]>([]);

  const show = useCallback<ShowToast>((type, message, duration = 3000) => {
    const id = nextId++;
    setToasts((prev) => [...prev, { id, type, message }]);
    setTimeout(
      () => setToasts((prev) => prev.filter((t) => t.id !== id)),
      duration,
    );
  }, []);

  return (
    <ToastContext.Provider value={show}>
      {children}
      {toasts.length > 0 &&
        createPortal(
          <div className="rua-ui-toast-container">
            {toasts.map((t) => (
              <div
                key={t.id}
                className={`rua-ui-toast rua-ui-toast--${t.type}`}
              >
                {t.message}
              </div>
            ))}
          </div>,
          document.body,
        )}
    </ToastContext.Provider>
  );
}

let _show: ShowToast | null = null;

function ToastRegistrar() {
  const ctx = useContext(ToastContext);
  useEffect(() => {
    _show = ctx;
    return () => {
      _show = null;
    };
  }, [ctx]);
  return null;
}

export function ToastProviderWithRegistrar({
  children,
}: {
  children: ReactNode;
}) {
  return (
    <ToastProvider>
      <ToastRegistrar />
      {children}
    </ToastProvider>
  );
}

type ToastArg =
  | ReactNode
  | { content?: ReactNode; message?: ReactNode; duration?: number };

function resolveArg(
  msg: ToastArg,
  dur?: number,
): [ReactNode, number | undefined] {
  if (
    msg != null &&
    typeof msg === "object" &&
    !Array.isArray(msg) &&
    ("content" in (msg as any) ||
      "message" in (msg as any) ||
      "duration" in (msg as any))
  ) {
    const obj = msg as {
      content?: ReactNode;
      message?: ReactNode;
      duration?: number;
    };
    return [obj.content ?? obj.message, obj.duration ?? dur];
  }
  return [msg as ReactNode, dur];
}

const toast = {
  success: (msg: ToastArg, duration?: number) => {
    const [m, d] = resolveArg(msg, duration);
    _show?.("success", m, d);
  },
  error: (msg: ToastArg, duration?: number) => {
    const [m, d] = resolveArg(msg, duration);
    _show?.("error", m, d);
  },
  info: (msg: ToastArg, duration?: number) => {
    const [m, d] = resolveArg(msg, duration);
    _show?.("info", m, d);
  },
  warning: (msg: ToastArg, duration?: number) => {
    const [m, d] = resolveArg(msg, duration);
    _show?.("warning", m, d);
  },
};

export default toast;
