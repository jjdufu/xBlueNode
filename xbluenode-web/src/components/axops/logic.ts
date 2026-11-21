import type {
  DeepHealthReport,
  RetentionPolicyRecord,
  TemplateInstallPlan,
  WorkItem,
  WorkItemStatus,
} from "../../lib/api/axops.ts";
import {
  type AxOpsTabKey,
  RETENTION_TARGET_META,
  type RetentionTargetMeta,
  WORK_ITEM_STATUS_COLUMNS,
} from "./constants.ts";

/**
 * Returns `true` when the caller holds `OPS_ADMIN` permission or built-in `ADMIN` role.
 */
export function hasOpsAdminAccess(
  permissions: readonly string[],
  roles: readonly string[]
): boolean {
  return roles.includes("ADMIN") || permissions.includes("OPS_ADMIN");
}

/**
 * Checks role/permission access for each `AxOps` sub-panel (`12-STEP14` §14.8):
 * - `health`, `retention`, `backups`: `OPS_ADMIN`
 * - `jobs`, `templates`: `OPS_ADMIN` or `DATA_READ`
 * - `work_items`: `OPS_ADMIN`, `DATA_READ`, or `FLOW_RUN`
 */
export function canAccessAxOpsTab(
  tab: AxOpsTabKey,
  permissions: readonly string[],
  roles: readonly string[]
): boolean {
  const isAdmin = hasOpsAdminAccess(permissions, roles);
  if (isAdmin) {
    return true;
  }
  switch (tab) {
    case "health":
    case "retention":
    case "backups":
      return false;
    case "jobs":
    case "templates":
      return permissions.includes("DATA_READ");
    case "work_items":
      return (
        permissions.includes("DATA_READ") || permissions.includes("FLOW_RUN")
      );
  }
}

/**
 * Resolves the default `AxOps` tab based on caller permissions:
 * - `OPS_ADMIN` (or `ADMIN` role) -> `"health"`
 * - Non-`OPS_ADMIN` (`DATA_READ` / `FLOW_RUN`) -> `"work_items"`
 */
export function resolveDefaultAxOpsTab(
  permissions: readonly string[],
  roles: readonly string[]
): AxOpsTabKey {
  if (hasOpsAdminAccess(permissions, roles)) {
    return "health";
  }
  if (permissions.includes("FLOW_RUN") || permissions.includes("DATA_READ")) {
    return "work_items";
  }
  return "work_items";
}

/**
 * Formats byte counts into concise binary units (`B`, `KB`, `MB`, `GB`, `TB`).
 */
export function formatByteSize(bytes: number | null | undefined): string {
  if (
    bytes === null ||
    bytes === undefined ||
    !Number.isFinite(bytes) ||
    bytes < 0
  ) {
    return "—";
  }
  if (bytes < 1024) {
    return `${Math.round(bytes)} B`;
  }
  const kb = bytes / 1024;
  if (kb < 1024) {
    return `${kb.toFixed(1)} KB`;
  }
  const mb = kb / 1024;
  if (mb < 1024) {
    return `${mb.toFixed(2)} MB`;
  }
  const gb = mb / 1024;
  if (gb < 1024) {
    return `${gb.toFixed(2)} GB`;
  }
  const tb = gb / 1024;
  return `${tb.toFixed(2)} TB`;
}

/**
 * Formats a duration in seconds into a compact Chinese/technical duration string.
 */
