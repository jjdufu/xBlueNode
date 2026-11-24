from __future__ import annotations

# Shared Pydantic v2 schemas for AxPoint Sidecar (Steps 01, 02, 04, 12).
from datetime import datetime
from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field

DataType = Literal[
    "STRING",
    "INT64",
    "FLOAT64",
    "DECIMAL",
    "BOOLEAN",
    "DATE",
    "TIMESTAMP",
    "BINARY",
    "JSON",
    "LIST",
    "STRUCT",
]

PiiCategory = Literal[
    "NONE",
    "PHONE",
    "ID_CARD",
    "EMAIL",
    "BANK_CARD",
    "NAME",
    "ADDRESS",
    "SECRET",
]

ConnectorType = Literal[
    "FILE_UPLOAD",
    "S3",
    "SFTP",
    "POSTGRES",
    "MYSQL",
    "MSSQL",
    "ORACLE",
    "SQLITE",
    "CLICKHOUSE",
    "DAMENG",
    "KINGBASE",
    "OPENGAUSS",
    "OCEANBASE",
    "TIDB",
    "DORIS",
    "STARROCKS",
    "HIVE",
    "MONGODB",
    "REDIS",
    "ELASTICSEARCH",
    "CASSANDRA",
    "HBASE",
    "NEO4J",
    "INFLUXDB",
    "TDENGINE",
    "IOTDB",
    "KAFKA",
    "MQTT",
    "PULSAR",
    "REST_API",
    "GRAPHQL",
    "WEBHOOK",
]

ConnectorCategory = Literal[
    "FILE",
    "RELATIONAL",
    "DOMESTIC",
    "ANALYTIC",
    "NOSQL",
    "TIMESERIES",
    "STREAM",
    "API",
]

SourceKind = Literal[
    "FILE_TABLE",
    "SQL_TABLE",
    "NOSQL_COLLECTION",
    "TIMESERIES",
    "STREAM",
    "API_RESOURCE",
    "WEBHOOK_STREAM",
    "DERIVED_VIEW",
]

SyncMode = Literal["SNAPSHOT", "APPEND", "UPSERT", "LOG_CDC"]
SchemaPolicy = Literal["STRICT", "ADDITIVE", "EVOLVE"]
QualityRuleKind = Literal[
    "NOT_NULL",
    "UNIQUE",
    "RANGE",
    "REGEX",
    "ENUM",
    "ALLOWED_VALUES",
    "ENUM_SET",
    "FRESHNESS",
    "FRESHNESS_MINUTES",
    "ROW_COUNT_MIN",
]
QualityOnFail = Literal["WARN", "BLOCK"]
ParseWarningCode = Literal[
    "ENCODING_FALLBACK",
    "FORMULA_NOT_CACHED",
    "TS_INFERRED",
    "TRUNCATED_PAGES",
    "OCR_LOW_CONFIDENCE",
    "TABLE_LOW_QUALITY",
    "COLUMN_RENAMED",
    "ROW_MALFORMED",
    "NESTED_DEPTH_LIMIT",
    "EARLY_OFFICE_CONVERTED",
]


class ColumnProfile(BaseModel):
    """Column profile inferred from tabular or connector data."""

    name: str
    ordinal: int
    data_type: DataType
    nullable: bool
    null_count: int = 0
    null_ratio: float = 0.0
    distinct_estimate: int = 0
    sample_values: list[str] = Field(default_factory=list, max_length=10)
    pii_category: PiiCategory = "NONE"
    pii_hit_ratio: float = 0.0
    source_comment: str = ""


class ExtractedTable(BaseModel):
    """Structured table extracted to Parquet staging during unified parsing."""

    table_name: str
    origin: str
    row_count: int
    staging_path: str
    columns: list[ColumnProfile]
    header_rows: int = 1


class ParseWarning(BaseModel):
    """Non-fatal diagnostic warning emitted during file parsing."""

    code: ParseWarningCode
    detail: str


