import { useMemo, useSyncExternalStore } from "react";
import { ENUM_LABELS } from "./enums";

export type Locale = "zh-CN" | "en-US";

export const AXPOINT_LOCALE_STORAGE_KEY = "axpoint_locale";

const localeListeners = new Set<() => void>();

function readStoredLocale(): Locale | null {
  try {
    if (typeof localStorage !== "undefined") {
      const raw = localStorage.getItem(AXPOINT_LOCALE_STORAGE_KEY);
      if (raw === "zh-CN" || raw === "en-US") {
        return raw;
      }
    }
  } catch {
    // Ignore restricted storage errors.
  }
  return null;
}

function detectBrowserLocale(): Locale {
  try {
    if (typeof navigator !== "undefined") {
      const primary =
        (Array.isArray(navigator.languages) && navigator.languages[0]) ||
        navigator.language ||
        "zh-CN";
      return primary.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
    }
  } catch {
    // Fallback for non-browser test runners.
  }
  return "zh-CN";
}

function syncHtmlLang(loc: Locale): void {
  if (typeof document !== "undefined" && document.documentElement) {
    document.documentElement.lang = loc;
  }
}

let currentLocale: Locale = (() => {
  const resolved = readStoredLocale() ?? detectBrowserLocale();
  syncHtmlLang(resolved);
  return resolved;
})();

export function getLocale(): Locale {
  return currentLocale;
}

export function setLocale(next: Locale): void {
  if (currentLocale === next) {
    syncHtmlLang(next);
    return;
  }
  currentLocale = next;
  syncHtmlLang(next);
  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(AXPOINT_LOCALE_STORAGE_KEY, next);
    }
  } catch {
    // Ignore storage write errors.
  }
  for (const fn of localeListeners) {
    fn();
  }
}

export function toggleLocale(): Locale {
  const next: Locale = currentLocale === "zh-CN" ? "en-US" : "zh-CN";
  setLocale(next);
  return next;
}

export function subscribeLocale(listener: () => void): () => void {
  localeListeners.add(listener);
  return () => {
    localeListeners.delete(listener);
  };
}

export function translateEnum(
  code: string | null | undefined,
  loc: Locale = currentLocale
): string {
  if (!code) return "-";
  const entry = ENUM_LABELS[code] ?? ENUM_LABELS[code.toUpperCase()];
  if (entry) {
    return loc === "zh-CN" ? entry.zh : entry.en;
  }
  return code;
}

export function enumLabel(
  domainOrCode: string,
  codeOrLoc?: string | null,
  maybeLoc?: Locale
): string {
  if (maybeLoc !== undefined) {
    return translateEnum(codeOrLoc, maybeLoc);
  }
  if (codeOrLoc === "zh-CN" || codeOrLoc === "en-US") {
    return translateEnum(domainOrCode, codeOrLoc);
  }
  if (codeOrLoc !== undefined) {
    return translateEnum(codeOrLoc, currentLocale);
  }
  return translateEnum(domainOrCode, currentLocale);
}

export function formatNumber(
  val: number | null | undefined,
  maxDecimals = 2,
  loc: Locale = currentLocale
): string {
  if (val === null || val === undefined || !Number.isFinite(val)) {
    return "-";
  }
  return new Intl.NumberFormat(loc, {
    maximumFractionDigits: maxDecimals,
  }).format(val);
}

export function formatDateTime(
  iso: string | null | undefined,
  short = false,
  loc: Locale = currentLocale
): string {
  if (!iso) return "-";
  const trimmed = iso.trim();
  const parsed = new Date(trimmed);
  if (Number.isNaN(parsed.getTime())) {
    return trimmed.length > 19 ? trimmed.slice(0, 19).replace("T", " ") : trimmed;
  }
  if (short) {
    const mm = String(parsed.getMonth() + 1).padStart(2, "0");
    const dd = String(parsed.getDate()).padStart(2, "0");
    const hh = String(parsed.getHours()).padStart(2, "0");
    const mi = String(parsed.getMinutes()).padStart(2, "0");
    return `${mm}-${dd} ${hh}:${mi}`;
  }
  return new Intl.DateTimeFormat(loc, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(parsed);
}

export function useI18n(): {
  readonly locale: Locale;
  readonly isZh: boolean;
  readonly setLocale: (next: Locale) => void;
  readonly toggleLocale: () => Locale;
  readonly tx: (zh: string, en: string) => string;
  readonly tEnum: (code: string | null | undefined) => string;
  readonly fmtNum: (val: number | null | undefined, maxDecimals?: number) => string;
  readonly fmtTime: (iso: string | null | undefined, short?: boolean) => string;
} {
  const locale = useSyncExternalStore(
    subscribeLocale,
    getLocale,
    (): Locale => "zh-CN"
  );
  return useMemo(() => {
    const isZh = locale === "zh-CN";
    return {
      locale,
      isZh,
      setLocale,
      toggleLocale,
      tx: (zh: string, en: string) => (isZh ? zh : en),
      tEnum: (code: string | null | undefined) => translateEnum(code, locale),
      fmtNum: (val: number | null | undefined, maxDecimals = 2) =>
        formatNumber(val, maxDecimals, locale),
      fmtTime: (iso: string | null | undefined, short = false) =>
        formatDateTime(iso, short, locale),
    };
  }, [locale]);
}
