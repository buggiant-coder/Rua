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
  FloatingFocusManager,
  type Placement,
} from "@floating-ui/react";
import "./dropdown.css";

export type DropdownItem =
  | {
      type?: never;
      key: string;
      label: ReactNode;
      icon?: ReactNode;
      danger?: boolean;
      disabled?: boolean;
      onClick?: () => void;
    }
  | { type: "divider"; key?: string };

export type MenuProps = {
  items?: DropdownItem[];
  selectedKeys?: string[];
  onClick?: (info: { key: string }) => void;
};

interface DropdownProps {
  trigger?: ReactNode | string[];
  items?: DropdownItem[];
  placement?: Placement;
  children?: ReactNode;
  menu?: MenuProps;
  overlayClassName?: string;
  dropdownRender?: (menu: ReactNode) => ReactNode;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

export default function Dropdown({
  trigger,
  items: itemsProp,
  placement = "bottom-start",
  children,
  menu,
  overlayClassName,
  open: controlledOpen,
  onOpenChange: controlledOnOpenChange,
  dropdownRender,
}: DropdownProps) {
  const triggerElement =
    typeof trigger === "string" || Array.isArray(trigger)
      ? children
      : trigger ?? children;
  const items: DropdownItem[] =
    itemsProp ??
    (menu?.items ?? []).map((item) => {
      if (!item || (item as any).type === "divider")
        return item as DropdownItem;
      const menuOnClick = menu?.onClick;
      const origClick = (item as any).onClick;
      return {
        ...item,
        onClick: () => {
          origClick?.();
          menuOnClick?.({ key: (item as any).key });
        },
      } as DropdownItem;
    });
  const actionItemCount = items.filter(
    (item) => item.type !== "divider",
  ).length;
  const [uncontrolledOpen, setUncontrolledOpen] = useState(false);
  const open = controlledOpen ?? uncontrolledOpen;
  const setOpen = (v: boolean) => {
    setUncontrolledOpen(v);
    controlledOnOpenChange?.(v);
  };

  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange: setOpen,
    placement,
    middleware: [offset(4), flip(), shift({ padding: 8 })],
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

  return (
    <>
      <span
        ref={refs.setReference}
        className="rua-ui-dropdown-trigger"
        {...getReferenceProps()}
      >
        {triggerElement}
      </span>
      {isMounted && (
        <FloatingPortal>
          <FloatingFocusManager context={context} modal={false}>
            <div
              ref={refs.setFloating}
              style={{ ...floatingStyles, ...transitionStyles }}
              className={[
                "rua-ui-dropdown",
                actionItemCount === 1 && "rua-ui-dropdown--single",
                overlayClassName,
              ]
                .filter(Boolean)
                .join(" ")}
              role="menu"
              {...getFloatingProps({
                onKeyDown: (event: React.KeyboardEvent<HTMLDivElement>) => {
                  const items = Array.from(
                    event.currentTarget.querySelectorAll<HTMLElement>(
                      '[role="menuitem"]:not([aria-disabled="true"])',
                    ),
                  );
                  const at = items.indexOf(
                    document.activeElement as HTMLElement,
                  );
                  if (
                    ["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)
                  ) {
                    event.preventDefault();
                    const next =
                      event.key === "Home"
                        ? 0
                        : event.key === "End"
                        ? items.length - 1
                        : (at +
                            (event.key === "ArrowDown" ? 1 : -1) +
                            items.length) %
                          items.length;
                    items[next]?.focus();
                  }
                  if (event.key === "Enter" || event.key === " ") {
                    if (at >= 0) {
                      event.preventDefault();
                      items[at].click();
                    }
                  }
                },
              })}
            >
              {dropdownRender
                ? dropdownRender(
                    <>
                      {items.map((item, i) => {
                        if (item.type === "divider") {
                          return (
                            <div
                              key={item.key ?? `divider-${i}`}
                              className="rua-ui-dropdown-divider"
                            />
                          );
                        }
                        const cls = [
                          "rua-ui-dropdown-item",
                          item.danger && "rua-ui-dropdown-item--danger",
                          item.disabled && "rua-ui-dropdown-item--disabled",
                        ]
                          .filter(Boolean)
                          .join(" ");
                        return (
                          <div
                            key={item.key}
                            className={cls}
                            role="menuitem"
                            tabIndex={item.disabled ? -1 : 0}
                            aria-disabled={item.disabled || undefined}
                            onClick={
                              item.disabled
                                ? undefined
                                : () => {
                                    item.onClick?.();
                                    setOpen(false);
                                  }
                            }
                          >
                            {item.icon && (
                              <span className="rua-ui-dropdown-item-icon">
                                {item.icon}
                              </span>
                            )}
                            <span className="rua-ui-dropdown-item-label">
                              {item.label}
                            </span>
                          </div>
                        );
                      })}
                    </>,
                  )
                : items.map((item, i) => {
                    if (item.type === "divider") {
                      return (
                        <div
                          key={item.key ?? `divider-${i}`}
                          className="rua-ui-dropdown-divider"
                        />
                      );
                    }
                    const cls = [
                      "rua-ui-dropdown-item",
                      item.danger && "rua-ui-dropdown-item--danger",
                      item.disabled && "rua-ui-dropdown-item--disabled",
                    ]
                      .filter(Boolean)
                      .join(" ");
                    return (
                      <div
                        key={item.key}
                        className={cls}
                        role="menuitem"
                        tabIndex={item.disabled ? -1 : 0}
                        aria-disabled={item.disabled || undefined}
                        onClick={
                          item.disabled
                            ? undefined
                            : () => {
                                item.onClick?.();
                                setOpen(false);
                              }
                        }
                      >
                        {item.icon && (
                          <span className="rua-ui-dropdown-item-icon">
                            {item.icon}
                          </span>
                        )}
                        <span className="rua-ui-dropdown-item-label">
                          {item.label}
                        </span>
                      </div>
                    );
                  })}
            </div>
          </FloatingFocusManager>
        </FloatingPortal>
      )}
    </>
  );
}