class UnifiedParseResponse(BaseModel):
    """Dual-track (Markdown + Parquet tables) response from POST /parse/unified."""

    parse_id: str
    title: str
    source_format: str
    engine: str
    sha256: str
    markdown: str
    tables: list[ExtractedTable]
    warnings: list[ParseWarning] = Field(default_factory=list)


class ParseResponse(BaseModel):
    """Text-track-only response from POST /parse."""

    title: str
    markdown: str
    engine: str


class ConnectorSpecModel(BaseModel):
    """Serializable ConnectorSpec card returned by GET /connectors/types."""

    connector_type: ConnectorType
    display_name: str
    category: ConnectorCategory
    config_schema: dict[str, Any]
    secret_fields: list[str]
    supports_modes: list[SyncMode]
    supports_discover: bool
    image_tier: Literal["lite", "full"]
    available: bool = True


class ConnectorActionRequest(BaseModel):
    """Request body for POST /connectors/test and POST /connectors/discover."""

    connector_type: ConnectorType
    config: dict[str, Any] = Field(default_factory=dict)
    secrets: dict[str, str] = Field(default_factory=dict)
    allow_private_network: bool = False


class ConnectorPreviewRequest(ConnectorActionRequest):
    """Request body for POST /connectors/preview."""

    model_config = ConfigDict(populate_by_name=True)

    ref: dict[str, Any] = Field(default_factory=dict)
    limit: int = Field(default=100, ge=1, le=1000)


class TestResult(BaseModel):
    """Response body for POST /connectors/test."""

    ok: bool
    message: str
    latency_ms: int


class DiscoveredColumn(BaseModel):
    """Column descriptor inside DiscoveredObject."""

    name: str
    native_type: str
    data_type: DataType
    nullable: bool = True


class DiscoveredObject(BaseModel):
    """Object discovered from a remote connector."""

    ref: dict[str, Any]
    display_name: str
    source_kind: SourceKind
    columns: list[DiscoveredColumn]
    primary_key: list[str] = Field(default_factory=list)
    suggested_cursor: str | None = None
    estimated_rows: int | None = None


class PreviewResponse(BaseModel):
    """Tabular preview payload for connector and lake previews."""

    columns: list[ColumnProfile]
    rows: list[dict[str, Any]]


class QualityRule(BaseModel):
    """Quality assertion rule evaluated prior to DuckLake commit."""

    id: str = ""
    dataset_id: str = ""
    rule_kind: QualityRuleKind
    column_name: str | None = None
    params: dict[str, Any] = Field(default_factory=dict)
    on_fail: QualityOnFail = "WARN"


class QualityRuleOutcome(BaseModel):
    """Evaluation outcome of a single QualityRule."""

    rule_id: str = ""
    rule_kind: str
    column_name: str | None = None
    passed: bool
    on_fail: QualityOnFail
    detail: str = ""


class QualityReport(BaseModel):
    """Aggregate quality validation report for a synchronization batch."""

    passed: bool = True
    blocked: bool = False
    rules: list[QualityRuleOutcome] = Field(default_factory=list)


class SchemaChange(BaseModel):
    """Detected schema diff during synchronization or staging commit."""

    added: list[ColumnProfile] = Field(default_factory=list)
    removed: list[str] = Field(default_factory=list)
    widened: list[tuple[str, str, str]] = Field(default_factory=list)
    narrowed: list[tuple[str, str, str]] = Field(default_factory=list)
    breaking: bool = False


