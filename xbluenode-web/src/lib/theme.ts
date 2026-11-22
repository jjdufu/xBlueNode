import { useMemo, useSyncExternalStore } from "react";

export type ThemeMode = "light" | "dark";

export const AXPOINT_THEME_STORAGE_KEY = "axpoint_theme";

const themeListeners = new Set<() => void>();

function readStoredTheme(): ThemeMode | null {
  try {
    if (typeof localStorage !== "undefined") {
      const raw = localStorage.getItem(AXPOINT_THEME_STORAGE_KEY);
      if (raw === "dark" || raw === "light") {
        return raw;
      }
    }
  } catch {
    // Ignore storage access errors in restricted environments.
  }
  return null;
}

function detectSystemTheme(): ThemeMode {
  try {
    if (
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches
    ) {
      return "dark";
    }
  } catch {
    // Ignore matchMedia errors in non-DOM test environments.
  }
  return "light";
}

function applyThemeToDom(mode: ThemeMode): void {
  if (typeof document !== "undefined" && document.documentElement) {
    document.documentElement.classList.toggle("dark", mode === "dark");
  }
}

let currentTheme: ThemeMode = (() => {
  const stored = readStoredTheme();
  const resolved = stored ?? detectSystemTheme();
  applyThemeToDom(resolved);
  return resolved;
})();

if (
  typeof window !== "undefined" &&
  typeof window.matchMedia === "function"
) {
  try {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handleSystemChange = (ev: MediaQueryListEvent) => {
      if (readStoredTheme() !== null) {
        return;
      }
      const next: ThemeMode = ev.matches ? "dark" : "light";
      if (next !== currentTheme) {
        currentTheme = next;
        applyThemeToDom(next);
        for (const fn of themeListeners) fn();
      }
    };
    if (typeof mq.addEventListener === "function") {
      mq.addEventListener("change", handleSystemChange);
    }
  } catch {
    // Ignore listener setup errors.
  }
}

export function getTheme(): ThemeMode {
  return currentTheme;
}

export function setTheme(mode: ThemeMode): void {
  currentTheme = mode;
  applyThemeToDom(mode);
  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(AXPOINT_THEME_STORAGE_KEY, mode);
    }
  } catch {
    // Ignore storage write errors.
  }
  for (const fn of themeListeners) {
    fn();
  }
}

export function toggleTheme(): ThemeMode {
  const next: ThemeMode = currentTheme === "dark" ? "light" : "dark";
  setTheme(next);
  return next;
}

export function subscribeTheme(listener: () => void): () => void {
  themeListeners.add(listener);
  return () => {
    themeListeners.delete(listener);
  };
}

export function useTheme(): {
  readonly theme: ThemeMode;
  readonly isDark: boolean;
  readonly setTheme: (mode: ThemeMode) => void;
  readonly toggleTheme: () => ThemeMode;
} {
  const theme = useSyncExternalStore(
    subscribeTheme,
    getTheme,
    (): ThemeMode => "light"
  );
  return useMemo(
    () => ({
      theme,
      isDark: theme === "dark",
      setTheme,
      toggleTheme,
    }),
    [theme]
  );
}
