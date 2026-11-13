from __future__ import annotations

# Shared Pydantic v2 schemas for AxPoint Parser & Connector Sidecar (Steps 01 & 02).
from typing import Any, Literal

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

SyncMode = Literal["SNAPSHOT", "APPEND", "UPSERT"]
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
