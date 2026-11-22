import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type {
  DeepHealthReport,
  OnboardingStatusResponse,
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
  findFirstIncompleteStepKey,
  formatByteSize,
  formatDurationSeconds,
  formatOnboardingStepMetric,
  formatWorkItemCode,
  getAllowedWorkItemTransitions,
  groupWorkItemsByStatus,
  hasOpsAdminAccess,
  isOnboardingComplete,
  mergeRetentionPoliciesWithDefaults,
  resolveAxOpsInitialTab,
  resolveDefaultAxOpsTab,
  resolveOnboardingNextTarget,
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

  it("mergeRetentionPoliciesWithDefaults returns all 12 targets and marks AUDIT as owner-script only", () => {
    const merged = mergeRetentionPoliciesWithDefaults([
      { target: "JOB", keep_days: 45 },
      { target: "AUDIT", keep_days: 730 },
    ]);
    assert.equal(merged.length, 12);
    const jobEntry = merged.find((m) => m.target === "JOB");
    const auditEntry = merged.find((m) => m.target === "AUDIT");
    const imDedupEntry = merged.find((m) => m.target === "IM_EVENT_DEDUP");
    assert.equal(jobEntry?.keep_days, 45);
    assert.equal(jobEntry?.ownerScriptOnly, false);
    assert.equal(auditEntry?.keep_days, 730);
    assert.equal(auditEntry?.ownerScriptOnly, true);
    assert.equal(imDedupEntry?.keep_days, 7);
    assert.equal(imDedupEntry?.ownerScriptOnly, false);
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

  it("resolveAxOpsInitialTab honors ?tab=templates for DATA_READ analysts while guarding admin tabs", () => {
    assert.equal(
      resolveAxOpsInitialTab("templates", ["DATA_READ"], ["ANALYST"]),
      "templates"
    );
    assert.equal(
      resolveAxOpsInitialTab("backups", ["DATA_READ"], ["ANALYST"]),
      "work_items"
    );
    assert.equal(
      resolveAxOpsInitialTab("backups", ["OPS_ADMIN"], ["OPERATOR"]),
      "backups"
    );
    assert.equal(
      resolveAxOpsInitialTab(null, ["DATA_READ"], ["ANALYST"]),
      "work_items"
    );
  });

  it("onboarding progress & next-step helpers resolve recommended route, first incomplete step, and 5/5 ready state", () => {
    const partialStatus: OnboardingStatusResponse = {
      completed_steps: 2,
      total_steps: 5,
      progress_pct: 40,
      recommended_next_step: "MAPPING_AND_FLOW",
      has_trial_subgraph: true,
      steps: [
        {
          step_no: 1,
          step_key: "DATA_INGEST",
          label_zh: "1. 数据接入",
          label_en: "1. Data Ingest",
          completed: true,
          count_primary: 3,
          count_secondary: 2,
          metric_label: "3 数据集 · 2 文档",
          target_path: "/hub",
        },
        {
          step_no: 2,
          step_key: "NODE_MODELING",
          label_zh: "2. 节点建模",
          label_en: "2. Node Modeling",
          completed: true,
          count_primary: 4,
          count_secondary: 3,
          metric_label: "",
          target_path: "/studio",
        },
        {
          step_no: 3,
          step_key: "MAPPING_AND_FLOW",
          label_zh: "3. 映射与编排",
          label_en: "3. Mapping & Flow",
          completed: false,
          count_primary: 0,
          count_secondary: 0,
          metric_label: "0 业务节点 · 0 流程",
          target_path: "/flow",
        },
        {
          step_no: 4,
          step_key: "SIM_VERIFICATION",
          label_zh: "4. 推演验证",
          label_en: "4. Simulation",
          completed: false,
          count_primary: 0,
          count_secondary: 0,
          metric_label: "0 推演方案",
          target_path: "/sim",
        },
        {
          step_no: 5,
          step_key: "COCKPIT_AND_AGENT",
          label_zh: "5. 驾驶舱与智能体",
          label_en: "5. Apps & Agents",
          completed: false,
          count_primary: 0,
          count_secondary: 0,
          metric_label: "0 驾驶舱 · 0 智能体",
          target_path: "/view",
        },
      ],
    };

    assert.equal(isOnboardingComplete(partialStatus), false);
    assert.equal(findFirstIncompleteStepKey(partialStatus), "MAPPING_AND_FLOW");
    assert.equal(resolveOnboardingNextTarget(partialStatus), "/flow");
    assert.equal(
      formatOnboardingStepMetric(partialStatus.steps[0]!, true),
      "3 数据集 · 2 文档"
    );
    assert.equal(
      formatOnboardingStepMetric(partialStatus.steps[0]!, false),
      "3 Datasets · 2 Docs"
    );
    assert.equal(
      formatOnboardingStepMetric(partialStatus.steps[1]!, true),
      "4 节点类型 · 3 关系"
    );

    const completeStatus: OnboardingStatusResponse = {
      ...partialStatus,
      completed_steps: 5,
      progress_pct: 100,
      recommended_next_step: null,
      steps: partialStatus.steps.map((s) => ({ ...s, completed: true })),
    };
    assert.equal(isOnboardingComplete(completeStatus), true);
    assert.equal(findFirstIncompleteStepKey(completeStatus), null);
    assert.equal(resolveOnboardingNextTarget(completeStatus), "/view");
    assert.equal(resolveOnboardingNextTarget(null), "/hub");
  });
});
