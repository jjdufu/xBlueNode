import React, { useEffect, useMemo, useRef, useState } from "react";
import Editor, { loader } from "@monaco-editor/react";
import {
  Bookmark,
  Eye,
  FileSearch,
  History,
  Play,
  Save,
  Trash2,
  X,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type {
  AnalyticsCatalogResponse,
  ExplainResult,
  QueryHistoryEntry,
  QueryParamSpec,
  SavedQuery,
} from "@/lib/api/analytics";
import { usePrincipal } from "@/lib/permissions";
import { useTheme } from "@/lib/theme";
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";

type PreloadedMonaco = NonNullable<
  Parameters<typeof loader.config>[0]["monaco"]
>;

interface WindowWithMonaco extends Window {
  monaco?: PreloadedMonaco;
}

function getLocalInjectedMonaco(): PreloadedMonaco | null {
  if (typeof window === "undefined") return null;
  const candidate = (window as WindowWithMonaco).monaco;
  if (candidate) {
    loader.config({ monaco: candidate });
    return candidate;
  }
  return null;
}

function detectMacPlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  const platform = navigator.platform || "";
  const ua = navigator.userAgent || "";
  return (
    /Mac|iPhone|iPod|iPad/i.test(platform) || /Mac OS X|Macintosh/i.test(ua)
  );
}

const DUCKDB_KEYWORDS: ReadonlySet<string> = new Set([
  "SELECT",
  "FROM",
  "WHERE",
  "GROUP",
  "BY",
  "HAVING",
  "ORDER",
  "LIMIT",
  "OFFSET",
  "JOIN",
  "LEFT",
  "RIGHT",
  "INNER",
  "OUTER",
  "FULL",
  "CROSS",
  "ON",
  "AS",
  "WITH",
  "UNION",
  "ALL",
  "DISTINCT",
  "CASE",
  "WHEN",
  "THEN",
  "ELSE",
  "END",
  "AND",
  "OR",
  "NOT",
  "IN",
  "IS",
  "NULL",
  "TRUE",
  "FALSE",
  "LIKE",
  "ILIKE",
  "BETWEEN",
  "EXISTS",
  "COUNT",
  "SUM",
  "AVG",
  "MIN",
  "MAX",
  "MEDIAN",
  "COALESCE",
  "NULLIF",
  "CAST",
  "DATE_TRUNC",
  "RANGE",
  "GENERATE_SERIES",
  "UNNEST",
  "FILTER",
  "OVER",
  "PARTITION",
  "QUALIFY",
  "GROUPING",
  "SETS",
  "CUBE",
  "ROLLUP",
  "ASC",
  "DESC",
  "EXPLAIN",
]);

interface CompletionItem {
  label: string;
  kind: "KEYWORD" | "DATASET" | "COLUMN" | "VIEW" | "LIVE";
  detail: string;
}

interface SqlToken {
  text: string;
  kind: "keyword" | "string" | "comment" | "param" | "number" | "plain";
}

function tokenizeDuckDbSql(sql: string): SqlToken[] {
  const tokens: SqlToken[] = [];
  let i = 0;
  const len = sql.length;

  while (i < len) {
    const ch = sql[i] ?? "";
    const next = sql[i + 1] ?? "";

    if (ch === "-" && next === "-") {
      let end = i + 2;
      while (end < len && sql[end] !== "\n") end += 1;
      tokens.push({ text: sql.slice(i, end), kind: "comment" });
      i = end;
      continue;
    }

    if (ch === "/" && next === "*") {
      let end = i + 2;
      while (end < len && !(sql[end] === "*" && sql[end + 1] === "/")) {
        end += 1;
      }
      end = Math.min(len, end + 2);
      tokens.push({ text: sql.slice(i, end), kind: "comment" });
      i = end;
      continue;
    }

    if (ch === "'") {
      let end = i + 1;
      while (end < len) {
        if (sql[end] === "'" && sql[end + 1] === "'") {
          end += 2;
          continue;
        }
        if (sql[end] === "'") {
          end += 1;
          break;
        }
        end += 1;
      }
      tokens.push({ text: sql.slice(i, end), kind: "string" });
      i = end;
      continue;
    }

    if (
      ch === ":" &&
      next !== ":" &&
      (i === 0 || sql[i - 1] !== ":") &&
      /[a-zA-Z_]/.test(next)
    ) {
      let end = i + 2;
      while (end < len && /[a-zA-Z0-9_]/.test(sql[end] ?? "")) end += 1;
      tokens.push({ text: sql.slice(i, end), kind: "param" });
      i = end;
      continue;
    }

    if (/[a-zA-Z_]/.test(ch)) {
      let end = i + 1;
      while (end < len && /[a-zA-Z0-9_.]/.test(sql[end] ?? "")) end += 1;
      const word = sql.slice(i, end);
      if (DUCKDB_KEYWORDS.has(word.toUpperCase())) {
        tokens.push({ text: word, kind: "keyword" });
      } else {
        tokens.push({ text: word, kind: "plain" });
      }
      i = end;
      continue;
    }

    if (/[0-9]/.test(ch)) {
      let end = i + 1;
      while (end < len && /[0-9.]/.test(sql[end] ?? "")) end += 1;
      tokens.push({ text: sql.slice(i, end), kind: "number" });
      i = end;
      continue;
    }

    tokens.push({ text: ch, kind: "plain" });
    i += 1;
  }

  return tokens;
}

