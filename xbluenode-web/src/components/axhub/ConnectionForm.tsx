import React, { useEffect, useMemo, useState } from "react";
import {
  CheckCircle2,
  Compass,
  Eye,
  KeyRound,
  Lock,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Trash2,
  XCircle,
} from "lucide-react";
import { useI18n } from "@/i18n";
import {
  axhubApi,
  type Connection,
  type ConnectionTestResult,
  type ConnectorSpec,
  type DiscoveredObject,
  type PreviewResponse,
  type SchemaPolicy,
} from "@/lib/api/axhub";
import type { ConnectorType, SourceKind, SyncMode } from "@/lib/api/enums";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  Drawer,
  EmptyState,
  Input,
  Modal,
  Select,
  StatusBadge,
  Textarea,
} from "@/components/ui/primitives";

interface SchemaPropertySpec {
  type?: string;
  title?: string;
  description?: string;
  default?: unknown;
  enum?: unknown[];
}

function isRecord(val: unknown): val is Record<string, unknown> {
  return typeof val === "object" && val !== null && !Array.isArray(val);
}

function extractSchemaProperties(
  schema: Record<string, unknown>
): { properties: Record<string, SchemaPropertySpec>; required: Set<string> } {
  const propsRaw = schema.properties;
  const reqRaw = schema.required;
  const properties: Record<string, SchemaPropertySpec> = {};

  if (isRecord(propsRaw)) {
    for (const [key, spec] of Object.entries(propsRaw)) {
      if (isRecord(spec)) {
        properties[key] = {
          type: typeof spec.type === "string" ? spec.type : undefined,
          title: typeof spec.title === "string" ? spec.title : undefined,
          description:
            typeof spec.description === "string" ? spec.description : undefined,
          default: spec.default,
          enum: Array.isArray(spec.enum) ? spec.enum : undefined,
        };
      }
    }
  }

  const required = new Set<string>(
    Array.isArray(reqRaw)
      ? reqRaw.filter((item): item is string => typeof item === "string")
      : []
  );

  return { properties, required };
}

