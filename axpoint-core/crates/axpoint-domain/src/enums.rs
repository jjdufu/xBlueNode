//! Canonical platform-wide enumerations defined in `docs/blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md`.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::AppError;

macro_rules! define_str_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident => $str_val:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(
                $(#[$vmeta])*
                #[serde(rename = $str_val)]
                $variant,
            )+
        }

        impl $name {
            /// All variants of this enumeration in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Returns the canonical wire and database string representation.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $str_val,)+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = AppError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($str_val => Ok(Self::$variant),)+
                    other => Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("unknown {} value: {other}", stringify!($name)),
                    )),
                }
            }
        }
    };
}

define_str_enum! {
    /// Logical column or property data type.
    pub enum DataType {
        /// UTF-8 text string.
        String => "STRING",
        /// 64-bit signed integer.
        Int64 => "INT64",
        /// IEEE-754 double precision float.
        Float64 => "FLOAT64",
        /// Exact fixed-point decimal.
        Decimal => "DECIMAL",
        /// Boolean flag.
        Boolean => "BOOLEAN",
        /// Calendar date without time.
        Date => "DATE",
        /// UTC timestamp with timezone.
        Timestamp => "TIMESTAMP",
        /// Raw byte sequence.
        Binary => "BINARY",
        /// Arbitrary JSON value.
        Json => "JSON",
        /// Homogeneous list/array value.
        List => "LIST",
        /// Structured record value.
        Struct => "STRUCT",
    }
}

define_str_enum! {
    /// Personally identifiable or sensitive information classification.
    pub enum PiiCategory {
        /// Not sensitive.
        None => "NONE",
        /// Mobile or landline phone number.
        Phone => "PHONE",
        /// National identity card number.
        IdCard => "ID_CARD",
        /// Electronic mail address.
        Email => "EMAIL",
        /// Bank or payment card number.
        BankCard => "BANK_CARD",
        /// Personal full name.
        Name => "NAME",
        /// Street or postal address.
        Address => "ADDRESS",
        /// Credential, key, or token secret.
        Secret => "SECRET",
    }
}

define_str_enum! {
    /// Dynamic masking rule applied across query, LLM prompt, and export boundaries.
    pub enum MaskRule {
        /// Return value unchanged.
        None => "NONE",
        /// Mask middle digits of a phone number.
        MaskPhone => "MASK_PHONE",
        /// Mask date/serial digits of an ID card.
        MaskIdCard => "MASK_ID_CARD",
        /// Mask local part of an email address.
        MaskEmail => "MASK_EMAIL",
        /// Mask all but trailing digits of a bank card.
        MaskBankCard => "MASK_BANK_CARD",
        /// Mask given name characters.
        MaskName => "MASK_NAME",
        /// Mask detailed street/building segments.
        MaskAddress => "MASK_ADDRESS",
        /// Deterministic tenant-keyed HMAC digest.
        HmacHash => "HMAC_HASH",
        /// Replace entire value with `[REDACTED]`.
        Redact => "REDACT",
    }
}

