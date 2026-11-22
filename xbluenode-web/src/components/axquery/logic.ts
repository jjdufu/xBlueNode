import type { ChatQueryScope } from "../../lib/api/analytics.ts";

export interface AtMentionMatch {
  readonly start: number;
  readonly end: number;
  readonly keyword: string;
}

function dedupeStrings(items: readonly string[] | undefined): string[] {
  if (!Array.isArray(items)) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of items) {
    if (typeof raw !== "string") continue;
    const trimmed = raw.trim();
    if (trimmed.length > 0 && !seen.has(trimmed)) {
      seen.add(trimmed);
      out.push(trimmed);
    }
  }
  return out;
}

export function normalizeChatQueryScope(
  scope: ChatQueryScope | null | undefined
): ChatQueryScope | undefined {
  if (!scope) return undefined;
  const datasetIds = dedupeStrings(scope.dataset_ids);
  const docIds = dedupeStrings(scope.doc_ids);
  const endpointSlugs = dedupeStrings(scope.endpoint_slugs);
  const docTitlePrefix =
    typeof scope.doc_title_prefix === "string" &&
    scope.doc_title_prefix.trim().length > 0
      ? scope.doc_title_prefix.trim()
      : undefined;
  const subgraphSlug =
    typeof scope.subgraph_slug === "string" &&
    scope.subgraph_slug.trim().length > 0
      ? scope.subgraph_slug.trim()
      : undefined;

  if (
    datasetIds.length === 0 &&
    docIds.length === 0 &&
    !docTitlePrefix &&
    !subgraphSlug &&
    endpointSlugs.length === 0
  ) {
    return undefined;
  }

  const out: ChatQueryScope = {};
  if (datasetIds.length > 0) out.dataset_ids = datasetIds;
  if (docIds.length > 0) out.doc_ids = docIds;
  if (docTitlePrefix) out.doc_title_prefix = docTitlePrefix;
  if (subgraphSlug) out.subgraph_slug = subgraphSlug;
  if (endpointSlugs.length > 0) out.endpoint_slugs = endpointSlugs;
  return out;
}

export function hasActiveChatScope(
  scope: ChatQueryScope | null | undefined
): boolean {
  return normalizeChatQueryScope(scope) !== undefined;
}

export function formatAppliedScopeBadge(
  scope: ChatQueryScope | null | undefined,
  isZh = true
): string | null {
  const norm = normalizeChatQueryScope(scope);
  if (!norm) return null;
  const parts: string[] = [];
  if (norm.dataset_ids && norm.dataset_ids.length > 0) {
    const n = norm.dataset_ids.length;
    parts.push(isZh ? `${n} 表` : `${n} ${n === 1 ? "table" : "tables"}`);
  }
  if (norm.doc_ids && norm.doc_ids.length > 0) {
    const n = norm.doc_ids.length;
    parts.push(isZh ? `${n} 文档` : `${n} ${n === 1 ? "doc" : "docs"}`);
  }
  if (norm.doc_title_prefix) {
    parts.push(
      isZh
        ? `前缀: ${norm.doc_title_prefix}`
        : `Prefix: ${norm.doc_title_prefix}`
    );
  }
  if (norm.subgraph_slug) {
    parts.push(isZh ? "1 子图" : "1 subgraph");
  }
  if (norm.endpoint_slugs && norm.endpoint_slugs.length > 0) {
    const n = norm.endpoint_slugs.length;
    parts.push(isZh ? `${n} 技能` : `${n} ${n === 1 ? "skill" : "skills"}`);
  }
  if (parts.length === 0) return null;
  return isZh ? `已限定 ${parts.join(" · ")}` : `Scoped ${parts.join(" · ")}`;
}

export function detectAtMention(
  text: string,
  caretIndex?: number | null
): AtMentionMatch | null {
  const end =
    typeof caretIndex === "number" && caretIndex >= 0 && caretIndex <= text.length
      ? caretIndex
      : text.length;
  const prefix = text.slice(0, end);
  const atPos = prefix.lastIndexOf("@");
  if (atPos < 0) return null;

  if (atPos > 0) {
    const prevChar = prefix[atPos - 1] ?? "";
    if (/[A-Za-z0-9_]/.test(prevChar)) {
      return null;
    }
  }

  const keyword = prefix.slice(atPos + 1);
  if (/[\s\r\n@]/.test(keyword)) {
    return null;
  }

  return {
    start: atPos,
    end,
    keyword,
  };
}

export function stripAtMention(
  text: string,
  mention: Pick<AtMentionMatch, "start" | "end">
): string {
  const before = text.slice(0, mention.start);
  const after = text.slice(mention.end);
  const joined = `${before}${after}`;
  return joined.replace(/ {2,}/g, " ").trimStart();
}