export function extractNamedSqlParams(sql: string): string[] {
  const found: string[] = [];
  const seen = new Set<string>();
  for (const tok of tokenizeDuckDbSql(sql)) {
    if (tok.kind === "param") {
      const name = tok.text.slice(1);
      if (name && !seen.has(name)) {
        seen.add(name);
        found.push(name);
      }
    }
  }
  return found;
}

function parseParamValue(raw: string): unknown {
  const trimmed = raw.trim();
  if (trimmed === "") return "";
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  if (/^-?\d+(\.\d+)?$/.test(trimmed)) {
    const num = Number(trimmed);
    if (Number.isFinite(num)) return num;
  }
  return raw;
}

export interface SqlEditorProps {
  sql: string;
  onChangeSql: (nextSql: string) => void;
  catalog: AnalyticsCatalogResponse;
  running: boolean;
  explaining: boolean;
  explainResult: ExplainResult | null;
  onCloseExplain: () => void;
  onRunSql: (opts: {
    sql: string;
    params: Record<string, unknown>;
    maxRows: number;
    allowLive: boolean;
    revealPii: boolean;
  }) => void;
  onExplainSql: (opts: {
    sql: string;
    params: Record<string, unknown>;
    maxRows: number;
    allowLive: boolean;
    revealPii: boolean;
  }) => void;
  onSaveQuery: (opts: {
    name: string;
    sql: string;
    parameters: QueryParamSpec[];
    shared: boolean;
  }) => Promise<void>;
  savedQueries: SavedQuery[];
  historyEntries: QueryHistoryEntry[];
  onLoadSavedQuery: (sq: SavedQuery) => void;
  onDeleteSavedQuery: (id: string) => void;
}

