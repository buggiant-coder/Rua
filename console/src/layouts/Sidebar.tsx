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
      hidden={!open}
      id="desktop-navigation"
      className={styles.sider}
      aria-label="navigation"
    >
      <NavigationPanel location={location} onNavigate={onNavigate} />
    </aside>
  );
}
