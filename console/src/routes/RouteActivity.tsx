import {
  createContext,
  useContext,
  useEffect,
  useState,
  type PropsWithChildren,
} from "react";
import { useLocation, useNavigation } from "react-router-dom";

const RouteActivityContext = createContext(true);

export function RouteActivityProvider({ children }: PropsWithChildren) {
  const location = useLocation();
  const navigation = useNavigation();
  const leaving =
    navigation.location !== undefined &&
    navigation.location.pathname !== location.pathname;

  // The old outlet stays mounted while loaders run; release its viewers now,
  // rather than letting them occupy the connections those loaders need.
  return (
    <RouteActivityContext.Provider value={!leaving}>
      {children}
    </RouteActivityContext.Provider>
  );
}

export function useRouteActivity() {
  return useContext(RouteActivityContext);
}

export function useRouteViewerVisible() {
  const active = useRouteActivity();
  const [visible, setVisible] = useState(
    () => typeof document === "undefined" || !document.hidden,
  );
  useEffect(() => {
    const onVisibilityChange = () => setVisible(!document.hidden);
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () =>
      document.removeEventListener("visibilitychange", onVisibilityChange);
  }, []);
  return active && visible;
}
