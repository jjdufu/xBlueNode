import { type FormEvent, useMemo, useState } from "react";
import {
  ArrowUpRight,
  Ban,
  FileKey2,
  Plus,
  RefreshCw,
  X,
} from "lucide-react";
import { Link } from "react-router";
import { ApiError } from "@/lib/api";
import type { Dataset } from "@/lib/api/axhub";
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
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";
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

  const handleCreateGrant = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const days = Math.min(365, Math.max(1, Number(expiresInDays) || 7));
    if (!resourceRef.trim() || !granteeRef.trim() || !reason.trim()) {
      setBanner("请填写目标资源、被授权主体与业务申请事由");
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
        `已提交跨部门数据授权申请（状态: ${GRANT_STATUS_LABELS[created.status]}），已同步创建 AxFlow 审批单`
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
    setBanner(null);
    try {
      await axguardApi.revokeGrant(grant.id);
      await onReload();
      setBanner(`已撤销授权单 ${grant.id.slice(0, 8)}...`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    }
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
          <select
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
            className="h-8 rounded-md border border-input bg-background px-2.5 text-xs"
          >
            <option value="">全部状态 ({grants.length})</option>
            {(
              ["PENDING", "ACTIVE", "REJECTED", "REVOKED", "EXPIRED"] as GrantStatus[]
            ).map((st) => (
              <option key={st} value={st}>
                {st} · {GRANT_STATUS_LABELS[st]}
              </option>
            ))}
          </select>

          <Link
            to="/flow"
            className="inline-flex items-center gap-1 rounded-md border border-border bg-background hover:bg-muted px-2.5 py-1.5 text-xs text-muted-foreground hover:text-foreground"
          >
            前往 AxFlow 审批收件箱
            <ArrowUpRight className="h-3.5 w-3.5 text-xblue" />
          </Link>
        </div>

        <div className="flex items-center gap-2">
          <Button variant="outline" type="button" onClick={() => void onReload()}>
            <RefreshCw className="h-3.5 w-3.5" />
            刷新
          </Button>
          <Button
            type="button"
            data-testid="open-create-grant-btn"
            onClick={() => {
              if (!resourceRef && datasets[0]) {
                setResourceRef(datasets[0].id);
              }
              setModalOpen(true);
            }}
          >
            <Plus className="h-3.5 w-3.5" />
            发起跨部门数据申请
          </Button>
        </div>
      </div>

      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-xs border-collapse">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-left text-muted-foreground">
                <th className="px-3 py-2.5 font-medium">目标受管资源</th>
                <th className="px-3 py-2.5 font-medium">被授权主体</th>
                <th className="px-3 py-2.5 font-medium">授权动作与附加行条件</th>
                <th className="px-3 py-2.5 font-medium">业务事由</th>
                <th className="px-3 py-2.5 font-medium">状态 / 有效期</th>
                <th className="px-3 py-2.5 font-medium text-right">治理操作</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {filteredGrants.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="px-4 py-8 text-center text-muted-foreground"
                  >
                    暂无跨部门数据授权申请记录
                  </td>
                </tr>
              ) : (
                filteredGrants.map((g) => (
                  <tr key={g.id} className="hover:bg-muted/30">
                    <td className="px-3 py-2.5">
                      <Badge variant="outline">{g.resource_kind}</Badge>
                      <div className="font-mono-tech mt-1 break-all">
                        {g.resource_ref}
                      </div>
                    </td>
                    <td className="px-3 py-2.5">
                      <Badge variant="neutral">{g.grantee_kind}</Badge>
                      <div className="font-mono-tech mt-1">{g.grantee_ref}</div>
                    </td>
                    <td className="px-3 py-2.5 space-y-1">
                      <div className="flex flex-wrap gap-1">
                        {g.actions.map((act) => (
                          <Badge key={act} variant="xblue">
                            {act}
                          </Badge>
                        ))}
                      </div>
                      {g.row_predicate && (
                        <div className="font-mono-tech text-[11px] text-muted-foreground">
                          行限制: {g.row_predicate}
                        </div>
                      )}
                    </td>
                    <td className="px-3 py-2.5 max-w-xs">
                      <div className="line-clamp-2">{g.reason}</div>
                    </td>
                    <td className="px-3 py-2.5">
                      <Badge
                        variant={
                          g.status === "ACTIVE"
                            ? "xblue"
                            : g.status === "PENDING"
                              ? "neutral"
                              : "outline"
                        }
                      >
                        {g.status} · {GRANT_STATUS_LABELS[g.status]}
                      </Badge>
                      <div className="text-[11px] font-mono-tech text-muted-foreground mt-1">
                        到期: {g.expires_at.slice(0, 10)}
                      </div>
                    </td>
                    <td className="px-3 py-2.5 text-right">
                      <div className="inline-flex items-center gap-1.5">
                        {g.status === "PENDING" && (
                          <Link
                            to="/flow"
                            className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-[11px] hover:bg-muted"
                          >
                            审批单
                            <ArrowUpRight className="h-3 w-3 text-xblue" />
                          </Link>
                        )}
                        {(g.status === "ACTIVE" || g.status === "PENDING") && (
                          <Button
                            variant="outline"
                            type="button"
                            onClick={() => void handleRevoke(g)}
                            data-testid={`revoke-grant-${g.id}`}
                            className="px-2 py-1 text-[11px]"
                          >
                            <Ban className="h-3 w-3" />
                            撤销
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
      {modalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
          <Card
            className="w-full max-w-lg p-5 space-y-4"
            data-testid="create-grant-modal"
          >
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="text-sm font-semibold flex items-center gap-2">
                <FileKey2 className="h-4 w-4 text-xblue" />
                发起跨部门数据授权申请（sec_grants）
              </div>
              <button
                type="button"
                onClick={() => setModalOpen(false)}
                className="text-muted-foreground hover:text-foreground cursor-pointer"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <form
              onSubmit={(e) => void handleCreateGrant(e)}
              className="space-y-3"
            >
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="text-xs font-medium">目标资源类别</label>
                  <select
                    value={resourceKind}
                    onChange={(e) =>
                      setResourceKind(e.target.value as GrantResourceKind)
                    }
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    {GRANT_RESOURCE_KINDS.map((k) => (
                      <option key={k.value} value={k.value}>
                        {k.label}
                      </option>
                    ))}
                  </select>
                </div>

                <div className="space-y-1">
                  <label className="text-xs font-medium">
                    目标资源标识 (resource_ref) *
                  </label>
                  <Input
                    required
                    list="grant-resource-datalist"
                    placeholder="数据集 UUID / 节点类型名"
                    value={resourceRef}
                    onChange={(e) => setResourceRef(e.target.value)}
                    data-testid="grant-resource-ref-input"
                    className="h-8 text-xs font-mono-tech"
                  />
                  <datalist id="grant-resource-datalist">
                    {datasets.map((d) => (
                      <option key={d.id} value={d.id}>
                        {d.display_name || d.name}
                      </option>
                    ))}
                  </datalist>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="text-xs font-medium">被授权主体类别</label>
                  <select
                    value={granteeKind}
                    onChange={(e) =>
                      setGranteeKind(e.target.value as GranteeKind)
                    }
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    {GRANTEE_KINDS.map((k) => (
                      <option key={k.value} value={k.value}>
                        {k.label}
                      </option>
                    ))}
                  </select>
                </div>

                <div className="space-y-1">
                  <label className="text-xs font-medium">
                    被授权主体标识 (grantee_ref) *
                  </label>
                  <Input
                    required
                    list="grant-grantee-datalist"
                    placeholder="如 south / ANALYST / 用户UUID"
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
                      users.map((u) => (
                        <option key={u.id} value={u.id}>
                          {u.username}
                        </option>
                      ))}
                  </datalist>
                </div>
              </div>

              <div className="grid grid-cols-12 gap-3">
                <div className="col-span-8 space-y-1">
                  <label className="text-xs font-medium">申请动作集合</label>
                  <div className="flex flex-wrap gap-1.5">
                    {GRANTABLE_ACTIONS.map((act) => {
                      const checked = actions.includes(act);
                      return (
                        <label
                          key={act}
                          className={cn(
                            "inline-flex items-center gap-1 rounded px-2 py-1 text-[11px] font-mono-tech border cursor-pointer",
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
                          {act}
                        </label>
                      );
                    })}
                  </div>
                </div>

                <div className="col-span-4 space-y-1">
                  <label className="text-xs font-medium">有效天数 (1-365)</label>
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
                  附加行级范围限制（可选，遵循 §13.6.2 语法）
                </label>
                <Input
                  placeholder="如 warehouse_code = 'W-2'"
                  value={rowPredicate}
                  onChange={(e) => setRowPredicate(e.target.value)}
                  data-testid="grant-row-predicate-input"
                  className="h-8 text-xs font-mono-tech"
                />
              </div>

              <div className="space-y-1">
                <label className="text-xs font-medium">业务申请事由 *</label>
                <Textarea
                  required
                  rows={2}
                  placeholder="请说明跨部门调阅机密数据的业务场景与使用范围..."
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
                  取消
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
                  {submitting ? "提交审批中..." : "提交授权申请"}
                </Button>
              </div>
            </form>
          </Card>
        </div>
      )}
    </div>
  );
}