define_str_enum! {
    /// Supported `AxHub` data source connector types.
    pub enum ConnectorType {
        /// Direct file upload.
        FileUpload => "FILE_UPLOAD",
        /// S3-compatible object storage.
        S3 => "S3",
        /// SSH File Transfer Protocol server.
        Sftp => "SFTP",
        /// PostgreSQL database.
        Postgres => "POSTGRES",
        /// `MySQL` / `MariaDB` database.
        Mysql => "MYSQL",
        /// Microsoft SQL Server.
        Mssql => "MSSQL",
        /// Oracle Database.
        Oracle => "ORACLE",
        /// `SQLite` database file.
        Sqlite => "SQLITE",
        /// `ClickHouse` analytical database.
        Clickhouse => "CLICKHOUSE",
        /// Dameng (DM8) relational database.
        Dameng => "DAMENG",
        /// `KingbaseES` relational database.
        Kingbase => "KINGBASE",
        /// `openGauss` relational database.
        Opengauss => "OPENGAUSS",
        /// `OceanBase` distributed database.
        Oceanbase => "OCEANBASE",
        /// `TiDB` distributed SQL database.
        Tidb => "TIDB",
        /// Apache Doris analytical warehouse.
        Doris => "DORIS",
        /// `StarRocks` analytical warehouse.
        Starrocks => "STARROCKS",
        /// Apache Hive data warehouse.
        Hive => "HIVE",
        /// `MongoDB` document store.
        Mongodb => "MONGODB",
        /// Redis key-value store.
        Redis => "REDIS",
        /// Elasticsearch search cluster.
        Elasticsearch => "ELASTICSEARCH",
        /// Apache Cassandra wide-column store.
        Cassandra => "CASSANDRA",
        /// Apache `HBase` wide-column store.
        Hbase => "HBASE",
        /// `Neo4j` property graph database.
        Neo4j => "NEO4J",
        /// `InfluxDB` time-series database.
        Influxdb => "INFLUXDB",
        /// `TDengine` time-series database.
        Tdengine => "TDENGINE",
        /// Apache `IoTDB` time-series database.
        Iotdb => "IOTDB",
        /// Apache Kafka / Redpanda stream.
        Kafka => "KAFKA",
        /// MQTT broker topic stream.
        Mqtt => "MQTT",
        /// Apache Pulsar topic stream.
        Pulsar => "PULSAR",
        /// REST HTTP API endpoint.
        RestApi => "REST_API",
        /// `GraphQL` HTTP endpoint.
        Graphql => "GRAPHQL",
        /// Inbound HTTP webhook receiver.
        Webhook => "WEBHOOK",
    }
}

define_str_enum! {
    /// Dataset origin category in `AxHub`.
    pub enum SourceKind {
        /// Table extracted from an uploaded file.
        FileTable => "FILE_TABLE",
        /// Table or view from a relational database.
        SqlTable => "SQL_TABLE",
        /// Collection or key pattern from a `NoSQL` store.
        NosqlCollection => "NOSQL_COLLECTION",
        /// Measurement or super-table from a time-series store.
        Timeseries => "TIMESERIES",
        /// Message queue or event stream topic.
        Stream => "STREAM",
        /// Polled REST or `GraphQL` API resource.
        ApiResource => "API_RESOURCE",
        /// Inbound webhook event buffer.
        WebhookStream => "WEBHOOK_STREAM",
        /// Derived SQL transformation view.
        DerivedView => "DERIVED_VIEW",
    }
}

define_str_enum! {
    /// Lake ingestion synchronization semantics.
    pub enum SyncMode {
        /// Full table replacement snapshot.
        Snapshot => "SNAPSHOT",
        /// Append-only incremental ingestion.
        Append => "APPEND",
        /// Primary-key merge/upsert ingestion.
        Upsert => "UPSERT",
    }
}

define_str_enum! {
    /// Data quality assertion rule kind.
    pub enum QualityRuleKind {
        /// Column must contain no null values.
        NotNull => "NOT_NULL",
        /// Column values must be unique.
        Unique => "UNIQUE",
        /// Numeric or temporal values must fall within `[min, max]`.
        Range => "RANGE",
        /// String values must match regular expression.
        Regex => "REGEX",
        /// Values must belong to an allowed set (`03-STEP02` canonical name).
        AllowedValues => "ALLOWED_VALUES",
        /// Values must belong to an allowed set (`A1` alias).
        EnumSet => "ENUM_SET",
        /// Latest timestamp must be within freshness threshold (`03-STEP02` canonical name).
        Freshness => "FRESHNESS",
        /// Latest timestamp must be within freshness threshold (`A1` alias).
        FreshnessMinutes => "FRESHNESS_MINUTES",
        /// Table must contain at least minimum rows.
        RowCountMin => "ROW_COUNT_MIN",
    }
}

