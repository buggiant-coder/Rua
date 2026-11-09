import { useContext, useMemo } from "react";
import type { ReactNode } from "react";
import { RenderStateContext, type RenderViewState } from "./useRenderState";
export function RenderStateRoot({
  store,
  children,
}: {
  store: RenderViewState;
  children: ReactNode;
}) {
  const value = useMemo(() => ({ store, path: "" }), [store]);
  return (
    <RenderStateContext.Provider value={value}>
      {children}
    </RenderStateContext.Provider>
  );
}

export function RenderStateScope({
  name,
  turnKey,
  children,
}: {
  name: string;
  turnKey?: string;
  children: ReactNode;
}) {
  const parent = useContext(RenderStateContext);
  const value = useMemo(
    () =>
      parent
        ? {
            store: parent.store,
            path: parent.path + "/" + encodeURIComponent(name),
            turnKey: turnKey ?? parent.turnKey,
          }
        : null,
    [parent, name, turnKey],
  );
  return (
    <RenderStateContext.Provider value={value}>
      {children}
    </RenderStateContext.Provider>
  );
}
