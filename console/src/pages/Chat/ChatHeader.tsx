import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { MoreVertical } from "lucide-react";
import { Btn, Dropdown, Tooltip, type MenuProps } from "@/ui";

export interface ChatHeaderAction {
  key: string;
  label: string;
  tooltip?: string;
  icon: ReactNode;
  disabled?: boolean;
  pressed?: boolean;
  onClick: () => void;
}

interface ChatHeaderProps {
  children: ReactNode;
  // Highest priority first; the first action stays beside More.
  actions: ChatHeaderAction[];
  moreLabel: string;
  menuItems: MenuProps["items"];
  menuFooter?: ReactNode;
}

export default function ChatHeader({
  children,
  actions,
  moreLabel,
  menuItems,
  menuFooter,
}: ChatHeaderProps) {
  const headerRef = useRef<HTMLElement>(null);
  const mainRef = useRef<HTMLDivElement>(null);
  const actionsRef = useRef<HTMLDivElement>(null);
  const moreRef = useRef<HTMLButtonElement>(null);
  const [visibleCount, setVisibleCount] = useState(actions.length);

  useLayoutEffect(() => {
    const header = headerRef.current!;
    const main = mainRef.current!;
    const actionRow = actionsRef.current!;
    const measure = () => {
      // Measure preferred width independently of the previous overflow decision.
      main.style.maxWidth = "none";
      const preferredWidth = main.getBoundingClientRect().width;
      main.style.maxWidth = "";
      const style = getComputedStyle(header);
      const available =
        header.clientWidth -
        parseFloat(style.paddingLeft) -
        parseFloat(style.paddingRight);
      const gap = parseFloat(style.columnGap) || 0;
      const actionGap = parseFloat(getComputedStyle(actionRow).columnGap) || 0;
      const buttonWidth = moreRef.current!.getBoundingClientRect().width;
      let count = actions.length;
      while (
        count > 1 &&
        preferredWidth + gap + (count + 1) * buttonWidth + count * actionGap >
          available
      )
        count -= 1;
      setVisibleCount(count);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(header);
    observer.observe(main);
    observer.observe(moreRef.current!);
    const contentObserver = new MutationObserver(measure);
    contentObserver.observe(main, {
      childList: true,
      characterData: true,
      subtree: true,
    });
    return () => {
      observer.disconnect();
      contentObserver.disconnect();
    };
  }, [actions.length]);

  const overflow = actions.slice(visibleCount);
  const items: MenuProps["items"] = [
    ...overflow.map(({ key, label, icon, disabled, onClick }) => ({
      key,
      label,
      icon,
      disabled,
      onClick,
    })),
    ...(overflow.length
      ? [{ type: "divider" as const, key: "header-overflow" }]
      : []),
    ...(menuItems ?? []),
  ];

  return (
    <header ref={headerRef} className="rua-owned-chat-header">
      <div className="rua-owned-chat-header-main">
        <div ref={mainRef} className="rua-owned-chat-header-scope">
          {children}
        </div>
      </div>
      <div ref={actionsRef} className="rua-owned-chat-header-actions">
        {actions.slice(0, visibleCount).map((action) => (
          <Tooltip key={action.key} title={action.tooltip ?? action.label}>
            <Btn
              icon={action.icon}
              aria-label={action.label}
              aria-pressed={action.pressed}
              disabled={action.disabled}
              onClick={action.onClick}
            />
          </Tooltip>
        ))}
        <Dropdown
          menu={{ items }}
          trigger={["click"]}
          placement="bottom-end"
          overlayClassName="rua-owned-chat-more-dropdown"
          dropdownRender={(menu) => (
            <>
              {menu}
              {menuFooter}
            </>
          )}
        >
          <Btn ref={moreRef} icon={<MoreVertical />} aria-label={moreLabel} />
        </Dropdown>
      </div>
    </header>
  );
}