define_str_enum! {
    /// Lifecycle status of background, sync, or export jobs.
    pub enum JobStatus {
        /// Queued and awaiting worker lease.
        Pending => "PENDING",
        /// Currently leased and executing.
        Running => "RUNNING",
        /// Finished successfully.
        Succeeded => "SUCCEEDED",
        /// Exhausted retries or encountered terminal failure.
        Failed => "FAILED",
        /// Explicitly canceled by an operator.
        Canceled => "CANCELED",
        /// Expired past deadline or retention window.
        Expired => "EXPIRED",
    }
}

define_str_enum! {
    /// Persistent `ax_jobs` task category.
    pub enum JobKind {
        /// Synchronize an `AxHub` dataset into `DuckLake`.
        SyncDataset => "SYNC_DATASET",
        /// Parse an uploaded file via `AxParser`.
        ParseFile => "PARSE_FILE",
        /// Execute a single `AxFlow` DAG operator step.
        FlowStep => "FLOW_STEP",
        /// Render and package a document export.
        Export => "EXPORT",
        /// Evaluate alerting rules.
        RuleEval => "RULE_EVAL",
        /// Compute time-series metric rollups.
        Rollup => "ROLLUP",
        /// Compact snapshots and clean orphan lake files.
        LakeMaintenance => "LAKE_MAINTENANCE",
        /// Enforce data retention cleanup policies.
        Retention => "RETENTION",
        /// Re-encrypt stored credentials under a rotated key.
        CredentialRotate => "CREDENTIAL_ROTATE",
        /// Execute platform backup workflow.
        Backup => "BACKUP",
        /// Generate vector embeddings for `BlueNode` records.
        EmbedNodes => "EMBED_NODES",
        /// Materialize `xBlueStudio` dataset-to-node mappings.
        MapMaterialize => "MAP_MATERIALIZE",
        /// Recompute dynamic graph rollup/derived metrics.
        GraphAggregate => "GRAPH_AGGREGATE",
        /// Synchronize hypergraph entities with business nodes.
        KnowledgeSync => "KNOWLEDGE_SYNC",
    }
}

define_str_enum! {
    /// Transactional `ax_outbox` event topic.
    pub enum OutboxTopic {
        /// Dataset synchronization completed.
        DatasetSynced => "dataset.synced",
        /// One or more business node metrics changed.
        NodeMetricsChanged => "node.metrics_changed",
        /// Business node created or updated.
        NodeUpserted => "node.upserted",
        /// Human approval step decided.
        ApprovalDecided => "approval.decided",
        /// Simulation decision applied to production.
        SimApplied => "sim.applied",
        /// Operational alert opened.
        AlertOpened => "alert.opened",
        /// Operational alert resolved.
        AlertResolved => "alert.resolved",
        /// Document export job finished.
        ExportFinished => "export.finished",
    }
}

define_str_enum! {
    /// Retention policy target table/artifact class.
    pub enum RetentionTarget {
        /// Raw `xb_metric_series` points.
        MetricSeries => "METRIC_SERIES",
        /// Minute-grain `xb_metric_rollups` rows.
        MetricRollupMinute => "METRIC_ROLLUP_MINUTE",
        /// Append-only `sec_audit_logs` records.
        Audit => "AUDIT",
        /// Generated export files on disk.
        ExportFile => "EXPORT_FILE",
        /// `AxFlow` step execution logs.
        FlowStepLog => "FLOW_STEP_LOG",
        /// Completed `ax_jobs` rows.
        Job => "JOB",
        /// Processed `ax_outbox` events.
        Outbox => "OUTBOX",
        /// Historical `DuckLake` snapshots.
        LakeSnapshot => "LAKE_SNAPSHOT",
        /// Inbound stream/webhook message buffers.
        Message => "MESSAGE",
        /// `AxAnalytics` SQL execution history.
        QueryHistory => "QUERY_HISTORY",
        /// `AxAgent` multi-turn conversations.
        Conversation => "CONVERSATION",
    }
}

define_str_enum! {
    /// Relationship cardinality constraint in `xBlueStudio`.
    pub enum Cardinality {
        /// 1:1 relationship.
        OneToOne => "ONE_TO_ONE",
        /// 1:N relationship.
        OneToMany => "ONE_TO_MANY",
        /// N:1 relationship.
        ManyToOne => "MANY_TO_ONE",
        /// N:M relationship.
        ManyToMany => "MANY_TO_MANY",
    }
}

