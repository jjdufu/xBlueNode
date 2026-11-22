import { z } from "zod";
import {
  clearAccessToken,
  getAccessToken,
  isStaticTokenMode,
  refreshAccessToken,
} from "./auth.ts";

/** Standard error response body from AxPoint Core & Sidecar (`A1` §5, with optional `details` on HTTP 422). */
export const ApiErrorBodySchema = z.object({
  error_code: z.string(),
  message: z.string(),
  trace_id: z.string(),
  details: z.unknown().optional(),
});

/** Structured HTTP error thrown by all API client helpers. */
export class ApiError extends Error {
  readonly status: number;
  readonly errorCode: string;
  override readonly message: string;
  readonly traceId: string;
  readonly details?: unknown;

  constructor(params: {
    status: number;
    errorCode: string;
    message: string;
    traceId: string;
    details?: unknown;
  }) {
    super(params.message);
    this.name = "ApiError";
    this.status = params.status;
    this.errorCode = params.errorCode;
    this.message = params.message;
    this.traceId = params.traceId;
    if (params.details !== undefined) {
      this.details = params.details;
    }
    Object.setPrototypeOf(this, ApiError.prototype);
  }
}

type UnauthorizedHandler = () => void;

let unauthorizedHandler: UnauthorizedHandler | null = null;

/** Registers a router-level callback invoked when a non-login request receives HTTP 401. */
export function setUnauthorizedHandler(handler: UnauthorizedHandler | null): void {
  unauthorizedHandler = handler;
}

function buildAuthHeaders(
  initHeaders?: HeadersInit,
  body?: BodyInit | null,
  tokenOverride?: string
): Headers {
  const headers = new Headers(initHeaders);
  if (tokenOverride) {
    headers.set("Authorization", `Bearer ${tokenOverride}`);
  } else if (!headers.has("Authorization")) {
    const token = getAccessToken();
    if (token) {
      headers.set("Authorization", `Bearer ${token}`);
    }
  }
  if (typeof body === "string" && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  return headers;
}

function extractPathname(url: string): string {
  try {
    if (url.startsWith("http://") || url.startsWith("https://")) {
      return new URL(url).pathname;
    }
    const qIdx = url.indexOf("?");
    return qIdx >= 0 ? url.slice(0, qIdx) : url;
  } catch {
    return url;
  }
}

function isAuthBypassPath(pathname: string): boolean {
  return (
    pathname === "/api/auth/login" ||
    pathname === "/api/auth/refresh" ||
    pathname === "/api/auth/logout" ||
    pathname === "/api/auth/config" ||
    pathname === "/api/auth/jwks.json" ||
    pathname.startsWith("/api/auth/oidc/")
  );
}

function shouldAttemptRefreshOn401(url: string, initHeaders?: HeadersInit): boolean {
  const pathname = extractPathname(url);
  if (isAuthBypassPath(pathname)) {
    return false;
  }
  if (isStaticTokenMode()) {
    return false;
  }
  const explicitHeaders = new Headers(initHeaders);
  if (explicitHeaders.has("Authorization")) {
    return false;
  }
  return true;
}

/** Parses structured `ApiError` from a non-2xx HTTP response. */
export async function buildApiError(resp: Response): Promise<ApiError> {
  const rawText = await resp.text();
  const trimmed = rawText.trim();
  if (trimmed.length > 0) {
    try {
      const parsed: unknown = JSON.parse(trimmed);
      const validated = ApiErrorBodySchema.safeParse(parsed);
      if (validated.success) {
        return new ApiError({
          status: resp.status,
          errorCode: validated.data.error_code,
          message: validated.data.message,
          traceId: validated.data.trace_id,
          details: validated.data.details,
        });
      }
    } catch {
      // Non-JSON response body; fall back to HTTP status error below.
    }
  }

  return new ApiError({
    status: resp.status,
    errorCode: `HTTP_${resp.status}`,
    message: trimmed || resp.statusText || `HTTP ${resp.status}`,
    traceId: resp.headers.get("x-request-id") ?? "",
  });
}

function handleUnauthorizedRedirect(): void {
  if (typeof window === "undefined" || window.location.pathname === "/login") {
    return;
  }
  clearAccessToken();
  if (unauthorizedHandler) {
    unauthorizedHandler();
  } else {
    window.location.replace("/login");
  }
}

async function fetchWithSingleFlightRefresh(
  url: string,
  init?: RequestInit
): Promise<Response> {
  const headers = buildAuthHeaders(init?.headers, init?.body);
  const resp = await fetch(url, {
    ...init,
    headers,
  });

  if (resp.status === 401 && shouldAttemptRefreshOn401(url, init?.headers)) {
    const nextToken = await refreshAccessToken();
    if (nextToken) {
      const retryHeaders = buildAuthHeaders(init?.headers, init?.body, nextToken);
      return fetch(url, {
        ...init,
        headers: retryHeaders,
      });
    }
  }

  return resp;
}

/** Sends an authenticated HTTP request and parses the JSON response. */
export async function requestJson<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await fetchWithSingleFlightRefresh(url, init);
  if (!resp.ok) {
    const apiErr = await buildApiError(resp);
    if (resp.status === 401 && extractPathname(url) !== "/api/auth/login") {
      handleUnauthorizedRedirect();
    }
    throw apiErr;
  }
  const text = await resp.text();
  if (text.trim().length === 0) {
    return {} as T;
  }
  return JSON.parse(text) as T;
}

/** Sends an authenticated `multipart/form-data` POST upload and parses the JSON response. */
export async function requestMultipart<T>(
  url: string,
  form: FormData,
  init?: Omit<RequestInit, "body" | "method">
): Promise<T> {
  const headers = new Headers(init?.headers);
  headers.delete("Content-Type");
  return requestJson<T>(url, {
    ...init,
    method: "POST",
    headers,
    body: form,
  });
}

/** Sends an authenticated HTTP request and returns the raw response Blob. */
export async function requestBlob(url: string, init?: RequestInit): Promise<Blob> {
  const resp = await fetchWithSingleFlightRefresh(url, init);
  if (!resp.ok) {
    const apiErr = await buildApiError(resp);
    if (resp.status === 401 && extractPathname(url) !== "/api/auth/login") {
      handleUnauthorizedRedirect();
    }
    throw apiErr;
  }
  return resp.blob();
}
