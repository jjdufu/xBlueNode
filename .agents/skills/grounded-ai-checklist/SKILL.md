---
name: grounded-ai-checklist
description: >-
  Enforce the 5-layer Ontology-Grounded Zero-Hallucination AI architecture across
  both cloud LLM APIs and locally deployed 7B-32B models (Ollama / vLLM) when
  building NL2SQL, RAG, Studio Copilot, or AxAgent features.
---

# Ontology-Grounded Zero-Hallucination AI Checklist

Activate this skill whenever designing, implementing, or reviewing any LLM-powered feature in `axpoint-llm`, `axpoint-analytics`, `axpoint-agent`, `xBlueStudio`, `AxQuery`, `AxSim`, or `xBlueView`.

## 1. Five Mandatory Engineering Guardrails

1. **Zero LLM Mental Arithmetic**
   - Never ask an LLM to sum, average, rank, compute ratios/deltas, or simulate causal impacts in prose.
   - Push all calculations to DuckDB (`axpoint-analytics`), `PgStore` graph/rollup queries, or `axpoint-domain::sim`.
2. **Ontology & Certified Metric Priority Routing**
   - Match user intent against certified `MetricDef`s and published `NodeTypeSpec` / `EdgeTypeSpec` definitions before falling back to SQL generation.
   - When matched, extract structured parameter slots and compile deterministically via `semantic.rs`.
   - When multi-table SQL is generated, validate AST whitelist (`sql_guard::guard`), `EXPLAIN` dry-run, and declared `MappingLinkSpec` join paths.
3. **Constrained Decoding & Deterministic Sampling (Cloud + Local 7B–32B Models)**
   - Lock `temperature = 0.0` (and fixed `seed` when supported) for `NL2SQL`, `EXTRACT`, and `SUMMARIZE` purposes.
   - Bind a strict Draft 2020-12 JSON Schema (`validate_instance_against_schema`) to every structured LLM call; pass `response_format: { type: "json_schema", ... }` with dynamic tenant enum lists (`NodeType`, `MetricDef`, `RollupGrain`) so local vLLM / Ollama decoders mask invalid tokens at sampling time.
   - Keep linked schema context compact (`<= 5` tables, `<= 15` columns/table, `<= 5` metrics) so local small models do not lose attention.
4. **Post-Generation Verifier & Explicit Abstention**
   - Pass all tabular natural-language summaries through `verify_or_fallback_summary` so any ungrounded number triggers immediate deterministic fallback.
   - Verify `[文N]` / `[Doc N]` citation indices in `DOC` / `HYBRID` responses against `1..=recalled_chunks.len()`; strip phantom citations and return deterministic abstention/clarification when recall hits are empty or ambiguous.
5. **Egress PII Tokenization & Draft Mutation Isolation**
   - Route all outbound prompts through `redact_messages_for_egress` and restore tokens via `restore_from_egress` according to caller `PiiRawRead` permission.
   - Write all AI-proposed schema, flow, or dashboard edits into `DRAFT` / `PROPOSED` state after recording a restorable snapshot; never mutate `PROD` directly.

## 2. Verification Commands

```bash
python3 scripts/hallucination-guard.py
cd axpoint-core && cargo test -p axpoint-analytics && cargo test -p axpoint-llm && cargo test -p axpoint-agent
```