define_str_enum! {
    /// Department subgraph alignment and merge strategy.
    pub enum SubgraphMergeStrategy {
        /// Reuse main-graph node type directly.
        ReuseMain => "REUSE_MAIN",
        /// Extend main-graph node type with subgraph properties.
        ExtendProps => "EXTEND_PROPS",
        /// Keep distinct types connected by a bridge relation.
        BridgeLink => "BRIDGE_LINK",
    }
}

define_str_enum! {
    /// Built-in `AxGraph` topology analysis algorithm.
    pub enum GraphAlgo {
        /// In/out/total degree centrality.
        Degree => "DEGREE",
        /// Damped `PageRank` centrality.
        Pagerank => "PAGERANK",
        /// Directed strongly connected components.
        StronglyConnected => "STRONGLY_CONNECTED",
        /// Undirected weakly connected components.
        WeaklyConnected => "WEAKLY_CONNECTED",
        /// Deterministic asynchronous label propagation communities.
        LabelPropagation => "LABEL_PROPAGATION",
        /// Connected components unified endpoint identifier.
        ConnectedComponents => "CONNECTED_COMPONENTS",
    }
}

define_str_enum! {
    /// Aggregation function for rollups, pivots, and metrics.
    pub enum AggFn {
        /// Arithmetic sum.
        Sum => "SUM",
        /// Arithmetic mean.
        Avg => "AVG",
        /// Row or value count.
        Count => "COUNT",
        /// Approximate or exact distinct count.
        CountDistinct => "COUNT_DISTINCT",
        /// Minimum value.
        Min => "MIN",
        /// Maximum value.
        Max => "MAX",
        /// Exact median (50th percentile).
        Median => "MEDIAN",
        /// Latest value ordered by timestamp.
        Last => "LAST",
    }
}

define_str_enum! {
    /// Provenance tag on `xb_metric_series` points.
    pub enum MetricSourceTag {
        /// Written by dataset/connector synchronization.
        Sync => "SYNC",
        /// Computed by graph rollup aggregation.
        Aggregation => "AGGREGATION",
        /// Computed by derived formula evaluation.
        Derived => "DERIVED",
        /// Emitted by an `AxFlow` or `xBlueView` writeback action.
        Action => "ACTION",
        /// Entered manually via API or Studio.
        Manual => "MANUAL",
        /// Written by a graph algorithm execution.
        Algorithm => "ALGORITHM",
    }
}

define_str_enum! {
    /// Pre-aggregation time bucket granularity.
    pub enum RollupGrain {
        /// 1-minute bucket (UTC).
        Minute => "MINUTE",
        /// 1-hour bucket (UTC).
        Hour => "HOUR",
        /// 1-day bucket (tenant timezone).
        Day => "DAY",
        /// 1-month bucket (tenant timezone).
        Month => "MONTH",
    }
}

define_str_enum! {
    /// `AxFlow` DAG node operator type.
    pub enum FlowOperator {
        /// Trigger dataset synchronization or file parse.
        SourceIngest => "SOURCE_INGEST",
        /// Materialize a `DuckDB` SQL transformation.
        SqlTransform => "SQL_TRANSFORM",
        /// Extract structured fields/triples via LLM.
        LlmExtract => "LLM_EXTRACT",
        /// Materialize dataset rows into `BlueNode`s.
        XblueMap => "XBLUE_MAP",
        /// Trigger dynamic graph rollup recomputation.
        AggTrigger => "AGG_TRIGGER",
        /// Run an `AxSim` causal simulation scenario.
        CausalSim => "CAUSAL_SIM",
        /// Pause workflow for human approval decision.
        HumanApproval => "HUMAN_APPROVAL",
        /// Emit writeback action log and outbox event.
        ActionEmit => "ACTION_EMIT",
        /// Invoke external HTTP endpoint.
        HttpCall => "HTTP_CALL",
        /// Delegate subtask to an `AxAgent` instance.
        AgentTask => "AGENT_TASK",
    }
}

