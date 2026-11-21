import { requestBlob, requestJson } from "./client";

export type Sensitivity = "PUBLIC" | "INTERNAL" | "CONFIDENTIAL" | "SECRET";
export type AuthSource = "LOCAL" | "OIDC";
export type UserAccountStatus = "ACTIVE" | "DISABLED";
export type PolicyTargetKind = "DATASET" | "NODE_TYPE";
export type GrantResourceKind = "DATASET" | "NODE_TYPE" | "SUBGRAPH" | "DASHBOARD" | "AGENT";
export type GranteeKind = "DEPT" | "ROLE" | "USER";
export type GrantStatus = "PENDING" | "ACTIVE" | "REJECTED" | "REVOKED" | "EXPIRED";

export type MaskRule =
  | "NONE"
  | "MASK_PHONE"
  | "MASK_ID_CARD"
  | "MASK_EMAIL"
  | "MASK_BANK_CARD"
  | "MASK_NAME"
  | "MASK_ADDRESS"
  | "HMAC_HASH"
  | "REDACT";

export interface SubjectSelector {
  roles?: string[];
  depts?: string[];
  users?: string[];
}

export interface DepartmentRecord {
  tenant_id: string;
  dept_id: string;
  name: string;
  parent_id: string | null;
}

export interface GuardUserRecord {
  id: string;
  tenant_id: string;
  username: string;
  display_name: string;
  email: string;
  dept_id: string;
  clearance: Sensitivity;
  auth_source: AuthSource;
  status: UserAccountStatus;
  roles: string[];
  attributes: Record<string, string[]>;
  failed_logins: number;
  locked_until: string | null;
  last_login_at: string | null;
  created_at: string;
}

export interface CreateGuardUserRequest {
  username: string;
  display_name?: string;
  email?: string;
  password: string;
  dept_id?: string;
  clearance?: Sensitivity;
  roles?: string[];
  attributes?: Record<string, string[]>;
}

export interface PatchGuardUserRequest {
  display_name?: string;
  email?: string;
  dept_id?: string;
  clearance?: Sensitivity;
  status?: UserAccountStatus;
  roles?: string[];
  unlock?: boolean;
}

export interface GuardRoleRecord {
  id: string;
  tenant_id: string;
  name: string;
  permissions: string[];
  is_builtin: boolean;
  created_at: string;
}

export interface CedarPolicyRecord {
  id: string;
  tenant_id: string;
  name: string;
  description: string;
  policy_text: string;
  enabled: boolean;
  version: number;
  updated_by: string | null;
  updated_at: string;
}

export interface ValidateCedarPolicyResponse {
  valid: boolean;
  errors: string[];
}

export interface SimulateCedarPolicyRequest {
  user_id: string;
  action: string;
  resource_kind: GrantResourceKind;
  resource_ref: string;
  owner_dept?: string;
  sensitivity?: Sensitivity;
  hour?: number;
  client_ip?: string;
}

export interface SimulateCedarPolicyResponse {
  decision: string;
  allowed: boolean;
  determining_policies: string[];
  diagnostics: string[];
}

export interface RowPolicyRecord {
  id: string;
  tenant_id: string;
  name: string;
  target_kind: PolicyTargetKind;
  target_ref: string;
  applies_to: SubjectSelector;
  predicate: string;
  enabled: boolean;
  created_at: string;
}

export interface SaveRowPolicyRequest {
  name: string;
  target_kind: PolicyTargetKind;
  target_ref: string;
  applies_to?: SubjectSelector;
  predicate: string;
  enabled?: boolean;
}

export interface RowPolicyDefaultRecord {
  tenant_id: string;
  target_kind: PolicyTargetKind;
  target_ref: string;
  default_deny: boolean;
}

export interface RowPolicyPreviewResponse {
  columns: Array<{ name: string; data_type: string }>;
  rows: Array<Record<string, unknown>>;
  row_count: number;
}

export interface ColumnPolicyRecord {
  id: string;
  tenant_id: string;
  target_kind: PolicyTargetKind;
  target_ref: string;
  column_name: string;
  mask_rule: MaskRule;
  exempt: SubjectSelector;
}

export interface PutColumnPolicyRequest {
  target_kind: PolicyTargetKind;
  target_ref: string;
  column_name: string;
  mask_rule: MaskRule;
  exempt?: SubjectSelector;
}

