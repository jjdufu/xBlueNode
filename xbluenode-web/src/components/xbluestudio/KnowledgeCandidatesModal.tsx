import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertCircle,
  BookOpenCheck,
  Check,
  FileText,
  RefreshCw,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import {
  xblueApi,
  type AcceptNodeCandidateResponse,
  type KnowledgeNodeCandidate,
  type NodeTypeSpec,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Input,
  Modal,
  Select,
} from "@/components/ui/primitives";

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
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

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
  const mainNodeTypesRef = useRef(mainNodeTypes);
  useEffect(() => {
    mainNodeTypesRef.current = mainNodeTypes;
  }, [mainNodeTypes]);

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
        const fallbackType = mainNodeTypesRef.current[0]?.name || "";
        for (const item of items) {
          if (!next[item.entity_id]) {
            next[item.entity_id] = item.suggested_type_name || fallbackType;
          }
        }
        return next;
      });
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("候选实体加载失败", "Failed to load candidates")
      );
    } finally {
      setLoading(false);
    }
  }, [tx]);

  useEffect(() => {
    if (open) {
      void fetchCandidates();
    }
  }, [open, fetchCandidates]);

  const handleAccept = async (item: KnowledgeNodeCandidate) => {
    if (!canEdit) return;
    const typeName = (
      typeNameDrafts[item.entity_id] || item.suggested_type_name
    ).trim();
    const nodeKey = (nodeKeyDrafts[item.entity_id] || "").trim();
    if (!typeName) {
      setErrorMsg(tx("需指定节点类型", "Node type required"));
      return;
    }
    if (!nodeKey) {
      setErrorMsg(tx("业务主键为空", "Node key required"));
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
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("确认建点失败", "Failed to create node")
      );
    } finally {
      setAcceptingId(null);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      testId="knowledge-candidates-modal"
      closeTestId="close-knowledge-candidates-btn"
      maxWidthClass="max-w-3xl"
      title={
        <>
          <BookOpenCheck className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("知识候选建点", "Knowledge Entity Candidates")}</span>
        </>
      }
      footer={
        <>
          <Button
            type="button"
            variant="outline"
            disabled={loading}
            onClick={() => void fetchCandidates()}
            data-testid="refresh-knowledge-candidates-btn"
          >
            <RefreshCw
              className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
            />
            {tx("刷新", "Refresh")}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            {tx("关闭", "Close")}
          </Button>
        </>
      }
    >
      {errorMsg && (
        <div className="rounded border border-red-500/40 bg-red-500/10 p-2.5 text-red-600 dark:text-red-400 flex items-center gap-1.5">
          <AlertCircle className="h-3.5 w-3.5 shrink-0" />
          <span>{errorMsg}</span>
        </div>
      )}

      {loading ? (
        <div className="py-10 text-center text-muted-foreground">
          {tx("检索高频实体候选中…", "Loading high-frequency entities…")}
        </div>
      ) : candidates.length === 0 ? (
        <div
          className="rounded border border-dashed border-border py-10 text-center text-muted-foreground"
          data-testid="knowledge-candidates-empty"
        >
          {tx("暂无待建点候选实体", "No candidate entities")}
        </div>
      ) : (
        <div className="space-y-2.5" data-testid="knowledge-candidates-list">
          {candidates.map((item) => {
            const acceptedNodeKey = acceptedMap[item.entity_id];
            const isAccepting = acceptingId === item.entity_id;
            const selectedType =
              typeNameDrafts[item.entity_id] ?? item.suggested_type_name;
            const typeOptions = Array.from(
              new Set([
                ...(item.suggested_type_name ? [item.suggested_type_name] : []),
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
                      <span className="text-sm font-semibold">{item.name}</span>
                      <Badge variant="outline">{item.entity_type}</Badge>
                      <Badge variant="xblue">
                        {tx("推荐:", "Suggested:")} {item.suggested_type_name}
                      </Badge>
                      <Badge variant="neutral">
                        {tx(`提及 ${item.occur_count} 次`, `${item.occur_count} mentions`)}
                      </Badge>
                      <span className="font-mono-tech text-[11px] text-muted-foreground">
                        {tx("置信度", "Confidence")}{" "}
                        {(item.confidence * 100).toFixed(0)}%
                      </span>
                    </div>

                    {item.sample_docs.length > 0 && (
                      <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
                        <FileText className="h-3 w-3 text-xblue shrink-0" />
                        <span>{tx("来源文档:", "Sources:")}</span>
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
                      variant="success"
                      data-testid={`candidate-accepted-badge-${item.entity_id}`}
                    >
                      <Check className="h-3 w-3 mr-1" />
                      {tx("已建点", "Created")} ({acceptedNodeKey})
                    </Badge>
                  )}
                </div>

                {!acceptedNodeKey && (
                  <div className="grid grid-cols-1 md:grid-cols-12 gap-2 items-end border-t border-border pt-2">
                    <div className="md:col-span-4">
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        {tx("目标节点类型", "Target Type")}
                      </label>
                      {typeOptions.length > 0 ? (
                        <Select
                          data-testid={`candidate-type-select-${item.entity_id}`}
                          value={selectedType}
                          disabled={!canEdit}
                          onChange={(e) =>
                            setTypeNameDrafts((prev) => ({
                              ...prev,
                              [item.entity_id]: e.target.value,
                            }))
                          }
                          className="h-8 w-full font-mono-tech"
                        >
                          {typeOptions.map((tn) => (
                            <option key={tn} value={tn}>
                              {tn}
                            </option>
                          ))}
                        </Select>
                      ) : (
                        <Input
                          data-testid={`candidate-type-select-${item.entity_id}`}
                          value={selectedType}
                          disabled={!canEdit}
                          onChange={(e) =>
                            setTypeNameDrafts((prev) => ({
                              ...prev,
                              [item.entity_id]: e.target.value,
                            }))
                          }
                          placeholder="Warehouse"
                          className="h-8 px-2 text-xs font-mono-tech"
                        />
                      )}
                    </div>

                    <div className="md:col-span-5">
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        {tx("业务主键", "Node Key")}
                      </label>
                      <Input
                        data-testid={`candidate-node-key-input-${item.entity_id}`}
                        value={nodeKeyDrafts[item.entity_id] ?? ""}
                        disabled={!canEdit}
                        onChange={(e) =>
                          setNodeKeyDrafts((prev) => ({
                            ...prev,
                            [item.entity_id]: e.target.value,
                          }))
                        }
                        placeholder="W-HZ-01"
                        className="h-8 px-2 text-xs font-mono-tech"
                      />
                    </div>

                    <div className="md:col-span-3">
                      <Button
                        type="button"
                        className="h-8 w-full text-xs"
                        data-testid={`accept-candidate-btn-${item.entity_id}`}
                        disabled={
                          !canEdit ||
                          isAccepting ||
                          !(nodeKeyDrafts[item.entity_id] ?? "").trim()
                        }
                        onClick={() => void handleAccept(item)}
                      >
                        <Check className="h-3 w-3" />
                        {isAccepting
                          ? tx("建点中…", "Creating…")
                          : tx("确认建点", "Accept")}
                      </Button>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </Modal>
  );
}
