import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type {
  DeepHealthReport,
  TemplateInstallPlan,
  WorkItem,
} from "../../lib/api/axops.ts";
import {
  readLoggedOutFlag,
  writeLoggedOutFlag,
} from "../../lib/api/auth.ts";
import {
  canAccessAxOpsTab,
  evaluateOverallDeepHealth,
  formatByteSize,
  formatDurationSeconds,
  formatWorkItemCode,
  getAllowedWorkItemTransitions,
  groupWorkItemsByStatus,
  hasOpsAdminAccess,
  mergeRetentionPoliciesWithDefaults,
  resolveDefaultAxOpsTab,
  summarizeTemplatePlanConflicts,
} from "./logic.ts";

describe("AxOps operations, templates & work-item logic", () => {
  it("resolveDefaultAxOpsTab selects health for OPS_ADMIN and work_items for non-admin operators", () => {
    assert.equal(resolveDefaultAxOpsTab(["OPS_ADMIN"], ["OPERATOR"]), "health");
    assert.equal(resolveDefaultAxOpsTab(["DATA_READ"], ["ADMIN"]), "health");
    assert.equal(
      resolveDefaultAxOpsTab(["DATA_READ", "FLOW_RUN"], ["OPERATOR"]),
      "work_items"
    );
    assert.equal(resolveDefaultAxOpsTab(["DATA_READ"], ["ANALYST"]), "work_items");
  });

  it("canAccessAxOpsTab gates admin-only panels while allowing jobs/templates/work_items for DATA_READ/FLOW_RUN", () => {
    assert.equal(hasOpsAdminAccess(["DATA_READ"], ["ANALYST"]), false);
    assert.equal(hasOpsAdminAccess(["OPS_ADMIN"], ["VIEWER"]), true);

    assert.equal(canAccessAxOpsTab("health", ["DATA_READ"], ["ANALYST"]), false);
    assert.equal(
      canAccessAxOpsTab("retention", ["DATA_READ"], ["ANALYST"]),
      false
    );
    assert.equal(canAccessAxOpsTab("backups", ["DATA_READ"], ["ANALYST"]), false);
    assert.equal(canAccessAxOpsTab("jobs", ["DATA_READ"], ["ANALYST"]), true);
    assert.equal(
      canAccessAxOpsTab("templates", ["DATA_READ"], ["ANALYST"]),
      true
    );
    assert.equal(
      canAccessAxOpsTab("work_items", ["FLOW_RUN"], ["OPERATOR"]),
      true
    );
  });

  it("formatByteSize formats bytes across B, KB, MB, GB, and TB cleanly", () => {
    assert.equal(formatByteSize(null), "—");
    assert.equal(formatByteSize(-10), "—");
    assert.equal(formatByteSize(512), "512 B");
    assert.equal(formatByteSize(2048), "2.0 KB");
    assert.equal(formatByteSize(5 * 1024 * 1024), "5.00 MB");
    assert.equal(formatByteSize(3 * 1024 * 1024 * 1024), "3.00 GB");
  });

  it("formatDurationSeconds formats sub-minute, minute, hour, and multi-day durations", () => {
    assert.equal(formatDurationSeconds(undefined), "—");
    assert.equal(formatDurationSeconds(45), "45s");
    assert.equal(formatDurationSeconds(125), "2m 5s");
    assert.equal(formatDurationSeconds(7260), "2h 1m");
    assert.equal(formatDurationSeconds(90_000), "1d 1h");
  });

  it("getAllowedWorkItemTransitions follows §14.7 lifecycle transitions", () => {
    assert.deepEqual(getAllowedWorkItemTransitions("OPEN"), [
      "IN_PROGRESS",
      "BLOCKED",
      "CANCELED",
    ]);
    assert.deepEqual(getAllowedWorkItemTransitions("IN_PROGRESS"), [
      "DONE",
      "BLOCKED",
      "CANCELED",
    ]);
    assert.deepEqual(getAllowedWorkItemTransitions("BLOCKED"), [
      "IN_PROGRESS",
      "OPEN",
      "CANCELED",
    ]);
    assert.deepEqual(getAllowedWorkItemTransitions("CANCELED"), ["OPEN"]);
  });

  it("summarizeTemplatePlanConflicts counts CREATE, RENAME, SKIP, and conflicts accurately", () => {
    const plan: TemplateInstallPlan = {
      pack_id: "supply_chain",
      pack_version: "1.0.0",
      on_conflict: "RENAME",
      conflicts: ["dataset:orders", "dashboard:supply-overview"],
      objects: [
        {
          category: "node_type",
          key: "Factory",
          source_name: "Factory",
          target_name: "Factory",
          conflict: false,
          action: "CREATE",
        },
        {
          category: "dataset",
          key: "orders",
          source_name: "orders",
          target_name: "orders_supply_chain",
          conflict: true,
          action: "RENAME",
        },
        {
          category: "dashboard",
          key: "overview",
          source_name: "supply-overview",
          target_name: "supply-overview",
          conflict: true,
          action: "SKIP",
        },
      ],
    };
    assert.deepEqual(summarizeTemplatePlanConflicts(plan), {
      totalObjects: 3,
      conflictCount: 2,
      createCount: 1,
      renameCount: 1,
      skipCount: 1,
    });
  });

  it("evaluateOverallDeepHealth detects healthy vs degraded components and pending migrations", () => {
    const healthy: DeepHealthReport = {
      database: { ok: true, latency_ms: 2, replication_lag_s: 0 },
      parser: { ok: true, version: "0.1.0" },
      lake: { ok: true, snapshot_id: 12 },
      llm: { ok: true, provider: "cassette" },
      embed: { ok: true },
      disk: { ok: true, data_root_free_bytes: 50_000_000_000 },
      jobs: { pending: 0, running: 1, failed_24h: 0, oldest_pending_s: 0 },
      outbox: { lag_s: 1, pending: 0 },
      migrations: { ok: true, current: 14, pending: 0 },
      version: "0.1.0",
    };
    assert.deepEqual(evaluateOverallDeepHealth(healthy), {
      allOk: true,
      degradedComponents: [],
    });

    const degraded: DeepHealthReport = {
      ...healthy,
      parser: { ok: false, version: "", error: "timeout" },
      migrations: { ok: true, current: 13, pending: 1 },
    };
    assert.deepEqual(evaluateOverallDeepHealth(degraded), {
      allOk: false,
      degradedComponents: ["parser", "migrations"],
    });
  });

  it("mergeRetentionPoliciesWithDefaults returns all 11 targets and marks AUDIT as owner-script only", () => {
    const merged = mergeRetentionPoliciesWithDefaults([
      { target: "JOB", keep_days: 45 },
      { target: "AUDIT", keep_days: 730 },
    ]);
    assert.equal(merged.length, 11);
    const jobEntry = merged.find((m) => m.target === "JOB");
    const auditEntry = merged.find((m) => m.target === "AUDIT");
    assert.equal(jobEntry?.keep_days, 45);
    assert.equal(jobEntry?.ownerScriptOnly, false);
    assert.equal(auditEntry?.keep_days, 730);
    assert.equal(auditEntry?.ownerScriptOnly, true);
  });

  it("groupWorkItemsByStatus and formatWorkItemCode partition Kanban cards deterministically", () => {
    const sampleBase: Omit<WorkItem, "id" | "item_no" | "code" | "status"> = {
      tenant_id: "default",
      title: "排查低库存告警",
      description: "",
      item_kind: "ISSUE",
      priority: "P0",
      assignee: null,
      source_kind: "ALERT",
      source_ref: "a-1",
      node_ids: ["00000000-0000-0000-0000-000000000021"],
      due_at: null,
      created_by: null,
      created_at: "2026-06-01T10:00:00Z",
      updated_at: "2026-06-01T10:00:00Z",
      closed_at: null,
    };
    const items: WorkItem[] = [
      { ...sampleBase, id: "1", item_no: 1, code: "WI-1", status: "OPEN" },
      { ...sampleBase, id: "2", item_no: 2, code: "", status: "IN_PROGRESS" },
      { ...sampleBase, id: "3", item_no: 3, code: "WI-3", status: "DONE" },
    ];
    const grouped = groupWorkItemsByStatus(items);
    assert.equal(grouped.OPEN.length, 1);
    assert.equal(grouped.IN_PROGRESS.length, 1);
    assert.equal(grouped.BLOCKED.length, 0);
    assert.equal(grouped.DONE.length, 1);
    assert.equal(formatWorkItemCode(items[1]!), "WI-2");
  });

  it("readLoggedOutFlag & writeLoggedOutFlag are safe in Node without browser sessionStorage", () => {
    writeLoggedOutFlag(true);
    assert.equal(typeof readLoggedOutFlag(), "boolean");
    writeLoggedOutFlag(false);
  });
});