export interface GrantRecord {
  id: string;
  tenant_id: string;
  resource_kind: GrantResourceKind;
  resource_ref: string;
  grantee_kind: GranteeKind;
  grantee_ref: string;
  actions: string[];
  row_predicate: string | null;
  reason: string;
  status: GrantStatus;
  approval_id: string | null;
  requested_by: string;
  expires_at: string;
  created_at: string;
  decided_at: string | null;
}

export interface CreateGrantRequest {
  resource_kind: GrantResourceKind;
  resource_ref: string;
  grantee_kind: GranteeKind;
  grantee_ref: string;
  actions?: string[];
  row_predicate?: string;
  reason: string;
  expires_in_days: number;
}

export interface SessionSummaryRecord {
  id: string;
  family_id: string;
  user_id: string;
  username: string;
  auth_source: AuthSource;
  user_agent: string;
  client_ip: string;
  expires_at: string;
  rotated_at: string | null;
  revoked_at: string | null;
  created_at: string;
}

export interface AuditLogEntry {
  id: number;
  tenant_id: string;
  user_id: string | null;
  username: string;
  action: string;
  resource_ref: string;
  status_code: number;
  client_ip: string;
  trace_id: string;
  detail_json: Record<string, unknown>;
  prev_hash: string;
  row_hash: string;
  created_at: string;
}

export interface AuditVerifyResponse {
  ok: boolean;
  checked_count: number;
  first_broken_id: number | null;
}

export interface AuditStatsResponse {
  by_action: Record<string, number>;
  by_user: Array<[string, number]>;
  by_day: Array<[string, number]>;
}

export interface PaginatedItems<T> {
  items: T[];
  next_cursor?: string | null;
}

function unwrapItems<T>(res: T[] | PaginatedItems<T> | Record<string, unknown>): T[] {
  if (Array.isArray(res)) {
    return res;
  }
  if (res && typeof res === "object" && "items" in res && Array.isArray(res.items)) {
    return res.items as T[];
  }
  return [];
}

function buildQuery(params: Record<string, string | number | undefined>): string {
  const sp = new URLSearchParams();
  for (const [key, val] of Object.entries(params)) {
    if (val !== undefined && val !== "") {
      sp.set(key, String(val));
    }
  }
  const qs = sp.toString();
  return qs ? `?${qs}` : "";
}

