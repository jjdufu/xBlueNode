import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  computeProactiveRefreshDelayMs,
  decodeJwtExp,
} from "../../lib/api/auth.ts";
import {
  buildDepartmentTree,
  collectDescendantDeptIds,
  isAccountLocked,
  resolveDefaultAxGuardTab,
  resolveEffectiveMaskRule,
  validateLocalPassword,
} from "./logic.ts";

function makeUnsignedJwt(payload: Record<string, unknown>): string {
  const encodeSeg = (obj: Record<string, unknown>): string =>
    btoa(JSON.stringify(obj))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/g, "");
  return `${encodeSeg({ alg: "EdDSA", typ: "JWT" })}.${encodeSeg(payload)}.sig_stub`;
}

describe("AxGuard auth & governance logic", () => {
  it("decodeJwtExp extracts exp claim safely and rejects PAT/malformed tokens", () => {
    const jwt = makeUnsignedJwt({ sub: "u1", exp: 1_900_000_900 });
    assert.equal(decodeJwtExp(jwt), 1_900_000_900);
    assert.equal(decodeJwtExp("axp_12345678_0123456789abcdef0123456789abcdef"), null);
    assert.equal(decodeJwtExp("invalid.token.payload"), null);
  });

  it("computeProactiveRefreshDelayMs schedules 60s before expiry with 5s floor", () => {
    const nowMs = 1_700_000_000_000;
    const expNineHundredSec = 1_700_000_900;
    assert.equal(
      computeProactiveRefreshDelayMs(expNineHundredSec, nowMs),
      840_000
    );
    const expSoon = 1_700_000_030;
    assert.equal(computeProactiveRefreshDelayMs(expSoon, nowMs), 5_000);
  });

  it("buildDepartmentTree constructs hierarchy and collectDescendantDeptIds prevents cycles", () => {
    const depts = [
      { tenant_id: "default", dept_id: "east_wh", name: "华东仓储", parent_id: "east" },
      { tenant_id: "default", dept_id: "default", name: "默认部门", parent_id: null },
      { tenant_id: "default", dept_id: "east", name: "华东大区", parent_id: "default" },
      { tenant_id: "default", dept_id: "south", name: "华南大区", parent_id: "default" },
    ];
    const tree = buildDepartmentTree(depts);
    assert.equal(tree.length, 1);
    assert.equal(tree[0]?.record.dept_id, "default");
    assert.equal(tree[0]?.children.length, 2);
    assert.equal(tree[0]?.children[0]?.record.dept_id, "east");
    assert.equal(tree[0]?.children[0]?.children[0]?.record.dept_id, "east_wh");

    const excluded = collectDescendantDeptIds(depts, "east");
    assert.deepEqual(Array.from(excluded).sort(), ["east", "east_wh"]);
  });

  it("validateLocalPassword enforces >= 12 chars and forbids username substring", () => {
    assert.equal(
      validateLocalPassword("short123", "ops_admin"),
      "口令长度必须不少于 12 个字符"
    );
    assert.equal(
      validateLocalPassword("My_ops_admin_2026!", "ops_admin"),
      "口令中不得包含用户名"
    );
    assert.equal(
      validateLocalPassword("Strong#Passphrase2026!", "ops_admin"),
      null
    );
  });

  it("resolveEffectiveMaskRule applies profile defaults unless overridden by explicit rule", () => {
    assert.deepEqual(resolveEffectiveMaskRule("PHONE", null), {
      effectiveRule: "MASK_PHONE",
      source: "PROFILE_DEFAULT",
    });
    assert.deepEqual(resolveEffectiveMaskRule("PHONE", "NONE"), {
      effectiveRule: "NONE",
      source: "EXPLICIT",
    });
    assert.deepEqual(resolveEffectiveMaskRule("SECRET", undefined), {
      effectiveRule: "REDACT",
      source: "PROFILE_DEFAULT",
    });
  });

  it("resolveDefaultAxGuardTab routes AUDIT_READ-only users directly to audit tab", () => {
    assert.equal(resolveDefaultAxGuardTab(["AUDIT_READ"], ["VIEWER"]), "audit");
    assert.equal(
      resolveDefaultAxGuardTab(["GUARD_ADMIN", "AUDIT_READ"], ["OPERATOR"]),
      "users"
    );
    assert.equal(resolveDefaultAxGuardTab(["DATA_READ"], ["ANALYST"]), "grants");
  });

  it("isAccountLocked checks future timestamp accurately", () => {
    const now = Date.parse("2026-06-01T10:00:00Z");
    assert.equal(isAccountLocked("2026-06-01T10:15:00Z", now), true);
    assert.equal(isAccountLocked("2026-06-01T09:59:59Z", now), false);
    assert.equal(isAccountLocked(null, now), false);
  });
});
