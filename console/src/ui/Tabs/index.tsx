import { useRef, useEffect, useState, useCallback } from "react";
import "./tabs.css";

interface TabItem {
  key: string;
  label: React.ReactNode;
  children?: React.ReactNode;
}

interface TabsProps {
  items: TabItem[];
  activeKey?: string;
  onChange?: (key: string) => void;
  className?: string;
}

export default function Tabs({
  items,
  activeKey,
  onChange,
  className,
}: TabsProps) {
  const navRef = useRef<HTMLDivElement>(null);
  const [ink, setInk] = useState({ left: 0, width: 0 });

  const updateInk = useCallback(() => {
    if (!navRef.current) return;
    const active = navRef.current.querySelector<HTMLElement>(
      ".rua-ui-tabs-tab--active",
    );
    if (active) {
      setInk({ left: active.offsetLeft, width: active.offsetWidth });
    }
  }, []);

  useEffect(() => {
    updateInk();
  }, [activeKey, items, updateInk]);

  const current = activeKey ?? items[0]?.key;

  return (
    <div className={`rua-ui-tabs${className ? ` ${className}` : ""}`}>
      <div className="rua-ui-tabs-nav" ref={navRef}>
        {items.map((item) => (
          <button
            key={item.key}
            type="button"
            role="tab"
            aria-selected={item.key === current}
            className={`rua-ui-tabs-tab${
              item.key === current ? " rua-ui-tabs-tab--active" : ""
            }`}
            onClick={() => onChange?.(item.key)}
          >
            {item.label}
          </button>
        ))}
        <span
          className="rua-ui-tabs-ink"
          style={
            {
              left: ink.left,
              width: ink.width,
              "--rua-tabs-ink-left": `${ink.left}px`,
            } as React.CSSProperties
          }
        />
      </div>
      {items.map((item) => (
        <div
          key={item.key}
          role="tabpanel"
          className="rua-ui-tabs-panel"
          hidden={item.key !== current}
        >
          {item.children}
        </div>
      ))}
    </div>
  );
}
