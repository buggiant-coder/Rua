import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import type { Dispatch, SetStateAction } from "react";
export type RenderViewState = {
  values: Map<string, { value: unknown; resetKey: unknown }>;
  protectedScopes: Map<string, number>;
  onProtectionChange?: () => void;
};

export type Scope = { store: RenderViewState; path: string; turnKey?: string };
export const RenderStateContext = createContext<Scope | null>(null);

// State survives presentation unmounts; changing defaults explicitly invalidates
// a saved preference. Updates remain local to this control, not all history.
export function useRenderState<T>(
  name: string,
  initial: T,
  resetKey?: unknown,
): [T, Dispatch<SetStateAction<T>>] {
  const scope = useContext(RenderStateContext);
  const key = (scope?.path ?? "") + "/" + name;
  const initialRef = useRef(initial);
  initialRef.current = initial;
  const read = useCallback(() => {
    const saved = scope?.store.values.get(key);
    return saved && Object.is(saved.resetKey, resetKey)
      ? (saved.value as T)
      : initialRef.current;
  }, [scope, key, resetKey]);
  const [value, setLocal] = useState(read);
  const current = useRef(value);
  current.current = value;
  const identity = useRef({ key, resetKey, store: scope?.store });
  useEffect(() => {
    if (
      identity.current.key === key &&
      Object.is(identity.current.resetKey, resetKey) &&
      identity.current.store === scope?.store
    )
      return;
    identity.current = { key, resetKey, store: scope?.store };
    const next = read();
    current.current = next;
    setLocal(next);
  }, [key, resetKey, scope, read]);
  const set = useCallback<Dispatch<SetStateAction<T>>>(
    (update) => {
      const next =
        typeof update === "function"
          ? (update as (value: T) => T)(current.current)
          : update;
      current.current = next;
      scope?.store.values.set(key, { value: next, resetKey });
      setLocal(next);
    },
    [scope, key, resetKey],
  );
  return [value, set];
}

export function useRenderProtection(active: boolean) {
  const scope = useContext(RenderStateContext);
  useEffect(() => {
    if (!active || !scope?.turnKey) return;
    const { store, turnKey } = scope;
    store.protectedScopes.set(
      turnKey,
      (store.protectedScopes.get(turnKey) ?? 0) + 1,
    );
    store.onProtectionChange?.();
    return () => {
      const count = (store.protectedScopes.get(turnKey) ?? 1) - 1;
      if (count) store.protectedScopes.set(turnKey, count);
      else store.protectedScopes.delete(turnKey);
      store.onProtectionChange?.();
    };
  }, [active, scope]);
}
