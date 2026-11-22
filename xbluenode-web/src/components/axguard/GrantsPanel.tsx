import { type FormEvent, useEffect, useMemo, useState } from "react";
import {
  ArrowUpRight,
  Ban,
  FileKey2,
  Plus,
  RefreshCw,
} from "lucide-react";
import { Link } from "react-router";
import { ApiError } from "@/lib/api";
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import {
  axguardApi,
  type DepartmentRecord,
  type GrantRecord,
  type GrantResourceKind,
  type GrantStatus,
  type GranteeKind,
  type GuardRoleRecord,
  type GuardUserRecord,
} from "@/lib/api/axguard";
import {
  Badge,
  Button,
  Card,
  Input,
  Modal,
  Select,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import {
  GRANTEE_KINDS,
  GRANT_RESOURCE_KINDS,
  GRANT_STATUS_LABELS,
} from "./constants";

interface GrantsPanelProps {
  grants: GrantRecord[];
  datasets: Dataset[];
  departments: DepartmentRecord[];
  roles: GuardRoleRecord[];
  users: GuardUserRecord[];
  onReload: () => Promise<void>;
}

const GRANTABLE_ACTIONS = [
  "DATA_READ",
  "EXPORT_DATA",
  "SIM_RUN",
  "AGENT_RUN",
] as const;

export function GrantsPanel({
  grants,
  datasets,
  departments,
  roles,
  users,
  onReload,
}: GrantsPanelProps) {
  const { isZh, tx, tEnum } = useI18n();
  const { me, can, canAny } = usePrincipal();

  const canAdminRevoke = canAny(["GUARD_ADMIN", "DATA_ADMIN"]);
  const canListUsers = can("GUARD_ADMIN");

  const [fallbackDatasets, setFallbackDatasets] = useState<Dataset[]>([]);
  const [fallbackUsers, setFallbackUsers] = useState<GuardUserRecord[]>([]);

  useEffect(() => {
    if (datasets.length > 0) return;
    let cancelled = false;
    void axhubApi
      .listDatasets({ limit: 100 })
      .then((res) => {
        if (!cancelled && Array.isArray(res?.items)) {
          setFallbackDatasets(res.items);
        }
      })
      .catch(() => null);
    return () => {
      cancelled = true;
    };
  }, [datasets.length]);

  useEffect(() => {
    if (users.length > 0 || !canListUsers) return;
    let cancelled = false;
    void axguardApi
      .listUsers()
      .then((list) => {
        if (!cancelled) setFallbackUsers(list);
      })
      .catch(() => null);
    return () => {
      cancelled = true;
    };
  }, [canListUsers, users.length]);

  const effectiveDatasets = datasets.length > 0 ? datasets : fallbackDatasets;
  const effectiveUsers = users.length > 0 ? users : fallbackUsers;

  const datasetById = useMemo(() => {
    const map = new Map<string, Dataset>();
    for (const d of effectiveDatasets) {
      map.set(d.id, d);
    }
    return map;
  }, [effectiveDatasets]);

  const userById = useMemo(() => {
    const map = new Map<string, GuardUserRecord>();
    for (const u of effectiveUsers) {
      map.set(u.id, u);
    }
    return map;
  }, [effectiveUsers]);

  const deptById = useMemo(() => {
    const map = new Map<string, DepartmentRecord>();
    for (const d of departments) {
      map.set(d.dept_id, d);
    }
    return map;
  }, [departments]);

  const [statusFilter, setStatusFilter] = useState<string>("");
  const [modalOpen, setModalOpen] = useState(false);
  const [resourceKind, setResourceKind] =
    useState<GrantResourceKind>("DATASET");
  const [resourceRef, setResourceRef] = useState("");
  const [granteeKind, setGranteeKind] = useState<GranteeKind>("DEPT");
  const [granteeRef, setGranteeRef] = useState("south");
  const [actions, setActions] = useState<string[]>(["DATA_READ"]);
  const [rowPredicate, setRowPredicate] = useState("");
  const [reason, setReason] = useState("");
  const [expiresInDays, setExpiresInDays] = useState("7");
  const [submitting, setSubmitting] = useState(false);
  const [banner, setBanner] = useState<string | null>(null);

  const filteredGrants = useMemo(() => {
    if (!statusFilter) {
      return grants;
    }
    return grants.filter((g) => g.status === statusFilter);
  }, [grants, statusFilter]);

  const formatGrantStatusBadge = (st: GrantStatus): string => {
    if (isZh) {
      return `${st} · ${GRANT_STATUS_LABELS[st]}`;
    }
    return tEnum(st);
  };

  const canRevokeGrant = (g: GrantRecord): boolean => {
    if (g.status !== "ACTIVE" && g.status !== "PENDING") {
      return false;
    }
    if (canAdminRevoke) {
      return true;
    }
    return (
      g.status === "PENDING" &&
      Boolean(me?.user_id && g.requested_by === me.user_id)
    );
  };

  const handleCreateGrant = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const days = Math.min(365, Math.max(1, Number(expiresInDays) || 7));
    if (!resourceRef.trim() || !granteeRef.trim() || !reason.trim()) {
      setBanner(
        tx(
          "目标资源、被授权主体与业务事由不能为空",
          "Target resource, grantee, and business justification are required"
        )
      );
      return;
    }

    setSubmitting(true);
    setBanner(null);
    try {
      const created = await axguardApi.createGrant({
        resource_kind: resourceKind,
        resource_ref: resourceRef.trim(),
        grantee_kind: granteeKind,
        grantee_ref: granteeRef.trim(),
        actions: actions.length > 0 ? actions : ["DATA_READ"],
        row_predicate: rowPredicate.trim() || undefined,
        reason: reason.trim(),
        expires_in_days: days,
      });
      setModalOpen(false);
      setReason("");
      setRowPredicate("");
      await onReload();
      setBanner(
        tx(
          `已提交跨部门数据授权申请（${GRANT_STATUS_LABELS[created.status]}）`,
          `Submitted cross-department grant request (${tEnum(created.status)})`
        )
      );
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setSubmitting(false);
    }
  };

  const handleRevoke = async (grant: GrantRecord) => {
    if (!canRevokeGrant(grant)) return;
    setBanner(null);
    try {
      await axguardApi.revokeGrant(grant.id);
      await onReload();
      setBanner(
        tx(
          `已撤销授权单 ${grant.id.slice(0, 8)}`,
          `Revoked grant ${grant.id.slice(0, 8)}`
        )
      );
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    }
  };

  const renderResourceCell = (g: GrantRecord) => {
    if (g.resource_kind === "DATASET") {
      const ds = datasetById.get(g.resource_ref);
      if (ds) {
        return (
          <div className="mt-1 space-y-0.5" title={g.resource_ref}>
            <div className="font-medium text-foreground">
              <span className="font-mono-tech font-semibold">{ds.name}</span>
              {ds.display_name && ds.display_name !== ds.name ? (
                <span className="text-muted-foreground ml-1">
                  ({ds.display_name})
                </span>
              ) : null}
            </div>
            <div className="text-[10px] font-mono-tech text-muted-foreground">
              ID: {g.resource_ref.slice(0, 8)}…
            </div>
          </div>
        );
      }
    }
    const isUuid = /^[0-9a-f]{8}-[0-9a-f]{4}-/i.test(g.resource_ref);
    return (
      <div
        className="font-mono-tech mt-1 break-all"
        title={g.resource_ref}
      >
        {isUuid ? `${g.resource_ref.slice(0, 8)}…` : g.resource_ref}
      </div>
    );
  };

  const renderGranteeCell = (g: GrantRecord) => {
    if (g.grantee_kind === "USER") {
      const u = userById.get(g.grantee_ref);
      if (u) {
        return (
          <div className="mt-1" title={g.grantee_ref}>
            <span className="font-mono-tech font-semibold">{u.username}</span>
            {u.display_name && u.display_name !== u.username ? (
              <span className="text-muted-foreground ml-1">
                ({u.display_name})
              </span>
            ) : null}
          </div>
        );
      }
    }
    if (g.grantee_kind === "DEPT") {
      const dept = deptById.get(g.grantee_ref);
      if (dept) {
        return (
          <div className="mt-1">
            <span className="font-mono-tech font-semibold">{dept.dept_id}</span>
            <span className="text-muted-foreground ml-1">({dept.name})</span>
          </div>
        );
      }
    }
    if (g.grantee_kind === "ROLE") {
      return (
        <div className="mt-1" title={g.grantee_ref}>
          <span className="font-medium">{tEnum(g.grantee_ref)}</span>
        </div>
      );
    }
    const isUuid = /^[0-9a-f]{8}-[0-9a-f]{4}-/i.test(g.grantee_ref);
    return (
      <div className="font-mono-tech mt-1" title={g.grantee_ref}>
        {isUuid ? `${g.grantee_ref.slice(0, 8)}…` : g.grantee_ref}
      </div>
    );
  };

  return (
    <div className="space-y-4" data-testid="axguard-grants-panel">
      {banner && (
        <div
          role="status"
          className="rounded-md border border-border bg-muted/50 px-3 py-2 text-xs"
        >
          {banner}
        </div>
      )}

      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Select
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
            className="h-8"
          >
            <option value="">
              {tx(`全部状态 (${grants.length})`, `All Statuses (${grants.length})`)}
            </option>
            {(
              [
                "PENDING",
                "ACTIVE",
                "REJECTED",
                "REVOKED",
                "EXPIRED",
              ] as GrantStatus[]
            ).map((st) => (
              <option key={st} value={st}>
                {formatGrantStatusBadge(st)}
              </option>
            ))}
          </Select>

          <Link
            to="/flow"
            className="inline-flex items-center gap-1 rounded-md border border-border bg-background hover:bg-muted px-2.5 py-1.5 text-xs text-muted-foreground hover:text-foreground"
          >
            {tx("审批收件箱", "Approval Inbox")}
            <ArrowUpRight className="h-3.5 w-3.5 text-xblue" />
          </Link>
        </div>

        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            type="button"
            onClick={() => void onReload()}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {tx("刷新", "Refresh")}
          </Button>
          <Button
            type="button"
            data-testid="open-create-grant-btn"
            onClick={() => {
              if (!resourceRef && effectiveDatasets[0]) {
                setResourceRef(effectiveDatasets[0].id);
              }
              setModalOpen(true);
            }}
          >
            <Plus className="h-3.5 w-3.5" />
            {tx("发起授权申请", "New Grant Request")}
          </Button>
        </div>
      </div>

      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-xs border-collapse">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-left text-muted-foreground">
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("受管资源", "Resource")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("被授权主体", "Grantee")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("授权动作与行限制", "Actions & Row Filter")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("申请事由", "Justification")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("状态 / 有效期", "Status / Expiry")}
                </th>
                <th className="px-3 py-2.5 font-medium text-right whitespace-nowrap">
                  {tx("操作", "Actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {filteredGrants.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="px-4 py-8 text-center text-muted-foreground"
                  >
                    {tx("暂无跨部门授权记录", "No cross-department grants")}
                  </td>
                </tr>
              ) : (
                filteredGrants.map((g) => (
                  <tr key={g.id} className="hover:bg-muted/30">
                    <td className="px-3 py-2.5">
                      <Badge variant="outline">{tEnum(g.resource_kind)}</Badge>
                      {renderResourceCell(g)}
                    </td>
                    <td className="px-3 py-2.5">
                      <Badge variant="neutral">{tEnum(g.grantee_kind)}</Badge>
                      {renderGranteeCell(g)}
                    </td>
                    <td className="px-3 py-2.5 space-y-1">
                      <div className="flex flex-wrap gap-1">
                        {g.actions.map((act) => (
                          <Badge key={act} variant="xblue" title={act}>
                            {tEnum(act)}
                          </Badge>
                        ))}
                      </div>
                      {g.row_predicate && (
                        <div className="font-mono-tech text-[11px] text-muted-foreground">
                          {g.row_predicate}
                        </div>
                      )}
                    </td>
                    <td className="px-3 py-2.5 max-w-xs">
                      <div className="line-clamp-2">{g.reason}</div>
                    </td>
                    <td className="px-3 py-2.5 whitespace-nowrap">
                      <Badge
                        variant={
                          g.status === "ACTIVE"
                            ? "xblue"
                            : g.status === "PENDING"
                              ? "neutral"
                              : "outline"
                        }
                      >
                        {formatGrantStatusBadge(g.status)}
                      </Badge>
                      <div className="text-[11px] font-mono-tech text-muted-foreground mt-1">
                        {tx("到期", "Expires")}: {g.expires_at.slice(0, 10)}
                      </div>
                    </td>
                    <td className="px-3 py-2.5 text-right whitespace-nowrap">
                      <div className="inline-flex items-center gap-1.5">
                        {g.status === "PENDING" && (
                          <Link
                            to="/flow"
                            className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-[11px] hover:bg-muted"
                          >
                            {tx("审批单", "Approval")}
                            <ArrowUpRight className="h-3 w-3 text-xblue" />
                          </Link>
                        )}
                        {canRevokeGrant(g) && (
                          <Button
                            variant="outline"
                            size="sm"
                            type="button"
                            onClick={() => void handleRevoke(g)}
                            data-testid={`revoke-grant-${g.id}`}
                          >
                            <Ban className="h-3 w-3" />
                            {canAdminRevoke
                              ? tx("撤销", "Revoke")
                              : tx("撤回", "Cancel")}
                          </Button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      </Card>

      {/* Create Cross-Department Grant Modal */}
      <Modal
        open={modalOpen}
        onClose={() => setModalOpen(false)}
        maxWidthClass="max-w-lg"
        testId="create-grant-modal"
        title={
          <>
            <FileKey2 className="h-4 w-4 text-xblue shrink-0" />
            <span>
              {tx("发起跨部门数据授权申请", "New Cross-Department Grant")}
            </span>
          </>
        }
      >
        <form
          onSubmit={(e) => void handleCreateGrant(e)}
          className="space-y-3"
        >
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("资源类别", "Resource Kind")}
              </label>
              <Select
                value={resourceKind}
                onChange={(e) =>
                  setResourceKind(e.target.value as GrantResourceKind)
                }
                className="h-8 w-full"
              >
                {GRANT_RESOURCE_KINDS.map((k) => (
                  <option key={k.value} value={k.value}>
                    {isZh ? k.label : k.enLabel}
                  </option>
                ))}
              </Select>
            </div>

            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("目标资源标识", "Target Resource")} *
              </label>
              {resourceKind === "DATASET" && effectiveDatasets.length > 0 ? (
                <Select
                  required
                  value={resourceRef}
                  onChange={(e) => setResourceRef(e.target.value)}
                  data-testid="grant-resource-ref-input"
                  className="h-8 w-full"
                >
                  {effectiveDatasets.map((d) => (
                    <option key={d.id} value={d.id}>
                      {d.name}
                      {d.display_name && d.display_name !== d.name
                        ? ` (${d.display_name})`
                        : ""}
                    </option>
                  ))}
                </Select>
              ) : (
                <Input
                  required
                  placeholder="demo_orders"
                  value={resourceRef}
                  onChange={(e) => setResourceRef(e.target.value)}
                  data-testid="grant-resource-ref-input"
                  className="h-8 text-xs font-mono-tech"
                />
              )}
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("被授权主体类别", "Grantee Kind")}
              </label>
              <Select
                value={granteeKind}
                onChange={(e) =>
                  setGranteeKind(e.target.value as GranteeKind)
                }
                className="h-8 w-full"
              >
                {GRANTEE_KINDS.map((k) => (
                  <option key={k.value} value={k.value}>
                    {isZh ? k.label : k.enLabel}
                  </option>
                ))}
              </Select>
            </div>

            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("被授权主体标识", "Grantee")} *
              </label>
              <Input
                required
                list="grant-grantee-datalist"
                placeholder="south"
                value={granteeRef}
                onChange={(e) => setGranteeRef(e.target.value)}
                data-testid="grant-grantee-ref-input"
                className="h-8 text-xs font-mono-tech"
              />
              <datalist id="grant-grantee-datalist">
                {granteeKind === "DEPT" &&
                  departments.map((d) => (
                    <option key={d.dept_id} value={d.dept_id}>
                      {d.name}
                    </option>
                  ))}
                {granteeKind === "ROLE" &&
                  roles.map((r) => <option key={r.name} value={r.name} />)}
                {granteeKind === "USER" &&
                  effectiveUsers.map((u) => (
                    <option key={u.id} value={u.id}>
                      {u.username}
                    </option>
                  ))}
              </datalist>
            </div>
          </div>

          <div className="grid grid-cols-12 gap-3">
            <div className="col-span-8 space-y-1">
              <label className="text-xs font-medium">
                {tx("申请动作", "Permitted Actions")}
              </label>
              <div className="flex flex-wrap gap-1.5">
                {GRANTABLE_ACTIONS.map((act) => {
                  const checked = actions.includes(act);
                  return (
                    <label
                      key={act}
                      title={act}
                      className={cn(
                        "inline-flex items-center gap-1 rounded px-2 py-1 text-[11px] border cursor-pointer",
                        checked
                          ? "border-xblue bg-xblue-soft text-xblue"
                          : "border-border text-muted-foreground"
                      )}
                    >
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(e) =>
                          setActions((prev) =>
                            e.target.checked
                              ? [...prev, act]
                              : prev.filter((x) => x !== act)
                          )
                        }
                        className="accent-[#2563eb]"
                      />
                      {tEnum(act)}
                    </label>
                  );
                })}
              </div>
            </div>

            <div className="col-span-4 space-y-1">
              <label className="text-xs font-medium">
                {tx("有效天数", "Validity (days)")}
              </label>
              <Input
                type="number"
                min={1}
                max={365}
                value={expiresInDays}
                onChange={(e) => setExpiresInDays(e.target.value)}
                data-testid="grant-expires-days-input"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
          </div>

          <div className="space-y-1">
            <label className="text-xs font-medium">
              {tx("附加行过滤条件", "Row Predicate")}
            </label>
            <Input
              placeholder="warehouse_code = 'W-2'"
              value={rowPredicate}
              onChange={(e) => setRowPredicate(e.target.value)}
              data-testid="grant-row-predicate-input"
              className="h-8 text-xs font-mono-tech"
            />
          </div>

          <div className="space-y-1">
            <label className="text-xs font-medium">
              {tx("业务事由", "Justification")} *
            </label>
            <Textarea
              required
              rows={2}
              placeholder={tx(
                "华南协同仓调拨核对履约订单",
                "Cross-region fulfillment reconciliation"
              )}
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              data-testid="grant-reason-input"
              className="text-xs"
            />
          </div>

          <div className="flex justify-end gap-2 pt-2 border-t border-border">
            <Button
              variant="outline"
              type="button"
              onClick={() => setModalOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="submit"
              disabled={
                submitting ||
                !resourceRef.trim() ||
                !granteeRef.trim() ||
                !reason.trim()
              }
              data-testid="submit-create-grant-btn"
            >
              {submitting
                ? tx("提交中…", "Submitting…")
                : tx("提交申请", "Submit")}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}
