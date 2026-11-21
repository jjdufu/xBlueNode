import { z } from "zod";
import { clearAccessToken, getAccessToken } from "./auth";

/** Standard three-field error response body from AxPoint Core & Sidecar (`A1` §5). */
export const ApiErrorBodySchema = z.object({
  error_code: z.string(),
  message: z.string(),
  trace_id: z.string(),
});

/** Structured HTTP error thrown by all API client helpers. */
export class ApiError extends Error {
  readonly status: number;
  readonly errorCode: string;
  override readonly message: string;
  readonly traceId: string;

  constructor(params: {
    status: number;
    errorCode: string;
    message: string;
    traceId: string;
  }) {
    super(params.message);
    this.name = "ApiError";
    this.status = params.status;
    this.errorCode = params.errorCode;
    this.message = params.message;
    this.traceId = params.traceId;
    Object.setPrototypeOf(this, ApiError.prototype);
  }
}

type UnauthorizedHandler = () => void;

let unauthorizedHandler: UnauthorizedHandler | null = null;

/** Registers a router-level callback invoked when a non-login request receives HTTP 401. */
export function setUnauthorizedHandler(handler: UnauthorizedHandler | null): void {
  unauthorizedHandler = handler;
}

function buildAuthHeaders(initHeaders?: HeadersInit): Headers {
  const headers = new Headers(initHeaders);
  if (!headers.has("Authorization")) {
    const token = getAccessToken();
    if (token) {
      headers.set("Authorization", `Bearer ${token}`);
    }
  }
  return headers;
}

async function buildApiError(resp: Response): Promise<ApiError> {
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

/** Sends an authenticated HTTP request and parses the JSON response. */
export async function requestJson<T>(url: string, init?: RequestInit): Promise<T> {
  const headers = buildAuthHeaders(init?.headers);
  const resp = await fetch(url, {
    ...init,
    headers,
  });
  if (!resp.ok) {
    const apiErr = await buildApiError(resp);
    if (resp.status === 401) {
      handleUnauthorizedRedirect();
    }
    throw apiErr;
  }
  return (await resp.json()) as T;
}

/** Sends an authenticated `multipart/form-data` POST upload and parses the JSON response. */
export async function requestMultipart<T>(
  url: string,
  form: FormData,
  init?: Omit<RequestInit, "body" | "method">
): Promise<T> {
  const headers = buildAuthHeaders(init?.headers);
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
  const headers = buildAuthHeaders(init?.headers);
  const resp = await fetch(url, {
    ...init,
    headers,
  });
  if (!resp.ok) {
    const apiErr = await buildApiError(resp);
    if (resp.status === 401) {
      handleUnauthorizedRedirect();
    }
    throw apiErr;
  }
  return resp.blob();
}