define_str_enum! {
    /// Execution trigger source for `AxFlow` pipelines.
    pub enum TriggerType {
        /// Triggered interactively by a user.
        Manual => "MANUAL",
        /// Triggered on a cron schedule.
        Cron => "CRON",
        /// Triggered by a matching `ax_outbox` event.
        OutboxEvent => "OUTBOX_EVENT",
        /// Triggered by an inbound flow webhook.
        Webhook => "WEBHOOK",
        /// Triggered via a published REST endpoint.
        Api => "API",
        /// Triggered via an MCP tool call.
        Mcp => "MCP",
        /// Triggered by an operational alert rule.
        Rule => "RULE",
        /// Triggered when applying an `AxSim` decision.
        SimApply => "SIM_APPLY",
    }
}

define_str_enum! {
    /// Lifecycle status of an `AxFlow` run.
    pub enum RunStatus {
        /// Run created and waiting to start.
        Pending => "PENDING",
        /// At least one step is executing or ready.
        Running => "RUNNING",
        /// Blocked on a `HUMAN_APPROVAL` gate.
        WaitingApproval => "WAITING_APPROVAL",
        /// All reachable terminal steps succeeded.
        Succeeded => "SUCCEEDED",
        /// Run terminated due to step failure.
        Failed => "FAILED",
        /// Canceled by user request.
        Canceled => "CANCELED",
    }
}

define_str_enum! {
    /// Lifecycle status of an individual `AxFlow` step.
    pub enum StepStatus {
        /// Waiting on upstream dependencies.
        Pending => "PENDING",
        /// Currently executing.
        Running => "RUNNING",
        /// Awaiting human approval decision.
        WaitingApproval => "WAITING_APPROVAL",
        /// Completed successfully.
        Succeeded => "SUCCEEDED",
        /// Failed after configured retries.
        Failed => "FAILED",
        /// Skipped due to branch condition or upstream failure.
        Skipped => "SKIPPED",
    }
}

define_str_enum! {
    /// Deployment and publishing stage environment.
    pub enum Environment {
        /// Mutable testing/staging environment.
        Test => "TEST",
        /// Version-pinned production environment.
        Prod => "PROD",
    }
}

define_str_enum! {
    /// Target resource kind exposed by `AxAgent` publishing.
    pub enum EndpointKind {
        /// Parameterized saved SQL query.
        SavedQuery => "SAVED_QUERY",
        /// Semantic layer metric definition.
        Metric => "METRIC",
        /// `AxFlow` pipeline.
        Flow => "FLOW",
        /// Configured `AxAgent` assistant.
        Agent => "AGENT",
        /// Published `xBlueStudio` domain object type.
        ObjectType => "OBJECT_TYPE",
    }
}

define_str_enum! {
    /// Operational rule and alert severity level.
    pub enum Severity {
        /// Informational notice.
        Info => "INFO",
        /// Warning requiring attention.
        Warn => "WARN",
        /// Critical incident requiring immediate action.
        Critical => "CRITICAL",
    }
}

define_str_enum! {
    /// Lifecycle state of an operational alert.
    pub enum AlertStatus {
        /// Newly triggered and unacknowledged.
        Open => "OPEN",
        /// Acknowledged by an operator.
        Ack => "ACK",
        /// Condition cleared or manually resolved.
        Resolved => "RESOLVED",
    }
}

define_str_enum! {
    /// Notification delivery channel kind.
    pub enum NotifyKind {
        /// Generic signed HTTP webhook.
        Webhook => "WEBHOOK",
        /// SMTP email delivery.
        Email => "EMAIL",
        /// Enterprise `WeChat` bot webhook.
        Wecom => "WECOM",
        /// `DingTalk` bot webhook.
        Dingtalk => "DINGTALK",
        /// Feishu / Lark bot webhook.
        Feishu => "FEISHU",
    }
}

