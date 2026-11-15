import { useCallback, useEffect, useState } from "react";
import {
  AlertCircle,
  BookOpenCheck,
  Check,
  FileText,
  RefreshCw,
  X,
} from "lucide-react";
import {
  xblueApi,
  type AcceptNodeCandidateResponse,
  type KnowledgeNodeCandidate,
  type NodeTypeSpec,
} from "@/lib/api/xblue";
import { Badge, Button, Input } from "@/components/ui/primitives";

export interface KnowledgeCandidatesModalProps {
  open: boolean;
  mainNodeTypes: NodeTypeSpec[];
  onClose: () => void;
  onCandidateAccepted?: (
    res: AcceptNodeCandidateResponse,
    candidate: KnowledgeNodeCandidate
  ) => void;
}

function buildDefaultNodeKey(
  item: KnowledgeNodeCandidate,
  index: number
): string {
  const prefixRaw = (item.suggested_type_name || "NODE")
    .replace(/[^a-zA-Z0-9]/g, "")
    .toUpperCase();
  const prefix = prefixRaw.slice(0, 4) || "NODE";
  const asciiName = item.name.replace(/[^a-zA-Z0-9_-]/g, "");
  if (asciiName.length >= 2) {
    return asciiName.toUpperCase();
  }
  const suffix = item.entity_id
    ? item.entity_id.replace(/-/g, "").slice(-6).toUpperCase()
    : String(index + 1).padStart(3, "0");
  return `${prefix}-${suffix}`;
}

