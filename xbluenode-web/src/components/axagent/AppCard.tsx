import { useMemo, useState } from "react";
import {
  Check,
  Copy,
  Download,
  Play,
  RefreshCw,
  Terminal,
  X,
} from "lucide-react";
import {
  getPublishedRunStatus,
  invokePublishedEndpoint,
  type EndpointSpec,
} from "@/lib/api/axagent";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";

interface SchemaPropertySpec {
  key: string;
  type: string;
  description: string;
  required: boolean;
  enumValues: string[] | null;
  itemEnumValues: string[] | null;
  format: string | null;
  defaultValue: unknown;
}

function extractSchemaProperties(
  schema: Record<string, unknown>
): SchemaPropertySpec[] {
  const propsRaw = schema.properties;
  if (
    typeof propsRaw !== "object" ||
    propsRaw === null ||
    Array.isArray(propsRaw)
  ) {
    return [];
  }
  const reqSet = new Set<string>(
    Array.isArray(schema.required)
      ? schema.required.filter((x): x is string => typeof x === "string")
      : []
  );

  const out: SchemaPropertySpec[] = [];
  for (const [key, defRaw] of Object.entries(
    propsRaw as Record<string, unknown>
  )) {
    if (typeof defRaw !== "object" || defRaw === null || Array.isArray(defRaw)) {
      continue;
    }
    const def = defRaw as Record<string, unknown>;
    const rawType = def.type;
    const resolvedType =
      typeof rawType === "string"
        ? rawType
        : Array.isArray(rawType)
          ? (rawType.find((t): t is string => t !== "null" && typeof t === "string") ??
            "string")
          : "string";

    const enumValues = Array.isArray(def.enum)
      ? def.enum.map((v) => String(v))
      : null;

    let itemEnumValues: string[] | null = null;
    if (
      resolvedType === "array" &&
      typeof def.items === "object" &&
      def.items !== null &&
      !Array.isArray(def.items) &&
      Array.isArray((def.items as { enum?: unknown }).enum)
    ) {
      itemEnumValues = ((def.items as { enum: unknown[] }).enum).map((v) =>
        String(v)
      );
    }

    out.push({
      key,
      type: resolvedType,
      description: typeof def.description === "string" ? def.description : "",
      required: reqSet.has(key),
      enumValues,
      itemEnumValues,
      format: typeof def.format === "string" ? def.format : null,
      defaultValue: def.default,
    });
  }
  return out;
}

function buildCurlCommand(endpoint: EndpointSpec, patToken?: string | null): string {
  const origin =
    typeof window !== "undefined" ? window.location.origin : "http://127.0.0.1:9995";
  const envPath = endpoint.environment.toLowerCase();
  const tokenPlaceholder =
    patToken && patToken.startsWith("axp_") ? patToken : "axp_<YOUR_PAT_TOKEN>";
  return `curl -X POST "${origin}/api/published/${envPath}/${endpoint.tenant_id}/${endpoint.slug}" \\\n  -H "Authorization: Bearer ${tokenPlaceholder}" \\\n  -H "Content-Type: application/json" \\\n  -d '{}'`;
}