define_str_enum! {
    /// Perturbation mode for an `AxSim` intervention.
    pub enum InterventionMode {
        /// Multiply baseline by `(1 + value)`.
        Ratio => "RATIO",
        /// Add `value` to baseline.
        Add => "ADD",
        /// Override metric directly to `value`.
        Absolute => "ABSOLUTE",
    }
}

define_str_enum! {
    /// Visual component type in `xBlueView` dashboards.
    pub enum WidgetKind {
        /// Single KPI value and trend card.
        KpiCard => "KPI_CARD",
        /// Paginated data table.
        Table => "TABLE",
        /// Cross-tabulated OLAP pivot table.
        PivotTable => "PIVOT_TABLE",
        /// Time-series or category line chart.
        LineChart => "LINE_CHART",
        /// Vertical or horizontal bar chart.
        BarChart => "BAR_CHART",
        /// Proportional pie/donut chart.
        PieChart => "PIE_CHART",
        /// Two-dimensional scatter plot.
        Scatter => "SCATTER",
        /// Flow Sankey diagram.
        Sankey => "SANKEY",
        /// Matrix heatmap.
        Heatmap => "HEATMAP",
        /// Radial gauge indicator.
        Gauge => "GAUGE",
        /// Multi-axis radar chart.
        Radar => "RADAR",
        /// Interactive node-link graph topology view.
        GraphTopology => "GRAPH_TOPOLOGY",
        /// Geographic vector/tile map.
        Map => "MAP",
        /// 2D facility floor plan with node placements.
        FloorPlan => "FLOOR_PLAN",
        /// 3D `glTF`/GLB digital twin scene.
        Scene3d => "SCENE_3D",
        /// Live operational alert feed.
        AlertList => "ALERT_LIST",
        /// Side-by-side `AxSim` scenario comparison view.
        SimCompare => "SIM_COMPARE",
        /// One-click writeback action button.
        ActionButton => "ACTION_BUTTON",
        /// Parameterized writeback form bound to an `AxFlow`.
        ActionForm => "ACTION_FORM",
        /// Rich Markdown narrative note (raw HTML disabled).
        MarkdownNote => "MARKDOWN_NOTE",
    }
}

define_str_enum! {
    /// Static visual asset category in `xBlueView`.
    pub enum AssetKind {
        /// Raster or SVG 2D floor plan image.
        FloorPlanImage => "FLOOR_PLAN_IMAGE",
        /// `GeoJSON` boundary collection.
        Geojson => "GEOJSON",
        /// Custom node marker icon.
        Icon => "ICON",
        /// Binary `glTF` 2.0 (`.glb`) 3D twin model.
        Model3d => "MODEL_3D",
    }
}

define_str_enum! {
    /// Document export file format in `xBlueView`.
    pub enum ExportFormat {
        /// Microsoft Excel `.xlsx` workbook.
        Xlsx => "XLSX",
        /// UTF-8 CSV file.
        Csv => "CSV",
        /// Typeset PDF document.
        Pdf => "PDF",
        /// Microsoft Word `.docx` report.
        Docx => "DOCX",
        /// Microsoft `PowerPoint` `.pptx` deck.
        Pptx => "PPTX",
        /// Plain Markdown document.
        Markdown => "MARKDOWN",
    }
}

define_str_enum! {
    /// Resource security classification and user clearance level.
    pub enum Sensitivity {
        /// Publicly accessible within tenant.
        Public => "PUBLIC",
        /// Internal operational data.
        Internal => "INTERNAL",
        /// Confidential business data.
        Confidential => "CONFIDENTIAL",
        /// Strictly restricted secret data.
        Secret => "SECRET",
    }
}

