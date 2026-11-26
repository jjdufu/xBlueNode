import { useCallback, useRef, useState } from "react";
import { useParams } from "react-router";
import {
  AlertTriangle,
  Bot,
  Languages,
  Moon,
  RotateCcw,
  Send,
  Square,
  Sun,
} from "lucide-react";
import { ApiError } from "@/lib/api/client";
import {
  postEmbedChat,
  streamEmbedChat,
  type AgentEvent,
  type MessageRecord,
} from "@/lib/api/axagent";
import {
  MessageView,
  type StreamingToolItem,
} from "@/components/axagent/MessageView";
import {
  Badge,
  Button,
  Card,
  IconButton,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";

interface EmbedErrorBannerState {
  status: number | null;
  title: string;
  detail: string;
}

export function AgentEmbedPage() {
  const { token } = useParams<{ token: string }>();
  const { isDark, toggleTheme } = useTheme();
  const { isZh, toggleLocale, tx } = useI18n();

  const [conversationId, setConversationId] = useState<string | null>(null);
  const [messages, setMessages] = useState<MessageRecord[]>([]);
  const [inputText, setInputText] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [streamingText, setStreamingText] = useState("");
  const [streamingTools, setStreamingTools] = useState<StreamingToolItem[]>([]);
  const [streamingSpeaker, setStreamingSpeaker] = useState<string | null>(null);
  const [errorState, setErrorState] = useState<EmbedErrorBannerState | null>(
    null
  );

  const abortRef = useRef<AbortController | null>(null);
  const localMsgIdRef = useRef<number>(-1);

  const classifyEmbedError = useCallback(
    (err: unknown): EmbedErrorBannerState => {
      if (err instanceof ApiError) {
        if (err.status === 404) {
          return {
            status: 404,
            title: tx(
              "嵌入链接无效或已撤销 (404)",
              "Embed Link Invalid or Revoked (404)"
            ),
            detail:
              err.message ||
              tx(
                "该链接不存在、已过期或已被撤销。",
                "This embed link was not found, expired, or revoked."
              ),
          };
        }
        if (err.status === 403) {
          return {
            status: 403,
            title: tx("来源域名受限 (403)", "Origin Not Allowed (403)"),
            detail:
              err.message ||
              tx(
                "当前来源不在允许名单内。",
                "Current origin is not permitted for this embed link."
              ),
          };
        }
        if (err.status === 429) {
          return {
            status: 429,
            title: tx("请求速率超限 (429)", "Rate Limit Exceeded (429)"),
            detail:
              err.message ||
              tx("请求过于频繁，稍后重试。", "Too many requests."),
          };
        }
        return {
          status: err.status,
          title: `${tx("请求异常", "Request Error")} (${err.errorCode})`,
          detail: err.message,
        };
      }
      return {
        status: null,
        title: tx("连接异常", "Connection Error"),
        detail:
          err instanceof Error
            ? err.message
            : tx("无法连接服务", "Unable to reach agent service"),
      };
    },
    [tx]
  );

  const handleResetConversation = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    setIsStreaming(false);
    setConversationId(null);
    setMessages([]);
    setStreamingText("");
    setStreamingTools([]);
    setStreamingSpeaker(null);
    setErrorState(null);
  }, []);

  const handleSend = useCallback(async () => {
    const trimmed = inputText.trim();
    if (!trimmed || !token || isStreaming) {
      return;
    }

    setErrorState(null);
    setInputText("");

    const userMsg: MessageRecord = {
      id: localMsgIdRef.current--,
      tenant_id: "embed",
      conversation_id: conversationId ?? "pending",
      role: "USER",
      content: trimmed,
      created_at: new Date().toISOString(),
    };

    setMessages((prev) => [...prev, userMsg]);
    setIsStreaming(true);
    setStreamingText("");
    setStreamingTools([]);
    setStreamingSpeaker(null);

    const controller = new AbortController();
    abortRef.current = controller;

    let resolvedConvId = conversationId;
    let accText = "";
    let finalAnswer = "";
    let lastSpeaker: string | undefined;
    let receivedAnyEvent = false;
    const accTools: StreamingToolItem[] = [];

    const onStreamEvent = (ev: AgentEvent) => {
      receivedAnyEvent = true;
      switch (ev.type) {
        case "conversation_started": {
          resolvedConvId = ev.conversation_id;
          setConversationId(ev.conversation_id);
          break;
        }
        case "agent_switched": {
          lastSpeaker = ev.agent_name;
          setStreamingSpeaker(ev.agent_name);
          break;
        }
        case "token": {
          accText += ev.delta;
          if (ev.agent_name) {
            lastSpeaker = ev.agent_name;
            setStreamingSpeaker(ev.agent_name);
          }
          setStreamingText(accText);
          break;
        }
        case "tool_call": {
          if (ev.agent_name) {
            lastSpeaker = ev.agent_name;
            setStreamingSpeaker(ev.agent_name);
          }
          accTools.push({
            id: ev.id,
            name: ev.name,
            arguments: ev.arguments,
            agentName: ev.agent_name,
          });
          setStreamingTools([...accTools]);
          break;
        }
        case "tool_result": {
          if (ev.agent_name) {
            lastSpeaker = ev.agent_name;
            setStreamingSpeaker(ev.agent_name);
          }
          const idx = accTools.findIndex((t) => t.id === ev.tool_call_id);
          if (idx >= 0) {
            const target = accTools[idx];
            if (target) {
              accTools[idx] = {
                ...target,
                resultContent: ev.content,
                truncated: ev.truncated,
                agentName: ev.agent_name ?? target.agentName,
              };
            }
          } else {
            accTools.push({
              id: ev.tool_call_id,
              name: ev.name,
              arguments: {},
              resultContent: ev.content,
              truncated: ev.truncated,
              agentName: ev.agent_name,
            });
          }
          setStreamingTools([...accTools]);
          break;
        }
        case "final": {
          resolvedConvId = ev.conversation_id;
          setConversationId(ev.conversation_id);
          finalAnswer = ev.answer || accText;
          break;
        }
        case "done": {
          if (ev.conversation_id) {
            resolvedConvId = ev.conversation_id;
            setConversationId(ev.conversation_id);
          }
          finalAnswer = ev.answer ?? accText;
          break;
        }
        case "error": {
          setErrorState({
            status: null,
            title: `${tx("执行失败", "Execution Error")} (${ev.error_code})`,
            detail: ev.message,
          });
          break;
        }
        case "approval_required": {
          resolvedConvId = ev.conversation_id;
          setConversationId(ev.conversation_id);
          finalAnswer = tx(
            `工具 ${ev.tool_name} 待管理员审批。`,
            `Tool ${ev.tool_name} requires administrator approval.`
          );
          break;
        }
      }
    };

    try {
      const reqBody = {
        message: trimmed,
        ...(conversationId ? { conversation_id: conversationId } : {}),
      };

      try {
        await streamEmbedChat(token, reqBody, onStreamEvent, controller.signal);
      } catch (streamErr) {
        if (controller.signal.aborted || streamErr instanceof ApiError) {
          throw streamErr;
        }
        if (!receivedAnyEvent) {
          const syncResp = await postEmbedChat(token, reqBody);
          resolvedConvId = syncResp.conversation_id;
          setConversationId(syncResp.conversation_id);
          finalAnswer = syncResp.answer;
          if (syncResp.messages.length > 0) {
            setMessages(syncResp.messages);
            return;
          }
        } else {
          throw streamErr;
        }
      }

      const convForLocal = resolvedConvId ?? "embed-conv";
      const appended: MessageRecord[] = [];
      if (accTools.length > 0) {
        appended.push({
          id: localMsgIdRef.current--,
          tenant_id: "embed",
          conversation_id: convForLocal,
          role: "ASSISTANT",
          content: "",
          tool_calls: accTools.map((tc) => ({
            id: tc.id,
            name: tc.name,
            arguments: tc.arguments,
          })),
          agent_name: lastSpeaker,
          created_at: new Date().toISOString(),
        });
        for (const tc of accTools) {
          if (tc.resultContent !== undefined) {
            appended.push({
              id: localMsgIdRef.current--,
              tenant_id: "embed",
              conversation_id: convForLocal,
              role: "TOOL",
              content: tc.resultContent,
              tool_call_id: tc.id,
              agent_name: tc.agentName,
              created_at: new Date().toISOString(),
            });
          }
        }
      }

      const finalContent = (finalAnswer || accText).trim();
      if (finalContent.length > 0) {
        appended.push({
          id: localMsgIdRef.current--,
          tenant_id: "embed",
          conversation_id: convForLocal,
          role: "ASSISTANT",
          content: finalContent,
          agent_name: lastSpeaker,
          created_at: new Date().toISOString(),
        });
      }

      if (appended.length > 0) {
        setMessages((prev) => [...prev, ...appended]);
      }
    } catch (err) {
      if (!controller.signal.aborted) {
        setErrorState(classifyEmbedError(err));
      }
    } finally {
      setIsStreaming(false);
      setStreamingText("");
      setStreamingTools([]);
      abortRef.current = null;
    }
  }, [classifyEmbedError, conversationId, inputText, isStreaming, token, tx]);

  return (
    <div
      data-testid="agent-embed-page"
      className="min-h-screen bg-background text-foreground flex flex-col"
    >
      {/* Top Bar: Left Language Switcher + Right Borderless Sun/Moon IconButton */}
      <div className="flex items-center justify-between px-4 py-2 shrink-0">
        <button
          type="button"
          data-testid="locale-toggle-btn"
          onClick={toggleLocale}
          aria-label={tx("切换语言", "Switch language")}
          title={tx("切换为 English", "Switch to 简体中文")}
          className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium text-muted-foreground hover:text-foreground hover:bg-muted transition-colors shrink-0 cursor-pointer"
        >
          <Languages className="h-3.5 w-3.5 text-xblue" />
          <span className="font-mono-tech">{isZh ? "中文" : "EN"}</span>
        </button>

        <IconButton
          data-testid="theme-toggle-btn"
          onClick={toggleTheme}
          aria-label={
            isDark
              ? tx("亮色模式", "Light mode")
              : tx("暗色模式", "Dark mode")
          }
          title={
            isDark
              ? tx("亮色模式", "Light mode")
              : tx("暗色模式", "Dark mode")
          }
        >
          {isDark ? (
            <Sun className="h-4 w-4" />
          ) : (
            <Moon className="h-4 w-4" />
          )}
        </IconButton>
      </div>

      <div className="flex-1 min-h-0 flex items-center justify-center px-3 pb-3 sm:px-6 sm:pb-4">
        <Card className="w-full max-w-3xl h-full max-h-[calc(100dvh-56px)] min-h-[360px] flex flex-col overflow-hidden shadow-xs">
          {/* Minimalist Standalone Embed Header */}
          <header className="border-b border-border px-4 py-3 flex items-center justify-between gap-2 bg-muted/20">
            <div className="flex items-center gap-2 min-w-0">
              <span className="inline-block h-2 w-2 rounded-full bg-xblue shrink-0" />
              <Bot className="h-4 w-4 text-xblue shrink-0" />
              <h1 className="text-sm font-semibold tracking-tight truncate">
                {tx("智能对话", "Assistant")}
              </h1>
              {conversationId ? (
                <Badge variant="xblue" data-testid="embed-conv-badge">
                  {tx("多轮会话", "Multi-turn")}
                </Badge>
              ) : (
                <Badge variant="outline">
                  {token ? `embed:${token.slice(0, 10)}` : "embed"}
                </Badge>
              )}
            </div>

            <div className="flex items-center gap-1.5 shrink-0">
              <Button
                type="button"
                variant="outline"
                size="sm"
                data-testid="embed-reset-btn"
                onClick={handleResetConversation}
              >
                <RotateCcw className="h-3.5 w-3.5" />
                {tx("新对话", "New Chat")}
              </Button>
            </div>
          </header>

          {/* Error Banner for 404 / 403 */}
          {errorState ? (
            <div
              data-testid="embed-error-banner"
              data-error-status={errorState.status ?? "unknown"}
              className="mx-4 mt-3 rounded-md border border-border bg-muted p-3 text-xs space-y-1"
            >
              <div className="flex items-center gap-2 font-semibold text-foreground">
                <AlertTriangle className="h-4 w-4 text-xblue shrink-0" />
                <span>{errorState.title}</span>
              </div>
              <p className="text-muted-foreground pl-6">{errorState.detail}</p>
            </div>
          ) : null}

          {/* Conversation Messages Area */}
          <div
            data-testid="embed-messages"
            className="flex-1 p-4 overflow-y-auto"
          >
            <MessageView
              messages={messages}
              streamingToolCalls={streamingTools}
              streamingText={streamingText}
              streamingAgentName={streamingSpeaker}
              isStreaming={isStreaming}
              emptyHint={tx("输入问题开始对话", "Ask a question to begin")}
            />
          </div>

          {/* Input Composer */}
          <footer className="border-t border-border p-3 bg-muted/20">
            <div className="flex items-end gap-2">
              <Textarea
                data-testid="embed-chat-input"
                value={inputText}
                onChange={(e) => setInputText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    void handleSend();
                  }
                }}
                rows={2}
                placeholder={
                  token
                    ? tx("输入问题…", "Ask a question…")
                    : tx("缺少有效嵌入令牌", "Missing embed token")
                }
                disabled={!token || isStreaming}
                className="resize-none text-xs sm:text-sm min-h-[52px]"
              />
              {isStreaming ? (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => abortRef.current?.abort()}
                  className="h-[52px] px-3.5 shrink-0"
                >
                  <Square className="h-3.5 w-3.5" />
                  {tx("停止", "Stop")}
                </Button>
              ) : (
                <Button
                  type="button"
                  data-testid="embed-send-btn"
                  disabled={!token || inputText.trim().length === 0}
                  onClick={() => void handleSend()}
                  className="h-[52px] px-4 shrink-0"
                >
                  <Send className="h-3.5 w-3.5" />
                  {tx("发送", "Send")}
                </Button>
              )}
            </div>
          </footer>
        </Card>
      </div>
    </div>
  );
}
