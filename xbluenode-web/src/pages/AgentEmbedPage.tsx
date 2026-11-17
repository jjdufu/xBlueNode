import { useParams } from "react-router";

export function AgentEmbedPage() {
  const { token } = useParams<{ token: string }>();

  return (
    <div className="min-h-screen bg-background text-foreground flex items-center justify-center p-4">
      <div className="w-full max-w-2xl rounded-lg border border-border p-6 space-y-2">
        <h1 className="text-base font-semibold tracking-tight">
          AxAgent · 智能体对话窗口
        </h1>
        <p className="text-xs text-muted-foreground font-mono-tech">
          {token ? `embed:${token.slice(0, 8)}` : "embed"}
        </p>
      </div>
    </div>
  );
}
