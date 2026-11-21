import type { Permission, TokenKind } from "./enums.ts";
import { ApiError, buildApiError, requestJson } from "./client.ts";

/** Authenticated principal returned by `GET /api/auth/me`. */
export interface MeResponse {
  user_id: string;
  tenant_id: string;
  username: string;
  dept_id: string;
  roles: string[];
  permissions: Permission[];
  token_kind: TokenKind;
}

/** Public auth configuration returned by `GET /api/auth/config` (`11-STEP13` §13.10). */
export interface AuthConfigResponse {
  env: string;
  oidc_enabled: boolean;
  oidc_display_name?: string;
  default_tenant: string;
}

/** Token response returned by `POST /api/auth/login` and `POST /api/auth/refresh` (`11-STEP13` §13.4). */
export interface AccessTokenResponse {
  access_token: string;
  token_type: string;
  expires_in: number;
}

/** Credentials submitted to `POST /api/auth/login`. */
export interface LoginCredentials {
  tenant?: string;
  username: string;
  password: string;
}

export type TokenMode = "none" | "jwt" | "pat" | "dev_static";

export const DEFAULT_AUTH_CONFIG: AuthConfigResponse = {
  env: "development",
  oidc_enabled: false,
  default_tenant: "default",
};

const PAT_SESSION_STORAGE_KEY = "axpoint_pat_token";

function readSessionPat(): string | null {
  try {
    if (typeof sessionStorage !== "undefined") {
      const raw = sessionStorage.getItem(PAT_SESSION_STORAGE_KEY)?.trim() ?? "";
      return raw.length > 0 ? raw : null;
    }
  } catch {
    // Ignore storage security errors in sandboxed/non-browser environments.
  }
  return null;
}

function writeSessionPat(token: string | null): void {
  try {
    if (typeof sessionStorage !== "undefined") {
      if (token && token.length > 0) {
        sessionStorage.setItem(PAT_SESSION_STORAGE_KEY, token);
      } else {
        sessionStorage.removeItem(PAT_SESSION_STORAGE_KEY);
      }
    }
  } catch {
    // Ignore storage security errors in sandboxed/non-browser environments.
  }
}

/**
 * Safely decodes the numeric `exp` claim (Unix epoch seconds) from a 3-segment JWT
 * without external dependencies. Returns `null` for non-JWT tokens (PAT / dev static)
 * or malformed payloads.
 */
export function decodeJwtExp(token: string): number | null {
  const trimmed = token.trim();
  const parts = trimmed.split(".");
  if (parts.length !== 3) {
    return null;
  }
  const payloadSeg = parts[1];
  if (!payloadSeg) {
    return null;
  }
  try {
    const normalized = payloadSeg.replace(/-/g, "+").replace(/_/g, "/");
    const padLen = (4 - (normalized.length % 4)) % 4;
    const padded = normalized + "=".repeat(padLen);
    const jsonText = atob(padded);
    const parsed: unknown = JSON.parse(jsonText);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      "exp" in parsed &&
      typeof parsed.exp === "number" &&
      Number.isFinite(parsed.exp)
    ) {
      return parsed.exp;
    }
  } catch {
    return null;
  }
  return null;
}

/**
 * Computes proactive refresh delay `max(5_000, (exp * 1000 - nowMs) - 60_000)` (`11-STEP13` §13.4.2).
 */
export function computeProactiveRefreshDelayMs(
  expSeconds: number,
  nowMs: number = Date.now()
): number {
  return Math.max(5_000, expSeconds * 1_000 - nowMs - 60_000);
}

const persistedPat = readSessionPat();
const initialDevToken = import.meta.env?.VITE_DEV_TOKEN?.trim() ?? "";

let memoryToken: string | null =
  persistedPat ?? (initialDevToken.length > 0 ? initialDevToken : null);
let tokenMode: TokenMode = persistedPat
  ? "pat"
  : initialDevToken.length > 0
    ? "dev_static"
    : "none";
let cachedMe: MeResponse | null = null;
let refreshTimer: ReturnType<typeof setTimeout> | null = null;
let inflightRefresh: Promise<string | null> | null = null;

const authListeners = new Set<() => void>();

function notifyAuthListeners(): void {
  for (const listener of authListeners) {
    listener();
  }
}

function clearRefreshTimer(): void {
  if (refreshTimer !== null) {
    clearTimeout(refreshTimer);
    refreshTimer = null;
  }
}

function scheduleProactiveRefresh(token: string): void {
  clearRefreshTimer();
  if (typeof window === "undefined") {
    return;
  }
  const exp = decodeJwtExp(token);
  if (exp === null) {
    return;
  }
  const delayMs = computeProactiveRefreshDelayMs(exp);
  refreshTimer = setTimeout(() => {
    refreshTimer = null;
    void refreshAccessToken();
  }, delayMs);
}

