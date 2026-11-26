import { useMemo, useSyncExternalStore } from "react";

export type ThemeMode = "light" | "dark";
export type ThemePreference = "auto" | "light" | "dark";

export const AXPOINT_THEME_STORAGE_KEY = "axpoint_theme";

const themeListeners = new Set<() => void>();

function readStoredPreference(): ThemePreference {
  try {
    if (typeof localStorage !== "undefined") {
      const raw = localStorage.getItem(AXPOINT_THEME_STORAGE_KEY);
      if (raw === "dark" || raw === "light" || raw === "auto") {
        return raw;
      }
    }
  } catch {
    // Ignore storage access errors in restricted environments.
  }
  return "auto";
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

function resolveThemeMode(pref: ThemePreference): ThemeMode {
  return pref === "auto" ? detectSystemTheme() : pref;
}

function applyThemeToDom(mode: ThemeMode): void {
  if (typeof document !== "undefined" && document.documentElement) {
    const docEl = document.documentElement;
    // Freeze CSS transitions during root theme swap so cards/borders paint in one atomic frame (no flash)
    let styleEl: HTMLStyleElement | null = null;
    if (document.head) {
      styleEl = document.createElement("style");
      styleEl.setAttribute("data-axpoint-theme-freeze", "1");
      styleEl.textContent =
        "*, *::before, *::after { transition: none !important; animation-duration: 0.001ms !important; }";
      document.head.appendChild(styleEl);
    }
    docEl.classList.toggle("dark", mode === "dark");
    docEl.style.colorScheme = mode;
    if (document.body && typeof window !== "undefined") {
      void window.getComputedStyle(document.body).backgroundColor;
    }
    if (styleEl) {
      const cleanup = () => {
        styleEl?.remove();
      };
      if (typeof window !== "undefined" && typeof window.requestAnimationFrame === "function") {
        window.requestAnimationFrame(() => {
          window.requestAnimationFrame(cleanup);
        });
      } else {
        cleanup();
      }
    }
  }
}

let currentPreference: ThemePreference = readStoredPreference();
let currentTheme: ThemeMode = (() => {
  const resolved = resolveThemeMode(currentPreference);
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
      if (currentPreference !== "auto") {
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

export function getThemePreference(): ThemePreference {
  return currentPreference;
}

export function setTheme(mode: ThemePreference): void {
  currentPreference = mode;
  currentTheme = resolveThemeMode(mode);
  applyThemeToDom(currentTheme);
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

/**
 * Cycles through system-aware tri-state (`auto` -> opposite-of-system -> explicit -> `auto`)
 * so clicking out of `auto` always produces immediate visual contrast.
 */
export function toggleTheme(): ThemeMode {
  const sys = detectSystemTheme();
  let next: ThemePreference;
  if (currentPreference === "auto") {
    next = sys === "dark" ? "light" : "dark";
  } else if (sys === "dark") {
    next = currentPreference === "light" ? "dark" : "auto";
  } else {
    next = currentPreference === "dark" ? "light" : "auto";
  }
  setTheme(next);
  return currentTheme;
}

export function subscribeTheme(listener: () => void): () => void {
  themeListeners.add(listener);
  return () => {
    themeListeners.delete(listener);
  };
}

export function useTheme(): {
  readonly theme: ThemeMode;
  readonly preference: ThemePreference;
  readonly isDark: boolean;
  readonly setTheme: (mode: ThemePreference) => void;
  readonly toggleTheme: () => ThemeMode;
} {
  const theme = useSyncExternalStore(
    subscribeTheme,
    getTheme,
    (): ThemeMode => "light"
  );
  const preference = useSyncExternalStore(
    subscribeTheme,
    getThemePreference,
    (): ThemePreference => "auto"
  );
  return useMemo(
    () => ({
      theme,
      preference,
      isDark: theme === "dark",
      setTheme,
      toggleTheme,
    }),
    [preference, theme]
  );
}
