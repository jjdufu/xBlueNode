import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import {
  CheckCircle2,
  Edit3,
  Link2,
  Plus,
  Power,
  Radio,
  Server,
  Trash2,
  Unlink,
  XCircle,
  Zap,
} from "lucide-react";
import {
  confirmImBindToken,
  createImConnector,
  deleteImBinding,
  deleteImConnector,
  listImBindings,
  listImConnectors,
  patchImConnector,
  previewImBindToken,
  testImConnector,
  type AgentRecord,
  type CreateImConnectorRequest,
  type ImBindChallengePreview,
  type ImConnectorConfig,
  type ImConnectorRecord,
  type ImConnectorSecretInput,
  type ImConnectorTestResponse,
  type ImIdentityPolicy,
  type ImPlatform,
  type ImUserBindingRecord,
  type PatchImConnectorRequest,
} from "@/lib/api/axagent";
import {
  Badge,
  Button,
  Card,
  EmptyState,
  Input,
  Modal,
  Select,
  StatusBadge,
  Switch,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";

const IM_PLATFORMS: readonly ImPlatform[] = ["FEISHU", "DINGTALK", "WECOM"];

const IM_IDENTITY_POLICIES: readonly ImIdentityPolicy[] = [
  "REQUIRE_USER_BIND",
  "CREATOR_DELEGATED",
];

function isImPlatform(val: string): val is ImPlatform {
  return val === "FEISHU" || val === "DINGTALK" || val === "WECOM";
}

function isImIdentityPolicy(val: string): val is ImIdentityPolicy {
  return val === "REQUIRE_USER_BIND" || val === "CREATOR_DELEGATED";
}

function credentialIdPlaceholder(platform: ImPlatform): string {
  switch (platform) {
    case "FEISHU":
      return "cli_a6b8c0d2e4f6";
    case "DINGTALK":
      return "dingoa1b2c3d4";
    case "WECOM":
      return "aib_9f8e7d6c";
  }
}

function buildSecretPayload(
  platform: ImPlatform,
  credId: string,
  credSecret: string
): ImConnectorSecretInput {
  switch (platform) {
    case "FEISHU":
      return { app_id: credId, app_secret: credSecret };
    case "DINGTALK":
      return { client_id: credId, client_secret: credSecret };
    case "WECOM":
      return { bot_id: credId, secret: credSecret };
  }
}

export interface ImConnectorPanelProps {
  agents: AgentRecord[];
  currentUserId?: string | null;
  bindToken?: string | null;
  onClearBindToken?: () => void;
}

export function ImConnectorPanel({
  agents,
  currentUserId = null,
  bindToken = null,
  onClearBindToken,
}: ImConnectorPanelProps) {
  const queryClient = useQueryClient();
  const { tx, tEnum, fmtTime } = useI18n();
  const { me, can } = usePrincipal();

  const canEdit = can("AGENT_EDIT");
  const canGuardAdmin = can("GUARD_ADMIN");
  const effectiveUserId = currentUserId ?? me?.user_id ?? "";

  const connectorsQuery = useQuery({
    queryKey: ["axagent", "im-connectors"],
    queryFn: () => listImConnectors(),
    refetchInterval: 5000,
  });

  const bindingsQuery = useQuery({
    queryKey: ["axagent", "im-bindings"],
    queryFn: () => listImBindings(),
  });

  const connectors = useMemo(
    () => connectorsQuery.data ?? [],
    [connectorsQuery.data]
  );
  const bindings = useMemo(
    () => bindingsQuery.data ?? [],
    [bindingsQuery.data]
  );

  const sortedAgents = useMemo(
    () =>
      [...agents].sort((a, b) => {
        const aPub = a.version > 0 ? 0 : 1;
        const bPub = b.version > 0 ? 0 : 1;
        if (aPub !== bPub) return aPub - bPub;
        return a.name.localeCompare(b.name);
      }),
    [agents]
  );

  const agentNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const ag of agents) {
      map.set(ag.id, ag.name);
    }
    return map;
  }, [agents]);

  // Create / Edit Connector Modal state
  const [modalOpen, setModalOpen] = useState(false);
  const [editingConnector, setEditingConnector] =
    useState<ImConnectorRecord | null>(null);
  const [name, setName] = useState("");
  const [platform, setPlatform] = useState<ImPlatform>("FEISHU");
  const [agentId, setAgentId] = useState("");
  const [identityPolicy, setIdentityPolicy] =
    useState<ImIdentityPolicy>("REQUIRE_USER_BIND");
  const [delegatedUserId, setDelegatedUserId] = useState("");
  const [credentialId, setCredentialId] = useState("");
  const [credentialSecret, setCredentialSecret] = useState("");
  const [endpointOverride, setEndpointOverride] = useState("");
  const [allowPrivateNetwork, setAllowPrivateNetwork] = useState(false);

  const [saving, setSaving] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [panelError, setPanelError] = useState<string | null>(null);
  const [panelFeedback, setPanelFeedback] = useState<string | null>(null);

  // Row actions state
  const [testingId, setTestingId] = useState<string | null>(null);
  const [togglingId, setTogglingId] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<
    Record<string, ImConnectorTestResponse>
  >({});

  // Delete Connector Confirmation Modal state
  const [deletingConnector, setDeletingConnector] =
    useState<ImConnectorRecord | null>(null);
  const [deletingBusy, setDeletingBusy] = useState(false);

  // Unbind state
  const [unbindingId, setUnbindingId] = useState<string | null>(null);

  // Challenge Bind Confirmation Modal state (`?im_bind=axim_...`)
  const activeChallengeToken = useMemo(() => {
    const trimmed = bindToken?.trim() ?? "";
    return trimmed.startsWith("axim_") ? trimmed : null;
  }, [bindToken]);

  const [bindPreview, setBindPreview] = useState<ImBindChallengePreview | null>(
    null
  );
  const [bindPreviewLoading, setBindPreviewLoading] = useState(false);
  const [bindModalError, setBindModalError] = useState<string | null>(null);
  const [confirmingBind, setConfirmingBind] = useState(false);

  useEffect(() => {
    const trimmed = bindToken?.trim() ?? "";
    if (trimmed.length > 0 && !trimmed.startsWith("axim_")) {
      onClearBindToken?.();
    }
  }, [bindToken, onClearBindToken]);

  useEffect(() => {
    if (modalOpen && !editingConnector && !agentId && sortedAgents[0]) {
      setAgentId(sortedAgents[0].id);
    }
  }, [modalOpen, editingConnector, agentId, sortedAgents]);

  useEffect(() => {
    if (!activeChallengeToken) {
      setBindPreview(null);
      setBindPreviewLoading(false);
      setBindModalError(null);
      return;
    }
    let cancelled = false;
    setBindPreview(null);
    setBindModalError(null);
    setBindPreviewLoading(true);

    void previewImBindToken(activeChallengeToken)
      .then((res) => {
        if (!cancelled) {
          setBindPreview(res);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setBindModalError(
            err instanceof Error
              ? err.message
              : tx("挑战码无效或已过期", "Invalid or expired challenge token")
          );
        }
      })
      .finally(() => {
        if (!cancelled) {
          setBindPreviewLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [activeChallengeToken, tx]);

  const refreshImData = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["axagent", "im-connectors"] }),
      queryClient.invalidateQueries({ queryKey: ["axagent", "im-bindings"] }),
    ]);
  }, [queryClient]);

  const openCreateModal = () => {
    if (!canEdit) return;
    setEditingConnector(null);
    setName("");
    setPlatform("FEISHU");
    setAgentId(sortedAgents[0]?.id ?? "");
    setIdentityPolicy("REQUIRE_USER_BIND");
    setDelegatedUserId(effectiveUserId);
    setCredentialId("");
    setCredentialSecret("");
    setEndpointOverride("");
    setAllowPrivateNetwork(false);
    setFormError(null);
    setModalOpen(true);
  };

  const openEditModal = (conn: ImConnectorRecord) => {
    if (!canEdit) return;
    setEditingConnector(conn);
    setName(conn.name);
    setPlatform(conn.platform);
    setAgentId(conn.agent_id);
    setIdentityPolicy(conn.identity_policy);
    setDelegatedUserId(conn.delegated_user_id ?? effectiveUserId);
    setCredentialId("");
    setCredentialSecret("");
    setEndpointOverride(conn.config?.endpoint_override ?? "");
    setAllowPrivateNetwork(Boolean(conn.config?.allow_private_network));
    setFormError(null);
    setModalOpen(true);
  };

  const handleChangePolicy = (nextPolicy: ImIdentityPolicy) => {
    setIdentityPolicy(nextPolicy);
    if (
      nextPolicy === "CREATOR_DELEGATED" &&
      delegatedUserId.trim().length === 0 &&
      effectiveUserId.length > 0
    ) {
      setDelegatedUserId(effectiveUserId);
    }
  };

  const handleSaveConnector = async () => {
    if (!canEdit) return;
    setFormError(null);

    const baseSchema = z.object({
      name: z
        .string()
        .trim()
        .min(1, tx("名称不能为空", "Name is required"))
        .max(64),
      agent_id: z
        .string()
        .trim()
        .min(1, tx("请选择智能体", "Agent is required")),
    });

    const parsedBase = baseSchema.safeParse({
      name,
      agent_id: agentId,
    });
    if (!parsedBase.success) {
      setFormError(
        parsedBase.error.issues[0]?.message ??
          tx("字段格式有误", "Invalid form input")
      );
      return;
    }

    const trimmedDelegated = delegatedUserId.trim();
    if (identityPolicy === "CREATOR_DELEGATED" && trimmedDelegated.length === 0) {
      setFormError(
        tx("委托用户 ID 不能为空", "Delegated User ID is required")
      );
      return;
    }

    const trimmedCredId = credentialId.trim();
    const trimmedCredSecret = credentialSecret.trim();
    const isCreate = editingConnector === null;

    if (isCreate && (trimmedCredId.length === 0 || trimmedCredSecret.length === 0)) {
      setFormError(
        tx("应用凭证 ID 与应用密钥不能为空", "Credential ID and Secret are required")
      );
      return;
    }
    if (
      !isCreate &&
      (trimmedCredId.length > 0 || trimmedCredSecret.length > 0) &&
      (trimmedCredId.length === 0 || trimmedCredSecret.length === 0)
    ) {
      setFormError(
        tx(
          "更新密钥时需同时填写应用凭证 ID 与应用密钥",
          "Provide both Credential ID and Secret when rotating credentials"
        )
      );
      return;
    }

    const trimmedEndpoint = endpointOverride.trim();
    if (
      trimmedEndpoint.length > 0 &&
      !(
        trimmedEndpoint.startsWith("http://") ||
        trimmedEndpoint.startsWith("https://") ||
        trimmedEndpoint.startsWith("ws://") ||
        trimmedEndpoint.startsWith("wss://")
      )
    ) {
      setFormError(tx("网关地址格式无效", "Invalid endpoint URL"));
      return;
    }

    const effectivePrivateNet = canGuardAdmin
      ? allowPrivateNetwork
      : Boolean(editingConnector?.config?.allow_private_network);

    const configPayload: ImConnectorConfig = {
      ...(editingConnector?.config ?? {}),
      endpoint_override: trimmedEndpoint.length > 0 ? trimmedEndpoint : null,
      allow_private_network: effectivePrivateNet,
    };

    setSaving(true);
    try {
      if (editingConnector) {
        const patchReq: PatchImConnectorRequest = {
          name: parsedBase.data.name,
          agent_id: parsedBase.data.agent_id,
          identity_policy: identityPolicy,
          delegated_user_id:
            identityPolicy === "CREATOR_DELEGATED" ? trimmedDelegated : null,
          config: configPayload,
          ...(trimmedCredId.length > 0 && trimmedCredSecret.length > 0
            ? {
                secret: buildSecretPayload(
                  editingConnector.platform,
                  trimmedCredId,
                  trimmedCredSecret
                ),
              }
            : {}),
        };
        await patchImConnector(editingConnector.id, patchReq);
      } else {
        const createReq: CreateImConnectorRequest = {
          name: parsedBase.data.name,
          platform,
          agent_id: parsedBase.data.agent_id,
          identity_policy: identityPolicy,
          ...(identityPolicy === "CREATOR_DELEGATED"
            ? { delegated_user_id: trimmedDelegated }
            : {}),
          secret: buildSecretPayload(platform, trimmedCredId, trimmedCredSecret),
          config: configPayload,
          enabled: true,
        };
        await createImConnector(createReq);
      }
      setModalOpen(false);
      setPanelFeedback(tx("已保存", "Saved"));
      await refreshImData();
    } catch (err) {
      setFormError(
        err instanceof Error
          ? err.message
          : tx("保存失败", "Failed to save connector")
      );
    } finally {
      setSaving(false);
    }
  };

  const handleTestConnector = async (conn: ImConnectorRecord) => {
    if (!canEdit) return;
    setTestingId(conn.id);
    setPanelError(null);
    try {
      const res = await testImConnector(conn.id);
      setTestResults((prev) => ({ ...prev, [conn.id]: res }));
    } catch (err) {
      setPanelError(
        err instanceof Error
          ? err.message
          : tx("测试失败", "Connection test failed")
      );
    } finally {
      setTestingId(null);
    }
  };

  const handleToggleEnabled = async (conn: ImConnectorRecord) => {
    if (!canEdit) return;
    setTogglingId(conn.id);
    setPanelError(null);
    try {
      await patchImConnector(conn.id, { enabled: !conn.enabled });
      await refreshImData();
    } catch (err) {
      setPanelError(
        err instanceof Error
          ? err.message
          : tx("状态更新失败", "Failed to update connector state")
      );
    } finally {
      setTogglingId(null);
    }
  };

  const handleConfirmDeleteConnector = async () => {
    if (!canEdit || !deletingConnector) return;
    setDeletingBusy(true);
    setPanelError(null);
    try {
      await deleteImConnector(deletingConnector.id);
      setDeletingConnector(null);
      await refreshImData();
    } catch (err) {
      setPanelError(
        err instanceof Error
          ? err.message
          : tx("删除失败", "Failed to delete connector")
      );
    } finally {
      setDeletingBusy(false);
    }
  };

  const canUnbindRow = useCallback(
    (row: ImUserBindingRecord): boolean => {
      if (canEdit) return true;
      return effectiveUserId.length === 0 || row.user_id === effectiveUserId;
    },
    [canEdit, effectiveUserId]
  );

  const handleUnbind = async (row: ImUserBindingRecord) => {
    if (!canUnbindRow(row)) return;
    setUnbindingId(row.id);
    setPanelError(null);
    try {
      await deleteImBinding(row.id);
      await refreshImData();
    } catch (err) {
      setPanelError(
        err instanceof Error
          ? err.message
          : tx("解绑失败", "Failed to unbind account")
      );
    } finally {
      setUnbindingId(null);
    }
  };

  const handleCloseBindModal = () => {
    setBindPreview(null);
    setBindModalError(null);
    onClearBindToken?.();
  };

  const handleConfirmBind = async () => {
    if (!activeChallengeToken) return;
    setConfirmingBind(true);
    setBindModalError(null);
    try {
      await confirmImBindToken(activeChallengeToken);
      setBindPreview(null);
      onClearBindToken?.();
      setPanelFeedback(tx("账号已绑定", "Account bound"));
      await refreshImData();
    } catch (err) {
      setBindModalError(
        err instanceof Error
          ? err.message
          : tx("绑定失败", "Account binding failed")
      );
    } finally {
      setConfirmingBind(false);
    }
  };

  return (
    <div className="space-y-6" data-testid="im-connector-panel">
      {/* Connectors Header */}
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Radio className="h-4 w-4 text-xblue shrink-0" />
          <h2 className="text-sm font-semibold">
            {tx("IM 连接器", "IM Connectors")}
          </h2>
          <Badge variant="neutral">{connectors.length}</Badge>
        </div>

        {canEdit ? (
          <Button
            type="button"
            data-testid="new-im-connector-btn"
            onClick={openCreateModal}
          >
            <Plus className="h-3.5 w-3.5" />
            {tx("新建", "New")}
          </Button>
        ) : null}
      </div>

      {panelError ? (
        <div
          role="alert"
          className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400"
        >
          {panelError}
        </div>
      ) : null}

      {panelFeedback ? (
        <div
          data-testid="im-panel-feedback"
          className="rounded-md border border-xblue/35 bg-xblue-soft/30 px-3 py-2 text-xs text-foreground"
        >
          {panelFeedback}
        </div>
      ) : null}

      {/* Connector Cards */}
      {connectors.length === 0 ? (
        <EmptyState
          title={tx("暂无连接器", "No connectors")}
          action={
            canEdit ? (
              <Button
                type="button"
                data-testid="empty-new-im-connector-btn"
                onClick={openCreateModal}
              >
                <Plus className="h-3.5 w-3.5" />
                {tx("新建", "New")}
              </Button>
            ) : undefined
          }
        />
      ) : (
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          {connectors.map((conn) => {
            const boundAgentName =
              agentNameById.get(conn.agent_id) ?? conn.agent_id;
            const testRes = testResults[conn.id];

            return (
              <Card
                key={conn.id}
                data-testid={`im-connector-card-${conn.name}`}
                className="p-4 space-y-3"
              >
                <div className="flex flex-wrap items-start justify-between gap-2">
                  <div className="space-y-1 min-w-0">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="font-semibold font-mono-tech text-sm truncate">
                        {conn.name}
                      </span>
                      <Badge
                        variant="xblue"
                        data-testid={`im-connector-platform-${conn.name}`}
                      >
                        {tEnum(conn.platform)}
                      </Badge>
                      <Badge
                        variant={
                          conn.identity_policy === "REQUIRE_USER_BIND"
                            ? "outline"
                            : "warning"
                        }
                        data-testid={`im-connector-policy-${conn.name}`}
                      >
                        {tEnum(conn.identity_policy)}
                      </Badge>
                    </div>
                    <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                      <span>{tx("智能体", "Agent")}:</span>
                      <span
                        data-testid={`im-connector-agent-${conn.name}`}
                        className="font-mono-tech text-foreground"
                      >
                        {boundAgentName}
                      </span>
                    </div>
                  </div>

                  <div className="flex items-center flex-wrap gap-1.5 shrink-0">
                    {!conn.enabled ? <StatusBadge status="DISABLED" /> : null}
                    <StatusBadge
                      status={conn.status}
                      data-testid={`im-connector-status-${conn.name}`}
                    />
                  </div>
                </div>

                {/* Lease Node & Last Heartbeat */}
                <div className="flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground border-t border-border/70 pt-2.5">
                  <Badge
                    variant={conn.leader_holder ? "neutral" : "outline"}
                    data-testid={`im-connector-leader-${conn.name}`}
                  >
                    <Server className="h-3 w-3 mr-1 shrink-0" />
                    {conn.leader_holder ?? tx("无租约", "Unclaimed")}
                  </Badge>
                  <span
                    data-testid={`im-connector-heartbeat-${conn.name}`}
                    className="font-mono-tech"
                  >
                    {tx("心跳", "Heartbeat")}: {fmtTime(conn.last_heartbeat_at)}
                  </span>
                </div>

                {conn.last_error ? (
                  <div
                    data-testid={`im-connector-error-${conn.name}`}
                    className="rounded border border-red-500/30 bg-red-500/8 px-2.5 py-1.5 text-[11px] text-red-600 dark:text-red-400 break-words font-mono-tech"
                  >
                    {conn.last_error}
                  </div>
                ) : null}

                {testRes ? (
                  <div
                    data-testid={`im-connector-test-result-${conn.name}`}
                    className={cn(
                      "flex items-center flex-wrap gap-1.5 rounded border border-border bg-muted/30 px-2.5 py-1.5 text-[11px] font-mono-tech",
                      testRes.ok
                        ? "text-xblue"
                        : "text-red-600 dark:text-red-400"
                    )}
                  >
                    {testRes.ok ? (
                      <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
                    ) : (
                      <XCircle className="h-3.5 w-3.5 shrink-0" />
                    )}
                    <span>{testRes.latency_ms}ms</span>
                    {testRes.endpoint_preview ? (
                      <span className="text-muted-foreground truncate">
                        · {testRes.endpoint_preview}
                      </span>
                    ) : null}
                    {testRes.message ? (
                      <span className="text-muted-foreground truncate">
                        · {testRes.message}
                      </span>
                    ) : null}
                  </div>
                ) : null}

                {/* Row Action Buttons */}
                {canEdit ? (
                  <div className="flex flex-wrap items-center justify-end gap-1.5 pt-1">
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      data-testid={`test-im-connector-btn-${conn.name}`}
                      disabled={testingId === conn.id}
                      onClick={() => void handleTestConnector(conn)}
                    >
                      <Zap className="h-3.5 w-3.5 text-xblue" />
                      {testingId === conn.id
                        ? tx("测试中…", "Testing…")
                        : tx("测试", "Test")}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      data-testid={`edit-im-connector-btn-${conn.name}`}
                      onClick={() => openEditModal(conn)}
                    >
                      <Edit3 className="h-3.5 w-3.5" />
                      {tx("编辑", "Edit")}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      data-testid={`toggle-im-connector-btn-${conn.name}`}
                      disabled={togglingId === conn.id}
                      onClick={() => void handleToggleEnabled(conn)}
                    >
                      <Power className="h-3.5 w-3.5" />
                      {conn.enabled
                        ? tx("停用", "Disable")
                        : tx("启用", "Enable")}
                    </Button>
                    <Button
                      type="button"
                      variant="danger"
                      size="sm"
                      data-testid={`delete-im-connector-btn-${conn.name}`}
                      onClick={() => setDeletingConnector(conn)}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      {tx("删除", "Delete")}
                    </Button>
                  </div>
                ) : null}
              </Card>
            );
          })}
        </div>
      )}

      {/* Bound External Accounts Table */}
      <Card className="overflow-hidden" data-testid="im-bindings-card">
        <div className="flex items-center justify-between gap-2 border-b border-border px-4 py-3">
          <div className="flex items-center gap-2">
            <Link2 className="h-4 w-4 text-xblue shrink-0" />
            <h3 className="text-sm font-semibold">
              {tx("已绑定账号", "Bound Accounts")}
            </h3>
            <Badge variant="neutral">{bindings.length}</Badge>
          </div>
        </div>

        {bindings.length === 0 ? (
          <div className="p-8 text-center text-xs text-muted-foreground">
            {tx("暂无绑定记录", "No bound accounts")}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead className="bg-muted/50 text-muted-foreground border-b border-border">
                <tr>
                  <th className="px-4 py-2.5 font-medium whitespace-nowrap">
                    {tx("平台", "Platform")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("外部昵称", "Display Name")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("外部账号 ID", "External ID")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("平台账号", "Account")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("绑定时间", "Bound At")}
                  </th>
                  <th className="px-4 py-2.5 font-medium text-right whitespace-nowrap">
                    {tx("操作", "Actions")}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {bindings.map((row) => (
                  <tr
                    key={row.id}
                    data-testid={`im-binding-row-${row.id}`}
                    className="hover:bg-muted/20"
                  >
                    <td className="px-4 py-2.5 whitespace-nowrap">
                      <Badge variant="outline">{tEnum(row.platform)}</Badge>
                    </td>
                    <td className="px-3 py-2.5 whitespace-nowrap font-medium">
                      {row.external_display_name || "-"}
                    </td>
                    <td className="px-3 py-2.5 whitespace-nowrap font-mono-tech text-muted-foreground">
                      {row.external_user_id}
                    </td>
                    <td className="px-3 py-2.5 whitespace-nowrap font-mono-tech">
                      {row.username ? (
                        <span>
                          {row.username}{" "}
                          <span className="text-muted-foreground">
                            ({row.user_id})
                          </span>
                        </span>
                      ) : (
                        row.user_id
                      )}
                    </td>
                    <td className="px-3 py-2.5 whitespace-nowrap font-mono-tech text-muted-foreground">
                      {fmtTime(row.bound_at)}
                    </td>
                    <td className="px-4 py-2.5 text-right whitespace-nowrap">
                      {canUnbindRow(row) ? (
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          data-testid={`unbind-im-account-btn-${row.id}`}
                          disabled={unbindingId === row.id}
                          onClick={() => void handleUnbind(row)}
                        >
                          <Unlink className="h-3.5 w-3.5" />
                          {tx("解绑", "Unbind")}
                        </Button>
                      ) : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      {/* Create / Edit Connector Modal */}
      <Modal
        open={modalOpen}
        onClose={() => setModalOpen(false)}
        maxWidthClass="max-w-lg"
        testId="im-connector-modal"
        title={
          editingConnector
            ? tx("编辑连接器", "Edit Connector")
            : tx("新建连接器", "New Connector")
        }
        footer={
          <>
            <Button
              type="button"
              variant="outline"
              data-testid="cancel-im-connector-btn"
              disabled={saving}
              onClick={() => setModalOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="button"
              data-testid="save-im-connector-btn"
              disabled={saving}
              onClick={() => void handleSaveConnector()}
            >
              {saving ? tx("保存中…", "Saving…") : tx("保存", "Save")}
            </Button>
          </>
        }
      >
        {formError ? (
          <div
            role="alert"
            className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400"
          >
            {formError}
          </div>
        ) : null}

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("名称", "Name")}
            </label>
            <Input
              data-testid="im-connector-name-input"
              aria-label={tx("名称", "Name")}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="feishu_ops_bot"
              className="font-mono-tech"
            />
          </div>

          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("平台", "Platform")}
            </label>
            <Select
              data-testid="im-connector-platform-select"
              aria-label={tx("平台", "Platform")}
              disabled={editingConnector !== null}
              value={platform}
              onChange={(e) => {
                if (isImPlatform(e.target.value)) {
                  setPlatform(e.target.value);
                }
              }}
              className="w-full"
            >
              {IM_PLATFORMS.map((plat) => (
                <option key={plat} value={plat}>
                  {tEnum(plat)}
                </option>
              ))}
            </Select>
          </div>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("智能体", "Agent")}
            </label>
            <Select
              data-testid="im-connector-agent-select"
              aria-label={tx("智能体", "Agent")}
              value={agentId}
              onChange={(e) => setAgentId(e.target.value)}
              className="w-full"
            >
              {sortedAgents.length === 0 ? (
                <option value="">{tx("无可用项", "None")}</option>
              ) : (
                sortedAgents.map((ag) => (
                  <option key={ag.id} value={ag.id}>
                    {ag.name} (v{ag.version})
                  </option>
                ))
              )}
            </Select>
          </div>

          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("身份策略", "Identity Policy")}
            </label>
            <Select
              data-testid="im-connector-policy-select"
              aria-label={tx("身份策略", "Identity Policy")}
              value={identityPolicy}
              onChange={(e) => {
                if (isImIdentityPolicy(e.target.value)) {
                  handleChangePolicy(e.target.value);
                }
              }}
              className="w-full"
            >
              {IM_IDENTITY_POLICIES.map((pol) => (
                <option key={pol} value={pol}>
                  {tEnum(pol)}
                </option>
              ))}
            </Select>
          </div>
        </div>

        {identityPolicy === "CREATOR_DELEGATED" ? (
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("委托用户 ID", "Delegated User ID")}
            </label>
            <Input
              data-testid="im-connector-delegated-user-input"
              aria-label={tx("委托用户 ID", "Delegated User ID")}
              value={delegatedUserId}
              onChange={(e) => setDelegatedUserId(e.target.value)}
              placeholder="00000000-0000-0000-0000-000000000000"
              className="font-mono-tech"
            />
          </div>
        ) : null}

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("应用凭证 ID", "Credential ID")}
            </label>
            <Input
              data-testid="im-connector-credential-id-input"
              aria-label={tx("应用凭证 ID", "Credential ID")}
              value={credentialId}
              onChange={(e) => setCredentialId(e.target.value)}
              placeholder={credentialIdPlaceholder(platform)}
              className="font-mono-tech"
            />
          </div>

          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("应用密钥", "Secret")}
            </label>
            <Input
              type="password"
              data-testid="im-connector-secret-input"
              aria-label={tx("应用密钥", "Secret")}
              value={credentialSecret}
              onChange={(e) => setCredentialSecret(e.target.value)}
              placeholder="••••••••••••"
              className="font-mono-tech"
            />
          </div>
        </div>

        <div className="border-t border-border/70 pt-3 space-y-3">
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("网关地址", "Endpoint Override")}
            </label>
            <Input
              data-testid="im-connector-endpoint-input"
              aria-label={tx("网关地址", "Endpoint Override")}
              value={endpointOverride}
              onChange={(e) => setEndpointOverride(e.target.value)}
              placeholder="https://open.feishu.cn"
              className="font-mono-tech"
            />
          </div>

          <div className="flex items-center justify-between gap-3">
            <label
              htmlFor="im-connector-private-net-switch"
              className="text-xs text-foreground"
            >
              {tx("内网地址放行", "Private Network")}
            </label>
            <Switch
              id="im-connector-private-net-switch"
              data-testid="im-connector-private-net-switch"
              aria-label={tx("内网地址放行", "Private Network")}
              disabled={!canGuardAdmin}
              checked={allowPrivateNetwork}
              onCheckedChange={setAllowPrivateNetwork}
            />
          </div>
        </div>
      </Modal>

      {/* Delete Connector Confirmation Modal */}
      <Modal
        open={deletingConnector !== null}
        onClose={() => {
          if (!deletingBusy) setDeletingConnector(null);
        }}
        maxWidthClass="max-w-md"
        testId="delete-im-connector-modal"
        title={tx("删除连接器", "Delete Connector")}
        footer={
          <>
            <Button
              type="button"
              variant="outline"
              disabled={deletingBusy}
              onClick={() => setDeletingConnector(null)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="button"
              variant="danger"
              data-testid="confirm-delete-im-connector-btn"
              disabled={deletingBusy}
              onClick={() => void handleConfirmDeleteConnector()}
            >
              {tx("删除", "Delete")}
            </Button>
          </>
        }
      >
        <p className="text-muted-foreground leading-relaxed">
          {deletingConnector
            ? tx(
                `删除连接器 "${deletingConnector.name}" 将立即断开出站长连接，关联渠道不再接收或回复消息。`,
                `Deleting connector "${deletingConnector.name}" immediately disconnects outbound streams and stops message delivery.`
              )
            : null}
        </p>
      </Modal>

      {/* One-Time Challenge Bind Confirmation Modal (`?im_bind=axim_...`) */}
      <Modal
        open={activeChallengeToken !== null}
        onClose={handleCloseBindModal}
        maxWidthClass="max-w-md"
        testId="im-bind-confirm-modal"
        title={tx("账号绑定", "Account Binding")}
        footer={
          <>
            <Button
              type="button"
              variant="outline"
              data-testid="cancel-im-bind-btn"
              disabled={confirmingBind}
              onClick={handleCloseBindModal}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="button"
              data-testid="confirm-im-bind-btn"
              disabled={
                confirmingBind || bindPreviewLoading || bindPreview === null
              }
              onClick={() => void handleConfirmBind()}
            >
              {confirmingBind
                ? tx("绑定中…", "Binding…")
                : tx("确认绑定", "Confirm")}
            </Button>
          </>
        }
      >
        {bindModalError ? (
          <div
            role="alert"
            data-testid="im-bind-error"
            className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400"
          >
            {bindModalError}
          </div>
        ) : null}

        {bindPreviewLoading ? (
          <div className="py-4 text-center text-xs text-muted-foreground font-mono-tech">
            {tx("加载中…", "Loading…")}
          </div>
        ) : bindPreview ? (
          <div className="space-y-3">
            <div className="rounded-md border border-border bg-muted/30 p-3 space-y-2 text-xs">
              <div className="flex items-center justify-between gap-2">
                <span className="text-muted-foreground">
                  {tx("连接器", "Connector")}
                </span>
                <span
                  data-testid="im-bind-connector-name"
                  className="font-semibold font-mono-tech"
                >
                  {bindPreview.connector_name}
                </span>
              </div>
              <div className="flex items-center justify-between gap-2">
                <span className="text-muted-foreground">
                  {tx("平台", "Platform")}
                </span>
                <Badge
                  variant="xblue"
                  data-testid="im-bind-platform"
                >
                  {tEnum(bindPreview.platform)}
                </Badge>
              </div>
              <div className="flex items-center justify-between gap-2">
                <span className="text-muted-foreground">
                  {tx("外部账号", "External Account")}
                </span>
                <span
                  data-testid="im-bind-external-user"
                  className="font-mono-tech text-right"
                >
                  {bindPreview.external_display_name
                    ? `${bindPreview.external_display_name} (${bindPreview.external_user_id})`
                    : bindPreview.external_user_id}
                </span>
              </div>
            </div>

            <p className="text-xs text-muted-foreground leading-relaxed">
              {tx(
                "来自该外部 IM 账号的对话将以当前平台账号身份执行，继承当前账号访问权限与行列脱敏策略。",
                "Conversations from this IM account execute under your current platform account permissions and row/column masking policies."
              )}
            </p>
          </div>
        ) : null}
      </Modal>
    </div>
  );
}
