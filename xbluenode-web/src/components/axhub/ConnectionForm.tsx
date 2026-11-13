import { useEffect, useMemo, useState } from "react";
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
import {
  axhubApi,
  type Connection,
  type ConnectionTestResult,
  type ConnectorSpec,
  type DiscoveredObject,
  type PreviewResponse,
  type SchemaPolicy,
} from "@/lib/api/axhub";
import type { ConnectorType, SyncMode } from "@/lib/api/enums";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";

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

const CATEGORY_LABELS: Record<string, string> = {
  FILE: "文件与对象存储",
  RELATIONAL: "关系型数据库",
  DOMESTIC: "国产数据库",
  ANALYTIC: "分析型数据库",
  NOSQL: "NoSQL 与图数据库",
  TIMESERIES: "时序数据库",
  STREAM: "消息流",
  API: "API 与 Webhook",
};

const FALLBACK_SPECS: ConnectorSpec[] = [
  {
    connector_type: "POSTGRES",
    display_name: "PostgreSQL",
    category: "RELATIONAL",
    config_schema: {
      properties: {
        host: { type: "string", title: "主机地址", default: "127.0.0.1" },
        port: { type: "integer", title: "端口", default: 5432 },
        database: { type: "string", title: "数据库名" },
        schema: { type: "string", title: "模式名", default: "public" },
        username: { type: "string", title: "用户名" },
        sslmode: {
          type: "string",
          title: "SSL 模式",
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
  const specs = connectorSpecs.length > 0 ? connectorSpecs : FALLBACK_SPECS;

  const [formOpen, setFormOpen] = useState(false);
  const [editingConn, setEditingConn] = useState<Connection | null>(null);
  const [selectedType, setSelectedType] = useState<ConnectorType>("POSTGRES");
  const [connName, setConnName] = useState("");
  const [allowPrivateNetwork, setAllowPrivateNetwork] = useState(false);
  const [configValues, setConfigValues] = useState<Record<string, unknown>>({});
  const [rawJsonConfig, setRawJsonConfig] = useState("{}");
  const [secretInputs, setSecretInputs] = useState<Record<string, string>>({});
  const [clearedSecrets, setClearedSecrets] = useState<Record<string, boolean>>({});
  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [statusBanner, setStatusBanner] = useState<string | null>(null);

  const [testingId, setTestingId] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<
    Record<string, ConnectionTestResult>
  >({});

  const [discoveringConn, setDiscoveringConn] = useState<Connection | null>(null);
  const [discoveredObjects, setDiscoveredObjects] = useState<DiscoveredObject[]>([]);
  const [discoverLoading, setDiscoverLoading] = useState(false);
  const [sourcePreview, setSourcePreview] = useState<{
    title: string;
    data: PreviewResponse;
  } | null>(null);
  const [quickCreateIdx, setQuickCreateIdx] = useState<number | null>(null);
  const [newDatasetName, setNewDatasetName] = useState("");
  const [newDatasetSyncMode, setNewDatasetSyncMode] = useState<SyncMode>("SNAPSHOT");
  const [newDatasetPolicy, setNewDatasetPolicy] = useState<SchemaPolicy>("ADDITIVE");
  const [newDatasetCron, setNewDatasetCron] = useState("");

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
    // Never prefill secret values; leave empty so unchanged secrets stay intact
    setSecretInputs({});
    setClearedSecrets({});
    setErrorMsg(null);
    setFormOpen(true);
  };

  const buildConfigObject = (): Record<string, unknown> => {
    if (nonSecretEntries.length === 0) {
      const parsed: unknown = JSON.parse(rawJsonConfig || "{}");
      if (!isRecord(parsed)) {
        throw new Error("配置 JSON 必须是对象结构");
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
    if (!connName.trim()) {
      setErrorMsg("请输入连接名称");
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
        setStatusBanner(`已更新连接「${connName.trim()}」`);
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
        setStatusBanner(`已创建连接「${connName.trim()}」`);
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

  const handleDelete = async (conn: Connection) => {
    setErrorMsg(null);
    try {
      await axhubApi.deleteConnection(conn.id);
      setStatusBanner(`已删除连接「${conn.name}」`);
      if (editingConn?.id === conn.id) {
        setFormOpen(false);
        setEditingConn(null);
      }
      await onChanged();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
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

  const handlePreviewObject = async (conn: Connection, obj: DiscoveredObject) => {
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
    if (!discoveringConn || !newDatasetName.trim()) return;
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
      setStatusBanner(`已创建数据集「${newDatasetName.trim()}」`);
      setQuickCreateIdx(null);
      if (onDatasetCreated) {
        await onDatasetCreated();
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleRotateCredentials = async () => {
    setErrorMsg(null);
    try {
      const res = await axhubApi.rotateCredentials();
      setStatusBanner(`已提交密钥环轮换任务 (${res.job_id})`);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const renderStatusBadge = (status: Connection["status"]) => {
    if (status === "OK") {
      return <Badge variant="xblue">OK</Badge>;
    }
    if (status === "FAILED") {
      return (
        <Badge variant="outline" className="text-foreground border-foreground/40">
          FAILED
        </Badge>
      );
    }
    return <Badge variant="neutral">UNTESTED</Badge>;
  };

  return (
    <Card className="p-4 space-y-4" data-testid="connections-section">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
        <div>
          <h2 className="text-sm font-semibold">数据源连接器管理</h2>
          <p className="text-xs text-muted-foreground">
            支持 {specs.length} 种连接器规格，秘密凭证通过 AES-256-GCM 密钥环加密托管，绝不回显明文
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => void handleRotateCredentials()}
            data-testid="rotate-credentials-btn"
          >
            <KeyRound className="h-3.5 w-3.5" />
            轮换密钥
          </Button>
          <Button
            type="button"
            onClick={openCreateForm}
            data-testid="new-connection-btn"
          >
            <Plus className="h-3.5 w-3.5" />
            新建连接
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div
          className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs flex items-center justify-between"
          data-testid="connection-error-banner"
        >
          <span>{errorMsg}</span>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground cursor-pointer"
            onClick={() => setErrorMsg(null)}
          >
            关闭
          </button>
        </div>
      )}

      {statusBanner && (
        <div
          className="rounded border border-xblue/30 bg-xblue-soft px-3 py-2 text-xs text-xblue flex items-center justify-between"
          data-testid="connection-status-banner"
        >
          <span>{statusBanner}</span>
          <button
            type="button"
            className="text-xblue hover:opacity-80 cursor-pointer"
            onClick={() => setStatusBanner(null)}
          >
            关闭
          </button>
        </div>
      )}

      {/* Existing Connections List */}
      {connections.length === 0 ? (
        <div className="rounded border border-dashed border-border py-6 text-center text-xs text-muted-foreground">
          暂无已配置的数据源连接，点击右上角「新建连接」接入外部数据源。
        </div>
      ) : (
        <div
          className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3"
          data-testid="connection-list"
        >
          {connections.map((conn) => {
            const testResult = testResults[conn.id];
            const specForConn = specs.find(
              (s) => s.connector_type === conn.connector_type
            );
            return (
              <div
                key={conn.id}
                className="rounded-md border border-border p-3 space-y-2.5 bg-background"
                data-testid={`connection-card-${conn.name}`}
              >
                <div className="flex items-start justify-between gap-2">
                  <div>
                    <div className="text-xs font-semibold flex items-center gap-1.5">
                      <span>{conn.name}</span>
                      {conn.allow_private_network && (
                        <Badge variant="outline">私网允许</Badge>
                      )}
                    </div>
                    <div className="text-[11px] font-mono-tech text-muted-foreground mt-0.5">
                      {conn.connector_type}
                      {conn.secret_fields_set.length > 0 && (
                        <span className="ml-2">
                          · 已加密字段: {conn.secret_fields_set.join(", ")}
                        </span>
                      )}
                    </div>
                  </div>
                  <div data-testid={`connection-status-${conn.name}`}>
                    {renderStatusBadge(conn.status)}
                  </div>
                </div>

                {testResult && (
                  <div
                    className="rounded bg-muted px-2.5 py-1.5 text-[11px] font-mono-tech flex items-center justify-between"
                    data-testid={`conn-test-result-${conn.name}`}
                  >
                    <span className="flex items-center gap-1">
                      {testResult.ok ? (
                        <CheckCircle2 className="h-3.5 w-3.5 text-xblue" />
                      ) : (
                        <XCircle className="h-3.5 w-3.5 text-muted-foreground" />
                      )}
                      {testResult.ok ? "连通正常" : testResult.message || "测试失败"}
                    </span>
                    <Badge variant={testResult.ok ? "xblue" : "outline"}>
                      {testResult.latency_ms} ms
                    </Badge>
                  </div>
                )}

                {!testResult && conn.last_test_message && (
                  <div className="text-[11px] text-muted-foreground truncate">
                    {conn.last_test_message}
                  </div>
                )}

                <div className="flex flex-wrap items-center gap-1.5 pt-1 border-t border-border">
                  <Button
                    type="button"
                    variant="outline"
                    className="px-2.5 py-1 text-[11px]"
                    disabled={testingId === conn.id}
                    onClick={() => void handleTest(conn)}
                    data-testid={`test-connection-btn-${conn.name}`}
                  >
                    {testingId === conn.id ? (
                      <RefreshCw className="h-3 w-3 animate-spin" />
                    ) : (
                      <Play className="h-3 w-3" />
                    )}
                    测试连接
                  </Button>

                  {(specForConn?.supports_discover ?? true) && (
                    <Button
                      type="button"
                      variant="outline"
                      className="px-2.5 py-1 text-[11px]"
                      onClick={() => void handleDiscover(conn)}
                      data-testid={`discover-connection-btn-${conn.name}`}
                    >
                      <Compass className="h-3 w-3" />
                      对象发现
                    </Button>
                  )}

                  <Button
                    type="button"
                    variant="ghost"
                    className="px-2 py-1 text-[11px]"
                    onClick={() => openEditForm(conn)}
                    data-testid={`edit-connection-btn-${conn.name}`}
                  >
                    <Pencil className="h-3 w-3" />
                    编辑
                  </Button>

                  <Button
                    type="button"
                    variant="ghost"
                    className="px-2 py-1 text-[11px]"
                    onClick={() => void handleDelete(conn)}
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

      {/* Dynamic Create / Edit Connection Form */}
      {formOpen && (
        <form
          onSubmit={(e) => void handleSubmit(e)}
          className="rounded-md border border-border bg-muted/30 p-4 space-y-4"
          data-testid="connection-form"
        >
          <div className="flex items-center justify-between border-b border-border pb-2.5">
            <h3 className="text-xs font-semibold">
              {editingConn
                ? `编辑连接 · ${editingConn.name}`
                : "新建数据源连接（按 config_schema 动态渲染）"}
            </h3>
            <Button
              type="button"
              variant="ghost"
              className="px-2 py-1 text-xs"
              onClick={() => {
                setFormOpen(false);
                setEditingConn(null);
              }}
            >
              取消
            </Button>
          </div>

          {!editingConn && (
            <div className="space-y-2">
              <label className="text-xs font-medium block">
                1. 选择连接器类型（共 {specs.length} 种）
              </label>
              <div className="space-y-2 max-h-48 overflow-y-auto rounded border border-border bg-background p-2.5">
                {groupedSpecs.map(([cat, catSpecs]) => (
                  <div key={cat} className="space-y-1">
                    <div className="text-[11px] font-mono-tech text-muted-foreground">
                      {CATEGORY_LABELS[cat] ?? cat}
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
                            {spec.display_name} ({spec.connector_type})
                            {spec.image_tier === "full" ? " [full]" : ""}
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
              <label className="text-xs font-medium block">连接名称 *</label>
              <Input
                value={connName}
                onChange={(e) => setConnName(e.target.value)}
                placeholder="例如 demo_pg"
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
                允许连接私有网络地址（DATA_ADMIN）
              </label>
            </div>
          </div>

          {/* Non-secret dynamic fields from config_schema */}
          <div className="space-y-2">
            <div className="text-xs font-medium flex items-center justify-between">
              <span>连接参数（{activeSpec.connector_type}）</span>
              <span className="text-[11px] font-mono-tech text-muted-foreground">
                支持同步模式: {activeSpec.supports_modes.join(" / ")}
              </span>
            </div>

            {nonSecretEntries.length > 0 ? (
              <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                {nonSecretEntries.map(([field, prop]) => {
                  const isReq = requiredFields.has(field);
                  const val = configValues[field];
                  return (
                    <div key={field} className="space-y-1">
                      <label className="text-xs text-muted-foreground block">
                        {prop.title ?? field}{" "}
                        <span className="font-mono-tech">({field})</span>
                        {isReq ? " *" : ""}
                      </label>
                      {prop.enum && prop.enum.length > 0 ? (
                        <select
                          value={String(val ?? "")}
                          onChange={(e) =>
                            setConfigValues((prev) => ({
                              ...prev,
                              [field]: e.target.value,
                            }))
                          }
                          className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-xs"
                          data-testid={`config-field-${field}`}
                        >
                          <option value="">请选择</option>
                          {prop.enum.map((opt) => (
                            <option key={String(opt)} value={String(opt)}>
                              {String(opt)}
                            </option>
                          ))}
                        </select>
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
                          启用 {prop.title ?? field}
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
                          placeholder={prop.description ?? "JSON 结构"}
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
                          value={val === undefined || val === null ? "" : String(val)}
                          onChange={(e) =>
                            setConfigValues((prev) => ({
                              ...prev,
                              [field]: e.target.value,
                            }))
                          }
                          placeholder={prop.description ?? field}
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
                  连接配置 JSON
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

          {/* Secret fields section: NEVER echoes plaintext */}
          {activeSpec.secret_fields.length > 0 && (
            <div className="space-y-2 rounded border border-border bg-background p-3">
              <div className="text-xs font-medium flex items-center gap-1.5">
                <Lock className="h-3.5 w-3.5 text-xblue" />
                加密秘密字段（仅单向提交至 Keyring，不回显明文）
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
                            已设置
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
                            ? "已设置（留空保持不变）"
                            : `输入 ${sf} 秘密值`
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
                          清除已保存的 {sf}
                        </label>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          <div className="flex justify-end gap-2">
            <Button
              type="submit"
              disabled={submitting}
              data-testid="connection-submit-btn"
            >
              {submitting
                ? "保存中..."
                : editingConn
                  ? "保存连接变更"
                  : "创建连接"}
            </Button>
          </div>
        </form>
      )}

      {/* Discovered Objects Panel */}
      {discoveringConn && (
        <div
          className="rounded-md border border-border p-3 space-y-3 bg-muted/20"
          data-testid="discover-panel"
        >
          <div className="flex items-center justify-between">
            <div className="text-xs font-semibold">
              源端对象发现 · {discoveringConn.name}
            </div>
            <Button
              type="button"
              variant="ghost"
              className="px-2 py-1 text-xs"
              onClick={() => {
                setDiscoveringConn(null);
                setSourcePreview(null);
              }}
            >
              收起
            </Button>
          </div>

          {discoverLoading ? (
            <div className="text-xs text-muted-foreground py-3">
              正在探查源端表 / 集合 / 主题结构...
            </div>
          ) : discoveredObjects.length === 0 ? (
            <div className="text-xs text-muted-foreground py-2">
              未发现可导入对象。
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
                      <div className="flex items-center gap-2">
                        <span className="font-mono-tech font-medium">
                          {obj.display_name}
                        </span>
                        <Badge variant="outline">{obj.source_kind}</Badge>
                        <span className="text-muted-foreground">
                          {obj.columns.length} 列
                          {obj.estimated_rows !== null
                            ? ` · 约 ${obj.estimated_rows} 行`
                            : ""}
                        </span>
                      </div>
                      <div className="flex items-center gap-1.5">
                        <Button
                          type="button"
                          variant="outline"
                          className="px-2 py-1 text-[11px]"
                          onClick={() =>
                            void handlePreviewObject(discoveringConn, obj)
                          }
                        >
                          <Eye className="h-3 w-3" />
                          源表预览
                        </Button>
                        <Button
                          type="button"
                          variant="primary"
                          className="px-2.5 py-1 text-[11px]"
                          onClick={() => {
                            setQuickCreateIdx(idx);
                            const normalized = obj.display_name
                              .toLowerCase()
                              .replace(/[^a-z0-9_]+/g, "_")
                              .replace(/^[^a-z]+/, "ds_");
                            setNewDatasetName(normalized.slice(0, 48));
                            setNewDatasetSyncMode(supportedModes[0] ?? "SNAPSHOT");
                          }}
                        >
                          <Plus className="h-3 w-3" />
                          注册为数据集
                        </Button>
                      </div>
                    </div>

                    {quickCreateIdx === idx && (
                      <div className="grid grid-cols-1 md:grid-cols-4 gap-2 pt-2 border-t border-border">
                        <Input
                          value={newDatasetName}
                          onChange={(e) => setNewDatasetName(e.target.value)}
                          placeholder="数据集标识 (^[a-z][a-z0-9_]*$)"
                        />
                        <select
                          value={newDatasetSyncMode}
                          onChange={(e) =>
                            setNewDatasetSyncMode(e.target.value as SyncMode)
                          }
                          className="h-9 rounded-md border border-input bg-background px-2 text-xs"
                        >
                          {(["SNAPSHOT", "APPEND", "UPSERT"] as const).map(
                            (m) => (
                              <option
                                key={m}
                                value={m}
                                disabled={!supportedModes.includes(m)}
                              >
                                {m}
                                {!supportedModes.includes(m) ? " (不支持)" : ""}
                              </option>
                            )
                          )}
                        </select>
                        <select
                          value={newDatasetPolicy}
                          onChange={(e) =>
                            setNewDatasetPolicy(e.target.value as SchemaPolicy)
                          }
                          className="h-9 rounded-md border border-input bg-background px-2 text-xs"
                        >
                          <option value="ADDITIVE">Schema: ADDITIVE</option>
                          <option value="STRICT">Schema: STRICT</option>
                          <option value="EVOLVE">Schema: EVOLVE</option>
                        </select>
                        <div className="flex gap-1.5">
                          <Input
                            value={newDatasetCron}
                            onChange={(e) => setNewDatasetCron(e.target.value)}
                            placeholder="Cron (可选)"
                          />
                          <Button
                            type="button"
                            onClick={() =>
                              void handleCreateDatasetFromObject(obj)
                            }
                          >
                            确定
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
                源数据抽样预览 · {sourcePreview.title}
              </div>
              <div className="overflow-x-auto rounded border border-border bg-background">
                <table className="w-full text-left text-xs">
                  <thead className="bg-muted border-b border-border">
                    <tr>
                      {sourcePreview.data.columns.map((col) => (
                        <th
                          key={col.name}
                          className="px-2.5 py-1.5 font-mono-tech font-medium"
                        >
                          {col.name}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {sourcePreview.data.rows.slice(0, 5).map((row, rIdx) => (
                      <tr key={rIdx} className="border-b border-border last:border-0">
                        {sourcePreview.data.columns.map((col) => (
                          <td
                            key={col.name}
                            className="px-2.5 py-1.5 font-mono-tech"
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
    </Card>
  );
}
