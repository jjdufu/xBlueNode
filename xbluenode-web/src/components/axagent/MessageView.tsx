import { useMemo, useState } from "react";
import ReactMarkdown from "react-markdown";
import {
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Cpu,
  Loader2,
  User,
  Wrench,
} from "lucide-react";
import type { MessageRecord, ToolCallSpec } from "@/lib/api/axagent";
import { Badge } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

export interface StreamingToolItem {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
  resultContent?: string;
  truncated?: boolean;
  agentName?: string;
}

export interface MessageViewProps {
  messages: MessageRecord[];
  streamingToolCalls?: StreamingToolItem[];
  streamingText?: string;
  streamingAgentName?: string | null;
  isStreaming?: boolean;
  emptyHint?: string;
}

interface ParsedToolOutput {
  formattedJson: string | null;
  rows: Record<string, unknown>[] | null;
  columns: string[];
}

function parseToolContent(raw: string): ParsedToolOutput {
  const trimmed = raw.trim();
  if (!trimmed) {
    return { formattedJson: null, rows: null, columns: [] };
  }
  try {
    const parsed: unknown = JSON.parse(trimmed);
    const formattedJson = JSON.stringify(parsed, null, 2);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      !Array.isArray(parsed) &&
      "rows" in parsed &&
      Array.isArray((parsed as { rows?: unknown }).rows)
    ) {
      const rawRows = (parsed as { rows: unknown[] }).rows.filter(
        (item): item is Record<string, unknown> =>
          typeof item === "object" && item !== null && !Array.isArray(item)
      );
      if (rawRows.length > 0) {
        const colSet = new Set<string>();
        for (const row of rawRows.slice(0, 10)) {
          for (const key of Object.keys(row)) {
            colSet.add(key);
          }
        }
        return {
          formattedJson,
          rows: rawRows,
          columns: Array.from(colSet),
        };
      }
    }
    return { formattedJson, rows: null, columns: [] };
  } catch {
    return { formattedJson: null, rows: null, columns: [] };
  }
}

function AssistantMarkdown({ content }: { content: string }) {
  return (
    <div className="leading-relaxed break-words text-xs sm:text-sm space-y-2 [&_p]:leading-relaxed [&_ul]:list-disc [&_ul]:pl-5 [&_ul]:space-y-1 [&_ol]:list-decimal [&_ol]:pl-5 [&_ol]:space-y-1 [&_h1]:text-base [&_h1]:font-semibold [&_h2]:text-sm [&_h2]:font-semibold [&_h3]:text-xs [&_h3]:font-semibold [&_strong]:font-semibold [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono-tech [&_code]:text-[11px] [&_pre]:rounded-md [&_pre]:border [&_pre]:border-border [&_pre]:bg-muted/50 [&_pre]:p-2.5 [&_pre]:overflow-x-auto [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_table]:w-full [&_table]:border-collapse [&_table]:text-xs [&_th]:border [&_th]:border-border [&_th]:bg-muted/50 [&_th]:px-2 [&_th]:py-1 [&_th]:text-left [&_td]:border [&_td]:border-border [&_td]:px-2 [&_td]:py-1">
      <ReactMarkdown>{content}</ReactMarkdown>
    </div>
  );
}

