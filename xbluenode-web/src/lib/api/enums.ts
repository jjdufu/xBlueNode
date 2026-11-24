/**
 * Canonical public enums synchronized across Rust (`axpoint-domain/src/enums.rs`),
 * Python (`axpoint-parser/schemas.py`), TypeScript, and PostgreSQL CHECK constraints.
 * Reference: `docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md` §4 & §6.
 */

export type DataType =
  | "STRING"
  | "INT64"
  | "FLOAT64"
  | "DECIMAL"
  | "BOOLEAN"
  | "DATE"
  | "TIMESTAMP"
  | "BINARY"
  | "JSON"
  | "LIST"
  | "STRUCT";

export type PiiCategory =
  | "NONE"
  | "PHONE"
  | "ID_CARD"
  | "EMAIL"
  | "BANK_CARD"
  | "NAME"
  | "ADDRESS"
  | "SECRET";

export type MaskRule =
  | "NONE"
  | "MASK_PHONE"
  | "MASK_ID_CARD"
  | "MASK_EMAIL"
  | "MASK_BANK_CARD"
  | "MASK_NAME"
  | "MASK_ADDRESS"
  | "HMAC_HASH"
  | "REDACT";

export type ConnectorType =
  | "FILE_UPLOAD"
  | "S3"
  | "SFTP"
  | "POSTGRES"
  | "MYSQL"
  | "MSSQL"
  | "ORACLE"
  | "SQLITE"
  | "CLICKHOUSE"
  | "DAMENG"
  | "KINGBASE"
  | "OPENGAUSS"
  | "OCEANBASE"
  | "TIDB"
  | "DORIS"
  | "STARROCKS"
  | "HIVE"
  | "MONGODB"
  | "REDIS"
  | "ELASTICSEARCH"
  | "CASSANDRA"
  | "HBASE"
  | "NEO4J"
  | "INFLUXDB"
  | "TDENGINE"
  | "IOTDB"
  | "KAFKA"
  | "MQTT"
  | "PULSAR"
  | "REST_API"
  | "GRAPHQL"
  | "WEBHOOK";

export type SourceKind =
  | "FILE_TABLE"
  | "SQL_TABLE"
  | "NOSQL_COLLECTION"
  | "TIMESERIES"
  | "STREAM"
  | "API_RESOURCE"
  | "WEBHOOK_STREAM"
  | "DERIVED_VIEW";

export type SyncMode = "SNAPSHOT" | "APPEND" | "UPSERT";

export type QualityRuleKind =
  | "NOT_NULL"
  | "UNIQUE"
  | "RANGE"
  | "REGEX"
  | "ALLOWED_VALUES"
  | "ENUM_SET"
  | "FRESHNESS"
  | "FRESHNESS_MINUTES"
  | "ROW_COUNT_MIN";

export type JobStatus =
  | "PENDING"
  | "RUNNING"
  | "SUCCEEDED"
  | "FAILED"
  | "CANCELED"
  | "EXPIRED";

export type JobKind =
  | "SYNC_DATASET"
  | "PARSE_FILE"
  | "FLOW_STEP"
  | "EXPORT"
  | "RULE_EVAL"
  | "ROLLUP"
  | "LAKE_MAINTENANCE"
  | "RETENTION"
  | "CREDENTIAL_ROTATE"
  | "BACKUP"
  | "EMBED_NODES"
  | "MAP_MATERIALIZE"
  | "GRAPH_AGGREGATE"
  | "KNOWLEDGE_SYNC";

export type OutboxTopic =
  | "dataset.synced"
  | "node.metrics_changed"
  | "node.upserted"
  | "approval.decided"
  | "sim.applied"
  | "alert.opened"
  | "alert.resolved"
  | "export.finished";

export type RetentionTarget =
  | "METRIC_SERIES"
  | "METRIC_ROLLUP_MINUTE"
  | "AUDIT"
  | "EXPORT_FILE"
  | "FLOW_STEP_LOG"
  | "JOB"
  | "OUTBOX"
  | "LAKE_SNAPSHOT"
  | "MESSAGE"
  | "QUERY_HISTORY"
  | "CONVERSATION"
  | "IM_EVENT_DEDUP";

export type Cardinality =
  | "ONE_TO_ONE"
  | "ONE_TO_MANY"
  | "MANY_TO_ONE"
  | "MANY_TO_MANY";