class SyncRequest(BaseModel):
    """Request body for POST /connectors/sync."""

    tenant_id: str
    dataset_id: str
    batch_id: str
    connector_type: ConnectorType
    config: dict[str, Any] = Field(default_factory=dict)
    secrets: dict[str, str] = Field(default_factory=dict)
    allow_private_network: bool = False
    source_ref: dict[str, Any] = Field(default_factory=dict)
    sync_mode: SyncMode
    primary_key: list[str] = Field(default_factory=list)
    cursor_column: str | None = None
    cursor_from: str | None = None
    delete_column: str | None = None
    stream_offsets: dict[str, str] = Field(default_factory=dict)
    continuous_stream: bool = False
    cdc_slot_meta: dict[str, Any] = Field(default_factory=dict)
    lake_table: str
    schema_policy: SchemaPolicy = "ADDITIVE"
    expected_schema: list[ColumnProfile] = Field(default_factory=list)
    quality_rules: list[QualityRule] = Field(default_factory=list)
    batch_rows: int = Field(default=50_000, ge=1, le=500_000)


class SyncResult(BaseModel):
    """Response body for POST /connectors/sync and items of /lake/commit_staging."""

    model_config = ConfigDict(populate_by_name=True)

    dataset_id: str
    rows_read: int
    rows_written: int
    rows_deleted: int
    total_rows: int | None = None
    cursor_to: str | None = None
    stream_offsets: dict[str, str] = Field(default_factory=dict)
    cdc_slot_meta: dict[str, Any] = Field(default_factory=dict)
    deleted_primary_keys: list[Any] = Field(default_factory=list)
    snapshot_id: int
    columns_schema: list[ColumnProfile] = Field(alias="schema", serialization_alias="schema")
    schema_change: SchemaChange
    quality: QualityReport


class CommitItem(BaseModel):
    """Single Parquet staging table commit instruction."""

    staging_path: str
    dataset_id: str
    lake_table: str
    batch_id: str
    schema_policy: SchemaPolicy = "ADDITIVE"
    expected_schema: list[ColumnProfile] = Field(default_factory=list)


class CommitStagingRequest(BaseModel):
    """Request body for POST /lake/commit_staging."""

    tenant_id: str
    parse_id: str
    items: list[CommitItem]


class LakeMaintenanceRequest(BaseModel):
    """Request body for POST /lake/maintenance."""

    tenant_id: str
    keep_days: int = Field(default=30, ge=1, le=3650)


class LakeMaintenanceResult(BaseModel):
    """Response body for POST /lake/maintenance."""

    merged_files: int
    expired_snapshots: int
    deleted_files: int


class LakePreviewRequest(BaseModel):
    """Request body for POST /lake/preview."""

    tenant_id: str
    lake_table: str
    limit: int = Field(default=100, ge=1, le=1000)
    snapshot_id: int | None = None


class LakeSnapshotInfo(BaseModel):
    """Snapshot entry returned by GET /lake/snapshots."""

    snapshot_id: int
    committed_at: str
    rows_added: int = 0
    rows_deleted: int = 0


class DropLakeTableRequest(BaseModel):
    """Request body for POST /lake/drop_table."""

    tenant_id: str
    lake_table: str


class MaterializeRequest(BaseModel):
    """Request body for POST /lake/materialize (Step 04 DERIVED_VIEW)."""

    tenant_id: str
    dataset_id: str = ""
    lake_table: str
    sql: str
    batch_id: str


ExportFormatLiteral = Literal["XLSX", "CSV", "PDF", "DOCX", "PPTX", "MARKDOWN"]
ChartRenderFormatLiteral = Literal["svg", "png"]
PageSizeLiteral = Literal["A4", "A3"]
OrientationLiteral = Literal["portrait", "landscape"]


class ExportColumnSpec(BaseModel):
    """Column metadata inside ExportTable (Step 12 §12.2)."""

    name: str
    data_type: DataType
    format: str | None = None


class ExportTable(BaseModel):
    """Tabular section inside ExportDocument (Step 12 §12.2)."""

    kind: Literal["TABLE"] = "TABLE"
    title: str = ""
    columns: list[ExportColumnSpec]
    rows: list[list[Any]] = Field(default_factory=list)
    totals: list[Any] | None = None
    rows_parquet_path: str | None = None
    row_kinds: list[str] = Field(default_factory=list)