function ToolCallCardItem({
  callId,
  toolName,
  args,
  resultContent,
  truncated,
  agentName,
  pending,
}: {
  callId: string;
  toolName: string;
  args: Record<string, unknown>;
  resultContent?: string;
  truncated?: boolean;
  agentName?: string;
  pending: boolean;
}) {
  const { tx } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const parsedResult = useMemo(
    () => (resultContent !== undefined ? parseToolContent(resultContent) : null),
    [resultContent]
  );

  const argKeys = Object.keys(args);
  const summaryPreview =
    argKeys.length > 0
      ? argKeys
          .slice(0, 2)
          .map((k) => {
            const val = args[k];
            const strVal =
              typeof val === "string" ? val : JSON.stringify(val ?? "");
            return `${k}: ${strVal.length > 36 ? `${strVal.slice(0, 36)}…` : strVal}`;
          })
          .join(", ")
      : tx("无参数", "no args");

  return (
    <div
      data-testid="tool-call-card"
      data-tool-name={toolName}
      data-call-id={callId}
      className="rounded-md border border-border bg-muted/40 text-xs overflow-hidden"
    >
      <button
        type="button"
        onClick={() => setExpanded((prev) => !prev)}
        className="w-full flex items-center justify-between gap-2 px-3 py-2 text-left hover:bg-muted/70 transition-colors cursor-pointer"
      >
        <div className="flex items-center gap-2 min-w-0 flex-wrap">
          {expanded ? (
            <ChevronDown className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
          )}
          <Wrench className="h-3.5 w-3.5 text-xblue shrink-0" />
          <span className="font-mono-tech font-semibold text-foreground">
            {toolName}
          </span>
          {agentName ? (
            <Badge variant="xblue" data-testid="specialist-badge">
              {agentName}
            </Badge>
          ) : null}
          <span className="text-muted-foreground truncate max-w-[280px] font-mono-tech">
            ({summaryPreview})
          </span>
        </div>
        <div className="flex items-center gap-1.5 shrink-0">
          {truncated ? (
            <Badge variant="outline">{tx("已截断", "Truncated")}</Badge>
          ) : null}
          {pending ? (
            <span className="inline-flex items-center gap-1 text-[11px] text-xblue font-mono-tech">
              <Loader2 className="h-3 w-3 animate-spin" />
              {tx("运行中…", "Running…")}
            </span>
          ) : (
            <span className="inline-flex items-center gap-1 text-[11px] text-muted-foreground font-mono-tech">
              <CheckCircle2 className="h-3 w-3 text-xblue" />
              {tx("已完成", "Done")}
            </span>
          )}
        </div>
      </button>

      {expanded ? (
        <div className="border-t border-border bg-background/80 p-3 space-y-2.5">
          <div>
            <div className="text-[11px] font-medium text-muted-foreground mb-1">
              {tx("调用参数", "Arguments")}
            </div>
            <pre className="rounded border border-border bg-muted/50 p-2 text-[11px] font-mono-tech overflow-x-auto max-h-40">
              {JSON.stringify(args, null, 2)}
            </pre>
          </div>

          {resultContent !== undefined ? (
            <div className="space-y-2">
              <div className="text-[11px] font-medium text-muted-foreground">
                {tx("返回结果", "Tool Output")}
              </div>
              {parsedResult?.rows && parsedResult.columns.length > 0 ? (
                <div className="overflow-x-auto rounded border border-border">
                  <table className="w-full text-left text-[11px]">
                    <thead className="bg-muted text-muted-foreground border-b border-border">
                      <tr>
                        {parsedResult.columns.map((col) => (
                          <th
                            key={col}
                            className="px-2 py-1 font-mono-tech font-medium"
                          >
                            {col}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-border">
                      {parsedResult.rows.slice(0, 10).map((row, idx) => (
                        <tr key={idx} className="bg-background">
                          {parsedResult.columns.map((col) => (
                            <td
                              key={col}
                              className="px-2 py-1 font-mono-tech whitespace-nowrap"
                            >
                              {row[col] === null || row[col] === undefined
                                ? "—"
                                : typeof row[col] === "object"
                                  ? JSON.stringify(row[col])
                                  : String(row[col])}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ) : null}
              <pre className="rounded border border-border bg-muted/50 p-2 text-[11px] font-mono-tech overflow-x-auto max-h-52 whitespace-pre-wrap break-all">
                {parsedResult?.formattedJson ?? resultContent}
              </pre>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export function MessageView({
  messages,
  streamingToolCalls = [],
  streamingText = "",
  streamingAgentName = null,
  isStreaming = false,
  emptyHint,
}: MessageViewProps) {
  const { tx, fmtNum } = useI18n();
  const resolvedEmptyHint =
    emptyHint ?? tx("输入问题开始多轮推理与问数", "Ask a question to start");

  const { pairedItems } = useMemo(() => {
    const toolResultMap = new Map<string, MessageRecord>();
    const toolCallNameMap = new Map<string, ToolCallSpec>();

    for (const msg of messages) {
      if (msg.tool_calls) {
        for (const tc of msg.tool_calls) {
          toolCallNameMap.set(tc.id, tc);
        }
      }
      if (msg.role === "TOOL" && msg.tool_call_id) {
        toolResultMap.set(msg.tool_call_id, msg);
      }
    }

    const consumedToolIds = new Set<number>();
    const items: Array<
      | {
          kind: "message";
          msg: MessageRecord;
          pairedTools: Array<{ call: ToolCallSpec; result?: MessageRecord }>;
        }
      | { kind: "orphan_tool"; msg: MessageRecord; inferredName: string }
    > = [];

    for (const msg of messages) {
      if (msg.role === "SYSTEM") {
        continue;
      }
      if (msg.role === "ASSISTANT") {
        const pairedTools: Array<{
          call: ToolCallSpec;
          result?: MessageRecord;
        }> = [];
        if (msg.tool_calls && msg.tool_calls.length > 0) {
          for (const tc of msg.tool_calls) {
            const matched = toolResultMap.get(tc.id);
            if (matched) {
              consumedToolIds.add(matched.id);
            }
            pairedTools.push({ call: tc, result: matched });
          }
        }
        items.push({ kind: "message", msg, pairedTools });
      } else if (msg.role === "USER") {
        items.push({ kind: "message", msg, pairedTools: [] });
      }
    }

    for (const msg of messages) {
      if (msg.role === "TOOL" && !consumedToolIds.has(msg.id)) {
        const matchedCall = msg.tool_call_id
          ? toolCallNameMap.get(msg.tool_call_id)
          : undefined;
        items.push({
          kind: "orphan_tool",
          msg,
          inferredName: matchedCall?.name ?? msg.agent_name ?? "tool_result",
        });
      }
    }

    return { pairedItems: items };
  }, [messages]);

  const hasAnyContent =
    pairedItems.length > 0 ||
    streamingToolCalls.length > 0 ||
    streamingText.trim().length > 0 ||
    isStreaming;

  if (!hasAnyContent) {
    return (
      <div className="h-full min-h-[260px] flex flex-col items-center justify-center text-center p-6 text-muted-foreground">
        <Bot className="h-8 w-8 mb-2.5 text-muted-foreground/60" />
        <p className="text-xs">{resolvedEmptyHint}</p>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {pairedItems.map((entry, idx) => {
        if (entry.kind === "orphan_tool") {
          return (
            <ToolCallCardItem
              key={`orphan-${entry.msg.id}-${idx}`}
              callId={entry.msg.tool_call_id ?? String(entry.msg.id)}
              toolName={entry.inferredName}
              args={{}}
              resultContent={entry.msg.content}
              agentName={entry.msg.agent_name}
              pending={false}
            />
          );
        }

        const { msg, pairedTools } = entry;
        const isUser = msg.role === "USER";
        const showBubble = isUser || msg.content.trim().length > 0;

        return (
          <div key={`msg-${msg.id}-${idx}`} className="space-y-2">
            {pairedTools.length > 0 ? (
              <div className="space-y-1.5 pl-0 md:pl-7">
                {pairedTools.map(({ call, result }) => (
                  <ToolCallCardItem
                    key={call.id}
                    callId={call.id}
                    toolName={call.name}
                    args={call.arguments}
                    resultContent={result?.content}
                    agentName={result?.agent_name ?? msg.agent_name}
                    pending={!result}
                  />
                ))}
              </div>
            ) : null}

            {showBubble ? (
              <div
                data-testid={
                  isUser ? "chat-message-user" : "chat-message-assistant"
                }
                className={cn(
                  "flex gap-2.5",
                  isUser ? "justify-end" : "justify-start"
                )}
              >
                {!isUser ? (
                  <div className="h-6 w-6 rounded-full border border-border bg-muted flex items-center justify-center shrink-0 mt-0.5">
                    <Bot className="h-3.5 w-3.5 text-xblue" />
                  </div>
                ) : null}

                <div
                  className={cn(
                    "max-w-[86%] rounded-lg px-3.5 py-2.5 text-sm space-y-1.5",
                    isUser
                      ? "bg-primary text-primary-foreground"
                      : "border border-border bg-card text-card-foreground"
                  )}
                >
                  {!isUser &&
                  (msg.agent_name ||
                    msg.prompt_tokens !== undefined ||
                    msg.completion_tokens !== undefined ||
                    msg.latency_ms !== undefined) ? (
                    <div className="flex items-center flex-wrap gap-1.5 pb-0.5">
                      {msg.agent_name ? (
                        <Badge variant="xblue" data-testid="specialist-badge">
                          <Cpu className="h-2.5 w-2.5 mr-1" />
                          {msg.agent_name}
                        </Badge>
                      ) : null}
                      {typeof msg.prompt_tokens === "number" ? (
                        <Badge variant="outline" title="Prompt Tokens">
                          in:{fmtNum(msg.prompt_tokens, 0)}
                        </Badge>
                      ) : null}
                      {typeof msg.completion_tokens === "number" ? (
                        <Badge variant="outline" title="Completion Tokens">
                          out:{fmtNum(msg.completion_tokens, 0)}
                        </Badge>
                      ) : null}
                      {typeof msg.latency_ms === "number" ? (
                        <Badge variant="outline">
                          {fmtNum(msg.latency_ms, 0)}ms
                        </Badge>
                      ) : null}
                    </div>
                  ) : null}

                  {isUser ? (
                    <div className="whitespace-pre-wrap leading-relaxed break-words text-xs sm:text-sm">
                      {msg.content}
                    </div>
                  ) : (
                    <AssistantMarkdown content={msg.content} />
                  )}
                </div>

                {isUser ? (
                  <div className="h-6 w-6 rounded-full border border-border bg-muted flex items-center justify-center shrink-0 mt-0.5">
                    <User className="h-3.5 w-3.5 text-muted-foreground" />
                  </div>
                ) : null}
              </div>
            ) : null}
          </div>
        );
      })}

      {streamingToolCalls.length > 0 ? (
        <div className="space-y-1.5 pl-0 md:pl-7">
          {streamingToolCalls.map((tc) => (
            <ToolCallCardItem
              key={`stream-tc-${tc.id}`}
              callId={tc.id}
              toolName={tc.name}
              args={tc.arguments}
              resultContent={tc.resultContent}
              truncated={tc.truncated}
              agentName={tc.agentName}
              pending={tc.resultContent === undefined}
            />
          ))}
        </div>
      ) : null}

      {isStreaming || streamingText.trim().length > 0 ? (
        <div
          data-testid="chat-message-streaming"
          className="flex gap-2.5 justify-start"
        >
          <div className="h-6 w-6 rounded-full border border-border bg-muted flex items-center justify-center shrink-0 mt-0.5">
            <Bot className="h-3.5 w-3.5 text-xblue" />
          </div>
          <div className="max-w-[86%] rounded-lg border border-border bg-card text-card-foreground px-3.5 py-2.5 space-y-1.5">
            {streamingAgentName ? (
              <div className="flex items-center gap-1.5">
                <Badge variant="xblue" data-testid="specialist-badge">
                  <Cpu className="h-2.5 w-2.5 mr-1" />
                  {streamingAgentName}
                </Badge>
              </div>
            ) : null}
            {streamingText.length > 0 ? (
              <AssistantMarkdown content={streamingText} />
            ) : (
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <Loader2 className="h-3.5 w-3.5 animate-spin text-xblue" />
                {tx("思考中…", "Thinking…")}
              </div>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}