export function SqlEditor({
  sql,
  onChangeSql,
  catalog,
  running,
  explaining,
  explainResult,
  onCloseExplain,
  onRunSql,
  onExplainSql,
  onSaveQuery,
  savedQueries,
  historyEntries,
  onLoadSavedQuery,
  onDeleteSavedQuery,
}: SqlEditorProps) {
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const { isDark } = useTheme();
  const canRevealPii = can("PII_RAW_READ");
  const isMac = useMemo(() => detectMacPlatform(), []);
  const shortcutLabel = isMac ? "⌘+Enter" : "Ctrl+Enter";

  const localMonaco = useMemo(() => getLocalInjectedMonaco(), []);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const highlightRef = useRef<HTMLPreElement | null>(null);
  const gutterRef = useRef<HTMLDivElement | null>(null);
  const latestTriggerRunRef = useRef<() => void>(() => {});

  const [allowLive, setAllowLive] = useState(false);
  const [revealPii, setRevealPii] = useState(false);
  const [maxRows, setMaxRows] = useState<number>(1000);
  const [paramValues, setParamValues] = useState<Record<string, string>>({});
  const [cursorWord, setCursorWord] = useState<{
    prefix: string;
    start: number;
    end: number;
  } | null>(null);
  const [focused, setFocused] = useState(false);

  const [saveDialogOpen, setSaveDialogOpen] = useState(false);
  const [saveName, setSaveName] = useState("");
  const [saveShared, setSaveShared] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const [sideDrawer, setSideDrawer] = useState<"none" | "saved" | "history">(
    "none"
  );

  const paramNames = useMemo(() => extractNamedSqlParams(sql), [sql]);

  const resolvedParams = useMemo(() => {
    const out: Record<string, unknown> = {};
    for (const p of paramNames) {
      out[p] = parseParamValue(paramValues[p] ?? "");
    }
    return out;
  }, [paramNames, paramValues]);

  const completionCatalog = useMemo<CompletionItem[]>(() => {
    const items: CompletionItem[] = [];
    const seen = new Set<string>();

    for (const ds of catalog.datasets ?? []) {
      if (!seen.has(ds.name)) {
        seen.add(ds.name);
        items.push({
          label: ds.name,
          kind: "DATASET",
          detail: ds.display_name || tx("数据集", "Dataset"),
        });
      }
      for (const col of ds.columns ?? []) {
        if (!seen.has(col.column_name)) {
          seen.add(col.column_name);
          items.push({
            label: col.column_name,
            kind: "COLUMN",
            detail: `${ds.name}.${col.column_name} (${col.data_type})`,
          });
        }
      }
    }

    for (const view of catalog.platform_views ?? []) {
      if (!seen.has(view.name)) {
        seen.add(view.name);
        items.push({
          label: view.name,
          kind: "VIEW",
          detail: tx("联邦视图", "Platform View"),
        });
      }
      for (const col of view.columns ?? []) {
        if (!seen.has(col.name)) {
          seen.add(col.name);
          items.push({
            label: col.name,
            kind: "COLUMN",
            detail: `${view.name} (${col.data_type})`,
          });
        }
      }
    }

    for (const live of catalog.live_connections ?? []) {
      if (!seen.has(live.alias)) {
        seen.add(live.alias);
        items.push({
          label: live.alias,
          kind: "LIVE",
          detail: live.connector_type,
        });
      }
    }

    for (const kw of DUCKDB_KEYWORDS) {
      items.push({ label: kw, kind: "KEYWORD", detail: "SQL" });
    }

    return items;
  }, [catalog, tx]);

  const activeSuggestions = useMemo(() => {
    if (!focused || !cursorWord || cursorWord.prefix.length < 2) return [];
    const q = cursorWord.prefix.toLowerCase();
    return completionCatalog
      .filter(
        (item) =>
          item.label.toLowerCase().startsWith(q) &&
          item.label.toLowerCase() !== q
      )
      .slice(0, 7);
  }, [completionCatalog, cursorWord, focused]);

  const updateCursorWord = (text: string, caretPos: number) => {
    let start = caretPos;
    while (start > 0 && /[a-zA-Z0-9_.]/.test(text[start - 1] ?? "")) {
      start -= 1;
    }
    const prefix = text.slice(start, caretPos);
    if (prefix.length >= 2) {
      setCursorWord({ prefix, start, end: caretPos });
    } else {
      setCursorWord(null);
    }
  };

  const applySuggestion = (item: CompletionItem) => {
    if (!cursorWord) return;
    const nextSql =
      sql.slice(0, cursorWord.start) + item.label + sql.slice(cursorWord.end);
    onChangeSql(nextSql);
    setCursorWord(null);
    requestAnimationFrame(() => {
      const el = textareaRef.current;
      if (el) {
        const pos = cursorWord.start + item.label.length;
        el.focus();
        el.setSelectionRange(pos, pos);
      }
    });
  };

  const triggerRun = () => {
    if (!sql.trim() || running) return;
    onRunSql({
      sql,
      params: resolvedParams,
      maxRows,
      allowLive,
      revealPii: canRevealPii && revealPii,
    });
  };

  useEffect(() => {
    latestTriggerRunRef.current = triggerRun;
  });

  const triggerExplain = () => {
    if (!sql.trim() || explaining) return;
    onExplainSql({
      sql,
      params: resolvedParams,
      maxRows,
      allowLive,
      revealPii: canRevealPii && revealPii,
    });
  };

  const handleTextareaKeyDown = (
    e: React.KeyboardEvent<HTMLTextAreaElement>
  ) => {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      setCursorWord(null);
      triggerRun();
      return;
    }
    if (e.key === "Escape" && cursorWord) {
      e.preventDefault();
      setCursorWord(null);
      return;
    }
    if (e.key === "Tab" && activeSuggestions.length > 0) {
      e.preventDefault();
      const first = activeSuggestions[0];
      if (first) applySuggestion(first);
    }
  };

  const handleScrollSync = (e: React.UIEvent<HTMLTextAreaElement>) => {
    const top = e.currentTarget.scrollTop;
    const left = e.currentTarget.scrollLeft;
    if (highlightRef.current) {
      highlightRef.current.scrollTop = top;
      highlightRef.current.scrollLeft = left;
    }
    if (gutterRef.current) {
      gutterRef.current.scrollTop = top;
    }
  };

  const lineCount = useMemo(
    () => Math.max(1, sql.split("\n").length),
    [sql]
  );

  const tokens = useMemo(() => tokenizeDuckDbSql(sql), [sql]);

  const handleConfirmSave = async () => {
    if (!saveName.trim()) return;
    setSaving(true);
    setSaveError(null);
    try {
      const paramSpecs: QueryParamSpec[] = paramNames.map((name) => {
        const val = resolvedParams[name];
        const dataType =
          typeof val === "number"
            ? Number.isInteger(val)
              ? "INT64"
              : "FLOAT64"
            : typeof val === "boolean"
              ? "BOOLEAN"
              : "STRING";
        return {
          name,
          data_type: dataType,
          default: val,
        };
      });
      await onSaveQuery({
        name: saveName.trim(),
        sql,
        parameters: paramSpecs,
        shared: saveShared,
      });
      setSaveName("");
      setSaveDialogOpen(false);
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const sqlSavedQueries = useMemo(
    () => savedQueries.filter((sq) => sq.kind === "SQL" || sq.kind === "ASK"),
    [savedQueries]
  );

  return (
    <div className="space-y-3" data-testid="analytics-sql-editor">
      {/* Toolbar */}
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/30 px-3 py-2">
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="primary"
            onClick={triggerRun}
            disabled={running || !sql.trim()}
            data-testid="sql-run-btn"
          >
            <Play className="h-3.5 w-3.5" />
            {running ? tx("运行中…", "Running…") : tx("运行", "Run")}
            <span className="font-mono-tech text-[10px] opacity-75">
              {shortcutLabel}
            </span>
          </Button>

          <Button
            type="button"
            variant="outline"
            onClick={triggerExplain}
            disabled={explaining || !sql.trim()}
            data-testid="sql-explain-btn"
          >
            <FileSearch className="h-3.5 w-3.5 text-xblue" />
            {explaining ? tx("解析中…", "Explaining…") : tx("执行计划", "Explain")}
          </Button>

          <label className="inline-flex items-center gap-1.5 rounded border border-border bg-background px-2.5 py-1.5 text-xs cursor-pointer">
            <input
              type="checkbox"
              checked={allowLive}
              onChange={(e) => setAllowLive(e.target.checked)}
              className="rounded border-input accent-xblue"
              data-testid="sql-allow-live-toggle"
            />
            <span>{tx("允许外部直连", "Allow Live")}</span>
          </label>

          {canRevealPii && (
            <label className="inline-flex items-center gap-1.5 rounded border border-border bg-background px-2.5 py-1.5 text-xs cursor-pointer">
              <input
                type="checkbox"
                checked={revealPii}
                onChange={(e) => setRevealPii(e.target.checked)}
                className="rounded border-input accent-xblue"
                data-testid="sql-reveal-pii-toggle"
              />
              <Eye className="h-3.5 w-3.5 text-xblue" />
              <span>{tx("显示敏感原文", "Reveal PII")}</span>
            </label>
          )}

          {canRevealPii && revealPii && (
            <Badge variant="warning" data-testid="sql-reveal-pii-audit-badge">
              {tx("原文调阅已记审计", "PII Audited")}
            </Badge>
          )}

          <div className="flex items-center gap-1 text-xs">
            <span className="text-muted-foreground">
              {tx("行数上限", "Max Rows")}
            </span>
            <Select
              value={maxRows}
              onChange={(e) => setMaxRows(Number(e.target.value))}
              className="h-7 font-mono-tech"
              data-testid="sql-max-rows-select"
            >
              <option value={100}>{fmtNum(100, 0)}</option>
              <option value={1000}>{fmtNum(1000, 0)}</option>
              <option value={10000}>{fmtNum(10000, 0)}</option>
            </Select>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          <Button
            type="button"
            variant="outline"
            onClick={() =>
              setSideDrawer((prev) => (prev === "saved" ? "none" : "saved"))
            }
            data-testid="sql-saved-drawer-btn"
          >
            <Bookmark className="h-3.5 w-3.5 text-xblue" />
            {tx("已保存", "Saved")} ({fmtNum(sqlSavedQueries.length, 0)})
          </Button>

          <Button
            type="button"
            variant="outline"
            onClick={() =>
              setSideDrawer((prev) => (prev === "history" ? "none" : "history"))
            }
            data-testid="sql-history-drawer-btn"
          >
            <History className="h-3.5 w-3.5 text-xblue" />
            {tx("历史", "History")} ({fmtNum(historyEntries.length, 0)})
          </Button>

          <Button
            type="button"
            variant="outline"
            onClick={() => {
              setSaveError(null);
              setSaveDialogOpen((prev) => !prev);
            }}
            data-testid="sql-save-open-btn"
          >
            <Save className="h-3.5 w-3.5 text-xblue" />
            {tx("保存查询", "Save Query")}
          </Button>
        </div>
      </div>

      {/* Save Query Inline Card */}
      {saveDialogOpen && (
        <Card
          className="flex flex-wrap items-center gap-2 bg-muted/20 p-3"
          data-testid="sql-save-dialog"
        >
          <Input
            value={saveName}
            onChange={(e) => setSaveName(e.target.value)}
            placeholder="orders_by_status"
            className="h-8 max-w-xs text-xs font-mono-tech"
            data-testid="sql-save-name-input"
          />
          <label className="inline-flex items-center gap-1.5 text-xs cursor-pointer">
            <input
              type="checkbox"
              checked={saveShared}
              onChange={(e) => setSaveShared(e.target.checked)}
              className="rounded border-input accent-xblue"
            />
            <span>{tx("租户共享", "Shared")}</span>
          </label>
          <Button
            type="button"
            variant="primary"
            size="sm"
            disabled={saving || !saveName.trim()}
            onClick={() => void handleConfirmSave()}
            data-testid="sql-save-confirm-btn"
          >
            {saving ? tx("保存中…", "Saving…") : tx("保存", "Save")}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => setSaveDialogOpen(false)}
          >
            {tx("取消", "Cancel")}
          </Button>
          {saveError && (
            <span className="text-xs text-red-600 dark:text-red-400 font-mono-tech">
              {saveError}
            </span>
          )}
        </Card>
      )}

      {/* Editor Surface + Optional Saved/History Side Panel */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-3">
        <div
          className={
            sideDrawer === "none" ? "lg:col-span-12" : "lg:col-span-8"
          }
        >
          <div className="relative rounded-md border border-border bg-card overflow-hidden">
            {localMonaco ? (
              <div className="h-44" data-testid="sql-monaco-container">
                <Editor
                  height="176px"
                  defaultLanguage="sql"
                  language="sql"
                  theme={isDark ? "vs-dark" : "light"}
                  value={sql}
                  onChange={(val) => onChangeSql(val ?? "")}
                  onMount={(editorInstance, monacoInstance) => {
                    editorInstance.addCommand(
                      monacoInstance.KeyMod.CtrlCmd |
                        monacoInstance.KeyCode.Enter,
                      () => {
                        latestTriggerRunRef.current();
                      }
                    );
                  }}
                  options={{
                    minimap: { enabled: false },
                    fontSize: 12,
                    lineNumbers: "on",
                    scrollBeyondLastLine: false,
                    wordWrap: "on",
                  }}
                />
              </div>
            ) : (
              <div className="relative flex h-44 font-mono-tech text-xs leading-5">
                {/* Line Number Gutter */}
                <div
                  ref={gutterRef}
                  aria-hidden="true"
                  className="w-10 shrink-0 select-none overflow-hidden border-r border-border bg-muted/40 py-3 pr-2 text-right text-[11px] leading-5 text-muted-foreground"
                >
                  {Array.from({ length: lineCount }, (_, idx) => (
                    <div key={idx + 1}>{idx + 1}</div>
                  ))}
                </div>

                {/* Highlight Layer + Editable Textarea */}
                <div className="relative flex-1 overflow-hidden">
                  <pre
                    ref={highlightRef}
                    aria-hidden="true"
                    className="pointer-events-none absolute inset-0 m-0 overflow-hidden whitespace-pre-wrap break-words p-3 font-mono-tech text-xs leading-5"
                  >
                    {tokens.map((tok, i) => {
                      if (tok.kind === "keyword") {
                        return (
                          <span key={i} className="font-semibold text-xblue">
                            {tok.text}
                          </span>
                        );
                      }
                      if (tok.kind === "comment") {
                        return (
                          <span
                            key={i}
                            className="italic text-muted-foreground"
                          >
                            {tok.text}
                          </span>
                        );
                      }
                      if (tok.kind === "param") {
                        return (
                          <span
                            key={i}
                            className="rounded bg-xblue-soft px-0.5 text-xblue"
                          >
                            {tok.text}
                          </span>
                        );
                      }
                      if (tok.kind === "string" || tok.kind === "number") {
                        return (
                          <span key={i} className="font-medium text-foreground">
                            {tok.text}
                          </span>
                        );
                      }
                      return (
                        <span key={i} className="text-foreground">
                          {tok.text}
                        </span>
                      );
                    })}
                    {sql.endsWith("\n") ? " " : ""}
                  </pre>

                  <textarea
                    ref={textareaRef}
                    value={sql}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoComplete="off"
                    autoCorrect="off"
                    aria-label={tx("SQL 编辑器", "SQL Editor")}
                    data-testid="sql-editor-textarea"
                    onFocus={() => setFocused(true)}
                    onBlur={() => {
                      setTimeout(() => setFocused(false), 150);
                    }}
                    onChange={(e) => {
                      const next = e.target.value;
                      onChangeSql(next);
                      updateCursorWord(next, e.target.selectionStart ?? 0);
                    }}
                    onKeyUp={(e) => {
                      if (
                        e.key !== "Escape" &&
                        e.key !== "Enter" &&
                        e.key !== "Tab"
                      ) {
                        updateCursorWord(
                          e.currentTarget.value,
                          e.currentTarget.selectionStart ?? 0
                        );
                      }
                    }}
                    onKeyDown={handleTextareaKeyDown}
                    onScroll={handleScrollSync}
                    placeholder="SELECT count(*) FROM demo_orders"
                    style={{
                      color: "transparent",
                      caretColor: "var(--foreground)",
                    }}
                    className="relative z-10 h-full w-full resize-none overflow-auto bg-transparent p-3 font-mono-tech text-xs leading-5 selection:bg-xblue/20 focus:outline-none"
                  />
                </div>

                {/* Schema Autocomplete Floating Popup */}
                {activeSuggestions.length > 0 && (
                  <div
                    className="absolute bottom-2 left-12 z-20 w-72 rounded-md border border-border bg-card shadow-sm p-1"
                    data-testid="sql-autocomplete-popup"
                  >
                    <div className="px-2 py-0.5 text-[10px] text-muted-foreground flex items-center justify-between">
                      <span>{tx("代码补全 · Tab", "Autocomplete · Tab")}</span>
                      <span>Esc</span>
                    </div>
                    {activeSuggestions.map((item) => (
                      <button
                        key={`${item.kind}:${item.label}`}
                        type="button"
                        onMouseDown={(e) => {
                          e.preventDefault();
                          applySuggestion(item);
                        }}
                        className="flex w-full items-center justify-between gap-2 rounded px-2 py-1 text-left text-xs hover:bg-muted cursor-pointer"
                      >
                        <span className="font-mono-tech font-medium text-foreground">
                          {item.label}
                        </span>
                        <span className="truncate text-[10px] text-muted-foreground">
                          {item.detail}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>

        {/* Saved Queries / History Side Drawer */}
        {sideDrawer !== "none" && (
          <Card className="lg:col-span-4 flex h-44 flex-col overflow-hidden">
            <div className="flex items-center justify-between border-b border-border bg-muted/30 px-3 py-1.5 text-xs font-medium">
              <span>
                {sideDrawer === "saved"
                  ? tx("已保存查询", "Saved Queries")
                  : tx("最近执行记录", "Execution History")}
              </span>
              <button
                type="button"
                onClick={() => setSideDrawer("none")}
                aria-label={tx("关闭", "Close")}
                className="text-muted-foreground hover:text-foreground cursor-pointer"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            </div>

            <div className="flex-1 overflow-y-auto p-2 space-y-1.5 text-xs">
              {sideDrawer === "saved" ? (
                sqlSavedQueries.length === 0 ? (
                  <div className="py-6 text-center text-[11px] text-muted-foreground">
                    {tx("暂无保存记录", "No saved queries")}
                  </div>
                ) : (
                  sqlSavedQueries.map((sq) => (
                    <div
                      key={sq.id}
                      className="flex items-center justify-between gap-2 rounded border border-border/70 px-2.5 py-1.5 hover:bg-muted/40"
                    >
                      <button
                        type="button"
                        onClick={() => onLoadSavedQuery(sq)}
                        className="min-w-0 flex-1 text-left cursor-pointer"
                      >
                        <div className="truncate font-mono-tech font-medium">
                          {sq.name}
                        </div>
                        <div className="truncate font-mono-tech text-[10px] text-muted-foreground">
                          {typeof sq.spec.sql === "string"
                            ? sq.spec.sql
                            : sq.kind}
                        </div>
                      </button>
                      <button
                        type="button"
                        onClick={() => onDeleteSavedQuery(sq.id)}
                        title={tx("删除", "Delete")}
                        aria-label={tx("删除", "Delete")}
                        className="text-muted-foreground hover:text-foreground cursor-pointer"
                      >
                        <Trash2 className="h-3 w-3" />
                      </button>
                    </div>
                  ))
                )
              ) : historyEntries.length === 0 ? (
                <div className="py-6 text-center text-[11px] text-muted-foreground">
                  {tx("暂无历史记录", "No execution history")}
                </div>
              ) : (
                historyEntries.map((entry) => (
                  <button
                    key={entry.id}
                    type="button"
                    onClick={() => onChangeSql(entry.sql_text)}
                    className="w-full rounded border border-border/70 px-2.5 py-1.5 text-left hover:bg-muted/40 cursor-pointer space-y-0.5"
                  >
                    <div className="flex items-center justify-between gap-1">
                      {entry.status === "TIMEOUT" ? (
                        <Badge variant="warning">
                          {tx("超时", "Timeout")}
                        </Badge>
                      ) : (
                        <StatusBadge status={entry.status} />
                      )}
                      <span className="font-mono-tech text-[10px] text-muted-foreground">
                        {entry.duration_ms !== null
                          ? `${fmtNum(entry.duration_ms, 0)} ms`
                          : "-"}
                      </span>
                    </div>
                    <div className="truncate font-mono-tech text-[11px] text-foreground">
                      {entry.sql_text}
                    </div>
                  </button>
                ))
              )}
            </div>
          </Card>
        )}
      </div>

      {/* :param Bindings Input Bar */}
      {paramNames.length > 0 && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-border bg-muted/20 px-3 py-2 text-xs"
          data-testid="sql-params-bar"
        >
          <span className="font-medium text-muted-foreground">
            {tx("命名参数", "Parameters")}:
          </span>
          {paramNames.map((pName) => (
            <div key={pName} className="flex items-center gap-1">
              <Badge variant="xblue">:{pName}</Badge>
              <Input
                value={paramValues[pName] ?? ""}
                onChange={(e) =>
                  setParamValues((prev) => ({
                    ...prev,
                    [pName]: e.target.value,
                  }))
                }
                placeholder="value"
                className="h-7 w-28 text-xs font-mono-tech"
                data-testid={`sql-param-input-${pName}`}
              />
            </div>
          ))}
        </div>
      )}

      {/* EXPLAIN Plan Output Card */}
      {explainResult && (
        <Card
          className="p-3 space-y-2 bg-muted/20"
          data-testid="sql-explain-card"
        >
          <div className="flex items-center justify-between border-b border-border pb-1.5 text-xs">
            <div className="flex items-center gap-2">
              <span className="font-semibold">
                {tx("查询执行计划", "Query Execution Plan")}
              </span>
              {explainResult.uses_platform_views && (
                <Badge variant="xblue">
                  {tx("联邦视图", "Platform Views")}
                </Badge>
              )}
              <Badge variant="outline">
                {tx("关联数据集", "Datasets")}{" "}
                {fmtNum(explainResult.referenced_datasets.length, 0)}
              </Badge>
            </div>
            <button
              type="button"
              onClick={onCloseExplain}
              aria-label={tx("关闭", "Close")}
              className="text-muted-foreground hover:text-foreground cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          <div className="text-[11px] font-mono-tech text-muted-foreground break-all">
            {explainResult.sql}
          </div>
          <pre className="max-h-40 overflow-auto rounded border border-border bg-background p-2.5 font-mono-tech text-[11px] leading-relaxed whitespace-pre-wrap">
            {explainResult.plan}
          </pre>
        </Card>
      )}
    </div>
  );
}