export type SubgraphMergeStrategy =
  | "REUSE_MAIN"
  | "EXTEND_PROPS"
  | "BRIDGE_LINK";

export type GraphAlgo =
  | "DEGREE"
  | "PAGERANK"
  | "STRONGLY_CONNECTED"
  | "WEAKLY_CONNECTED"
  | "CONNECTED_COMPONENTS"
  | "LABEL_PROPAGATION";

export type AggFn =
  | "SUM"
  | "AVG"
  | "COUNT"
  | "COUNT_DISTINCT"
  | "MIN"
  | "MAX"
  | "MEDIAN"
  | "LAST";

export type MetricSourceTag =
  | "SYNC"
  | "AGGREGATION"
  | "DERIVED"
  | "ACTION"
  | "MANUAL"
  | "ALGORITHM";

export type RollupGrain = "MINUTE" | "HOUR" | "DAY" | "MONTH";

export type FlowOperator =
  | "SOURCE_INGEST"
  | "SQL_TRANSFORM"
  | "LLM_EXTRACT"
  | "XBLUE_MAP"
  | "AGG_TRIGGER"
  | "CAUSAL_SIM"
  | "HUMAN_APPROVAL"
  | "ACTION_EMIT"
  | "HTTP_CALL"
  | "AGENT_TASK";

export type TriggerType =
  | "MANUAL"
  | "CRON"
  | "OUTBOX_EVENT"
  | "WEBHOOK"
  | "API"
  | "MCP"
  | "RULE"
  | "SIM_APPLY";

export type RunStatus =
  | "PENDING"
  | "RUNNING"
  | "WAITING_APPROVAL"
  | "SUCCEEDED"
  | "FAILED"
  | "CANCELED";

export type StepStatus =
  | "PENDING"
  | "RUNNING"
  | "WAITING_APPROVAL"
  | "SUCCEEDED"
  | "FAILED"
  | "SKIPPED";

export type Environment = "TEST" | "PROD";

export type EndpointKind =
  | "SAVED_QUERY"
  | "METRIC"
  | "FLOW"
  | "AGENT"
  | "OBJECT_TYPE";

export type Severity = "INFO" | "WARN" | "CRITICAL";

export type AlertStatus = "OPEN" | "ACK" | "RESOLVED";

export type NotifyKind =
  | "WEBHOOK"
  | "EMAIL"
  | "WECOM"
  | "DINGTALK"
  | "FEISHU";

export type InterventionMode = "RATIO" | "ADD" | "ABSOLUTE";

export type WidgetKind =
  | "KPI_CARD"
  | "TABLE"
  | "PIVOT_TABLE"
  | "LINE_CHART"
  | "BAR_CHART"
  | "PIE_CHART"
  | "SCATTER"
  | "SANKEY"
  | "HEATMAP"
  | "GAUGE"
  | "RADAR"
  | "GRAPH_TOPOLOGY"
  | "MAP"
  | "FLOOR_PLAN"
  | "SCENE_3D"
  | "ALERT_LIST"
  | "SIM_COMPARE"
  | "ACTION_BUTTON"
  | "ACTION_FORM"
  | "MARKDOWN_NOTE";

export type AssetKind =
  | "FLOOR_PLAN_IMAGE"
  | "GEOJSON"
  | "ICON"
  | "MODEL_3D";

export type ExportFormat =
  | "XLSX"
  | "CSV"
  | "PDF"
  | "DOCX"
  | "PPTX"
  | "MARKDOWN";

export type Sensitivity =
  | "PUBLIC"
  | "INTERNAL"
  | "CONFIDENTIAL"
  | "SECRET";

export type Permission =
  | "DATA_READ"
  | "DATA_SYNC"
  | "DATA_ADMIN"
  | "STUDIO_EDIT"
  | "STUDIO_PUBLISH"
  | "FLOW_EDIT"
  | "FLOW_RUN"
  | "FLOW_PUBLISH"
  | "APPROVAL_DECIDE"
  | "RULE_EDIT"
  | "ALERT_ACK"
  | "AGENT_EDIT"
  | "AGENT_RUN"
  | "SIM_RUN"
  | "SIM_APPLY"
  | "VIEW_EDIT"
  | "VIEW_SHARE"
  | "EXPORT_DATA"
  | "PII_RAW_READ"
  | "GUARD_ADMIN"
  | "OPS_ADMIN";

