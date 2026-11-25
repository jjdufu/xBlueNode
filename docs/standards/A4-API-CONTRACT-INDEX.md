# A4 · 接口契约索引

本附录列出 Core（`:9995`）与 Sidecar（`:8001`，仅内部网络）的全部接口，并指向定义所在的 Step 小节。请求与响应字段以 Step 文件为准，`docs/API_CONTRACT.md` 按本索引组织面向调用方的说明。约定：

- 前缀、分页、错误体、序列化见 [A1](./A1-CONVENTIONS-ENUMS-ROUTES.md) §2、§5、§7。
- “鉴权”列：权限名表示需要 `AuthPrincipal` 且具备该权限（`A / B` 表示读 / 写分别需要的权限）；“公开”表示无需令牌；其他凭证见 [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.2。
- 列表接口均支持 `?limit=&cursor=`，响应 `{items, next_cursor}`。

---

## 1. 系统、认证与令牌

| 方法与路径 | 鉴权 | Step | 定义 |
| :--- | :--- | :--- | :--- |
| `GET /api/health` | 公开 | 仓库已有 | — |
| `GET /api/stats`、`GET /api/graph/topology` | `DATA_READ` | 仓库已有 / 00 | [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.6.5 |
| `GET /api/auth/me` | 已登录 | 00 | [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.8 |
| `GET /api/auth/tokens`、`POST /api/auth/tokens`、`DELETE /api/auth/tokens/{id}` | 已登录 | 09 | [08](../../plans/completed/v1-foundation-step00-14/08-STEP09-AXAGENT-PUBLISHING-MCP-AGENTS.md) §09.8 |
| `POST /api/auth/login`、`POST /api/auth/refresh`、`POST /api/auth/logout`、`POST /api/auth/logout-all` | 见 §13.2 | 13 | [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.3～§13.4 |
| `GET /api/auth/oidc/start`、`GET /api/auth/oidc/callback` | 公开（state / PKCE） | 13 | [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.5 |
| `GET /api/auth/jwks.json`、`GET /api/auth/config` | 公开 | 13 | [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.4.1 |
| `POST /api/auth/password` | 已登录 | 13 | [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.3 |
| `GET /metrics`（只监听 `AXPOINT_METRICS_ADDR`） | 网络隔离 | 00 | [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.6.7 |

## 2. AxQuery 与文档（`/api/documents`、`/api/chat`）

| 方法与路径 | 鉴权 | Step | 定义 |
| :--- | :--- | :--- | :--- |
| `GET /api/documents`、`DELETE /api/documents/{id}` | `DATA_READ` / `DATA_SYNC` | 仓库已有 / 00 | [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.6.5 |
| `POST /api/documents/ingest`、`POST /api/documents/upload` | `DATA_SYNC` | 仓库已有 / 00 | [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.6.5 |
| `POST /api/chat/query`（`session_id?`、`mode?: AUTO|DOC|DATA`） | `DATA_READ` | 仓库已有 / 04 | [05](../../plans/completed/v1-foundation-step00-14/05-STEP04-AXANALYTICS-OLAP-AND-NL2SQL.md) §04.8 |

## 3. AxHub（`/api/axhub`）

| 方法与路径 | 鉴权 | 定义 |
| :--- | :--- | :--- |
| `GET /connector-types` | `DATA_READ` | [03](../../plans/completed/v1-foundation-step00-14/03-STEP02-AXHUB-CONNECTORS-AND-SYNC.md) §02.9.5 |
| `GET /connections`、`POST /connections`、`GET /connections/{id}`、`PATCH /connections/{id}`、`DELETE /connections/{id}` | `DATA_READ` / `DATA_ADMIN` | 同上 |
| `POST /connections/{id}/test`、`GET /connections/{id}/discover` | `DATA_ADMIN` | 同上 |
| `POST /connections/{id}/preview` | `DATA_READ` | 同上 |
| `POST /credentials/rotate` | `DATA_ADMIN` | §02.2 |
| `GET /datasets`、`POST /datasets`、`GET /datasets/{id}`、`PATCH /datasets/{id}`、`DELETE /datasets/{id}` | `DATA_READ` / `DATA_SYNC` / `DATA_ADMIN` | §02.9.5 |
| `POST /datasets/{id}/sync`、`GET /datasets/{id}/sync-jobs`、`GET /datasets/{id}/preview`、`GET /datasets/{id}/snapshots` | `DATA_SYNC` / `DATA_READ` | 同上 |
| `GET /datasets/{id}/columns`、`PUT /datasets/{id}/columns/{column}` | `DATA_READ` / `DATA_ADMIN` | 同上 |
| `GET /datasets/{id}/quality-rules`、`POST /datasets/{id}/quality-rules`、`DELETE /quality-rules/{rule_id}` | `DATA_READ` / `DATA_ADMIN` | §02.9.4 |
| `GET /datasets/{id}/lineage` | `DATA_READ` | §02.9.5 |
| `POST /uploads`、`GET /uploads/{id}` | `DATA_SYNC` / `DATA_READ` | §02.6.1 |
| `POST /webhooks`、`GET /webhooks`、`DELETE /webhooks/{id}` | `DATA_ADMIN` | §02.7.3 |
| `POST /webhooks/{tenant_id}/{slug}/ingest` | Webhook 秘密（HMAC / Bearer） | §02.7.3 |
| `GET /catalog/search`、`GET /glossary`、`POST /glossary`、`DELETE /glossary/{id}` | `DATA_READ` / `DATA_ADMIN` | §02.9.5 |

## 4. AxGraph 与 xBlueStudio（`/api/xblue`）

| 方法与路径 | 鉴权 | 定义 |
| :--- | :--- | :--- |
| `GET /nodes`、`POST /nodes`、`POST /nodes/bulk`、`GET /nodes/{id}`、`PATCH /nodes/{id}`、`DELETE /nodes/{id}`、`GET /nodes/search`、`POST /geo/convex-hull` | `DATA_READ` / `STUDIO_EDIT` | [04](../../plans/completed/v1-foundation-step00-14/04-STEP03-AXGRAPH-STORAGE-AND-GRAPH-QUERY.md) §03.7、[06-STEP28](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/06-STEP28-SIM-EVENT-QUEUE-DEADLINE-AND-RUST-GEOSHAPE.md) §28.3 |
| `GET /nodes/{id}/documents`、`GET /nodes/{id}/metrics/{metric}` | `DATA_READ` | 同上 |
| `GET /links`、`POST /links`、`DELETE /links/{id}`、`POST /links/metrics/write`、`GET /links/{id}/explain/{metric}` | `DATA_READ` / `STUDIO_EDIT` / `DATA_SYNC` | 同上、[04-STEP26](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md) §26.3 |
| `POST /metrics/write` | `DATA_SYNC` | 同上 |
| `POST /metrics/window`、`GET /metrics/catalog` | `DATA_READ` | 同上 |
| `GET /graph/neighbors`、`GET /graph/path`、`GET /graph/subgraph` | `DATA_READ` | 同上 |
| `POST /graph/algorithms/{algo}`、`GET /graph/algorithms/{algo}/results` | `STUDIO_EDIT` / `DATA_READ` | 同上 |
| `POST /entities/resolve`、`GET /entities/candidates`、`POST /entities/links/{entity_id}/{node_id}/confirm`、`POST /entities/links/{entity_id}/{node_id}/reject` | `STUDIO_EDIT` | 同上 |
| `GET /subgraphs`、`POST /subgraphs`、`PATCH /subgraphs/{slug}`、`DELETE /subgraphs/{slug}` | `DATA_READ` / `STUDIO_EDIT` | [06](../../plans/completed/v1-foundation-step00-14/06-STEP05-06-XBLUESTUDIO-MODELING-AND-ROLLUP.md) §05.7 |
| `POST /subgraphs/{slug}/merge-preview`、`POST /subgraphs/{slug}/merge`、`POST /subgraphs/{slug}/dry-run-materialize`、`POST /subgraphs/{slug}/reset-nodes` | `STUDIO_EDIT` / `STUDIO_PUBLISH` / `DATA_SYNC` | §05.7、[03](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/03-STEP17-AXBUILDER-MCP-AND-DEV-SUBGRAPH.md) §17.4 |
| `GET /subgraphs/{slug}/lineage/{node_type}`、`GET /subgraphs/{slug}/spec-doc` | `DATA_READ` | [04-STEP18](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/04-STEP18-STUDIO-COPILOT-LINEAGE-AND-SPEC-DOC.md) §18.4、§18.5 |
| `POST /subgraphs/from-template`、`POST /subgraphs/{slug}/package-agent`、`GET /subgraphs/{slug}/consumption`、`POST /subgraphs/{slug}/certify`、`GET /subgraphs/{slug}/competency-suite`、`PUT /subgraphs/{slug}/competency-suite`、`POST /subgraphs/{slug}/competency-verify`、`GET /subgraphs/{slug}/harness-runs` | `STUDIO_EDIT` / `AGENT_EDIT` / `DATA_READ` | [05-STEP19](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/05-STEP19-ONBOARDING-TEMPLATE-HUB-AND-APP-PACKAGING.md) §19.2、§19.3；[04-STEP26](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md) §26.4；[07-STEP29](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/07-STEP29-STREAM-WORKERS-LOG-CDC-AND-AUTONOMOUS-HARNESS.md) §29.4 |
| `GET /schema`、`POST /schema`、`PUT /schema`、`POST /schema/validate` | `DATA_READ` / `STUDIO_PUBLISH` / `STUDIO_EDIT` | §05.7 |
| `GET /schema/versions`、`GET /schema/versions/{v}`、`GET /schema/versions/{a}/diff/{b}`、`POST /schema/versions/{v}/restore` | `DATA_READ` / `STUDIO_PUBLISH` | 同上 |
| `GET /drafts`、`POST /drafts`、`GET /drafts/{id}`、`PUT /drafts/{id}`、`POST /drafts/{id}/discard`、`POST /drafts/ai`、`POST /drafts/{id}/publish` | `STUDIO_EDIT` / `STUDIO_PUBLISH` | 同上 |
| `POST /drafts/{id}/copilot`、`GET /drafts/{id}/snapshots`、`POST /drafts/{id}/snapshots/{snap_id}/restore`、`POST /drafts/{id}/confirm-proposals` | `DATA_READ` / `STUDIO_EDIT` | [04-STEP18](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/04-STEP18-STUDIO-COPILOT-LINEAGE-AND-SPEC-DOC.md) §18.3 |
| `GET /mappings`、`GET /mappings/{id}/runs`、`POST /mappings/{id}/run`、`POST /mappings/{id}/preview` | `DATA_READ` / `STUDIO_EDIT` | 同上 |
| `GET /objects`、`GET /objects/{type_name}`、`GET /objects/{type_name}/{key}`、`POST /objects/{type_name}/{key}/actions/{action_name}` | `DATA_READ` / `FLOW_RUN` | §05.6、§05.7、[04-STEP26](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md) §26.2 |
| `POST /graphql`、`GET /graphql/sdl` | `DATA_READ` | §05.6、§05.7 |
| `POST /expr/validate`、`POST /expr/evaluate`、`POST /aggregate/recompute`、`GET /nodes/{id}/explain/{metric}` | `STUDIO_EDIT` / `DATA_READ` | [06](../../plans/completed/v1-foundation-step00-14/06-STEP05-06-XBLUESTUDIO-MODELING-AND-ROLLUP.md) §06.6 |
| `GET /knowledge/node-candidates`、`POST /knowledge/node-candidates/{entity_id}/accept` | `STUDIO_EDIT` | 同上 |

## 5. AxAnalytics（`/api/analytics`）

| 方法与路径 | 鉴权 | 定义 |
| :--- | :--- | :--- |
| `GET /catalog` | `DATA_READ` | [05](../../plans/completed/v1-foundation-step00-14/05-STEP04-AXANALYTICS-OLAP-AND-NL2SQL.md) §04.9 |
| `POST /sql/run`（`{sql, params?, max_rows?, timeout_ms?, allow_live?, reveal_pii?}`）、`POST /sql/explain` | `DATA_READ` | §04.3；`reveal_pii` 见 [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.7 |
| `POST /axql/compile`、`POST /axql/run` | `DATA_READ` | [05-STEP27](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/05-STEP27-AXQL-ONTOLOGY-COMPILER-AND-DYNAMIC-VISIBILITY.md) §27.2 |
| `POST /pivot` | `DATA_READ` | §04.4 |
| `GET /metrics`、`POST /metrics`、`GET /metrics/{name}`、`PATCH /metrics/{name}`、`DELETE /metrics/{name}` | `DATA_READ` / `STUDIO_EDIT` | §04.5 |
| `POST /metrics/{name}/query`、`POST /metrics/query-multi` | `DATA_READ` | 同上 |
| `GET /saved-queries`、`POST /saved-queries`、`GET /saved-queries/{id}`、`PATCH /saved-queries/{id}`、`DELETE /saved-queries/{id}`、`POST /saved-queries/{id}/run` | `DATA_READ` / `VIEW_EDIT` | §04.9 |
| `POST /ask`（可选 `context`，见 [10](../../plans/completed/v1-foundation-step00-14/10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §11.6）、`POST /ask/{turn_id}/feedback`、`GET /ask/sessions/{session_id}` | `DATA_READ` | §04.6、[02-STEP16](../../plans/completed/v2-ux-and-grounded-ai-step15-22/grounded-ai/02-STEP16-ONTOLOGY-GROUNDED-ASK-AND-CITATION-GUARD.md) §16.2 |
| `POST /grounding-eval/run`、`GET /grounding-eval/runs` | `GUARD_ADMIN` \| `OPS_ADMIN` / `DATA_READ` | [02-STEP16](../../plans/completed/v2-ux-and-grounded-ai-step15-22/grounded-ai/02-STEP16-ONTOLOGY-GROUNDED-ASK-AND-CITATION-GUARD.md) §16.5 |
| `POST /transform/draft`、`PATCH /transform/{id}`、`POST /transform/{id}/apply`、`POST /transform/{id}/discard` | `DATA_SYNC` | §04.7 |
| `GET /history` | `DATA_READ` | §04.9 |

## 6. AxFlow（`/api/axflow`）

| 方法与路径 | 鉴权 | 定义 |
| :--- | :--- | :--- |
| `GET /operators` | `DATA_READ` | [07](../../plans/completed/v1-foundation-step00-14/07-STEP07-08-AXFLOW-ORCHESTRATION-RULES-ALERTS.md) §07.8 |
| `GET /flows`、`POST /flows`、`GET /flows/{id}`、`PUT /flows/{id}`、`DELETE /flows/{id}`、`POST /flows/{id}/validate` | `DATA_READ` / `FLOW_EDIT` | 同上 |
| `GET /flows/{id}/versions`、`POST /flows/{id}/versions`、`POST /flows/{id}/deploy` | `DATA_READ` / `FLOW_PUBLISH`（`TEST` 部署为 `FLOW_EDIT`） | 同上 |
| `POST /flows/{id}/runs`、`POST /flows/{id}/run`（等价于 `TEST` 运行） | `FLOW_RUN` | 同上 |
| `GET /flows/{id}/runs`、`GET /runs/{run_id}`、`GET /runs/{run_id}/steps`、`POST /runs/{run_id}/cancel`、`POST /runs/{run_id}/retry` | `DATA_READ` / `FLOW_RUN` | 同上 |
| `POST /flows/{id}/hook` | `FLOW_PUBLISH` | 同上 |
| `POST /hooks/{tenant_id}/{slug}` | 钩子秘密 | 同上 |
| `GET /approvals`、`POST /approvals/{id}/decide` | `DATA_READ` / `APPROVAL_DECIDE` | 同上 |
| `GET /actions` | `DATA_READ` | 同上 |
| `GET /rules`、`POST /rules`、`POST /rules/dry-run`、`GET /rules/{id}`、`PUT /rules/{id}`、`DELETE /rules/{id}`、`POST /rules/{id}/dry-run` | `DATA_READ` / `RULE_EDIT` | §08.6 |
| `GET /alerts`、`GET /alerts/{id}`、`GET /alerts/{id}/events` | `DATA_READ` | 同上 |
| `POST /alerts/{id}/ack`、`POST /alerts/{id}/resolve`、`POST /alerts/ack-batch` | `ALERT_ACK` | 同上 |
| `GET /channels`、`POST /channels`、`PUT /channels/{id}`、`DELETE /channels/{id}`、`POST /channels/{id}/test` | `DATA_READ` / `RULE_EDIT` | 同上 |
| `GET /silences`、`POST /silences`、`DELETE /silences/{id}` | `DATA_READ` / `ALERT_ACK` | 同上 |

## 7. AxAgent、已发布端点与 MCP（完整路径）

| 方法与路径 | 鉴权 | 定义 |
| :--- | :--- | :--- |
| `GET /api/axagent/providers`、`POST /api/axagent/providers`、`PUT /api/axagent/providers/{id}`、`DELETE /api/axagent/providers/{id}`、`POST /api/axagent/providers/{id}/test`、`GET /api/axagent/usage` | `AGENT_EDIT` | [08](../../plans/completed/v1-foundation-step00-14/08-STEP09-AXAGENT-PUBLISHING-MCP-AGENTS.md) §09.9 |
| `GET /api/axagent/llm-preferences`、`PUT /api/axagent/llm-preferences` | 已登录用户 | [01-STEP15](../../plans/completed/v2-ux-and-grounded-ai-step15-22/grounded-ai/01-STEP15-CONSTRAINED-DECODING-AND-LLM-CAPS.md) §15.4.2 |
| `GET /api/axagent/endpoints`、`POST /api/axagent/endpoints`、`PUT /api/axagent/endpoints/{id}`、`DELETE /api/axagent/endpoints/{id}`、`POST /api/axagent/endpoints/{id}/promote` | `DATA_READ` / `STUDIO_PUBLISH` | 同上 |
| `GET /api/axagent/endpoints/openapi.json`、`GET /api/axagent/endpoints/{id}/skill.json` | `DATA_READ` | 同上 |
| `POST /api/axagent/apps/{endpoint_id}/run` | 端点要求的权限 | 同上 |
| `POST /api/published/{env}/{tenant_id}/{slug}`、`GET /api/published/{env}/{tenant_id}/{slug}/runs/{run_id}` | `axp_` 个人访问令牌 | 同上 |
| `POST /api/mcp`、`GET /api/mcp`、`DELETE /api/mcp` | `axp_` 个人访问令牌 | 同上 |
| `GET /api/axagent/mcp-clients`、`POST /api/axagent/mcp-clients`、`DELETE /api/axagent/mcp-clients/{id}`、`POST /api/axagent/mcp-clients/{id}/sync` | `AGENT_EDIT` | 同上 |
| `GET /api/axagent/agents`、`POST /api/axagent/agents`、`GET /api/axagent/agents/{id}`、`PUT /api/axagent/agents/{id}`、`DELETE /api/axagent/agents/{id}`、`POST /api/axagent/agents/{id}/publish` | `DATA_READ` / `AGENT_EDIT` | 同上 |
| `POST /api/axagent/agents/{id}/chat` | `AGENT_RUN` | 同上 |
| `GET /api/axagent/conversations`、`GET /api/axagent/conversations/{id}/messages`、`DELETE /api/axagent/conversations/{id}` | `AGENT_RUN` | 同上 |
| `POST /api/axagent/agents/{id}/embeds`、`GET /api/axagent/agents/{id}/embeds`、`DELETE /api/axagent/embeds/{embed_id}` | `VIEW_SHARE` | 同上 |
| `POST /api/axagent/embed/{token}/chat` | `axe_` 嵌入令牌 | 同上 |
| `GET /api/axagent/im-connectors`、`POST /api/axagent/im-connectors`、`PATCH /api/axagent/im-connectors/{id}`、`DELETE /api/axagent/im-connectors/{id}`、`POST /api/axagent/im-connectors/{id}/test` | `AGENT_EDIT` | [07-STEP21](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/07-STEP21-IM-BIDIRECTIONAL-AGENT-CONNECTORS.md) §21.4.1 |
| `GET /api/axagent/im-bindings`、`DELETE /api/axagent/im-bindings/{id}`、`GET /api/axagent/im-bind/preview`、`POST /api/axagent/im-bind/confirm` | 已登录用户 / `GUARD_ADMIN` | §21.4.2 |

## 8. AxSim（`/api/axsim`）

| 方法与路径 | 鉴权 | 定义 |
| :--- | :--- | :--- |
| `GET /sandboxes`、`POST /sandboxes`、`POST /sandboxes/preflight`、`GET /sandboxes/{id}`、`DELETE /sandboxes/{id}`、`GET /sandboxes/{id}/graph` | `DATA_READ` / `SIM_RUN` | [09](../../plans/completed/v1-foundation-step00-14/09-STEP10-AXSIM-SIMULATION-AND-DECISION.md) §10.2、§10.9；[06-STEP20](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/06-STEP20-SIM-PREFLIGHT-SCOPED-QUERY-AND-VIEW-ZONES.md) §20.1.2 |
| `PATCH /sandboxes/{id}/nodes/{node_id}`、`POST /sandboxes/{id}/nodes`、`DELETE /sandboxes/{id}/nodes/{node_id}` | `SIM_RUN` | §10.2.3 |
| `POST /sandboxes/{id}/links`、`PATCH /sandboxes/{id}/links/{link_id}`、`DELETE /sandboxes/{id}/links/{link_id}` | `SIM_RUN` | 同上 |
| `POST /sandboxes/{id}/reset`、`POST /sandboxes/{id}/refresh-base` | `SIM_RUN` | 同上 |
| `POST /sandboxes/{id}/simulate` | `SIM_RUN` | §10.9 |
| `GET /sandboxes/{id}/scenarios`、`POST /sandboxes/{id}/scenarios`、`GET /scenarios/{id}`、`POST /scenarios/{id}/rerun`、`DELETE /scenarios/{id}` | `DATA_READ` / `SIM_RUN` | §10.7 |
| `GET /scenarios/{id}/impacts`、`GET /scenarios/{id}/checkpoints`、`GET /scenarios/compare`（`?ids=&metrics=&briefing=`） | `DATA_READ` | §10.5、§10.7.2 |
| `POST /ask` | `SIM_RUN` | §10.7.3 |
| `POST /scenarios/{id}/decisions`、`GET /decisions/{id}`、`POST /decisions/{id}/rebase` | `SIM_RUN`（审批需 `SIM_APPLY`） | §10.8 |

## 9. xBlueView（`/api/xblueview`）

| 方法与路径 | 鉴权 | 定义 |
| :--- | :--- | :--- |
| `GET /dashboards`、`POST /dashboards`、`GET /dashboards/{id}`、`PUT /dashboards/{id}`、`DELETE /dashboards/{id}`、`GET /dashboards/by-slug/{slug}` | `DATA_READ` / `VIEW_EDIT` | [10](../../plans/completed/v1-foundation-step00-14/10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §11.7 |
| `POST /dashboards/ai-generate`、`POST /dashboards/ai-draft` | `VIEW_EDIT` | §11.6、[05-STEP19](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/05-STEP19-ONBOARDING-TEMPLATE-HUB-AND-APP-PACKAGING.md) §19.3.2 |
| `POST /dashboards/{id}/publish`、`POST /dashboards/{id}/duplicate`、`PUT /dashboards/{id}/layout`、`POST /dashboards/{id}/apply-zone-preset`、`POST /dashboards/{id}/preflight` | `VIEW_EDIT` / `VIEW_SHARE` | §11.7、[06-STEP20](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/06-STEP20-SIM-PREFLIGHT-SCOPED-QUERY-AND-VIEW-ZONES.md) §20.3 |
| `POST /dashboards/{id}/widgets`、`PUT /widgets/{id}`、`DELETE /widgets/{id}` | `VIEW_EDIT` | 同上 |
| `GET /charts`、`POST /charts`、`GET /charts/{id}`、`PUT /charts/{id}`、`DELETE /charts/{id}`、`POST /charts/{id}/pin` | `DATA_READ` / `VIEW_EDIT` | [03-STEP25](../../plans/completed/v3-craft-and-world-runtime-step23-30/craft-ux/03-STEP25-CHART-STUDIO-VTABLE-PIVOT-AND-MAGNETIC-GRID.md) §25.2 |
| `POST /widgets/{id}/data`、`POST /widgets/preview` | `DATA_READ` | §11.2 |
| `POST /widgets/{id}/invoke` | `FLOW_RUN` | §11.6 |
| `GET /dashboards/{id}/stream`（SSE） | `DATA_READ` | §11.5 |
| `GET /assets`、`POST /assets`、`GET /assets/{id}/content`、`DELETE /assets/{id}`、`GET /assets/{id}/placements`、`PUT /assets/{id}/placements`、`GET /assets/{id}/bindings3d`、`PUT /assets/{id}/bindings3d` | `DATA_READ` / `VIEW_EDIT` | §11.4 |
| `POST /exports`、`GET /exports`、`GET /exports/{id}`、`GET /exports/{id}/download`、`DELETE /exports/{id}` | `EXPORT_DATA` | §12.5.1 |
| `GET /export-templates`、`POST /export-templates`、`DELETE /export-templates/{id}` | `EXPORT_DATA` / `VIEW_EDIT` | §12.4 |
| `POST /shares`、`GET /shares`、`DELETE /shares/{id}` | `VIEW_SHARE` | §12.5.2 |
| `GET /share/{token}`、`POST /share/{token}/unlock`、`POST /share/{token}/widgets/{widget_id}/data`、`GET /share/{token}/assets/{asset_id}/content`、`GET /share/{token}/download` | `axs_` 分享令牌 | 同上 |

## 10. AxGuard（`/api/axguard`）

| 方法与路径 | 鉴权 | 定义 |
| :--- | :--- | :--- |
| `GET /users`、`POST /users`、`PATCH /users/{id}`、`POST /users/{id}/reset-password`、`PUT /users/{id}/attributes` | `GUARD_ADMIN` | [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.10 |
| `GET /roles`、`POST /roles`、`PATCH /roles/{id}`、`DELETE /roles/{id}` | `GUARD_ADMIN` | 同上 |
| `GET /departments`、`PUT /departments/{dept_id}`、`DELETE /departments/{dept_id}` | `GUARD_ADMIN` | 同上 |
| `GET /policies`、`PUT /policies/{name}`、`DELETE /policies/{name}`、`POST /policies/validate`、`POST /policies/simulate` | `GUARD_ADMIN` | §13.6.1 |
| `GET /row-policies`、`POST /row-policies`、`PUT /row-policies/{id}`、`DELETE /row-policies/{id}`、`GET /row-policy-defaults`、`PUT /row-policy-defaults`、`POST /row-policies/preview`、`POST /row-policies/simulate-state` | `GUARD_ADMIN` | §13.6.2、[05-STEP27](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/05-STEP27-AXQL-ONTOLOGY-COMPILER-AND-DYNAMIC-VISIBILITY.md) §27.3 |
| `GET /column-policies`、`PUT /column-policies`、`DELETE /column-policies/{id}` | `GUARD_ADMIN` | §13.7 |
| `GET /grants`、`POST /grants`、`DELETE /grants/{id}` | `DATA_READ` / `GUARD_ADMIN` | §13.9.1 |
| `GET /audit`、`GET /audit/export.csv`、`GET /audit/verify`、`GET /audit/stats` | `GUARD_ADMIN` | §13.9.2 |
| `GET /sessions`、`DELETE /sessions/{family_id}` | `GUARD_ADMIN` | §13.10 |
| `POST /debug/llm-egress-preview`（仅非生产环境注册） | `GUARD_ADMIN` | §13.11 |

## 11. AxOps（`/api/axops`）

| 方法与路径 | 鉴权 | Step | 定义 |
| :--- | :--- | :--- | :--- |
| `GET /jobs`、`GET /jobs/{id}` | `DATA_READ` | 00 | [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.6.5 |
| `POST /jobs/rollup-now` | `OPS_ADMIN` | 03 | [04](../../plans/completed/v1-foundation-step00-14/04-STEP03-AXGRAPH-STORAGE-AND-GRAPH-QUERY.md) |
| `POST /jobs/{id}/retry`、`POST /jobs/{id}/cancel` | `OPS_ADMIN` | 14 | [12](../../plans/completed/v1-foundation-step00-14/12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md) §14.8 |
| `GET /health/deep`、`GET /outbox/stats`、`GET /system/info` | `OPS_ADMIN` | 14 | §14.4.3 |
| `GET /retention`、`PUT /retention/{target}`、`POST /retention/run-now` | `OPS_ADMIN` | 14 | §14.4.1 |
| `GET /backups`、`POST /backups`、`GET /backups/{id}` | `OPS_ADMIN`（`default` 租户） | 14 | §14.3 |
| `GET /pulse` | `DATA_READ` | 32 | [02-STEP32](../../plans/completed/v4-modeling-console-and-industry-worlds-step31-35/console-and-ia/02-STEP32-LIVE-OPERATIONAL-DASHBOARD-AND-PULSE-API.md) §32.1 |
| `GET /onboarding-status` | `DATA_READ` | 19 | [05-STEP19](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/05-STEP19-ONBOARDING-TEMPLATE-HUB-AND-APP-PACKAGING.md) §19.1.1 |
| `GET /templates`、`GET /templates/{pack_id}`、`GET /templates/{pack_id}/preview`、`POST /templates/{pack_id}/plan`、`POST /templates/{pack_id}/install`、`GET /templates/installs`、`POST /templates/installs/{id}/apply`、`POST /templates/installs/{id}/uninstall` | `DATA_READ` / `OPS_ADMIN` | 14 / 19 | §14.5、[05-STEP19](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/05-STEP19-ONBOARDING-TEMPLATE-HUB-AND-APP-PACKAGING.md) §19.2 |
| `POST /demo/{scenario}/load` | `OPS_ADMIN` | 14 | §14.6 |
| `POST /demo/showcase/load` | `OPS_ADMIN` | 35 | [05-STEP35](../../plans/completed/v4-modeling-console-and-industry-worlds-step31-35/industry-worlds/05-STEP35-SHOWCASE-VECTOR-SEEDER-E2E-AND-ACCEPTANCE.md) §35.0 |
| `GET /work-items`、`POST /work-items`、`GET /work-items/{id}`、`PATCH /work-items/{id}`、`POST /work-items/{id}/comments` | `DATA_READ` / `FLOW_RUN` | 14 | §14.7 |

---

## 12. Sidecar（`axpoint-parser`，内部令牌 `X-AxPoint-Internal-Token`）

| 方法与路径 | Step | 定义 |
| :--- | :--- | :--- |
| `GET /health` | 00 | 公开（容器健康检查） |
| `POST /parse`、`POST /parse/unified` | 01 | [02](../../plans/completed/v1-foundation-step00-14/02-STEP01-AXPARSER-FILE-PARSING.md) §01.2 |
| `GET /connectors/types`、`POST /connectors/test`、`POST /connectors/discover`、`POST /connectors/preview`、`POST /connectors/sync` | 02 | [03](../../plans/completed/v1-foundation-step00-14/03-STEP02-AXHUB-CONNECTORS-AND-SYNC.md) §02.3.4 |
| `POST /lake/commit_staging`、`POST /lake/maintenance`、`POST /lake/preview`、`GET /lake/snapshots` | 02 | 同上 |
| `POST /lake/materialize` | 04 | [05](../../plans/completed/v1-foundation-step00-14/05-STEP04-AXANALYTICS-OLAP-AND-NL2SQL.md) §04.7 |
| `POST /render/chart` | 12 | [10](../../plans/completed/v1-foundation-step00-14/10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §12.3 |
| `POST /export/{format}` | 12 | §12.4 |
| `POST /pii/analyze`、`POST /pii/anonymize` | 13 | [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.8.1 |

Core 调用 Sidecar 时附加请求头 `X-AxPoint-Internal-Token`、`X-AxPoint-Tenant`、`X-AxPoint-Timezone`、`traceparent`（A1 §6 第 5 条）。

---

## 13. 一致性检查

```bash
# 每个 Rust 路由文件中注册的路径都出现在本附录中（脚本输出缺失项，期望为空）
python3 scripts/check_routes.py --routes axpoint-core/crates/axpoint-server/src/routes --index docs/standards/A4-API-CONTRACT-INDEX.md
```

`scripts/check_routes.py` 用正则提取 `.route("<path>", ...)` 与 `nest("<prefix>", ...)` 组合出完整路径，把 `{param}` 归一化后与本附录表格第一列中的路径比对；作为 Step 14 全局门禁的一部分执行。
