import { requestJson } from "./client";

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

export interface ChatQueryResponse {
  answer: string;
  hits: FusedHit[];
  seed_entities: string[];
  elapsed_ms: number;
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

export const api = {
  health: () => requestJson<HealthInfo>("/api/health"),
  stats: (tenantId = "default") =>
    requestJson<SystemStats>(`/api/stats?tenant_id=${encodeURIComponent(tenantId)}`),
  topology: (tenantId = "default") =>
    requestJson<TopologyGraph>(`/api/graph/topology?tenant_id=${encodeURIComponent(tenantId)}`),
  ingestText: (payload: {
    tenant_id?: string;
    title: string;
    source_name: string;
    content: string;
  }) =>
    requestJson<IngestResponse>("/api/documents/ingest", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  uploadFile: async (file: File, tenantId = "default"): Promise<IngestResponse> => {
    const form = new FormData();
    form.append("tenant_id", tenantId);
    form.append("file", file);
    return requestJson<IngestResponse>("/api/documents/upload", {
      method: "POST",
      body: form,
    });
  },
  chatQuery: (payload: { tenant_id?: string; query: string; top_k?: number }) =>
    requestJson<ChatQueryResponse>("/api/chat/query", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
};