export type BuiltinRole = "ADMIN" | "ANALYST" | "OPERATOR" | "VIEWER";

export type AuditAction =
  | "LOGIN"
  | "LOGOUT"
  | "TOKEN_ISSUE"
  | "TOKEN_REVOKE"
  | "QUERY_SQL"
  | "ASK_DATA"
  | "RAG_ASK"
  | "DATA_SYNC"
  | "FILE_UPLOAD"
  | "SCHEMA_PUBLISH"
  | "FLOW_RUN"
  | "APPROVAL_DECIDE"
  | "MCP_CALL"
  | "AGENT_CHAT"
  | "SIM_APPLY"
  | "EXPORT_FILE"
  | "SHARE_CREATE"
  | "SHARE_ACCESS"
  | "POLICY_CHANGE"
  | "BACKUP_RUN"
  | "TEMPLATE_INSTALL"
  | "BUILDER_MCP_MUTATE"
  | "TRIAL_TEMPLATE_CLONE"
  | "STUDIO_SNAPSHOT_RESTORE"
  | "IM_ACCOUNT_BIND"
  | "CHART_PUBLISH"
  | "OBJECT_ACTION_INVOKE"
  | "TRIAL_TICK_CERTIFY";

export type TokenKind =
  | "ACCESS"
  | "API_TOKEN"
  | "DEV_STATIC"
  | "EMBED"
  | "SHARE";

export type StructuredOutputMode = "NONE" | "JSON_OBJECT" | "JSON_SCHEMA";

export type GroundingMode =
  | "SEMANTIC_METRIC"
  | "DOMAIN_OBJECT"
  | "GUARDED_SQL"
  | "GROUNDED_DOC"
  | "ABSTAINED"
  | "CLARIFICATION";

export type TokenProfile = "GENERAL" | "BUILDER";

export type DesignMaturity = "PROPOSED" | "CONFIRMED";

export type SemanticLayerTier =
  | "STATIC_FACT"
  | "EVENT_DYNAMIC"
  | "CUBE_ROLLUP";

export type CopilotActionMode = "DIAGNOSE_ONLY" | "PATCH_DRAFT";

export type ImPlatform = "FEISHU" | "DINGTALK" | "WECOM";

export type ImIdentityPolicy = "REQUIRE_USER_BIND" | "CREATOR_DELEGATED";

export type ImConnectorStatus =
  | "DISCONNECTED"
  | "CONNECTING"
  | "CONNECTED"
  | "DEGRADED"
  | "ERROR";

export type CockpitZoneLayout = "FREE_GRID" | "STANDARD_FIVE_ZONE";

export type SemanticZoneKind =
  | "CONTROL"
  | "KPI"
  | "SCENE"
  | "INSIGHT"
  | "DECISION";

export type StudioCanvasViewMode = "ONTOLOGY_TOPOLOGY" | "MAPPING_PIPELINE";

export type GridCompactMode = "VERTICAL_COMPACT" | "FREE_FLOAT";

export type ActionSideEffectTier =
  | "SELF_ONLY"
  | "RELATED_NODES"
  | "EXTERNAL_EFFECT";

export type RuntimeCertLevel =
  | "L0_INVALID"
  | "L1_CONFIGURED"
  | "L2_RUNNABLE"
  | "L3_VERIFIED"
  | "L4_CERTIFIED";

export type PropagationPathKind =
  | "VERTEX_TO_VERTEX"
  | "VERTEX_TO_EDGE"
  | "EDGE_TO_VERTEX"
  | "EDGE_TO_EDGE";

export type RowPolicyMode = "STATIC_ATTR" | "STATE_DRIVEN";

export type SimEventTriggerKind = "AT_STEP" | "METRIC_THRESHOLD";

export type SimThresholdOp = "LT" | "LTE" | "GT" | "GTE";

export type GeoPredicateOp =
  | "BBOX"
  | "GEO_WITHIN_POLYGON"
  | "GEO_DWITHIN_METERS";

export type DeadlineWindowStatus =
  | "SATISFIED"
  | "ACTIONABLE_WINDOW"
  | "IMMEDIATE_BREACH_UNAVOIDABLE_BY_LAG";