export function KnowledgeCandidatesModal({
  open,
  mainNodeTypes,
  onClose,
  onCandidateAccepted,
}: KnowledgeCandidatesModalProps) {
  const [candidates, setCandidates] = useState<KnowledgeNodeCandidate[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const [nodeKeyDrafts, setNodeKeyDrafts] = useState<Record<string, string>>(
    {}
  );
  const [typeNameDrafts, setTypeNameDrafts] = useState<Record<string, string>>(
    {}
  );
  const [acceptingId, setAcceptingId] = useState<string | null>(null);
  const [acceptedMap, setAcceptedMap] = useState<Record<string, string>>({});

  const fetchCandidates = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await xblueApi.listKnowledgeNodeCandidates();
      const items = (res.items ?? []).filter((c) => c.occur_count >= 3);
      setCandidates(items);
      setNodeKeyDrafts((prev) => {
        const next = { ...prev };
        items.forEach((item, idx) => {
          if (!next[item.entity_id]) {
            next[item.entity_id] = buildDefaultNodeKey(item, idx);
          }
        });
        return next;
      });
      setTypeNameDrafts((prev) => {
        const next = { ...prev };
        for (const item of items) {
          if (!next[item.entity_id]) {
            next[item.entity_id] =
              item.suggested_type_name || mainNodeTypes[0]?.name || "";
          }
        }
        return next;
      });
    } catch (err) {
      setErrorMsg(
        err instanceof Error ? err.message : "加载知识库候选实体列表失败"
      );
    } finally {
      setLoading(false);
    }
  }, [mainNodeTypes]);

  useEffect(() => {
    if (open) {
      void fetchCandidates();
    }
  }, [open, fetchCandidates]);

  if (!open) return null;

  const handleAccept = async (item: KnowledgeNodeCandidate) => {
    const typeName = (
      typeNameDrafts[item.entity_id] || item.suggested_type_name
    ).trim();
    const nodeKey = (nodeKeyDrafts[item.entity_id] || "").trim();
    if (!typeName) {
      setErrorMsg("请为候选实体指定目标节点类型 (type_name)");
      return;
    }
    if (!nodeKey) {
      setErrorMsg(`请为实体「${item.name}」输入非空业务主键 node_key`);
      return;
    }

    setAcceptingId(item.entity_id);
    setErrorMsg(null);
    try {
      const res = await xblueApi.acceptKnowledgeNodeCandidate(item.entity_id, {
        type_name: typeName,
        node_key: nodeKey,
        title: item.name,
      });
      setAcceptedMap((prev) => ({
        ...prev,
        [item.entity_id]: res.node.node_key || nodeKey,
      }));
      onCandidateAccepted?.(res, item);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "确认建点请求失败");
    } finally {
      setAcceptingId(null);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      data-testid="knowledge-candidates-modal"
    >
      <div className="w-full max-w-3xl max-h-[86vh] flex flex-col rounded-md border border-border bg-card shadow-xl">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div className="flex items-center gap-2">
            <BookOpenCheck className="h-4 w-4 text-xblue" />
            <div>
              <h2 className="text-sm font-semibold">
                知识候选建点 · 超图高频实体反哺业务节点
              </h2>
              <p className="text-[11px] text-muted-foreground">
                筛选知识超图中未链接业务节点、类型对齐且提及次数 ≥ 3 的高频实体，一键晋升为企业主图节点并确认实体链接
              </p>
            </div>
          </div>

          <div className="flex items-center gap-1.5">
            <Button
              type="button"
              variant="outline"
              className="h-7 px-2.5 py-1 text-xs"
              disabled={loading}
              onClick={() => void fetchCandidates()}
              data-testid="refresh-knowledge-candidates-btn"
            >
              <RefreshCw
                className={`h-3 w-3 ${loading ? "animate-spin" : ""}`}
              />
              刷新
            </Button>
            <button
              type="button"
              onClick={onClose}
              data-testid="close-knowledge-candidates-btn"
              className="rounded p-1 hover:bg-muted text-muted-foreground cursor-pointer"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-4 space-y-3 text-xs">
          {errorMsg && (
            <div className="rounded border border-red-500/40 bg-red-500/10 p-2.5 text-red-600 dark:text-red-400 flex items-center gap-1.5">
              <AlertCircle className="h-3.5 w-3.5 shrink-0" />
              <span>{errorMsg}</span>
            </div>
          )}

          {loading ? (
            <div className="py-10 text-center text-muted-foreground">
              正在检索知识超图未链接高频实体候选...
            </div>
          ) : candidates.length === 0 ? (
            <div
              className="rounded border border-dashed border-border py-10 text-center text-muted-foreground"
              data-testid="knowledge-candidates-empty"
            >
              当前暂无满足条件（未链接且提及频次 ≥ 3）的知识库候选实体。
            </div>
          ) : (
            <div
              className="space-y-2.5"
              data-testid="knowledge-candidates-list"
            >
              {candidates.map((item) => {
                const acceptedNodeKey = acceptedMap[item.entity_id];
                const isAccepting = acceptingId === item.entity_id;
                const selectedType =
                  typeNameDrafts[item.entity_id] ?? item.suggested_type_name;
                const typeOptions = Array.from(
                  new Set([
                    ...(item.suggested_type_name
                      ? [item.suggested_type_name]
                      : []),
                    ...mainNodeTypes.map((nt) => nt.name),
                  ])
                );

                return (
                  <div
                    key={item.entity_id}
                    data-testid={`knowledge-candidate-item-${item.entity_id}`}
                    className="rounded border border-border bg-background p-3 space-y-2.5"
                  >
                    <div className="flex flex-wrap items-start justify-between gap-2">
                      <div className="space-y-1">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span className="text-sm font-semibold">
                            {item.name}
                          </span>
                          <Badge variant="outline">{item.entity_type}</Badge>
                          <Badge variant="xblue">
                            推荐类型: {item.suggested_type_name}
                          </Badge>
                          <Badge variant="neutral">
                            提及 {item.occur_count} 次
                          </Badge>
                          <span className="font-mono-tech text-[11px] text-muted-foreground">
                            置信度 {(item.confidence * 100).toFixed(0)}%
                          </span>
                        </div>

                        {item.sample_docs.length > 0 && (
                          <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
                            <FileText className="h-3 w-3 text-xblue shrink-0" />
                            <span>来源文档:</span>
                            {item.sample_docs.map((docTitle, dIdx) => (
                              <span
                                key={`${item.entity_id}-doc-${dIdx}`}
                                className="rounded bg-muted px-1.5 py-0.5 text-foreground"
                              >
                                {docTitle}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>

                      {acceptedNodeKey && (
                        <Badge
                          variant="xblue"
                          data-testid={`candidate-accepted-badge-${item.entity_id}`}
                        >
                          <Check className="h-3 w-3 mr-1" />
                          已建点 ({acceptedNodeKey})
                        </Badge>
                      )}
                    </div>

                    {!acceptedNodeKey && (
                      <div className="grid grid-cols-1 md:grid-cols-12 gap-2 items-end border-t border-border pt-2">
                        <div className="md:col-span-4">
                          <label className="block text-[11px] text-muted-foreground mb-0.5">
                            目标节点类型 (type_name)
                          </label>
                          {typeOptions.length > 0 ? (
                            <select
                              data-testid={`candidate-type-select-${item.entity_id}`}
                              value={selectedType}
                              onChange={(e) =>
                                setTypeNameDrafts((prev) => ({
                                  ...prev,
                                  [item.entity_id]: e.target.value,
                                }))
                              }
                              className="h-7 w-full rounded border border-input bg-background px-2 font-mono-tech text-xs"
                            >
                              {typeOptions.map((tn) => (
                                <option key={tn} value={tn}>
                                  {tn}
                                </option>
                              ))}
                            </select>
                          ) : (
                            <Input
                              data-testid={`candidate-type-select-${item.entity_id}`}
                              value={selectedType}
                              onChange={(e) =>
                                setTypeNameDrafts((prev) => ({
                                  ...prev,
                                  [item.entity_id]: e.target.value,
                                }))
                              }
                              placeholder="Warehouse"
                              className="h-7 px-2 text-xs font-mono-tech"
                            />
                          )}
                        </div>

                        <div className="md:col-span-5">
                          <label className="block text-[11px] text-muted-foreground mb-0.5">
                            业务主键 (node_key)
                          </label>
                          <Input
                            data-testid={`candidate-node-key-input-${item.entity_id}`}
                            value={nodeKeyDrafts[item.entity_id] ?? ""}
                            onChange={(e) =>
                              setNodeKeyDrafts((prev) => ({
                                ...prev,
                                [item.entity_id]: e.target.value,
                              }))
                            }
                            placeholder="例如 W-HZ-01"
                            className="h-7 px-2 text-xs font-mono-tech"
                          />
                        </div>

                        <div className="md:col-span-3">
                          <Button
                            type="button"
                            className="h-7 w-full px-2.5 py-1 text-xs"
                            data-testid={`accept-candidate-btn-${item.entity_id}`}
                            disabled={
                              isAccepting ||
                              !(nodeKeyDrafts[item.entity_id] ?? "").trim()
                            }
                            onClick={() => void handleAccept(item)}
                          >
                            <Check className="h-3 w-3" />
                            {isAccepting ? "建点中..." : "确认建点"}
                          </Button>
                        </div>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between border-t border-border px-4 py-2.5 text-[11px] text-muted-foreground">
          <span>
            建点后将自动写入企业主图并触发 KNOWLEDGE_SYNC 超图双向实体对齐
          </span>
          <Button
            type="button"
            variant="outline"
            className="h-7 px-3 py-1 text-xs"
            onClick={onClose}
          >
            关闭
          </Button>
        </div>
      </div>
    </div>
  );
}