/** Subscribes to token/principal state transitions (`useSyncExternalStore`-compatible). */
export function subscribeAuthChange(listener: () => void): () => void {
  authListeners.add(listener);
  return () => {
    authListeners.delete(listener);
  };
}

/** Reads the in-memory access token (never persisted to `localStorage`). */
export function getAccessToken(): string | null {
  return memoryToken;
}

/** Returns the active token lifecycle mode. */
export function getTokenMode(): TokenMode {
  return tokenMode;
}

/** Returns `true` when authenticated via an explicit static PAT or `VITE_DEV_TOKEN`. */
export function isStaticTokenMode(): boolean {
  return tokenMode === "pat" || tokenMode === "dev_static";
}

/** Reads the most recently fetched `/api/auth/me` principal, if available. */
export function getCachedMe(): MeResponse | null {
  return cachedMe;
}

/** Stores a short-lived `EdDSA` access JWT in memory and schedules proactive silent refresh. */
export function setSessionAccessToken(token: string): void {
  const trimmed = token.trim();
  if (trimmed.length === 0) {
    clearAccessToken();
    return;
  }
  memoryToken = trimmed;
  tokenMode = "jwt";
  writeSessionPat(null);
  scheduleProactiveRefresh(trimmed);
  notifyAuthListeners();
}

/** Stores an explicit PAT in memory and `sessionStorage` for the current tab session. */
export function setPatAccessToken(token: string): void {
  const trimmed = token.trim();
  if (trimmed.length === 0) {
    clearAccessToken();
    return;
  }
  clearRefreshTimer();
  memoryToken = trimmed;
  tokenMode = "pat";
  writeSessionPat(trimmed);
  notifyAuthListeners();
}

/**
 * Stores an access token in memory.
 * Automatically distinguishes `EdDSA` JWTs (in-memory + proactive refresh) from static PATs (`sessionStorage`).
 */
export function setAccessToken(token: string | null): void {
  const trimmed = token?.trim() ?? "";
  if (trimmed.length === 0) {
    clearAccessToken();
    return;
  }
  if (decodeJwtExp(trimmed) !== null) {
    setSessionAccessToken(trimmed);
  } else {
    setPatAccessToken(trimmed);
  }
}

/** Clears the in-memory access token, `sessionStorage` PAT, and any pending refresh timer. */
export function clearAccessToken(): void {
  clearRefreshTimer();
  memoryToken = null;
  tokenMode = "none";
  cachedMe = null;
  writeSessionPat(null);
  notifyAuthListeners();
}

/**
 * Single-flight silent session refresh (`POST /api/auth/refresh` with `credentials: "include"`).
 * Concurrent callers await the same in-flight promise so refresh rotation cookies are never replayed.
 */
export function refreshAccessToken(): Promise<string | null> {
  if (inflightRefresh !== null) {
    return inflightRefresh;
  }

  inflightRefresh = (async (): Promise<string | null> => {
    try {
      const resp = await fetch("/api/auth/refresh", {
        method: "POST",
        credentials: "include",
        headers: {
          Accept: "application/json",
        },
      });
      if (!resp.ok) {
        if (tokenMode === "jwt") {
          clearRefreshTimer();
          memoryToken = null;
          tokenMode = "none";
          cachedMe = null;
          notifyAuthListeners();
        }
        return null;
      }
      const data: unknown = await resp.json();
      if (
        typeof data === "object" &&
        data !== null &&
        "access_token" in data &&
        typeof data.access_token === "string" &&
        data.access_token.trim().length > 0
      ) {
        const nextToken = data.access_token.trim();
        setSessionAccessToken(nextToken);
        return nextToken;
      }
      return null;
    } catch {
      if (tokenMode === "jwt") {
        clearRefreshTimer();
        memoryToken = null;
        tokenMode = "none";
        cachedMe = null;
        notifyAuthListeners();
      }
      return null;
    } finally {
      inflightRefresh = null;
    }
  })();

  return inflightRefresh;
}

/**
 * Fetches public login configuration (`GET /api/auth/config`), gracefully falling back
 * to `{ env: "development", oidc_enabled: false, default_tenant: "default" }` when offline or unmocked.
 */
