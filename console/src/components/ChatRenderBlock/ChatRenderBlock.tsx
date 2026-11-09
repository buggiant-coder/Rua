import { ChevronUp } from "lucide-react";
import { useRenderState } from "./useRenderState";
import type { MouseEvent, ReactNode } from "react";
import "./chat-render-block.css";

type ChatRenderBlockProps = {
  stateKey?: string;
  children: ReactNode;
  className?: string;
  ariaLabel?: string;
  title?: ReactNode;
  actions?: ReactNode;
  sticky?: boolean;
};

function classNames(...values: Array<string | false | undefined>): string {
  return values.filter(Boolean).join(" ");
}

export function ChatRenderBlock({
  stateKey = "block",
  children,
  className,
  ariaLabel = "Rendered block",
  title,
  actions,
  sticky = true,
}: ChatRenderBlockProps) {
  const [collapsed, setCollapsed] = useRenderState(stateKey, false);
  const actionLabel = collapsed ? "Expand block" : "Collapse block";
  const hasTitle = title !== undefined && title !== null && title !== false;
  const toggleCollapsed = (event: MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    setCollapsed((value) => !value);
  };

  return (
    <div
      className={classNames(
        "rua-render-block",
        collapsed && "is-collapsed",
        !sticky && "rua-render-block--non-sticky",
        className,
      )}
      aria-label={ariaLabel}
      data-collapsed={collapsed ? "true" : "false"}
    >
      <div
        className={classNames(
          "rua-render-block-bar",
          hasTitle && "has-title",
        )}
      >
        {hasTitle && <div className="rua-render-block-title">{title}</div>}
        <div className="rua-render-block-controls">
          {actions && (
            <div className="rua-render-block-actions">{actions}</div>
          )}
          <button
            type="button"
            className="rua-render-block-toggle"
            title={actionLabel}
            aria-label={actionLabel}
            aria-expanded={!collapsed}
            onClick={toggleCollapsed}
          >
            <ChevronUp aria-hidden="true" size={15} />
          </button>
        </div>
      </div>
      <div className="rua-render-block-body" hidden={collapsed}>
        {children}
      </div>
    </div>
  );
}