define_str_enum! {
    /// Fine-grained RBAC/ABAC permission token in `AxGuard`.
    pub enum Permission {
        /// Read datasets, graph nodes, queries, and dashboards.
        DataRead => "DATA_READ",
        /// Upload documents, write metrics, and trigger dataset syncs.
        DataSync => "DATA_SYNC",
        /// Manage connections, credentials, column metadata, and quality rules.
        DataAdmin => "DATA_ADMIN",
        /// Edit `xBlueStudio` drafts, nodes, links, mappings, and metrics.
        StudioEdit => "STUDIO_EDIT",
        /// Publish `xBlueStudio` schema versions and `AxAgent` endpoints.
        StudioPublish => "STUDIO_PUBLISH",
        /// Create and edit `AxFlow` DAG definitions.
        FlowEdit => "FLOW_EDIT",
        /// Trigger `AxFlow` runs, action writebacks, and work items.
        FlowRun => "FLOW_RUN",
        /// Publish `AxFlow` versions to production and create hooks.
        FlowPublish => "FLOW_PUBLISH",
        /// Approve or reject `HUMAN_APPROVAL` gates.
        ApprovalDecide => "APPROVAL_DECIDE",
        /// Manage alerting rules and notification channels.
        RuleEdit => "RULE_EDIT",
        /// Acknowledge, resolve, or silence operational alerts.
        AlertAck => "ALERT_ACK",
        /// Configure LLM providers, MCP clients, and `AxAgent` definitions.
        AgentEdit => "AGENT_EDIT",
        /// Converse with published `AxAgent` assistants.
        AgentRun => "AGENT_RUN",
        /// Create `AxSim` sandboxes and run simulations.
        SimRun => "SIM_RUN",
        /// Approve and apply `AxSim` decisions to production.
        SimApply => "SIM_APPLY",
        /// Create and edit `xBlueView` dashboards and saved queries.
        ViewEdit => "VIEW_EDIT",
        /// Create external share links and agent embeds.
        ViewShare => "VIEW_SHARE",
        /// Run and download `xBlueView` document exports.
        ExportData => "EXPORT_DATA",
        /// Reveal unmasked PII values with mandatory audit logging.
        PiiRawRead => "PII_RAW_READ",
        /// Manage users, roles, Cedar policies, grants, and audit verification.
        GuardAdmin => "GUARD_ADMIN",
        /// Manage backups, retention policies, templates, and deep health.
        OpsAdmin => "OPS_ADMIN",
    }
}

impl Permission {
    /// Returns the complete set of all platform permissions (used by `ADMIN` and dev token).
    #[must_use]
    pub fn all_set() -> BTreeSet<Self> {
        Self::ALL.iter().copied().collect()
    }
}

define_str_enum! {
    /// Built-in tenant role name seeded by `0002_platform_foundation.sql`.
    pub enum BuiltinRole {
        /// Tenant administrator with all permissions.
        Admin => "ADMIN",
        /// Data and modeling analyst.
        Analyst => "ANALYST",
        /// Operations and workflow executor.
        Operator => "OPERATOR",
        /// Read-only viewer.
        Viewer => "VIEWER",
    }
}

define_str_enum! {
    /// Security-relevant action recorded in `sec_audit_logs`.
    pub enum AuditAction {
        /// Interactive or OIDC login.
        Login => "LOGIN",
        /// Session logout.
        Logout => "LOGOUT",
        /// Access or personal API token issued.
        TokenIssue => "TOKEN_ISSUE",
        /// Session or API token revoked.
        TokenRevoke => "TOKEN_REVOKE",
        /// Controlled SQL or published data query executed.
        QuerySql => "QUERY_SQL",
        /// Natural-language-to-SQL question executed.
        AskData => "ASK_DATA",
        /// Hypergraph RAG question executed.
        RagAsk => "RAG_ASK",
        /// Dataset synchronization triggered.
        DataSync => "DATA_SYNC",
        /// File uploaded for ingestion.
        FileUpload => "FILE_UPLOAD",
        /// `xBlueStudio` schema published.
        SchemaPublish => "SCHEMA_PUBLISH",
        /// `AxFlow` pipeline executed.
        FlowRun => "FLOW_RUN",
        /// Human approval gate decided.
        ApprovalDecide => "APPROVAL_DECIDE",
        /// MCP server tool invoked.
        McpCall => "MCP_CALL",
        /// `AxAgent` conversation turn executed.
        AgentChat => "AGENT_CHAT",
        /// `AxSim` scenario decision applied.
        SimApply => "SIM_APPLY",
        /// Document exported.
        ExportFile => "EXPORT_FILE",
        /// External share link created.
        ShareCreate => "SHARE_CREATE",
        /// External share link accessed.
        ShareAccess => "SHARE_ACCESS",
        /// `AxGuard` security or masking policy changed.
        PolicyChange => "POLICY_CHANGE",
        /// Platform backup triggered.
        BackupRun => "BACKUP_RUN",
        /// Industry template pack installed.
        TemplateInstall => "TEMPLATE_INSTALL",
    }
}