export async function fetchAuthConfig(): Promise<AuthConfigResponse> {
  try {
    const headers: Record<string, string> = {
      Accept: "application/json",
    };
    if (memoryToken) {
      headers.Authorization = `Bearer ${memoryToken}`;
    }
    const resp = await fetch("/api/auth/config", {
      method: "GET",
      headers,
    });
    if (!resp.ok) {
      return { ...DEFAULT_AUTH_CONFIG };
    }
    const raw: unknown = await resp.json();
    if (typeof raw !== "object" || raw === null) {
      return { ...DEFAULT_AUTH_CONFIG };
    }
    const obj = raw as Record<string, unknown>;
    return {
      env:
        typeof obj.env === "string" && obj.env.trim().length > 0
          ? obj.env.trim()
          : DEFAULT_AUTH_CONFIG.env,
      oidc_enabled: Boolean(obj.oidc_enabled),
      oidc_display_name:
        typeof obj.oidc_display_name === "string" &&
        obj.oidc_display_name.trim().length > 0
          ? obj.oidc_display_name.trim()
          : undefined,
      default_tenant:
        typeof obj.default_tenant === "string" &&
        obj.default_tenant.trim().length > 0
          ? obj.default_tenant.trim()
          : DEFAULT_AUTH_CONFIG.default_tenant,
    };
  } catch {
    return { ...DEFAULT_AUTH_CONFIG };
  }
}

/**
 * Authenticates via local username + password (`POST /api/auth/login` with `credentials: "include"`),
 * storing the returned `EdDSA` access token in memory and scheduling proactive silent refresh.
 */
export async function loginWithPassword(
  creds: LoginCredentials
): Promise<AccessTokenResponse> {
  const tenant = creds.tenant?.trim() || "default";
  const username = creds.username.trim();
  const resp = await fetch("/api/auth/login", {
    method: "POST",
    credentials: "include",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
    },
    body: JSON.stringify({
      tenant,
      username,
      password: creds.password,
    }),
  });

  if (!resp.ok) {
    throw await buildApiError(resp);
  }

  const data = (await resp.json()) as AccessTokenResponse;
  if (!data.access_token || typeof data.access_token !== "string") {
    throw new ApiError({
      status: 500,
      errorCode: "INVALID_AUTH_RESPONSE",
      message: "服务端未返回有效的访问令牌",
      traceId: "",
    });
  }
  setSessionAccessToken(data.access_token);
  return data;
}

/** Builds a validated internal OIDC start URL (`/api/auth/oidc/start?tenant=...&return_to=...`). */
export function buildOidcStartUrl(tenant: string, returnTo?: string): string {
  const safeTenant = tenant.trim() || "default";
  const safeReturnTo =
    returnTo && returnTo.startsWith("/") && !returnTo.startsWith("//")
      ? returnTo
      : "/";
  const sp = new URLSearchParams({
    tenant: safeTenant,
    return_to: safeReturnTo,
  });
  return `/api/auth/oidc/start?${sp.toString()}`;
}

/** Validates the current or candidate token via `GET /api/auth/me`. */
export async function fetchMe(candidateToken?: string): Promise<MeResponse> {
  const headers: Record<string, string> = {};
  if (candidateToken !== undefined) {
    const trimmed = candidateToken.trim();
    if (trimmed.length === 0) {
      throw new ApiError({
        status: 401,
        errorCode: "TOKEN_MISSING",
        message: "访问令牌不能为空",
        traceId: "",
      });
    }
    headers.Authorization = `Bearer ${trimmed}`;
  } else if (memoryToken) {
    headers.Authorization = `Bearer ${memoryToken}`;
  }
  const me = await requestJson<MeResponse>("/api/auth/me", { headers });
  if (me && typeof me.username === "string") {
    cachedMe = me;
    notifyAuthListeners();
  }
  return me;
}

/** Revokes the current session family and clears HttpOnly refresh cookie (`POST /api/auth/logout`). */
export async function logout(): Promise<void> {
  try {
    const headers: Record<string, string> = {};
    if (memoryToken) {
      headers.Authorization = `Bearer ${memoryToken}`;
    }
    await fetch("/api/auth/logout", {
      method: "POST",
      credentials: "include",
      headers,
    });
  } catch {
    // Best-effort network call; always clear local session state below.
  } finally {
    clearAccessToken();
  }
}

/** Revokes all active sessions for the current user and advances `tokens_valid_after` (`POST /api/auth/logout-all`). */
export async function logoutAll(): Promise<void> {
  try {
    await requestJson<Record<string, unknown>>("/api/auth/logout-all", {
      method: "POST",
      credentials: "include",
    });
  } finally {
    clearAccessToken();
  }
}

/** Updates the current local user's password (`POST /api/auth/password`), invalidating prior sessions. */
export async function changePassword(
  oldPassword: string,
  newPassword: string
): Promise<void> {
  await requestJson<Record<string, unknown>>("/api/auth/password", {
    method: "POST",
    credentials: "include",
    body: JSON.stringify({
      old_password: oldPassword,
      new_password: newPassword,
    }),
  });
}
