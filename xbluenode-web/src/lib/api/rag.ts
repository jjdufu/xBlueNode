import { requestJson, requestMultipart } from "./client";

export type RecallChannel =
  | "vector"
  | "bm25"
  | "entity_link"
  | "context"
  | "multi_hop";

export interface HealthInfo {
  status: string;
  service: string;
  embedding_model: string;
  embedding_dim: number | null;
}

export interface SystemStats {
  doc_count: number;
  para_count: number;
  entity_count: number;
  edge_count: number;
  embedding_count: number;
  embedding_dims: number[];
  timestamp: string;
}

export interface IngestResponse {
  doc_id: string;
  title: string;
  para_count: number;
  entity_count: number;
  edge_count: number;
  embedding_model: string;
  embedding_dim: number;
}

export interface FusedHit {
  para_id: string;
  doc_id: string;
  doc_title: string;
  content: string;
  rrf_score: number;
  channels: RecallChannel[];
  traces: string[];
}

import type {
  AskCitation,
  AskIntent,
  ChartSuggestion,
  ChatQueryMode,
  QueryColumn,
  RelatedNodeSummary,
} from "./analytics";

export interface ChatQueryResponse {
  answer: string;
  hits: FusedHit[];
  seed_entities: string[];
  elapsed_ms: number;
  intent?: AskIntent;
  session_id?: string;
  turn_id?: string;
  citations?: AskCitation[];
  sql?: string;
  columns?: QueryColumn[];
  rows?: Record<string, unknown>[];
  chart?: ChartSuggestion | null;
  related_nodes?: RelatedNodeSummary[];
}

export interface TopologyNode {
  id: string;
  node_type: "DOC" | "PARA" | "ENTITY" | string;
  label: string;
  weight: number;
}

export interface TopologyEdge {
  id: string;
  source: string;
  target: string;
  rel_type: string;
  fact: string;
}

export interface TopologyGraph {
  nodes: TopologyNode[];
  edges: TopologyEdge[];
}

export interface DocumentSummary {
  id: string;
  title: string;
  source_name: string;
  summary: string;
  para_count: number;
  entity_count: number;
  created_at: string;
  updated_at: string;
}

export interface DocumentListResponse {
  items: DocumentSummary[];
  total: number;
  next_cursor: string | null;
}

/** Uploads a document via `POST /api/documents/upload` with Bearer authorization. */
export async function uploadDocument(file: File): Promise<IngestResponse> {
  const form = new FormData();
  form.append("file", file);
  return requestMultipart<IngestResponse>("/api/documents/upload", form);
}

export const api = {
  health: () => requestJson<HealthInfo>("/api/health"),
  stats: () => requestJson<SystemStats>("/api/stats"),
  topology: () => requestJson<TopologyGraph>("/api/graph/topology"),
  listDocuments: (params?: { q?: string; limit?: number; cursor?: string }) => {
    const qs = new URLSearchParams();
    if (params?.q && params.q.trim().length > 0) qs.set("q", params.q.trim());
    if (typeof params?.limit === "number") qs.set("limit", String(params.limit));
    if (params?.cursor) qs.set("cursor", params.cursor);
    const suffix = qs.toString();
    return requestJson<DocumentListResponse>(
      suffix ? `/api/documents?${suffix}` : "/api/documents"
    );
  },
  deleteDocument: (id: string) =>
    requestJson<void>(`/api/documents/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  ingestText: (payload: {
    title: string;
    source_name: string;
    content: string;
  }) =>
    requestJson<IngestResponse>("/api/documents/ingest", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  uploadFile: (file: File): Promise<IngestResponse> => uploadDocument(file),
  uploadDocument: (file: File): Promise<IngestResponse> => uploadDocument(file),
  chatQuery: (payload: {
    query: string;
    top_k?: number;
    session_id?: string;
    mode?: ChatQueryMode;
  }) =>
    requestJson<ChatQueryResponse>("/api/chat/query", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
};