const FALLBACK_SPECS: ConnectorSpec[] = [
  {
    connector_type: "POSTGRES",
    display_name: "PostgreSQL",
    category: "RELATIONAL",
    config_schema: {
      properties: {
        host: { type: "string", title: "Host", default: "127.0.0.1" },
        port: { type: "integer", title: "Port", default: 5432 },
        database: { type: "string", title: "Database" },
        schema: { type: "string", title: "Schema", default: "public" },
        username: { type: "string", title: "Username" },
        sslmode: {
          type: "string",
          title: "SSL Mode",
          enum: ["disable", "prefer", "require"],
          default: "prefer",
        },
      },
      required: ["host", "port", "database", "username"],
    },
    secret_fields: ["password"],
    supports_modes: ["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover: true,
    image_tier: "lite",
    available: true,
  },
];

export interface ConnectionFormProps {
  connections: Connection[];
  connectorSpecs: ConnectorSpec[];
  onChanged: () => Promise<void>;
  onDatasetCreated?: () => Promise<void>;
}

export function ConnectionForm({
  connections,
  connectorSpecs,
  onChanged,
  onDatasetCreated,
}: ConnectionFormProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canDataAdmin = can("DATA_ADMIN");
  const canDataSync = can("DATA_SYNC");

  const specs = connectorSpecs.length > 0 ? connectorSpecs : FALLBACK_SPECS;

  const [formOpen, setFormOpen] = useState(false);
  const [editingConn, setEditingConn] = useState<Connection | null>(null);
  const [selectedType, setSelectedType] = useState<ConnectorType>("POSTGRES");
  const [connName, setConnName] = useState("");
  const [allowPrivateNetwork, setAllowPrivateNetwork] = useState(false);
  const [configValues, setConfigValues] = useState<Record<string, unknown>>({});
  const [rawJsonConfig, setRawJsonConfig] = useState("{}");
  const [secretInputs, setSecretInputs] = useState<Record<string, string>>({});
  const [clearedSecrets, setClearedSecrets] = useState<Record<string, boolean>>(
    {}
  );
  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [statusBanner, setStatusBanner] = useState<string | null>(null);

  const [pendingDeleteConn, setPendingDeleteConn] = useState<Connection | null>(
    null
  );
  const [deletingConn, setDeletingConn] = useState(false);

  const [testingId, setTestingId] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<
    Record<string, ConnectionTestResult>
  >({});

  const [discoveringConn, setDiscoveringConn] = useState<Connection | null>(
    null
  );
  const [discoveredObjects, setDiscoveredObjects] = useState<
    DiscoveredObject[]
  >([]);
  const [discoverLoading, setDiscoverLoading] = useState(false);
  const [sourcePreview, setSourcePreview] = useState<{
    title: string;
    data: PreviewResponse;
  } | null>(null);
  const [quickCreateIdx, setQuickCreateIdx] = useState<number | null>(null);
  const [newDatasetName, setNewDatasetName] = useState("");
  const [newDatasetSyncMode, setNewDatasetSyncMode] =
    useState<SyncMode>("SNAPSHOT");
  const [newDatasetPolicy, setNewDatasetPolicy] =
    useState<SchemaPolicy>("ADDITIVE");
  const [newDatasetCron, setNewDatasetCron] = useState("");

  const categoryLabel = (cat: string): string => {
    switch (cat) {
      case "FILE":
        return tx("文件与对象存储", "File & Object Storage");
      case "RELATIONAL":
        return tx("关系型数据库", "Relational Databases");
      case "DOMESTIC":
        return tx("国产数据库", "Domestic Databases");
      case "ANALYTIC":
        return tx("分析型数据库", "Analytical Engines");
      case "NOSQL":
        return tx("文档与图数据库", "Document & Graph");
      case "TIMESERIES":
        return tx("时序数据库", "Time-Series");
      case "STREAM":
        return tx("消息流", "Streaming");
      case "API":
        return tx("接口与推送", "API & Webhooks");
      default:
        return cat;
    }
  };

  const formatSourceKind = (kind: SourceKind): string => {
    switch (kind) {
      case "NOSQL_COLLECTION":
        return tx("文档集合", "NoSQL Collection");
      case "TIMESERIES":
        return tx("时序表", "Time-Series");
      case "STREAM":
        return tx("流数据", "Stream");
      case "API_RESOURCE":
        return tx("接口资源", "API Resource");
      default:
        return tEnum(kind);
    }
  };

  const formatTestMessage = (msg: string): string => {
    const trimmed = msg.trim();
    if (!trimmed) return "";
    if (trimmed.toLowerCase() === "connected") {
      return tEnum("connected");
    }
    if (/\s+connected$/i.test(trimmed)) {
      return `${trimmed.replace(/\s+connected$/i, "")} · ${tEnum("connected")}`;
    }
    return trimmed;
  };

  const activeSpec = useMemo(
    () =>
      specs.find((s) => s.connector_type === selectedType) ??
      specs[0] ??
      FALLBACK_SPECS[0],
    [specs, selectedType]
  );

  const groupedSpecs = useMemo(() => {
    const map = new Map<string, ConnectorSpec[]>();
    for (const spec of specs) {
      const cat = spec.category || "OTHER";
      const bucket = map.get(cat) ?? [];
      bucket.push(spec);
      map.set(cat, bucket);
    }
    return Array.from(map.entries());
  }, [specs]);

  const { properties: schemaProps, required: requiredFields } = useMemo(
    () => extractSchemaProperties(activeSpec.config_schema),
    [activeSpec]
  );

  const secretFieldSet = useMemo(
    () => new Set(activeSpec.secret_fields),
    [activeSpec]
  );

  const nonSecretEntries = useMemo(
    () =>
      Object.entries(schemaProps).filter(([field]) => !secretFieldSet.has(field)),
    [schemaProps, secretFieldSet]
  );

  const resetForType = (spec: ConnectorSpec) => {
    const { properties } = extractSchemaProperties(spec.config_schema);
    const defaults: Record<string, unknown> = {};
    for (const [k, prop] of Object.entries(properties)) {
      if (!spec.secret_fields.includes(k) && prop.default !== undefined) {
        defaults[k] = prop.default;
      }
    }
    setConfigValues(defaults);
    setRawJsonConfig(JSON.stringify(defaults, null, 2));
    setSecretInputs({});
    setClearedSecrets({});
  };

  useEffect(() => {
    if (!editingConn) {
      resetForType(activeSpec);
    }
  }, [activeSpec, editingConn]);

  const openCreateForm = () => {
    if (!canDataAdmin) return;
    setEditingConn(null);
    setConnName("");
    setAllowPrivateNetwork(false);
    setErrorMsg(null);
    const initialSpec =
      specs.find((s) => s.connector_type === "POSTGRES") ?? specs[0];
    if (initialSpec) {
      setSelectedType(initialSpec.connector_type);
      resetForType(initialSpec);
    }
    setFormOpen(true);
  };

  const openEditForm = (conn: Connection) => {
    if (!canDataAdmin) return;
    setEditingConn(conn);
    setSelectedType(conn.connector_type);
    setConnName(conn.name);
    setAllowPrivateNetwork(conn.allow_private_network);
    const safeCopy: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(conn.config)) {
      safeCopy[k] = v;
    }
    setConfigValues(safeCopy);
    setRawJsonConfig(JSON.stringify(safeCopy, null, 2));
    setSecretInputs({});
    setClearedSecrets({});
    setErrorMsg(null);
    setFormOpen(true);
  };

  const buildConfigObject = (): Record<string, unknown> => {
    if (nonSecretEntries.length === 0) {
      const parsed: unknown = JSON.parse(rawJsonConfig || "{}");
      if (!isRecord(parsed)) {
        throw new Error(tx("配置 JSON 需为对象结构", "Config JSON must be an object"));
      }
      return parsed;
    }
    const out: Record<string, unknown> = {};
    for (const [key, prop] of nonSecretEntries) {
      const val = configValues[key];
      if (val === undefined || val === "") {
        continue;
      }
      if (prop.type === "integer" || prop.type === "number") {
        const num = typeof val === "number" ? val : Number(val);
        if (!Number.isNaN(num)) {
          out[key] = prop.type === "integer" ? Math.trunc(num) : num;
        }
      } else if (prop.type === "object" || prop.type === "array") {
        if (typeof val === "string") {
          out[key] = JSON.parse(val);
        } else {
          out[key] = val;
        }
      } else {
        out[key] = val;
      }
    }
    return out;
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canDataAdmin) return;
    if (!connName.trim()) {
      setErrorMsg(tx("连接名称不能为空", "Connection name is required"));
      return;
    }
    setSubmitting(true);
    setErrorMsg(null);
    try {
      const builtConfig = buildConfigObject();
      if (editingConn) {
        const patchSecrets: Record<string, string | null> = {};
        for (const sf of activeSpec.secret_fields) {
          if (clearedSecrets[sf]) {
            patchSecrets[sf] = null;
          } else {
            const typed = secretInputs[sf]?.trim() ?? "";
            if (typed.length > 0) {
              patchSecrets[sf] = typed;
            }
          }
        }
        await axhubApi.updateConnection(editingConn.id, {
          name: connName.trim(),
          config: builtConfig,
          secrets:
            Object.keys(patchSecrets).length > 0 ? patchSecrets : undefined,
          allow_private_network: allowPrivateNetwork,
        });
        setStatusBanner(
          tx(`已更新连接「${connName.trim()}」`, `Updated "${connName.trim()}"`)
        );
      } else {
        const createSecrets: Record<string, string> = {};
        for (const sf of activeSpec.secret_fields) {
          const typed = secretInputs[sf]?.trim() ?? "";
          if (typed.length > 0) {
            createSecrets[sf] = typed;
          }
        }
        await axhubApi.createConnection({
          name: connName.trim(),
          connector_type: selectedType,
          config: builtConfig,
          secrets:
            Object.keys(createSecrets).length > 0 ? createSecrets : undefined,
          allow_private_network: allowPrivateNetwork,
        });
        setStatusBanner(
          tx(`已创建连接「${connName.trim()}」`, `Created "${connName.trim()}"`)
        );
      }
      setFormOpen(false);
      setEditingConn(null);
      await onChanged();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  const handleTest = async (conn: Connection) => {
    setTestingId(conn.id);
    setErrorMsg(null);
    try {
      const res = await axhubApi.testConnection(conn.id);
      setTestResults((prev) => ({ ...prev, [conn.id]: res }));
      await onChanged();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setTestingId(null);
    }
  };

  const handleConfirmDelete = async () => {
    if (!pendingDeleteConn || !canDataAdmin) return;
    setDeletingConn(true);
    setErrorMsg(null);
    try {
      const target = pendingDeleteConn;
      await axhubApi.deleteConnection(target.id);
      setStatusBanner(
        tx(`已删除连接「${target.name}」`, `Deleted "${target.name}"`)
      );
      if (editingConn?.id === target.id) {
        setFormOpen(false);
        setEditingConn(null);
      }
      setPendingDeleteConn(null);
      await onChanged();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingConn(false);
    }
  };

  const handleDiscover = async (conn: Connection) => {
    setDiscoveringConn(conn);
    setDiscoverLoading(true);
    setDiscoveredObjects([]);
    setSourcePreview(null);
    setQuickCreateIdx(null);
    setErrorMsg(null);
    try {
      const items = await axhubApi.discoverConnection(conn.id);
      setDiscoveredObjects(Array.isArray(items) ? items : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDiscoverLoading(false);
    }
  };

  const handlePreviewObject = async (
    conn: Connection,
    obj: DiscoveredObject
  ) => {
    setErrorMsg(null);
    try {
      const preview = await axhubApi.previewConnection(conn.id, {
        ref: obj.ref,
        limit: 20,
      });
      setSourcePreview({ title: obj.display_name, data: preview });
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCreateDatasetFromObject = async (obj: DiscoveredObject) => {
    if (
      (!canDataSync && !canDataAdmin) ||
      !discoveringConn ||
      !newDatasetName.trim()
    ) {
      return;
    }
    setErrorMsg(null);
    try {
      await axhubApi.createDataset({
        connection_id: discoveringConn.id,
        name: newDatasetName.trim(),
        display_name: obj.display_name,
        source_kind: obj.source_kind,
        source_ref: obj.ref,
        sync_mode: newDatasetSyncMode,
        primary_key: obj.primary_key,
        cursor_column: obj.suggested_cursor,
        schema_policy: newDatasetPolicy,
        schedule_cron: newDatasetCron.trim() || null,
      });
      setStatusBanner(
        tx(
          `已注册数据集「${newDatasetName.trim()}」`,
          `Registered dataset "${newDatasetName.trim()}"`
        )
      );
      setQuickCreateIdx(null);
      if (onDatasetCreated) {
        await onDatasetCreated();
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleRotateCredentials = async () => {
    if (!canDataAdmin) return;
    setErrorMsg(null);
    try {
      const res = await axhubApi.rotateCredentials();
      setStatusBanner(
        tx(
          `密钥轮换任务已入队 (${res.job_id.slice(0, 8)})`,
          `Key rotation queued (${res.job_id.slice(0, 8)})`
        )
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const renderStatusBadge = (status: Connection["status"]) => {
    if (status === "UNTESTED") {
      return <Badge variant="neutral">{tx("未测试", "Untested")}</Badge>;
    }
    return <StatusBadge status={status} />;
  };

  return (
    <Card className="px-3.5 py-2.5 space-y-2.5" data-testid="connections-section">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2 text-xs">
          <span className="font-semibold text-foreground">
            {tx("数据源连接", "Data Connections")}
          </span>
          <span className="text-[11px] font-mono-tech text-muted-foreground">
            {fmtNum(connections.length, 0)} · {fmtNum(specs.length, 0)}{" "}
            {tx("种连接器", "Specs")}
          </span>
        </div>
        <div className="flex items-center gap-1.5">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={!canDataAdmin}
            onClick={() => void handleRotateCredentials()}
            data-testid="rotate-credentials-btn"
          >
            <KeyRound className="h-3.5 w-3.5" />
            {tx("轮换密钥", "Rotate Keys")}
          </Button>
          <Button
            type="button"
            size="sm"
            disabled={!canDataAdmin}
            onClick={openCreateForm}
            data-testid="new-connection-btn"
          >
            <Plus className="h-3.5 w-3.5" />
            {tx("新建连接", "New Connection")}
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div
          className="rounded border border-red-500/30 bg-red-500/5 px-3 py-1.5 text-xs flex items-center justify-between"
          data-testid="connection-error-banner"
        >
          <span className="text-red-600 dark:text-red-400">{errorMsg}</span>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground cursor-pointer"
            onClick={() => setErrorMsg(null)}
          >
            {tx("关闭", "Dismiss")}
          </button>
        </div>
      )}

      {statusBanner && (
        <div
          className="rounded border border-xblue/30 bg-xblue-soft px-3 py-1.5 text-xs text-xblue flex items-center justify-between"
          data-testid="connection-status-banner"
        >
          <span>{statusBanner}</span>
          <button
            type="button"
            className="text-xblue hover:opacity-80 cursor-pointer"
            onClick={() => setStatusBanner(null)}
          >
            {tx("关闭", "Dismiss")}
          </button>
        </div>
      )}

      {/* Compact Single-Line Connection Strip */}
      {connections.length === 0 ? (
        <EmptyState
          className="py-5"
          title={tx("暂无数据源连接", "No data connections configured")}
          action={
            canDataAdmin ? (
              <Button size="sm" onClick={openCreateForm}>
                <Plus className="h-3.5 w-3.5" />
                {tx("新建连接", "New Connection")}
              </Button>
            ) : undefined
          }
        />
      ) : (
        <div
          className="divide-y divide-border rounded-md border border-border/80 bg-background"
          data-testid="connection-list"
        >
          {connections.map((conn) => {
            const testResult = testResults[conn.id];
            const specForConn = specs.find(
              (s) => s.connector_type === conn.connector_type
            );
            const nonRedundantMsg =
              !testResult &&
              conn.last_test_message &&
              conn.last_test_message.trim().toLowerCase() !== "connected"
                ? formatTestMessage(conn.last_test_message)
                : "";
            return (
              <div
                key={conn.id}
                className="flex flex-wrap items-center justify-between gap-3 px-3 py-2 text-xs hover:bg-muted/35 transition-colors"
                data-testid={`connection-card-${conn.name}`}
              >
                <div className="flex flex-wrap items-center gap-2 min-w-0">
                  <span className="font-mono-tech font-semibold text-foreground truncate">
                    {conn.name}
                  </span>
                  <div data-testid={`connection-status-${conn.name}`}>
                    {renderStatusBadge(conn.status)}
                  </div>
                  <span className="text-[11px] font-mono-tech text-muted-foreground">
                    {specForConn?.display_name ?? conn.connector_type}
                  </span>
                  {conn.secret_fields_set.length > 0 && (
                    <span className="text-[11px] text-muted-foreground">
                      · {tx("已加密", "Encrypted")} ({conn.secret_fields_set.join(", ")})
                    </span>
                  )}
                  {conn.allow_private_network && (
                    <Badge variant="outline">
                      {tx("私网互通", "Private Net")}
                    </Badge>
                  )}
                  {testResult && (
                    <span
                      className="inline-flex items-center gap-1 rounded bg-muted px-2 py-0.5 text-[11px] font-mono-tech"
                      data-testid={`conn-test-result-${conn.name}`}
                    >
                      {testResult.ok ? (
                        <CheckCircle2 className="h-3 w-3 text-emerald-600 dark:text-emerald-400 shrink-0" />
                      ) : (
                        <XCircle className="h-3 w-3 text-red-500 shrink-0" />
                      )}
                      <span>
                        {testResult.ok
                          ? `${fmtNum(testResult.latency_ms, 0)} ms`
                          : `${testResult.message || tx("失败", "Failed")} (${fmtNum(testResult.latency_ms, 0)} ms)`}
                      </span>
                    </span>
                  )}
                  {nonRedundantMsg && (
                    <span
                      className="text-[11px] text-muted-foreground truncate max-w-[220px]"
                      title={conn.last_test_message ?? undefined}
                    >
                      · {nonRedundantMsg}
                    </span>
                  )}
                </div>

                <div className="flex items-center gap-1 shrink-0">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={testingId === conn.id}
                    onClick={() => void handleTest(conn)}
                    className="text-xblue hover:text-xblue hover:bg-xblue-soft/70"
                    data-testid={`test-connection-btn-${conn.name}`}
                  >
                    {testingId === conn.id ? (
                      <RefreshCw className="h-3 w-3 animate-spin" />
                    ) : (
                      <Play className="h-3 w-3" />
                    )}
                    {tx("测试", "Test")}
                  </Button>

                  {(specForConn?.supports_discover ?? true) && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => void handleDiscover(conn)}
                      className="text-xblue hover:text-xblue hover:bg-xblue-soft/70"
                      data-testid={`discover-connection-btn-${conn.name}`}
                    >
                      <Compass className="h-3 w-3" />
                      {tx("探查", "Discover")}
                    </Button>
                  )}

                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={!canDataAdmin}
                    onClick={() => openEditForm(conn)}
                    data-testid={`edit-connection-btn-${conn.name}`}
                  >
                    <Pencil className="h-3 w-3" />
                    {tx("编辑", "Edit")}
                  </Button>

                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={!canDataAdmin}
                    onClick={() => setPendingDeleteConn(conn)}
                    aria-label={tx("删除", "Delete")}
                    className="hover:text-red-600 hover:bg-red-500/10"
                    data-testid={`delete-connection-btn-${conn.name}`}
                  >
                    <Trash2 className="h-3 w-3" />
                  </Button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {/* Dynamic Create / Edit Connection Modal */}
      <Modal
        open={formOpen}
        onClose={() => {
          setFormOpen(false);
          setEditingConn(null);
        }}
        maxWidthClass="max-w-2xl"
        title={
          editingConn
            ? `${tx("编辑连接", "Edit Connection")} · ${editingConn.name}`
            : tx("新建数据源连接", "New Data Connection")
        }
      >
        <form
          onSubmit={(e) => void handleSubmit(e)}
          className="space-y-4"
          data-testid="connection-form"
        >

          {!editingConn && (
            <div className="space-y-2">
              <label className="text-xs font-medium block">
                {tx("连接器类型", "Connector Type")}
              </label>
              <div className="space-y-2 max-h-48 overflow-y-auto rounded border border-border bg-background p-2.5">
                {groupedSpecs.map(([cat, catSpecs]) => (
                  <div key={cat} className="space-y-1">
                    <div className="text-[11px] font-medium text-muted-foreground">
                      {categoryLabel(cat)}
                    </div>
                    <div className="flex flex-wrap gap-1.5">
                      {catSpecs.map((spec) => {
                        const selected = spec.connector_type === selectedType;
                        return (
                          <button
                            key={spec.connector_type}
                            type="button"
                            onClick={() => setSelectedType(spec.connector_type)}
                            data-testid={`connector-type-option-${spec.connector_type}`}
                            className={`rounded border px-2 py-1 text-[11px] font-mono-tech transition-colors cursor-pointer ${
                              selected
                                ? "border-xblue bg-xblue-soft text-xblue font-medium"
                                : "border-border bg-background text-foreground hover:bg-muted"
                            }`}
                          >
                            {spec.display_name}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-xs font-medium block">
                {tx("连接名称", "Connection Name")} *
              </label>
              <Input
                value={connName}
                onChange={(e) => setConnName(e.target.value)}
                placeholder="demo_pg"
                data-testid="connection-name-input"
              />
            </div>

            <div className="space-y-1 flex flex-col justify-end">
              <label className="inline-flex items-center gap-2 text-xs cursor-pointer py-2">
                <input
                  type="checkbox"
                  checked={allowPrivateNetwork}
                  onChange={(e) => setAllowPrivateNetwork(e.target.checked)}
                  className="rounded border-border"
                  data-testid="allow-private-network-checkbox"
                />
                <span>
                  {tx("允许连接私有网络地址", "Allow private network addresses")}
                </span>
              </label>
            </div>
          </div>

          {/* Non-secret dynamic fields */}
          <div className="space-y-2">
            <div className="text-xs font-medium flex flex-wrap items-center justify-between gap-2">
              <span>
                {tx("连接参数", "Parameters")} · {activeSpec.display_name}
              </span>
              <span className="text-[11px] text-muted-foreground">
                {tx("支持模式", "Sync Modes")}:{" "}
                {activeSpec.supports_modes.map((m) => tEnum(m)).join(" / ")}
              </span>
            </div>

            {nonSecretEntries.length > 0 ? (
              <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                {nonSecretEntries.map(([field, prop]) => {
                  const isReq = requiredFields.has(field);
                  const val = configValues[field];
                  const labelTitle = prop.title?.trim() || field;
                  const showFieldCode =
                    Boolean(prop.title) &&
                    prop.title!.trim().toLowerCase() !==
                      field.trim().toLowerCase();

                  return (
                    <div key={field} className="space-y-1">
                      <label className="text-xs text-muted-foreground block">
                        {labelTitle}
                        {showFieldCode && (
                          <span className="ml-1 font-mono-tech text-[11px]">
                            ({field})
                          </span>
                        )}
                        {isReq ? " *" : ""}
                      </label>
                      {prop.enum && prop.enum.length > 0 ? (
                        <Select
                          value={String(val ?? "")}
                          onChange={(e) =>
                            setConfigValues((prev) => ({
                              ...prev,
                              [field]: e.target.value,
                            }))
                          }
                          className="w-full"
                          data-testid={`config-field-${field}`}
                        >
                          <option value="">-</option>
                          {prop.enum.map((opt) => (
                            <option key={String(opt)} value={String(opt)}>
                              {String(opt)}
                            </option>
                          ))}
                        </Select>
                      ) : prop.type === "boolean" ? (
                        <label className="inline-flex items-center gap-2 text-xs py-2 cursor-pointer">
                          <input
                            type="checkbox"
                            checked={Boolean(val)}
                            onChange={(e) =>
                              setConfigValues((prev) => ({
                                ...prev,
                                [field]: e.target.checked,
                              }))
                            }
                            data-testid={`config-field-${field}`}
                          />
                          <span>{labelTitle}</span>
                        </label>
                      ) : prop.type === "object" || prop.type === "array" ? (
                        <Textarea
                          rows={2}
                          value={
                            typeof val === "string"
                              ? val
                              : val !== undefined
                                ? JSON.stringify(val)
                                : ""
                          }
                          onChange={(e) =>
                            setConfigValues((prev) => ({
                              ...prev,
                              [field]: e.target.value,
                            }))
                          }
                          placeholder="{}"
                          className="font-mono-tech text-xs"
                          data-testid={`config-field-${field}`}
                        />
                      ) : (
                        <Input
                          type={
                            prop.type === "integer" || prop.type === "number"
                              ? "number"
                              : "text"
                          }
                          value={
                            val === undefined || val === null ? "" : String(val)
                          }
                          onChange={(e) =>
                            setConfigValues((prev) => ({
                              ...prev,
                              [field]: e.target.value,
                            }))
                          }
                          placeholder={
                            prop.default !== undefined
                              ? String(prop.default)
                              : field
                          }
                          data-testid={`config-field-${field}`}
                        />
                      )}
                    </div>
                  );
                })}
              </div>
            ) : (
              <div className="space-y-1">
                <label className="text-xs text-muted-foreground block">
                  {tx("参数 JSON", "Config JSON")}
                </label>
                <Textarea
                  rows={3}
                  value={rawJsonConfig}
                  onChange={(e) => setRawJsonConfig(e.target.value)}
                  className="font-mono-tech text-xs"
                  data-testid="config-raw-json"
                />
              </div>
            )}
          </div>

          {/* Secret fields section */}
          {activeSpec.secret_fields.length > 0 && (
            <div className="space-y-2 rounded border border-border bg-background p-3">
              <div className="text-xs font-medium flex items-center gap-1.5">
                <Lock className="h-3.5 w-3.5 text-xblue" />
                <span>{tx("加密凭证", "Encrypted Credentials")}</span>
              </div>
              <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                {activeSpec.secret_fields.map((sf) => {
                  const alreadySet =
                    editingConn?.secret_fields_set.includes(sf) ?? false;
                  const isCleared = Boolean(clearedSecrets[sf]);
                  return (
                    <div key={sf} className="space-y-1">
                      <div className="flex items-center justify-between text-xs">
                        <span className="font-mono-tech">{sf}</span>
                        {alreadySet && !isCleared && (
                          <Badge
                            variant="xblue"
                            data-testid={`secret-set-badge-${sf}`}
                          >
                            {tx("已设置", "Configured")}
                          </Badge>
                        )}
                      </div>
                      <Input
                        type="password"
                        autoComplete="new-password"
                        disabled={isCleared}
                        value={secretInputs[sf] ?? ""}
                        onChange={(e) =>
                          setSecretInputs((prev) => ({
                            ...prev,
                            [sf]: e.target.value,
                          }))
                        }
                        placeholder={
                          alreadySet
                            ? tx(
                                "已设置（留空保持不变）",
                                "Configured (leave blank to keep)"
                              )
                            : "••••••••"
                        }
                        data-testid={`secret-field-${sf}`}
                      />
                      {editingConn && alreadySet && (
                        <label className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground cursor-pointer">
                          <input
                            type="checkbox"
                            checked={isCleared}
                            onChange={(e) =>
                              setClearedSecrets((prev) => ({
                                ...prev,
                                [sf]: e.target.checked,
                              }))
                            }
                          />
                          <span>
                            {tx(`清除 ${sf}`, `Clear ${sf}`)}
                          </span>
                        </label>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          <div className="flex justify-end gap-2 pt-2 border-t border-border">
            <Button
              type="button"
              variant="outline"
              onClick={() => {
                setFormOpen(false);
                setEditingConn(null);
              }}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="submit"
              disabled={submitting || !canDataAdmin}
              data-testid="connection-submit-btn"
            >
              {submitting ? tx("保存中…", "Saving…") : tx("保存", "Save")}
            </Button>
          </div>
        </form>
      </Modal>

      {/* Discovered Objects Drawer */}
      <Drawer
        open={Boolean(discoveringConn)}
        onClose={() => {
          setDiscoveringConn(null);
          setSourcePreview(null);
        }}
        widthClass="max-w-3xl"
        testId="discover-panel"
        title={
          discoveringConn
            ? `${tx("源端对象探查", "Discovered Objects")} · ${discoveringConn.name}`
            : tx("源端对象探查", "Discovered Objects")
        }
      >
        {discoveringConn && (
          <div className="space-y-3">

          {discoverLoading ? (
            <div className="text-xs text-muted-foreground py-3">
              {tx("探查中…", "Discovering…")}
            </div>
          ) : discoveredObjects.length === 0 ? (
            <div className="text-xs text-muted-foreground py-2">
              {tx("未发现可导入对象", "No importable objects found")}
            </div>
          ) : (
            <div className="space-y-2">
              {discoveredObjects.map((obj, idx) => {
                const connSpec = specs.find(
                  (s) => s.connector_type === discoveringConn.connector_type
                );
                const supportedModes = connSpec?.supports_modes ?? [
                  "SNAPSHOT",
                  "APPEND",
                  "UPSERT",
                ];
                return (
                  <div
                    key={`${obj.display_name}-${idx}`}
                    className="rounded border border-border bg-background p-2.5 space-y-2 text-xs"
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono-tech font-medium">
                          {obj.display_name}
                        </span>
                        <Badge variant="outline">
                          {formatSourceKind(obj.source_kind)}
                        </Badge>
                        <span className="text-muted-foreground font-mono-tech">
                          {fmtNum(obj.columns.length, 0)} {tx("列", "cols")}
                          {obj.estimated_rows !== null
                            ? ` · ~${fmtNum(obj.estimated_rows, 0)} ${tx("行", "rows")}`
                            : ""}
                        </span>
                      </div>
                      <div className="flex items-center gap-1.5">
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={() =>
                            void handlePreviewObject(discoveringConn, obj)
                          }
                        >
                          <Eye className="h-3 w-3" />
                          {tx("预览", "Preview")}
                        </Button>
                        <Button
                          type="button"
                          variant="primary"
                          size="sm"
                          disabled={!canDataSync && !canDataAdmin}
                          onClick={() => {
                            setQuickCreateIdx(idx);
                            const normalized = obj.display_name
                              .toLowerCase()
                              .replace(/[^a-z0-9_]+/g, "_")
                              .replace(/^[^a-z]+/, "ds_");
                            setNewDatasetName(normalized.slice(0, 48));
                            setNewDatasetSyncMode(
                              supportedModes[0] ?? "SNAPSHOT"
                            );
                          }}
                        >
                          <Plus className="h-3 w-3" />
                          {tx("注册数据集", "Register")}
                        </Button>
                      </div>
                    </div>

                    {quickCreateIdx === idx && (
                      <div className="grid grid-cols-1 md:grid-cols-4 gap-2 pt-2 border-t border-border">
                        <Input
                          value={newDatasetName}
                          onChange={(e) => setNewDatasetName(e.target.value)}
                          placeholder="demo_orders"
                        />
                        <Select
                          value={newDatasetSyncMode}
                          onChange={(e) =>
                            setNewDatasetSyncMode(e.target.value as SyncMode)
                          }
                        >
                          {(["SNAPSHOT", "APPEND", "UPSERT"] as const).map(
                            (m) => (
                              <option
                                key={m}
                                value={m}
                                disabled={!supportedModes.includes(m)}
                              >
                                {tEnum(m)}
                              </option>
                            )
                          )}
                        </Select>
                        <Select
                          value={newDatasetPolicy}
                          onChange={(e) =>
                            setNewDatasetPolicy(e.target.value as SchemaPolicy)
                          }
                        >
                          <option value="ADDITIVE">{tEnum("ADDITIVE")}</option>
                          <option value="STRICT">{tEnum("STRICT")}</option>
                          <option value="EVOLVE">{tEnum("EVOLVE")}</option>
                        </Select>
                        <div className="flex gap-1.5">
                          <Input
                            value={newDatasetCron}
                            onChange={(e) => setNewDatasetCron(e.target.value)}
                            placeholder="0 */6 * * *"
                          />
                          <Button
                            type="button"
                            onClick={() =>
                              void handleCreateDatasetFromObject(obj)
                            }
                          >
                            {tx("保存", "Save")}
                          </Button>
                        </div>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}

          {sourcePreview && (
            <div className="space-y-1.5 pt-2 border-t border-border">
              <div className="text-xs font-medium">
                {tx("源表预览", "Source Preview")} · {sourcePreview.title}
              </div>
              <div className="max-h-60 overflow-auto rounded border border-border bg-background">
                <table className="w-full text-left text-xs">
                  <thead className="sticky top-0 bg-muted border-b border-border">
                    <tr>
                      {sourcePreview.data.columns.map((col) => (
                        <th
                          key={col.name}
                          className="px-2.5 py-1.5 font-mono-tech font-medium whitespace-nowrap"
                        >
                          {col.name}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {sourcePreview.data.rows.slice(0, 10).map((row, rIdx) => (
                      <tr
                        key={rIdx}
                        className="border-b border-border last:border-0"
                      >
                        {sourcePreview.data.columns.map((col) => (
                          <td
                            key={col.name}
                            className="px-2.5 py-1.5 font-mono-tech whitespace-nowrap"
                          >
                            {String(row[col.name] ?? "")}
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
          </div>
        )}
      </Drawer>

      <ConfirmDialog
        open={Boolean(pendingDeleteConn)}
        onClose={() => setPendingDeleteConn(null)}
        onConfirm={() => void handleConfirmDelete()}
        loading={deletingConn}
        title={tx("删除数据源连接", "Delete Connection")}
        description={tx(
          `确定删除连接「${pendingDeleteConn?.name ?? ""}」及其加密凭证？`,
          `Delete connection "${pendingDeleteConn?.name ?? ""}" and its encrypted credentials?`
        )}
      />
    </Card>
  );
}
