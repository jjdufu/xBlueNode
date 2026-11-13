import { useState } from "react";
import { Send } from "lucide-react";
import { api, type ChatQueryResponse, type RecallChannel } from "@/lib/api";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

const CHANNEL_LABEL: Record<RecallChannel, string> = {
  vector: "Vector 语义",
  bm25: "BM25 关键词",
  entity_link: "实体关联",
  context: "上下文扩展",
  multi_hop: "多跳推理",
};

export function AxQueryPage() {
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [resp, setResp] = useState<ChatQueryResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleAsk = async () => {
    if (!query.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const data = await api.chatQuery({ query: query.trim(), top_k: 6 });
      setResp(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="space-y-6">
      <div className="border-b border-border pb-4">
        <h1 className="text-lg font-semibold tracking-tight">AxQuery · 五路融合问答</h1>
        <p className="text-xs text-muted-foreground mt-0.5">
          并行触发 Vector / BM25 / EntityLink / Context / MultiHop 五路召回与 RRF 重排序
        </p>
      </div>

      <div className="flex gap-2">
        <Input
          placeholder="输入业务或知识库问题，例如：什么是动态超图架构？"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void handleAsk();
          }}
        />
        <Button onClick={() => void handleAsk()} disabled={loading || !query.trim()}>
          <Send className="h-3.5 w-3.5" />
          {loading ? "检索中..." : "提问"}
        </Button>
      </div>

      {error && (
        <Card className="p-4 border-foreground/40 bg-muted/40 text-xs">
          <span className="font-mono-tech font-semibold">请求失败：</span>
          {error}
        </Card>
      )}

      {resp && (
        <div className="grid grid-cols-1 lg:grid-cols-12 gap-5">
          <Card className="lg:col-span-7 p-5 space-y-4">
            <div className="flex items-center justify-between border-b border-border pb-2.5">
              <span className="text-sm font-medium">AxPoint 合成回答</span>
              <span className="text-[11px] font-mono-tech text-muted-foreground">
                耗时 {resp.elapsed_ms} ms
              </span>
            </div>
            <div className="text-sm leading-relaxed whitespace-pre-wrap">
              {resp.answer}
            </div>
            {resp.seed_entities.length > 0 && (
              <div className="pt-3 border-t border-border flex flex-wrap items-center gap-1.5">
                <span className="text-[11px] text-muted-foreground mr-1">
                  提取种子实体：
                </span>
                {resp.seed_entities.map((ent) => (
                  <Badge key={ent} variant="xblue">
                    {ent}
                  </Badge>
                ))}
              </div>
            )}
          </Card>

          <div className="lg:col-span-5 space-y-3">
            <div className="text-xs font-medium text-muted-foreground uppercase tracking-wider">
              五路召回溯源证据 ({resp.hits.length})
            </div>
            {resp.hits.map((hit, idx) => (
              <Card key={hit.para_id} className="p-3.5 space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-xs font-medium truncate">
                    [{idx + 1}] {hit.doc_title}
                  </span>
                  <span className="text-[11px] font-mono-tech text-xblue shrink-0">
                    RRF {hit.rrf_score.toFixed(4)}
                  </span>
                </div>
                <div className="flex flex-wrap gap-1">
                  {hit.channels.map((ch) => (
                    <Badge key={ch} variant={ch === "multi_hop" ? "xblue" : "neutral"}>
                      {CHANNEL_LABEL[ch]}
                    </Badge>
                  ))}
                </div>
                <p className="text-xs text-muted-foreground line-clamp-3 leading-relaxed">
                  {hit.content}
                </p>
                {hit.traces.length > 0 && (
                  <div className="text-[11px] font-mono-tech text-muted-foreground bg-muted/60 rounded px-2 py-1">
                    {hit.traces.join(" | ")}
                  </div>
                )}
              </Card>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