export const axguardApi = {
  async listUsers(): Promise<GuardUserRecord[]> {
    const res = await requestJson<GuardUserRecord[] | PaginatedItems<GuardUserRecord>>("/api/axguard/users");
    return unwrapItems(res);
  },

  createUser(req: CreateGuardUserRequest): Promise<GuardUserRecord> {
    return requestJson<GuardUserRecord>("/api/axguard/users", {
      method: "POST",
      body: JSON.stringify(req),
    });
  },

  patchUser(userId: string, req: PatchGuardUserRequest): Promise<GuardUserRecord> {
    return requestJson<GuardUserRecord>(`/api/axguard/users/${encodeURIComponent(userId)}`, {
      method: "PATCH",
      body: JSON.stringify(req),
    });
  },

  resetUserPassword(userId: string, newPassword: string): Promise<{ ok: boolean }> {
    return requestJson<{ ok: boolean }>(
      `/api/axguard/users/${encodeURIComponent(userId)}/reset-password`,
      {
        method: "POST",
        body: JSON.stringify({ new_password: newPassword }),
      }
    );
  },

  putUserAttributes(
    userId: string,
    attributes: Record<string, string[]>
  ): Promise<Record<string, string[]>> {
    return requestJson<Record<string, string[]>>(
      `/api/axguard/users/${encodeURIComponent(userId)}/attributes`,
      {
        method: "PUT",
        body: JSON.stringify({ attributes }),
      }
    );
  },

  async listRoles(): Promise<GuardRoleRecord[]> {
    const res = await requestJson<GuardRoleRecord[] | PaginatedItems<GuardRoleRecord>>("/api/axguard/roles");
    return unwrapItems(res);
  },

  createRole(name: string, permissions: string[]): Promise<GuardRoleRecord> {
    return requestJson<GuardRoleRecord>("/api/axguard/roles", {
      method: "POST",
      body: JSON.stringify({ name, permissions }),
    });
  },

  patchRole(roleId: string, req: { name?: string; permissions?: string[] }): Promise<GuardRoleRecord> {
    return requestJson<GuardRoleRecord>(`/api/axguard/roles/${encodeURIComponent(roleId)}`, {
      method: "PATCH",
      body: JSON.stringify(req),
    });
  },

  deleteRole(roleId: string): Promise<{ deleted: boolean }> {
    return requestJson<{ deleted: boolean }>(`/api/axguard/roles/${encodeURIComponent(roleId)}`, {
      method: "DELETE",
    });
  },

  async listDepartments(): Promise<DepartmentRecord[]> {
    const res = await requestJson<DepartmentRecord[] | PaginatedItems<DepartmentRecord>>(
      "/api/axguard/departments"
    );
    return unwrapItems(res);
  },

  upsertDepartment(
    deptId: string,
    req: { name: string; parent_id?: string | null }
  ): Promise<DepartmentRecord> {
    return requestJson<DepartmentRecord>(`/api/axguard/departments/${encodeURIComponent(deptId)}`, {
      method: "PUT",
      body: JSON.stringify(req),
    });
  },

  deleteDepartment(deptId: string): Promise<{ deleted: boolean }> {
    return requestJson<{ deleted: boolean }>(`/api/axguard/departments/${encodeURIComponent(deptId)}`, {
      method: "DELETE",
    });
  },

  async listPolicies(): Promise<CedarPolicyRecord[]> {
    const res = await requestJson<CedarPolicyRecord[] | PaginatedItems<CedarPolicyRecord>>(
      "/api/axguard/policies"
    );
    return unwrapItems(res);
  },

  putPolicy(
    name: string,
    req: { policy_text: string; description?: string; enabled?: boolean }
  ): Promise<CedarPolicyRecord> {
    return requestJson<CedarPolicyRecord>(`/api/axguard/policies/${encodeURIComponent(name)}`, {
      method: "PUT",
      body: JSON.stringify(req),
    });
  },

  deletePolicy(name: string): Promise<{ deleted: boolean }> {
    return requestJson<{ deleted: boolean }>(`/api/axguard/policies/${encodeURIComponent(name)}`, {
      method: "DELETE",
    });
  },

  validatePolicy(policyText: string): Promise<ValidateCedarPolicyResponse> {
    return requestJson<ValidateCedarPolicyResponse>("/api/axguard/policies/validate", {
      method: "POST",
      body: JSON.stringify({ policy_text: policyText }),
    });
  },

  simulatePolicy(req: SimulateCedarPolicyRequest): Promise<SimulateCedarPolicyResponse> {
    return requestJson<SimulateCedarPolicyResponse>("/api/axguard/policies/simulate", {
      method: "POST",
      body: JSON.stringify(req),
    });
  },

  async listRowPolicies(): Promise<RowPolicyRecord[]> {
    const res = await requestJson<RowPolicyRecord[] | PaginatedItems<RowPolicyRecord>>(
      "/api/axguard/row-policies"
    );
    return unwrapItems(res);
  },

  createRowPolicy(req: SaveRowPolicyRequest): Promise<RowPolicyRecord> {
    return requestJson<RowPolicyRecord>("/api/axguard/row-policies", {
      method: "POST",
      body: JSON.stringify(req),
    });
  },

  updateRowPolicy(id: string, req: SaveRowPolicyRequest): Promise<RowPolicyRecord> {
    return requestJson<RowPolicyRecord>(`/api/axguard/row-policies/${encodeURIComponent(id)}`, {
      method: "PUT",
      body: JSON.stringify(req),
    });
  },

  deleteRowPolicy(id: string): Promise<{ deleted: boolean }> {
    return requestJson<{ deleted: boolean }>(`/api/axguard/row-policies/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  },

  async listRowPolicyDefaults(): Promise<RowPolicyDefaultRecord[]> {
    const res = await requestJson<RowPolicyDefaultRecord[] | PaginatedItems<RowPolicyDefaultRecord>>(
      "/api/axguard/row-policy-defaults"
    );
    return unwrapItems(res);
  },

  putRowPolicyDefault(req: {
    target_kind: PolicyTargetKind;
    target_ref: string;
    default_deny: boolean;
  }): Promise<RowPolicyDefaultRecord> {
    return requestJson<RowPolicyDefaultRecord>("/api/axguard/row-policy-defaults", {
      method: "PUT",
      body: JSON.stringify(req),
    });
  },

  previewRowPolicy(userId: string, datasetId: string): Promise<RowPolicyPreviewResponse> {
    return requestJson<RowPolicyPreviewResponse>("/api/axguard/row-policies/preview", {
      method: "POST",
      body: JSON.stringify({ user_id: userId, dataset_id: datasetId }),
    });
  },

  async listColumnPolicies(): Promise<ColumnPolicyRecord[]> {
    const res = await requestJson<ColumnPolicyRecord[] | PaginatedItems<ColumnPolicyRecord>>(
      "/api/axguard/column-policies"
    );
    return unwrapItems(res);
  },

  putColumnPolicy(req: PutColumnPolicyRequest): Promise<ColumnPolicyRecord> {
    return requestJson<ColumnPolicyRecord>("/api/axguard/column-policies", {
      method: "PUT",
      body: JSON.stringify(req),
    });
  },

  deleteColumnPolicy(id: string): Promise<{ deleted: boolean }> {
    return requestJson<{ deleted: boolean }>(`/api/axguard/column-policies/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  },

  async listGrants(): Promise<GrantRecord[]> {
    const res = await requestJson<GrantRecord[] | PaginatedItems<GrantRecord>>("/api/axguard/grants");
    return unwrapItems(res);
  },

  createGrant(req: CreateGrantRequest): Promise<GrantRecord> {
    return requestJson<GrantRecord>("/api/axguard/grants", {
      method: "POST",
      body: JSON.stringify(req),
    });
  },

  revokeGrant(id: string): Promise<{ deleted: boolean }> {
    return requestJson<{ deleted: boolean }>(`/api/axguard/grants/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  },

  async listSessions(userId?: string): Promise<SessionSummaryRecord[]> {
    const res = await requestJson<SessionSummaryRecord[] | PaginatedItems<SessionSummaryRecord>>(
      `/api/axguard/sessions${buildQuery({ user_id: userId })}`
    );
    return unwrapItems(res);
  },

  revokeSessionFamily(familyId: string): Promise<{ deleted: boolean }> {
    return requestJson<{ deleted: boolean }>(`/api/axguard/sessions/${encodeURIComponent(familyId)}`, {
      method: "DELETE",
    });
  },

  async queryAuditLogs(params: {
    action?: string;
    username?: string;
    resource?: string;
    from?: string;
    to?: string;
    status_code?: number;
    limit?: number;
    cursor?: string;
  }): Promise<PaginatedItems<AuditLogEntry>> {
    const res = await requestJson<PaginatedItems<AuditLogEntry> | AuditLogEntry[]>(
      `/api/axguard/audit${buildQuery(params)}`
    );
    if (Array.isArray(res)) {
      return { items: res, next_cursor: null };
    }
    return {
      items: Array.isArray(res?.items) ? res.items : [],
      next_cursor: res?.next_cursor ?? null,
    };
  },

  exportAuditCsv(params: {
    action?: string;
    username?: string;
    resource?: string;
    from?: string;
    to?: string;
    status_code?: number;
  }): Promise<Blob> {
    return requestBlob(`/api/axguard/audit/export.csv${buildQuery(params)}`);
  },

  async verifyAuditChain(params?: {
    from_id?: number;
    to_id?: number;
  }): Promise<AuditVerifyResponse> {
    const res = await requestJson<Partial<AuditVerifyResponse>>(
      `/api/axguard/audit/verify${buildQuery(params ?? {})}`
    );
    return {
      ok: typeof res?.ok === "boolean" ? res.ok : true,
      checked_count: typeof res?.checked_count === "number" ? res.checked_count : 0,
      first_broken_id:
        typeof res?.first_broken_id === "number" ? res.first_broken_id : null,
    };
  },

  async queryAuditStats(params?: { from?: string; to?: string }): Promise<AuditStatsResponse> {
    const res = await requestJson<Partial<AuditStatsResponse>>(
      `/api/axguard/audit/stats${buildQuery(params ?? {})}`
    );
    return {
      by_action:
        res?.by_action && typeof res.by_action === "object" ? res.by_action : {},
      by_user: Array.isArray(res?.by_user) ? res.by_user : [],
      by_day: Array.isArray(res?.by_day) ? res.by_day : [],
    };
  },
};
