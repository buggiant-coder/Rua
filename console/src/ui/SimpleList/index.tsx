import type { ReactNode } from "react";
import "./simplelist.css";

interface SimpleListProps<T> {
  items?: T[];
  dataSource?: T[];
  renderItem: (item: T, index: number) => ReactNode;
  empty?: ReactNode;
  className?: string;
}

export default function SimpleList<T>({
  items: itemsProp,
  dataSource,
  renderItem,
  empty,
  className,
}: SimpleListProps<T>) {
  const items = itemsProp ?? dataSource ?? [];
  const cls = ["rua-ui-list", className].filter(Boolean).join(" ");

  if (items.length === 0 && empty) {
    return <div className={`${cls} rua-ui-list--empty`}>{empty}</div>;
  }

  return (
    <ul className={cls}>
      {items.map((item, i) => (
        <li key={i} className="rua-ui-list-item">
          {renderItem(item, i)}
        </li>
      ))}
    </ul>
  );
}
