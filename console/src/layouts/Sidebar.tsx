import NavigationPanel from "./NavigationPanel";
import styles from "./index.module.less";

interface SidebarProps {
  location: {
    pathname: string;
    search: string;
  };
  onNavigate: (path: string) => void;
  open?: boolean;
}

export default function Sidebar({
  location,
  onNavigate,
  open = true,
}: SidebarProps) {
  return (
    <aside
      {...(!open ? { inert: "" } : {})}
      aria-hidden={!open}
      id="desktop-navigation"
      className={`${styles.sider} ${!open ? styles.siderCollapsed : ""}`}
      aria-label="navigation"
    >
      <NavigationPanel location={location} onNavigate={onNavigate} />
    </aside>
  );
}