export function formatDurationSeconds(
  seconds: number | null | undefined
): string {
  if (
    seconds === null ||
    seconds === undefined ||
    !Number.isFinite(seconds) ||
    seconds < 0
  ) {
    return "—";
  }
  const whole = Math.floor(seconds);
  if (whole < 60) {
    return `${whole}s`;
  }
  const minutes = Math.floor(whole / 60);
  const remSec = whole % 60;
  if (minutes < 60) {
    return remSec > 0 ? `${minutes}m ${remSec}s` : `${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  const remMin = minutes % 60;
  if (hours < 24) {
    return remMin > 0 ? `${hours}h ${remMin}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const remHours = hours % 24;
  return remHours > 0 ? `${days}d ${remHours}h` : `${days}d`;
}

/**
 * Returns recommended next status transitions for a work item (`12-STEP14` §14.7):
 * `OPEN -> IN_PROGRESS -> DONE`, and any active state can transition to `BLOCKED` or `CANCELED`.
 */
export function getAllowedWorkItemTransitions(
  current: WorkItemStatus
): readonly WorkItemStatus[] {
  switch (current) {
    case "OPEN":
      return ["IN_PROGRESS", "BLOCKED", "CANCELED"];
    case "IN_PROGRESS":
      return ["DONE", "BLOCKED", "CANCELED"];
    case "BLOCKED":
      return ["IN_PROGRESS", "OPEN", "CANCELED"];
    case "DONE":
      return ["IN_PROGRESS", "OPEN"];
    case "CANCELED":
      return ["OPEN"];
  }
}

export interface TemplatePlanSummary {
  readonly totalObjects: number;
  readonly conflictCount: number;
  readonly createCount: number;
  readonly renameCount: number;
  readonly skipCount: number;
}

/**
 * Summarizes object creation vs conflict actions (`RENAME` / `SKIP`) in a `TemplateInstallPlan`.
 */
export function summarizeTemplatePlanConflicts(
  plan: TemplateInstallPlan | null | undefined
): TemplatePlanSummary {
  if (!plan || !Array.isArray(plan.objects)) {
    return {
      totalObjects: 0,
      conflictCount: 0,
      createCount: 0,
      renameCount: 0,
      skipCount: 0,
    };
  }

  let conflictCount = 0;
  let createCount = 0;
  let renameCount = 0;
  let skipCount = 0;

  for (const obj of plan.objects) {
    const act = (obj.action ?? "").toUpperCase();
    if (obj.conflict) {
      conflictCount += 1;
    }
    if (act === "SKIP" || act === "SKIPPED") {
      skipCount += 1;
    } else if (act === "RENAME" || act === "RENAMED") {
      renameCount += 1;
    } else {
      createCount += 1;
    }
  }

  const explicitConflicts = Array.isArray(plan.conflicts)
    ? plan.conflicts.length
    : 0;

  return {
    totalObjects: plan.objects.length,
    conflictCount: Math.max(conflictCount, explicitConflicts),
    createCount,
    renameCount,
    skipCount,
  };
}

/**
 * Evaluates overall deep health status across all 7 probed subsystems (`12-STEP14` §14.4.3).
 */
export function evaluateOverallDeepHealth(
  report: DeepHealthReport | null | undefined
): { allOk: boolean; degradedComponents: string[] } {
  if (!report) {
    return { allOk: false, degradedComponents: [] };
  }
  const degraded: string[] = [];
  if (!report.database?.ok) degraded.push("database");
  if (!report.parser?.ok) degraded.push("parser");
  if (!report.lake?.ok) degraded.push("lake");
  if (!report.llm?.ok) degraded.push("llm");
  if (!report.embed?.ok) degraded.push("embed");
  if (!report.disk?.ok) degraded.push("disk");
  if (!report.migrations?.ok || (report.migrations?.pending ?? 0) > 0) {
    degraded.push("migrations");
  }
  return {
    allOk: degraded.length === 0,
    degradedComponents: degraded,
  };
}

/**
 * Merges server-returned `RetentionPolicyRecord[]` with all 11 canonical `RetentionTarget` entries
 * in deterministic display order.
 */
export function mergeRetentionPoliciesWithDefaults(
  records: readonly RetentionPolicyRecord[]
): Array<RetentionTargetMeta & { keep_days: number }> {
  const byTarget = new Map<string, number>();
  for (const rec of records) {
    byTarget.set(rec.target, rec.keep_days);
  }
  return RETENTION_TARGET_META.map((meta) => ({
    ...meta,
    keep_days: byTarget.get(meta.target) ?? meta.defaultKeepDays,
  }));
}

/**
 * Groups `WorkItem[]` into the 5 status buckets for Kanban rendering, preserving server ordering within each column.
 */
export function groupWorkItemsByStatus(
  items: readonly WorkItem[]
): Record<WorkItemStatus, WorkItem[]> {
  const grouped: Record<WorkItemStatus, WorkItem[]> = {
    OPEN: [],
    IN_PROGRESS: [],
    BLOCKED: [],
    DONE: [],
    CANCELED: [],
  };
  for (const col of WORK_ITEM_STATUS_COLUMNS) {
    grouped[col.status] = [];
  }
  for (const item of items) {
    if (item.status in grouped) {
      grouped[item.status].push(item);
    }
  }
  return grouped;
}

/**
 * Formats `WorkItem` code consistently (`WI-<item_no>` fallback if `code` is empty).
 */
export function formatWorkItemCode(
  item: Pick<WorkItem, "code" | "item_no">
): string {
  const trimmed = item.code?.trim() ?? "";
  return trimmed.length > 0 ? trimmed : `WI-${item.item_no}`;
}
