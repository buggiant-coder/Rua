import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  type ReactNode,
} from "react";
import {
  COLOR_SCHEME_NAMES,
  type ColorSchemeName,
  type SkinName,
} from "../styles/skin-configs";

export type ThemeMode = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "rua-theme";
const SKIN_STORAGE_KEY = "rua-skin";
const COLOR_SCHEME_STORAGE_KEY = "rua-color-scheme";
const DEFAULT_SKIN: SkinName = "standard";
const DEFAULT_COLOR_SCHEME: ColorSchemeName = "blue";

interface ThemeContextValue {
  themeMode: ThemeMode;

  isDark: boolean;
  setThemeMode: (mode: ThemeMode) => void;

  toggleTheme: () => void;

  skin: SkinName;

  setSkin: (name: SkinName) => void;

  colorScheme: ColorSchemeName;

  setColorScheme: (name: ColorSchemeName) => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  themeMode: "light",
  isDark: false,
  setThemeMode: () => {},
  toggleTheme: () => {},
  skin: DEFAULT_SKIN,
  setSkin: () => {},
  colorScheme: DEFAULT_COLOR_SCHEME,
  setColorScheme: () => {},
});

function getInitialMode(): ThemeMode {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark" || stored === "system") {
      return stored;
    }
  } catch {}
  return "system";
}

function getInitialSkin(): SkinName {
  try {
    const stored = localStorage.getItem(SKIN_STORAGE_KEY);
    if (
      stored === "industrial" ||
      stored === "standard" ||
      stored === "cute" ||
      stored === "pill"
    ) {
      return stored;
    }
  } catch {}
  return DEFAULT_SKIN;
}

function getInitialColorScheme(): ColorSchemeName {
  try {
    const stored = localStorage.getItem(COLOR_SCHEME_STORAGE_KEY);
    if (COLOR_SCHEME_NAMES.includes(stored as ColorSchemeName)) {
      return stored as ColorSchemeName;
    }
  } catch {}
  return DEFAULT_COLOR_SCHEME;
}

function getSystemPrefersDark(): boolean {
  try {
    return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
  } catch {
    return false;
  }
}

function resolveIsDark(mode: ThemeMode): boolean {
  if (mode === "dark") return true;
  if (mode === "light") return false;
  return getSystemPrefersDark();
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [themeMode, setThemeModeState] = useState<ThemeMode>(getInitialMode);
  const [isDark, setIsDark] = useState<boolean>(() => resolveIsDark(themeMode));
  const [skin, setSkinState] = useState<SkinName>(getInitialSkin);
  const [colorScheme, setColorSchemeState] = useState<ColorSchemeName>(
    getInitialColorScheme,
  );

  useEffect(() => {
    const html = document.documentElement;
    if (isDark) {
      html.classList.add("dark-mode");
    } else {
      html.classList.remove("dark-mode");
    }
  }, [isDark]);

  useEffect(() => {
    document.documentElement.setAttribute("data-skin", skin);
  }, [skin]);

  useEffect(() => {
    document.documentElement.setAttribute("data-color-scheme", colorScheme);
  }, [colorScheme]);

  useEffect(() => {
    if (themeMode !== "system") return;

    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!mq) return;

    setIsDark(mq.matches);

    const handler = (e: MediaQueryListEvent | MediaQueryList) => {
      setIsDark(e.matches);
    };

    if (typeof mq.addEventListener === "function") {
      mq.addEventListener(
        "change",
        handler as (e: MediaQueryListEvent) => void,
      );
      return () =>
        mq.removeEventListener(
          "change",
          handler as (e: MediaQueryListEvent) => void,
        );
    }

    type LegacyMQL = MediaQueryList & {
      addListener: (cb: (e: MediaQueryList) => void) => void;
      removeListener: (cb: (e: MediaQueryList) => void) => void;
    };
    const legacy = mq as LegacyMQL;
    if (typeof legacy.addListener === "function") {
      legacy.addListener(handler as (e: MediaQueryList) => void);
      return () =>
        legacy.removeListener(handler as (e: MediaQueryList) => void);
    }
    return undefined;
  }, [themeMode]);

  const setThemeMode = useCallback((mode: ThemeMode) => {
    setThemeModeState(mode);
    setIsDark(resolveIsDark(mode));
    try {
      if (mode === "system") {
        localStorage.removeItem(STORAGE_KEY);
      } else {
        localStorage.setItem(STORAGE_KEY, mode);
      }
    } catch {}
  }, []);

  const toggleTheme = useCallback(() => {
    setThemeMode(isDark ? "light" : "dark");
  }, [isDark, setThemeMode]);

  const setSkin = useCallback((name: SkinName) => {
    setSkinState(name);
    try {
      localStorage.setItem(SKIN_STORAGE_KEY, name);
    } catch {}
  }, []);

  const setColorScheme = useCallback((name: ColorSchemeName) => {
    setColorSchemeState(name);
    try {
      localStorage.setItem(COLOR_SCHEME_STORAGE_KEY, name);
    } catch {}
  }, []);

  return (
    <ThemeContext.Provider
      value={{
        themeMode,
        isDark,
        setThemeMode,
        toggleTheme,
        skin,
        setSkin,
        colorScheme,
        setColorScheme,
      }}
    >
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  return useContext(ThemeContext);
}