define_str_enum! {
    /// Authentication credential category resolved into an `AuthPrincipal`.
    pub enum TokenKind {
        /// Short-lived `EdDSA` access JWT.
        Access => "ACCESS",
        /// Personal access token (`axp_...`).
        ApiToken => "API_TOKEN",
        /// Non-production static development token (`AXPOINT_DEV_TOKEN`).
        DevStatic => "DEV_STATIC",
        /// Scoped agent embed token (`axe_...`).
        Embed => "EMBED",
        /// Scoped dashboard share token (`axs_...`).
        Share => "SHARE",
    }
}

define_str_enum! {
    /// Workload purpose used by `LlmRouter` (`08-STEP09` §09.2).
    pub enum Purpose {
        /// General-purpose conversational chat.
        Chat => "CHAT",
        /// Dense vector embedding generation.
        Embed => "EMBED",
        /// Natural-language-to-SQL generation and repair.
        Nl2Sql => "NL2SQL",
        /// Entity-relation triple or structured field extraction.
        Extract => "EXTRACT",
        /// Tool-calling autonomous agent loop (`supports_tools = true`).
        Agent => "AGENT",
        /// Grounded synthesis and summarization.
        Summarize => "SUMMARIZE",
    }
}

define_str_enum! {
    /// Output payload shape declared by an `AxAgent` published endpoint (`08-STEP09` §09.1).
    pub enum OutputShape {
        /// Tabular `{columns, rows}` response.
        Rows => "ROWS",
        /// Domain object `{items, next_cursor}` response.
        Object => "OBJECT",
        /// Conversational text/answer response.
        Text => "TEXT",
        /// Asynchronous `AxFlow` `{run_id}` response.
        Run => "RUN",
    }
}

define_str_enum! {
    /// Originating channel of an `AxAgent` conversation (`08-STEP09` §09.1).
    pub enum ConversationChannel {
        /// Interactive multi-turn workbench UI.
        Workbench => "WORKBENCH",
        /// External unauthenticated `axe_...` embed widget.
        Embed => "EMBED",
        /// Published REST API or sub-agent invocation.
        Api => "API",
        /// `AxFlow` `AGENT_TASK` DAG operator.
        Flow => "FLOW",
        /// MCP tool invocation.
        Mcp => "MCP",
    }
}

define_str_enum! {
    /// Lifecycle status of an `AxAgent` conversation (`08-STEP09` §09.1).
    pub enum ConversationStatus {
        /// Active and ready for messages.
        Active => "ACTIVE",
        /// Paused on an `approval_required_tools` human gate.
        WaitingApproval => "WAITING_APPROVAL",
        /// Explicitly closed or archived.
        Closed => "CLOSED",
    }
}

define_str_enum! {
    /// Speaker role of a message in `ax_messages` (`08-STEP09` §09.1).
    pub enum MessageRole {
        /// End-user message.
        User => "USER",
        /// Model assistant message (may include `tool_calls`).
        Assistant => "ASSISTANT",
        /// Tool execution result linked via `tool_call_id`.
        Tool => "TOOL",
        /// System instruction or step-limit directive.
        System => "SYSTEM",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_topic_round_trips_dotted_wire_format() {
        for topic in OutboxTopic::ALL {
            let s = topic.as_str();
            assert!(s.contains('.'));
            assert_eq!(OutboxTopic::from_str(s).ok(), Some(*topic));
        }
    }

    #[test]
    fn permission_all_set_has_twenty_one_entries() {
        assert_eq!(Permission::all_set().len(), 21);
    }
}