function downloadRowsAsCsv(slug: string, rows: Record<string, unknown>[]) {
  if (rows.length === 0) return;
  const colSet = new Set<string>();
  for (const r of rows) {
    for (const k of Object.keys(r)) colSet.add(k);
  }
  const cols = Array.from(colSet);
  const escapeCsv = (val: unknown): string => {
    if (val === null || val === undefined) return "";
    const str = typeof val === "object" ? JSON.stringify(val) : String(val);
    if (/[",\n\r]/.test(str)) {
      return `"${str.replace(/"/g, '""')}"`;
    }
    return str;
  };
  const lines = [
    cols.map(escapeCsv).join(","),
    ...rows.map((r) => cols.map((c) => escapeCsv(r[c])).join(",")),
  ];
  const blob = new Blob([lines.join("\n")], { type: "text/csv;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = `${slug}.csv`;
  link.click();
  URL.revokeObjectURL(url);
}

export interface AppCardProps {
  endpoint: EndpointSpec;
  activePat?: string | null;
}

export function AppCard({ endpoint, activePat = null }: AppCardProps) {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [copiedCurl, setCopiedCurl] = useState(false);

  const schemaFields = useMemo(
    () => extractSchemaProperties(endpoint.input_schema),
    [endpoint.input_schema]
  );

  const [formValues, setFormValues] = useState<Record<string, unknown>>(() => {
    const initial: Record<string, unknown> = {};
    for (const f of extractSchemaProperties(endpoint.input_schema)) {
      if (f.defaultValue !== undefined) {
        initial[f.key] = f.defaultValue;
      } else if (f.enumValues && f.enumValues.length > 0) {
        initial[f.key] = f.enumValues[0];
      } else if (f.type === "boolean") {
        initial[f.key] = false;
      } else if (f.type === "array" && f.itemEnumValues) {
        initial[f.key] = [];
      }
    }
    return initial;
  });

  const [rawJsonInput, setRawJsonInput] = useState("{}");
  const [patInput, setPatInput] = useState(activePat ?? "");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<Record<string, unknown> | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const handleCopyCurl = async () => {
    try {
      await navigator.clipboard.writeText(buildCurlCommand(endpoint, activePat));
      setCopiedCurl(true);
      setTimeout(() => setCopiedCurl(false), 1800);
    } catch {
      // Ignore in restricted clipboard environments.
    }
  };

  const handleRun = async () => {
    setErrorMsg(null);
    let payload: Record<string, unknown> = {};

    if (schemaFields.length === 0) {
      try {
        const parsed: unknown = JSON.parse(rawJsonInput.trim() || "{}");
        if (
          typeof parsed !== "object" ||
          parsed === null ||
          Array.isArray(parsed)
        ) {
          setErrorMsg("请求体必须是 JSON 对象");
          return;
        }
        payload = parsed as Record<string, unknown>;
      } catch {
        setErrorMsg("JSON 入参格式有误");
        return;
      }
    } else {
      for (const field of schemaFields) {
        const raw = formValues[field.key];
        if (raw === undefined || raw === "") {
          continue;
        }
        if (field.type === "number" || field.type === "integer") {
          const num = Number(raw);
          if (!Number.isNaN(num)) {
            payload[field.key] = num;
          }
        } else if (field.type === "boolean") {
          payload[field.key] = Boolean(raw);
        } else if (field.type === "array") {
          if (Array.isArray(raw)) {
            payload[field.key] = raw;
          } else if (typeof raw === "string") {
            payload[field.key] = raw
              .split(",")
              .map((s) => s.trim())
              .filter((s) => s.length > 0);
          }
        } else if (field.type === "object") {
          if (typeof raw === "string" && raw.trim().length > 0) {
            try {
              const obj: unknown = JSON.parse(raw.trim());
              if (typeof obj === "object" && obj !== null && !Array.isArray(obj)) {
                payload[field.key] = obj;
              }
            } catch {
              setErrorMsg(`字段 ${field.key} 不是合法 JSON 对象`);
              return;
            }
          } else if (typeof raw === "object" && raw !== null) {
            payload[field.key] = raw;
          }
        } else {
          payload[field.key] = String(raw);
        }
      }
    }

    setRunning(true);
    try {
      const effectivePat = patInput.trim() || activePat || null;
      const resp = await invokePublishedEndpoint(endpoint, payload, {
        patToken: effectivePat,
      });
      setResult(resp);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "调用应用失败");
    } finally {
      setRunning(false);
    }
  };

  const handleRefreshRunStatus = async (runId: string) => {
    const effectivePat = patInput.trim() || activePat || "";
    if (!effectivePat.startsWith("axp_")) return;
    setRunning(true);
    try {
      const statusResp = await getPublishedRunStatus(
        endpoint,
        runId,
        effectivePat
      );
      setResult(statusResp);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "刷新运行状态失败");
    } finally {
      setRunning(false);
    }
  };

  const resultRows: Record<string, unknown>[] | null = useMemo(() => {
    if (!result) return null;
    if (Array.isArray(result.rows)) {
      return result.rows.filter(
        (r): r is Record<string, unknown> =>
          typeof r === "object" && r !== null && !Array.isArray(r)
      );
    }
    return null;
  }, [result]);

  const resultObjects: Record<string, unknown>[] | null = useMemo(() => {
    if (!result) return null;
    if (Array.isArray(result.items)) {
      return result.items.filter(
        (r): r is Record<string, unknown> =>
          typeof r === "object" && r !== null && !Array.isArray(r)
      );
    }
    return null;
  }, [result]);

  const rowColumns = useMemo(() => {
    if (!resultRows || resultRows.length === 0) return [];
    const cols = new Set<string>();
    for (const r of resultRows.slice(0, 20)) {
      for (const k of Object.keys(r)) cols.add(k);
    }
    return Array.from(cols);
  }, [resultRows]);

  return (
    <>
      <Card
        data-testid={`app-card-${endpoint.slug}`}
        className="p-4 flex flex-col justify-between gap-4 hover:border-xblue/50 transition-colors"
      >
        <div className="space-y-2">
          <div className="flex items-start justify-between gap-2">
            <div>
              <h3 className="text-sm font-semibold text-foreground">
                {endpoint.title}
              </h3>
              <p className="text-[11px] font-mono-tech text-muted-foreground">
                /{endpoint.environment.toLowerCase()}/{endpoint.tenant_id}/
                {endpoint.slug}
              </p>
            </div>
            <div className="flex items-center gap-1 shrink-0">
              <Badge
                variant={endpoint.environment === "PROD" ? "xblue" : "neutral"}
              >
                {endpoint.environment}
              </Badge>
              <Badge variant="outline">{endpoint.kind}</Badge>
            </div>
          </div>

          <p className="text-xs text-muted-foreground line-clamp-2 min-h-[32px]">
            {endpoint.description || "自动生成交互式参数表单，一键运行并可视化输出结果。"}
          </p>
        </div>

        <div className="flex items-center justify-between gap-2 pt-2 border-t border-border">
          <Badge variant="outline">输出: {endpoint.output_shape}</Badge>
          <div className="flex items-center gap-1.5">
            <Button
              type="button"
              variant="ghost"
              onClick={() => void handleCopyCurl()}
              className="h-7 px-2"
            >
              {copiedCurl ? (
                <Check className="h-3.5 w-3.5 text-xblue" />
              ) : (
                <Copy className="h-3.5 w-3.5" />
              )}
              cURL
            </Button>
            <Button
              type="button"
              data-testid={`open-app-btn-${endpoint.slug}`}
              onClick={() => {
                setPatInput(activePat ?? "");
                setDrawerOpen(true);
              }}
              className="h-7 px-3"
            >
              <Play className="h-3 w-3" />
              运行应用
            </Button>
          </div>
        </div>
      </Card>

      {drawerOpen ? (
        <div
          data-testid="app-runner-drawer"
          className="fixed inset-0 z-50 flex justify-end bg-black/50"
        >
          <div className="w-full max-w-2xl bg-background border-l border-border p-5 overflow-y-auto space-y-5">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="space-y-0.5">
                <div className="flex items-center gap-2">
                  <h2 className="text-sm font-semibold">{endpoint.title}</h2>
                  <Badge
                    variant={
                      endpoint.environment === "PROD" ? "xblue" : "neutral"
                    }
                  >
                    {endpoint.environment}
                  </Badge>
                  <Badge variant="outline">{endpoint.kind}</Badge>
                </div>
                <p className="text-[11px] font-mono-tech text-muted-foreground">
                  slug: {endpoint.slug} · shape: {endpoint.output_shape}
                </p>
              </div>
              <Button
                type="button"
                variant="ghost"
                onClick={() => setDrawerOpen(false)}
                className="h-7 w-7 p-0"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>

            {/* Dynamic Schema Form */}
            <div className="rounded-md border border-border bg-muted/20 p-4 space-y-3 text-xs">
              <div className="font-semibold text-foreground">输入参数表单</div>
              {schemaFields.length === 0 ? (
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    JSON 请求参数
                  </label>
                  <Textarea
                    value={rawJsonInput}
                    onChange={(e) => setRawJsonInput(e.target.value)}
                    rows={3}
                    className="font-mono-tech text-xs"
                  />
                </div>
              ) : (
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  {schemaFields.map((field) => (
                    <div
                      key={field.key}
                      className={
                        field.type === "object" ? "sm:col-span-2 space-y-1" : "space-y-1"
                      }
                    >
                      <label className="text-[11px] font-medium text-foreground flex items-center gap-1">
                        <span className="font-mono-tech">{field.key}</span>
                        {field.required ? (
                          <span className="text-xblue">*</span>
                        ) : null}
                      </label>

                      {field.enumValues ? (
                        <select
                          data-testid={`app-field-${field.key}`}
                          value={String(formValues[field.key] ?? "")}
                          onChange={(e) =>
                            setFormValues((prev) => ({
                              ...prev,
                              [field.key]: e.target.value,
                            }))
                          }
                          className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                        >
                          <option value="">-- 请选择 --</option>
                          {field.enumValues.map((opt) => (
                            <option key={opt} value={opt}>
                              {opt}
                            </option>
                          ))}
                        </select>
                      ) : field.type === "boolean" ? (
                        <label className="flex items-center gap-2 h-8">
                          <input
                            type="checkbox"
                            data-testid={`app-field-${field.key}`}
                            checked={Boolean(formValues[field.key])}
                            onChange={(e) =>
                              setFormValues((prev) => ({
                                ...prev,
                                [field.key]: e.target.checked,
                              }))
                            }
                            className="accent-[#2563eb]"
                          />
                          <span className="text-muted-foreground">启用</span>
                        </label>
                      ) : field.type === "array" && field.itemEnumValues ? (
                        <div className="flex flex-wrap gap-1.5 pt-0.5">
                          {field.itemEnumValues.map((opt) => {
                            const arr = Array.isArray(formValues[field.key])
                              ? (formValues[field.key] as string[])
                              : [];
                            const selected = arr.includes(opt);
                            return (
                              <label
                                key={opt}
                                className="inline-flex items-center gap-1 rounded border border-border bg-background px-2 py-0.5 text-[11px] font-mono-tech cursor-pointer"
                              >
                                <input
                                  type="checkbox"
                                  checked={selected}
                                  onChange={() => {
                                    const next = selected
                                      ? arr.filter((x) => x !== opt)
                                      : [...arr, opt];
                                    setFormValues((prev) => ({
                                      ...prev,
                                      [field.key]: next,
                                    }));
                                  }}
                                  className="accent-[#2563eb]"
                                />
                                {opt}
                              </label>
                            );
                          })}
                        </div>
                      ) : field.type === "object" ? (
                        <Textarea
                          data-testid={`app-field-${field.key}`}
                          rows={2}
                          value={
                            typeof formValues[field.key] === "string"
                              ? (formValues[field.key] as string)
                              : formValues[field.key]
                                ? JSON.stringify(formValues[field.key])
                                : ""
                          }
                          onChange={(e) =>
                            setFormValues((prev) => ({
                              ...prev,
                              [field.key]: e.target.value,
                            }))
                          }
                          placeholder='{"from": "2026-01-01"}'
                          className="font-mono-tech text-xs"
                        />
                      ) : (
                        <Input
                          data-testid={`app-field-${field.key}`}
                          type={
                            field.type === "number" || field.type === "integer"
                              ? "number"
                              : field.format === "date"
                                ? "date"
                                : "text"
                          }
                          value={String(formValues[field.key] ?? "")}
                          onChange={(e) =>
                            setFormValues((prev) => ({
                              ...prev,
                              [field.key]: e.target.value,
                            }))
                          }
                          placeholder={
                            field.type === "array"
                              ? "逗号分隔，如 W-1, W-2"
                              : field.description || field.key
                          }
                          className="h-8 text-xs font-mono-tech"
                        />
                      )}
                    </div>
                  ))}
                </div>
              )}

              {/* Optional PAT testing token */}
              <div className="pt-2 border-t border-border flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
                <div className="flex-1 flex items-center gap-2">
                  <Terminal className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                  <Input
                    data-testid="app-pat-input"
                    value={patInput}
                    onChange={(e) => setPatInput(e.target.value)}
                    placeholder="可选：填入 axp_... 令牌走公开端点 /api/published 测试（留空使用当前登录主体执行）"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <Button
                  type="button"
                  data-testid="app-run-submit-btn"
                  disabled={running}
                  onClick={() => void handleRun()}
                  className="h-8 px-4 shrink-0"
                >
                  <Play className="h-3.5 w-3.5" />
                  {running ? "执行中…" : "立即执行"}
                </Button>
              </div>
            </div>

            {errorMsg ? (
              <div className="rounded-md border border-border bg-muted px-3 py-2 text-xs text-foreground">
                {errorMsg}
              </div>
            ) : null}

            {/* Output Section */}
            {result ? (
              <div
                data-testid="app-run-result"
                className="space-y-3 text-xs border-t border-border pt-4"
              >
                <div className="flex items-center justify-between">
                  <span className="font-semibold text-foreground">执行结果</span>
                  {resultRows && resultRows.length > 0 ? (
                    <Button
                      type="button"
                      variant="outline"
                      data-testid="app-export-csv-btn"
                      onClick={() =>
                        downloadRowsAsCsv(endpoint.slug, resultRows)
                      }
                      className="h-7 px-2.5"
                    >
                      <Download className="h-3.5 w-3.5" />
                      导出 CSV ({resultRows.length} 行)
                    </Button>
                  ) : null}
                </div>

                {/* ROWS Table */}
                {resultRows ? (
                  resultRows.length === 0 ? (
                    <p className="text-muted-foreground py-4 text-center border border-dashed border-border rounded-md">
                      查询成功，返回 0 行记录
                    </p>
                  ) : (
                    <div className="overflow-x-auto rounded-md border border-border">
                      <table className="w-full text-left text-xs">
                        <thead className="bg-muted text-muted-foreground border-b border-border">
                          <tr>
                            {rowColumns.map((c) => (
                              <th
                                key={c}
                                className="px-3 py-2 font-mono-tech font-medium"
                              >
                                {c}
                              </th>
                            ))}
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-border">
                          {resultRows.map((row, rIdx) => (
                            <tr key={rIdx} className="bg-background">
                              {rowColumns.map((c) => (
                                <td
                                  key={c}
                                  className="px-3 py-1.5 font-mono-tech whitespace-nowrap"
                                >
                                  {row[c] === null || row[c] === undefined
                                    ? "—"
                                    : typeof row[c] === "object"
                                      ? JSON.stringify(row[c])
                                      : String(row[c])}
                                </td>
                              ))}
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )
                ) : resultObjects ? (
                  /* OBJECT Cards */
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                    {resultObjects.map((obj, idx) => (
                      <Card key={idx} className="p-3 space-y-2">
                        <div className="flex items-center justify-between gap-2">
                          <span className="font-semibold">
                            {typeof obj.title === "string"
                              ? obj.title
                              : String(obj.node_key ?? `Object #${idx + 1}`)}
                          </span>
                          <Badge variant="xblue">
                            {String(obj.node_key ?? obj.type_name ?? "OBJECT")}
                          </Badge>
                        </div>
                        <pre className="rounded border border-border bg-muted/40 p-2 text-[11px] font-mono-tech overflow-x-auto max-h-40">
                          {JSON.stringify(obj, null, 2)}
                        </pre>
                      </Card>
                    ))}
                  </div>
                ) : typeof result.answer === "string" ? (
                  /* TEXT Prose */
                  <Card className="p-3.5 whitespace-pre-wrap leading-relaxed">
                    {result.answer}
                  </Card>
                ) : typeof result.run_id === "string" ? (
                  /* RUN Status */
                  <Card className="p-3.5 flex items-center justify-between gap-2">
                    <div className="space-y-1">
                      <div className="font-medium">流程异步执行已触发</div>
                      <div className="font-mono-tech text-[11px] text-muted-foreground">
                        run_id: {result.run_id} · status:{" "}
                        {String(result.status ?? "RUNNING")}
                      </div>
                    </div>
                    {(patInput.trim() || activePat || "").startsWith("axp_") ? (
                      <Button
                        type="button"
                        variant="outline"
                        onClick={() =>
                          void handleRefreshRunStatus(String(result.run_id))
                        }
                        className="h-7 px-2.5"
                      >
                        <RefreshCw className="h-3.5 w-3.5" />
                        刷新状态
                      </Button>
                    ) : null}
                  </Card>
                ) : (
                  <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto">
                    {JSON.stringify(result, null, 2)}
                  </pre>
                )}
              </div>
            ) : null}
          </div>
        </div>
      ) : null}
    </>
  );
}