class ExportChart(BaseModel):
    """Chart section inside ExportDocument (Step 12 §12.2)."""

    kind: Literal["CHART"] = "CHART"
    title: str = ""
    widget_kind: str
    echarts_option: dict[str, Any]
    data_table: ExportTable | None = None
    width_px: int = Field(default=960, ge=100, le=4096)
    height_px: int = Field(default=540, ge=100, le=4096)


class ExportKpi(BaseModel):
    """KPI card section inside ExportDocument (Step 12 §12.2)."""

    kind: Literal["KPI"] = "KPI"
    label: str
    value: str
    compare: str | None = None


class ExportText(BaseModel):
    """Heading, paragraph, or Markdown section inside ExportDocument (Step 12 §12.2)."""

    kind: Literal["HEADING", "PARAGRAPH", "MARKDOWN"]
    text: str
    level: int = Field(default=1, ge=1, le=6)


ExportSectionUnion = Annotated[
    ExportTable | ExportChart | ExportKpi | ExportText,
    Field(discriminator="kind"),
]


class ExportDocument(BaseModel):
    """Unified multi-format export document model shared with Rust axpoint-domain::export."""

    title: str
    subtitle: str = ""
    generated_at: datetime
    generated_by: str
    timezone: str = "UTC"
    watermark: str | None = None
    sections: list[ExportSectionUnion] = Field(default_factory=list)
    template_path: str | None = None
    placeholders: dict[str, str] = Field(default_factory=dict)
    csv_bom: bool = False
    page_size: PageSizeLiteral = "A4"
    orientation: OrientationLiteral = "portrait"


class RenderChartRequest(BaseModel):
    """Request body for POST /render/chart (Step 12 §12.3)."""

    option: dict[str, Any]
    width: int = Field(default=960, ge=64, le=4096)
    height: int = Field(default=540, ge=64, le=4096)
    format: ChartRenderFormatLiteral = "png"
    theme: Literal["light", "dark"] | None = None


class SidecarExportRequest(BaseModel):
    """Request body for POST /export/{format} (Step 12 §12.4)."""

    tenant_id: str
    job_id: str
    document: ExportDocument


class SidecarExportResponse(BaseModel):
    """Response body for POST /export/{format} (Step 12 §12.4)."""

    file_path: str
    file_size: int
    sha256: str
    file_name: str


MaskRuleLiteral = Literal[
    "NONE",
    "MASK_PHONE",
    "MASK_ID_CARD",
    "MASK_EMAIL",
    "MASK_BANK_CARD",
    "MASK_NAME",
    "MASK_ADDRESS",
    "HMAC_HASH",
    "REDACT",
]


class PiiSpanModel(BaseModel):
    """Detected PII character span returned by POST /pii/analyze (Step 13 §13.8.1)."""

    category: PiiCategory
    start: int = Field(ge=0)
    end: int = Field(ge=0)
    score: float = Field(ge=0.0, le=1.0)


class PiiAnalyzeRequest(BaseModel):
    """Request body for POST /pii/analyze (Step 13 §13.8.1)."""

    texts: list[str]
    language: str = "zh"
    score_threshold: float = Field(default=0.6, ge=0.0, le=1.0)


class PiiAnalyzeResponse(BaseModel):
    """Response body for POST /pii/analyze (Step 13 §13.8.1)."""

    results: list[list[PiiSpanModel]]


class PiiAnonymizeRequest(BaseModel):
    """Request body for POST /pii/anonymize (Step 13 §13.8.1)."""

    texts: list[str]
    rules: dict[PiiCategory, MaskRuleLiteral] = Field(default_factory=dict)
    language: str = "zh"
    score_threshold: float = Field(default=0.6, ge=0.0, le=1.0)
    tenant_id: str = "default"


class PiiAnonymizeResponse(BaseModel):
    """Response body for POST /pii/anonymize (Step 13 §13.8.1)."""

    texts: list[str]
