import type { PiiCategory } from "@/lib/api/enums";
import type {
  DepartmentRecord,
  MaskRule,
  SubjectSelector,
} from "@/lib/api/axguard";

export interface DepartmentTreeNode {
  readonly record: DepartmentRecord;
  readonly children: DepartmentTreeNode[];
  readonly depth: number;
}

/**
 * Builds a deterministic, cycle-safe hierarchical department tree from flat `DepartmentRecord[]`.
 */
export function buildDepartmentTree(
  departments: readonly DepartmentRecord[]
): DepartmentTreeNode[] {
  const byId = new Map<string, DepartmentRecord>();
  for (const dept of departments) {
    byId.set(dept.dept_id, dept);
  }

  const childrenMap = new Map<string | null, DepartmentRecord[]>();
  for (const dept of departments) {
    const parentKey =
      dept.parent_id &&
      dept.parent_id !== dept.dept_id &&
      byId.has(dept.parent_id)
        ? dept.parent_id
        : null;
    const bucket = childrenMap.get(parentKey) ?? [];
    bucket.push(dept);
    childrenMap.set(parentKey, bucket);
  }

  for (const bucket of childrenMap.values()) {
    bucket.sort((a, b) => {
      if (a.dept_id === "default") return -1;
      if (b.dept_id === "default") return 1;
      return a.dept_id.localeCompare(b.dept_id);
    });
  }

  const visited = new Set<string>();
  function buildNodes(
    parentKey: string | null,
    depth: number
  ): DepartmentTreeNode[] {
    const list = childrenMap.get(parentKey) ?? [];
    const nodes: DepartmentTreeNode[] = [];
    for (const item of list) {
      if (visited.has(item.dept_id)) {
        continue;
      }
      visited.add(item.dept_id);
      nodes.push({
        record: item,
        depth,
        children: buildNodes(item.dept_id, depth + 1),
      });
    }
    return nodes;
  }

  const roots = buildNodes(null, 0);

  // Attach any remaining disconnected cycle participants as top-level fallbacks so no record is hidden.
  for (const dept of departments) {
    if (!visited.has(dept.dept_id)) {
      visited.add(dept.dept_id);
      roots.push({
        record: dept,
        depth: 0,
        children: buildNodes(dept.dept_id, 1),
      });
    }
  }

  return roots;
}

/**
 * Collects `deptId` and all of its descendants so re-parenting dropdowns cannot introduce cycles.
 */
export function collectDescendantDeptIds(
  departments: readonly DepartmentRecord[],
  rootDeptId: string
): Set<string> {
  const excluded = new Set<string>([rootDeptId]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const d of departments) {
      if (
        d.parent_id &&
        excluded.has(d.parent_id) &&
        !excluded.has(d.dept_id)
      ) {
        excluded.add(d.dept_id);
        changed = true;
      }
    }
  }
  return excluded;
}

/**
 * Validates local password policy (`11-STEP13` §13.3): length >= 12 and must not contain username.
 */
export function validateLocalPassword(
  password: string,
  username?: string
): string | null {
  if (password.length < 12) {
    return "口令长度必须不少于 12 个字符";
  }
  const cleanUser = username?.trim() ?? "";
  if (
    cleanUser.length > 0 &&
    password.toLowerCase().includes(cleanUser.toLowerCase())
  ) {
    return "口令中不得包含用户名";
  }
  return null;
}

/**
 * Infers default `MaskRule` from profiled `PiiCategory` (`11-STEP13` §13.7),
 * or applies explicit override when a `ColumnPolicyRecord` exists.
 */
export function inferDefaultMaskRule(category: PiiCategory): MaskRule {
  switch (category) {
    case "NONE":
      return "NONE";
    case "PHONE":
      return "MASK_PHONE";
    case "ID_CARD":
      return "MASK_ID_CARD";
    case "EMAIL":
      return "MASK_EMAIL";
    case "BANK_CARD":
      return "MASK_BANK_CARD";
    case "NAME":
      return "MASK_NAME";
    case "ADDRESS":
      return "MASK_ADDRESS";
    case "SECRET":
      return "REDACT";
  }
}

export function resolveEffectiveMaskRule(
  piiCategory: PiiCategory,
  explicitRule?: MaskRule | null
): { effectiveRule: MaskRule; source: "EXPLICIT" | "PROFILE_DEFAULT" | "NONE" } {
  if (explicitRule !== undefined && explicitRule !== null) {
    return { effectiveRule: explicitRule, source: "EXPLICIT" };
  }
  const inferred = inferDefaultMaskRule(piiCategory);
  if (inferred !== "NONE") {
    return { effectiveRule: inferred, source: "PROFILE_DEFAULT" };
  }
  return { effectiveRule: "NONE", source: "NONE" };
}

/**
 * Checks whether an account's `locked_until` timestamp is still active.
 */
export function isAccountLocked(
  lockedUntil: string | null | undefined,
  nowMs: number = Date.now()
): boolean {
  if (!lockedUntil) {
    return false;
  }
  const ts = Date.parse(lockedUntil);
  return Number.isFinite(ts) && ts > nowMs;
}

/**
 * Formats a `SubjectSelector` into a concise human-readable summary.
 */
export function formatSubjectSelector(
  selector: SubjectSelector | undefined,
  emptyLabel: string
): string {
  if (!selector) {
    return emptyLabel;
  }
  const parts: string[] = [];
  if (selector.roles && selector.roles.length > 0) {
    parts.push(`角色: ${selector.roles.join(", ")}`);
  }
  if (selector.depts && selector.depts.length > 0) {
    parts.push(`部门: ${selector.depts.join(", ")}`);
  }
  if (selector.users && selector.users.length > 0) {
    parts.push(`用户: ${selector.users.length} 人`);
  }
  return parts.length > 0 ? parts.join(" · ") : emptyLabel;
}

/**
 * Resolves the initial `AxGuard` tab key based on caller permissions (`GUARD_ADMIN` vs `AUDIT_READ` vs `DATA_READ`).
 */
export type AxGuardTabKey =
  | "users"
  | "roles"
  | "depts"
  | "policies"
  | "row_policies"
  | "column_policies"
  | "grants"
  | "audit";

export function resolveDefaultAxGuardTab(
  permissions: readonly string[],
  roles: readonly string[]
): AxGuardTabKey {
  if (roles.includes("ADMIN") || permissions.includes("GUARD_ADMIN")) {
    return "users";
  }
  if (permissions.includes("AUDIT_READ")) {
    return "audit";
  }
  if (permissions.includes("DATA_READ")) {
    return "grants";
  }
  return "users";
}
