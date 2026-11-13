import type { Permission, TokenKind } from "./enums";
import { requestJson } from "./client";

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

const initialDevToken = import.meta.env.VITE_DEV_TOKEN?.trim() ?? "";
let memoryToken: string | null = initialDevToken.length > 0 ? initialDevToken : null;

/** Reads the in-memory access token (never persisted to localStorage). */
export function getAccessToken(): string | null {
  return memoryToken;
}

/** Stores an access token in memory. */
export function setAccessToken(token: string | null): void {
  const trimmed = token?.trim() ?? "";
  memoryToken = trimmed.length > 0 ? trimmed : null;
}

/** Clears the in-memory access token. */
export function clearAccessToken(): void {
  memoryToken = null;
}

/** Validates the current or candidate token via `GET /api/auth/me`. */
export async function fetchMe(candidateToken?: string): Promise<MeResponse> {
  const headers: Record<string, string> = {};
  const tok = candidateToken?.trim() ?? memoryToken;
  if (tok) {
    headers.Authorization = `Bearer ${tok}`;
  }
  return requestJson<MeResponse>("/api/auth/me", { headers });
}
