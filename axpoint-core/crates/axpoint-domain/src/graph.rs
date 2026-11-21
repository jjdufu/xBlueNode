//! `AxGraph` domain types, time-series rollup sketches (`HyperLogLog++` & top-K frequency),
//! entity name normalization, and graph query/algorithm DTOs (Step 03).

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};
use std::sync::LazyLock;

use chrono::{DateTime, Duration, Utc};
use hyperloglogplus::{HyperLogLog, HyperLogLogPlus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::AppError;
use crate::enums::{AggFn, GraphAlgo, MetricSourceTag, RollupGrain};
use crate::topology::{BlueLink, BlueNode};

/// `HyperLogLog++` register precision (`p = 14`, 16,384 registers, ~0.81% standard error).
pub const HLL_PRECISION: u8 = 14;

/// Maximum distinct keys retained in `xb_metric_rollups.freq` before folding into [`FREQ_OTHER_KEY`].
pub const FREQ_TOP_K: usize = 100;

/// Overflow bucket key for truncated categorical frequency distributions.
pub const FREQ_OTHER_KEY: &str = "__other__";

/// Embedded organization suffixes loaded from `normalize_suffixes.txt`, sorted longest-first.
static ORG_SUFFIXES: LazyLock<Vec<String>> = LazyLock::new(|| {
    let raw = include_str!("normalize_suffixes.txt");
    let mut list: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|s| s.nfkc().collect::<String>().to_lowercase())
        .collect();
    list.sort_by(|a, b| {
        b.chars()
            .count()
            .cmp(&a.chars().count())
            .then_with(|| a.cmp(b))
    });
    list.dedup();
    list
});

/// Deterministic 64-bit `FNV-1a` + `SplitMix64` hasher so `HyperLogLogPlus` sketches
/// serialize and merge identically across worker processes and restarts.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicHasher {
    state: u64,
}

impl DeterministicHasher {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    #[must_use]
    const fn new() -> Self {
        Self {
            state: Self::OFFSET_BASIS,
        }
    }
}

impl Hasher for DeterministicHasher {
    fn finish(&self) -> u64 {
        // SplitMix64 finalizer for uniform avalanche across all 64 bits required by HLL++.
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state ^= u64::from(byte);
            self.state = self.state.wrapping_mul(Self::FNV_PRIME);
        }
    }
}

/// Stateless, `serde`-serializable `BuildHasher` producing [`DeterministicHasher`] instances.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct DeterministicBuildHasher;

impl BuildHasher for DeterministicBuildHasher {
    type Hasher = DeterministicHasher;

    fn build_hasher(&self) -> Self::Hasher {
        DeterministicHasher::new()
    }
}

type StringHll = HyperLogLogPlus<str, DeterministicBuildHasher>;

/// Geographic WGS-84 coordinate (`lng` in `[-180, 180]`, `lat` in `[-90, 90]`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    /// Longitude in degrees (`-180.0..=180.0`).
    pub lng: f64,
    /// Latitude in degrees (`-90.0..=90.0`).
    pub lat: f64,
}

impl GeoPoint {
    /// Validates WGS-84 longitude and latitude bounds.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `lng` or `lat` is non-finite or out of range.
    pub fn validate(&self) -> Result<(), AppError> {
        if !self.lng.is_finite() || !(-180.0..=180.0).contains(&self.lng) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("geo.lng out of [-180, 180]: {}", self.lng),
            ));
        }
        if !self.lat.is_finite() || !(-90.0..=90.0).contains(&self.lat) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("geo.lat out of [-90, 90]: {}", self.lat),
            ));
        }
        Ok(())
    }
}

/// Strategy for updating `xb_nodes.metrics` during node upsert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MetricsMerge {
    /// Merge incoming keys onto existing `metrics` (`metrics || $new`).
    #[default]
    Merge,
    /// Replace the entire `metrics` map with the incoming map.
    Replace,
}

/// Business-key reference `(type_name, node_key)` to a `BlueNode`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeRef {
    /// Node type name, e.g., `Warehouse`.
    pub type_name: String,
    /// Business key within the node type, e.g., `W-1`.
    pub node_key: String,
}

/// Full `xb_nodes` persistence record.
pub type NodeRecord = BlueNode;

/// Single node entry inside [`BulkNodesRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkNodeInput {
    /// Node type name.
    pub type_name: String,
    /// Business key.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Arbitrary JSON properties.
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Numeric metrics snapshot.
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
    /// Merge mode for `metrics`.
    #[serde(default)]
    pub metrics_merge: MetricsMerge,
    /// Optional WGS-84 coordinate.
    #[serde(default)]
    pub geo: Option<GeoPoint>,
    /// Optional source dataset UUID.
    #[serde(default)]
    pub source_dataset_id: Option<Uuid>,
    /// Optional source row primary key string.
    #[serde(default)]
    pub source_row_key: Option<String>,
}

/// Link endpoint selector accepting either an existing node UUID or a `(type_name, node_key)` reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LinkEndpointRef {
    /// Direct node UUID.
    Id(Uuid),
    /// Business-key reference resolved within the tenant.
    Key(NodeRef),
}

/// Single link entry inside [`BulkNodesRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkLinkInput {
    /// Relationship type name, e.g., `SUPPLIES`.
    pub link_type: String,
    /// Source node selector.
    pub src: LinkEndpointRef,
    /// Destination node selector.
    pub dst: LinkEndpointRef,
    /// Optional propagation weight (`(0.0, 1.0]`); defaults to `xb_edge_types.impact_weight` or `0.6`.
    #[serde(default)]
    pub weight: Option<f64>,
    /// Arbitrary JSON link properties.
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Optional validity start timestamp.
    #[serde(default)]
    pub valid_from: Option<DateTime<Utc>>,
    /// Optional validity end timestamp.
    #[serde(default)]
    pub valid_to: Option<DateTime<Utc>>,
}

/// Request payload for `POST /api/xblue/nodes/bulk` (up to 5,000 nodes + 20,000 links).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkNodesRequest {
    /// Nodes to upsert in a single `UNNEST` statement.
    #[serde(default)]
    pub nodes: Vec<BulkNodeInput>,
    /// Links to upsert after node resolution.
    #[serde(default)]
    pub links: Vec<BulkLinkInput>,
}

/// Unresolved link reported when an endpoint reference could not be found.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedLink {
    /// Zero-based index in `BulkNodesRequest.links`.
    pub index: usize,
    /// Relationship type name.
    pub link_type: String,
    /// Reason description.
    pub reason: String,
}

/// Response payload for `POST /api/xblue/nodes/bulk`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkNodesResponse {
    /// Count of upserted nodes.
    pub upserted_nodes: usize,
    /// Count of upserted links.
    pub upserted_links: usize,
    /// Links skipped due to unresolved endpoints.
    pub unresolved: Vec<UnresolvedLink>,
}

/// Partial update payload for `PATCH /api/xblue/nodes/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchNodeRequest {
    /// Updated display title.
    #[serde(default)]
    pub title: Option<String>,
    /// Properties merged or replaced on the node.
    #[serde(default)]
    pub properties: Option<serde_json::Map<String, serde_json::Value>>,
    /// Metrics update map.
    #[serde(default)]
    pub metrics: Option<BTreeMap<String, f64>>,
    /// Merge strategy for `metrics`.
    #[serde(default)]
    pub metrics_merge: MetricsMerge,
    /// Optional geographic coordinate (`Some(None)` clears geo when represented via explicit patch; `Some(pt)` sets geo).
    #[serde(default)]
    pub geo: Option<GeoPoint>,
    /// Optimistic concurrency guard (`409 VERSION_CONFLICT` on mismatch).
    #[serde(default)]
    pub if_row_version: Option<i64>,
}

/// Query filter for `GET /api/xblue/nodes`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeListQuery {
    /// Optional node type filter.
    #[serde(default)]
    pub type_name: Option<String>,
    /// Optional substring/title filter.
    #[serde(default)]
    pub q: Option<String>,
    /// Optional geographic bounding box `"minLng,minLat,maxLng,maxLat"`.
    #[serde(default)]
    pub bbox: Option<String>,
    /// Page size (default 200, max 10,000 for bbox queries).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Explicitly request unmasked PII properties (`403 PERMISSION_DENIED` unless authorized, `11-STEP13` §13.7).
    #[serde(default)]
    pub reveal_pii: bool,
}

impl NodeListQuery {
    /// Parses `bbox` (`"minLng,minLat,maxLng,maxLat"`) into validated `[min_lng, min_lat, max_lng, max_lat]`.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `bbox` is malformed or out of WGS-84 bounds.
    pub fn parse_bbox(&self) -> Result<Option<[f64; 4]>, AppError> {
        let Some(raw) = self.bbox.as_deref() else {
            return Ok(None);
        };
        let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
        let [min_lng_s, min_lat_s, max_lng_s, max_lat_s] = parts.as_slice() else {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "bbox must have 4 comma-separated numbers: minLng,minLat,maxLng,maxLat",
            ));
        };
        let min_lng: f64 = min_lng_s
            .parse()
            .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid minLng in bbox"))?;
        let min_lat: f64 = min_lat_s
            .parse()
            .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid minLat in bbox"))?;
        let max_lng: f64 = max_lng_s
            .parse()
            .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid maxLng in bbox"))?;
        let max_lat: f64 = max_lat_s
            .parse()
            .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid maxLat in bbox"))?;
        GeoPoint {
            lng: min_lng,
            lat: min_lat,
        }
        .validate()?;
        GeoPoint {
            lng: max_lng,
            lat: max_lat,
        }
        .validate()?;
        if min_lng > max_lng || min_lat > max_lat {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "bbox min coordinates must be <= max coordinates",
            ));
        }
        Ok(Some([min_lng, min_lat, max_lng, max_lat]))
    }
}

/// Response payload for `GET /api/xblue/nodes`, supporting both `A1` pagination (`items`, `next_cursor`)
/// and snapshot consumers (`nodes`, `links`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeListResponse {
    /// Paginated/filtered node records (`A1` §5 standard field).
    pub items: Vec<BlueNode>,
    /// Alias of `items` for `AxSimSnapshot` compatibility.
    pub nodes: Vec<BlueNode>,
    /// Incident links for snapshot consumers.
    pub links: Vec<BlueLink>,
    /// Next pagination cursor, if more items remain.
    pub next_cursor: Option<String>,
}

/// Full `xb_links` record returned by Step 03 link and graph APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkRecord {
    /// Link UUID.
    pub id: Uuid,
    /// Link type name.
    pub link_type: String,
    /// Source node UUID.
    pub src_node_id: Uuid,
    /// Destination node UUID.
    pub dst_node_id: Uuid,
    /// Causal propagation weight.
    pub weight: f64,
    /// Arbitrary JSON link properties.
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Optional source dataset UUID.
    #[serde(default)]
    pub source_dataset_id: Option<Uuid>,
    /// Optional validity start timestamp.
    #[serde(default)]
    pub valid_from: Option<DateTime<Utc>>,
    /// Optional validity end timestamp.
    #[serde(default)]
    pub valid_to: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Request payload for `POST /api/xblue/links`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertLinkRequest {
    /// Relationship type name.
    pub link_type: String,
    /// Source node UUID.
    pub src_node_id: Uuid,
    /// Destination node UUID.
    pub dst_node_id: Uuid,
    /// Propagation weight (defaults to `xb_edge_types.impact_weight` or `0.6`).
    #[serde(default)]
    pub weight: Option<f64>,
    /// Link JSON properties.
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Optional validity start timestamp.
    #[serde(default)]
    pub valid_from: Option<DateTime<Utc>>,
    /// Optional validity end timestamp.
    #[serde(default)]
    pub valid_to: Option<DateTime<Utc>>,
}

/// Query filter for `GET /api/xblue/links`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinkListQuery {
    /// Filter links incident to `node_id` (source or destination).
    #[serde(default)]
    pub node_id: Option<Uuid>,
    /// Filter by `link_type`.
    #[serde(default)]
    pub link_type: Option<String>,
    /// Maximum rows to return (default 500, max 5,000).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Single time-series observation inside [`WriteMetricsRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricPointInput {
    /// Direct node UUID (provide either `node_id` or `node`).
    #[serde(default)]
    pub node_id: Option<Uuid>,
    /// Business-key node selector (provide either `node_id` or `node`).
    #[serde(default)]
    pub node: Option<NodeRef>,
    /// Metric identifier matching `^[a-z][a-z0-9_]{0,62}$`.
    pub metric: String,
    /// Observation timestamp.
    pub ts: DateTime<Utc>,
    /// Numeric value (at least one of `value` or `value_text` is required).
    #[serde(default)]
    pub value: Option<f64>,
    /// Categorical/text value (at least one of `value` or `value_text` is required).
    #[serde(default)]
    pub value_text: Option<String>,
}

/// Request payload for `POST /api/xblue/metrics/write` (up to 50,000 points).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteMetricsRequest {
    /// Time-series points to upsert.
    pub points: Vec<MetricPointInput>,
    /// Provenance tag (defaults to `SYNC`).
    #[serde(default = "default_metric_source_tag")]
    pub source_tag: MetricSourceTag,
    /// Optional ingestion batch UUID.
    #[serde(default)]
    pub batch_id: Option<Uuid>,
}

const fn default_metric_source_tag() -> MetricSourceTag {
    MetricSourceTag::Sync
}

/// Response payload for `POST /api/xblue/metrics/write`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteMetricsResponse {
    /// Number of points written.
    pub written: usize,
    /// Number of late-arrival minute buckets marked dirty in `xb_rollup_dirty`.
    pub dirty_minutes: usize,
    /// Enqueued `ROLLUP` job UUID.
    pub rollup_job_id: Uuid,
}

/// Query granularity for `GET /api/xblue/nodes/{id}/metrics/{metric}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MetricQueryGrain {
    /// Raw points from `xb_metric_series`.
    Raw,
    /// 1-minute buckets from `xb_metric_rollups`.
    Minute,
    /// 1-hour buckets from `xb_metric_rollups`.
    Hour,
    /// 1-day buckets from `xb_metric_rollups`.
    Day,
    /// 1-month buckets from `xb_metric_rollups`.
    Month,
}

impl MetricQueryGrain {
    /// Automatically selects query granularity based on `[from, to]` duration (§03.3.3).
    #[must_use]
    pub fn auto_select(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        let span = to.signed_duration_since(from);
        if span <= Duration::hours(6) {
            Self::Raw
        } else if span <= Duration::days(7) {
            Self::Minute
        } else if span <= Duration::days(90) {
            Self::Hour
        } else {
            Self::Day
        }
    }

    /// Promotes to the next coarser granularity when point count exceeds `5,000`.
    #[must_use]
    pub const fn promote(self) -> Option<Self> {
        match self {
            Self::Raw => Some(Self::Minute),
            Self::Minute => Some(Self::Hour),
            Self::Hour => Some(Self::Day),
            Self::Day => Some(Self::Month),
            Self::Month => None,
        }
    }

    /// Converts non-`RAW` query granularity into [`RollupGrain`].
    #[must_use]
    pub const fn as_rollup_grain(self) -> Option<RollupGrain> {
        match self {
            Self::Raw => None,
            Self::Minute => Some(RollupGrain::Minute),
            Self::Hour => Some(RollupGrain::Hour),
            Self::Day => Some(RollupGrain::Day),
            Self::Month => Some(RollupGrain::Month),
        }
    }
}

/// Query parameters for `GET /api/xblue/nodes/{id}/metrics/{metric}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetricSeriesQuery {
    /// Start timestamp (inclusive).
    pub from: DateTime<Utc>,
    /// End timestamp (exclusive).
    pub to: DateTime<Utc>,
    /// Optional explicit granularity (`RAW`, `MINUTE`, `HOUR`, `DAY`, `MONTH`).
    #[serde(default)]
    pub grain: Option<MetricQueryGrain>,
    /// Aggregation function (defaults to `AVG`).
    #[serde(default = "default_agg_fn")]
    pub agg: AggFn,
}

const fn default_agg_fn() -> AggFn {
    AggFn::Avg
}

/// Single returned point in `GET /api/xblue/nodes/{id}/metrics/{metric}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSeriesPoint {
    /// Bucket or observation timestamp.
    pub ts: DateTime<Utc>,
    /// Aggregated numeric value (or `COUNT` / `COUNT_DISTINCT`).
    pub value: Option<f64>,
    /// Optional categorical value (for `LAST` on text metrics).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_text: Option<String>,
}

/// Pre-aggregated rollup bucket stored in `xb_metric_rollups`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollupRow {
    /// Node UUID.
    pub node_id: Uuid,
    /// Metric name.
    pub metric: String,
    /// Rollup bucket granularity.
    pub grain: RollupGrain,
    /// Bucket start timestamp.
    pub bucket_start: DateTime<Utc>,
    /// Total observation count in bucket.
    pub cnt: i64,
    /// Sum of numeric values.
    pub sum_v: Option<f64>,
    /// Minimum numeric value.
    pub min_v: Option<f64>,
    /// Maximum numeric value.
    pub max_v: Option<f64>,
    /// Latest numeric value by `last_ts`.
    pub last_v: Option<f64>,
    /// Latest text value by `last_ts`.
    pub last_text: Option<String>,
    /// Timestamp of the latest observation in bucket.
    pub last_ts: DateTime<Utc>,
    /// Serialized `HyperLogLog++` sketch bytes (`p = 14`) for `value_text`.
    pub hll: Option<Vec<u8>>,
    /// Top-100 frequency map for `value_text` (overflow in `__other__`).
    pub freq: Option<BTreeMap<String, u64>>,
}

/// Window type for `POST /api/xblue/metrics/window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WindowKind {
    /// Non-overlapping fixed-size buckets.
    Tumbling,
    /// Overlapping sliding window evaluated over minute rollups.
    Sliding,
}

/// Window specification (`size` and optional `step` such as `"15m"`, `"1h"`, `"3h"`, `"1d"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowSpec {
    /// `TUMBLING` or `SLIDING`.
    pub kind: WindowKind,
    /// Window duration string (`"1m"`, `"15m"`, `"1h"`, `"3h"`, `"1d"`).
    pub size: String,
    /// Step duration string for `SLIDING` windows (defaults to `size` for `TUMBLING`).
    #[serde(default)]
    pub step: Option<String>,
}

impl WindowSpec {
    /// Parses a human duration token (`"<n>m"`, `"<n>h"`, `"<n>d"`) into [`Duration`].
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if the duration format is invalid or non-positive.
    pub fn parse_duration(spec: &str) -> Result<Duration, AppError> {
        let trimmed = spec.trim();
        if trimmed.len() < 2 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid duration '{spec}': expected <n>m, <n>h, or <n>d"),
            ));
        }
        let (num_str, unit) = trimmed.split_at(trimmed.len().saturating_sub(1));
        let count: i64 = num_str.parse().map_err(|_| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid duration number in '{spec}'"),
            )
        })?;
        if count <= 0 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "window duration must be > 0",
            ));
        }
        match unit {
            "m" => Ok(Duration::minutes(count)),
            "h" => Ok(Duration::hours(count)),
            "d" => Ok(Duration::days(count)),
            _ => Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unsupported duration unit '{unit}' in '{spec}'"),
            )),
        }
    }
}

/// Grouping dimension for `POST /api/xblue/metrics/window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowGroupBy {
    /// Group per `node_id`.
    Node,
    /// Group per `type_name`.
    Type,
    /// Aggregate across all selected nodes into one series.
    #[default]
    None,
}

/// Request payload for `POST /api/xblue/metrics/window`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowQuery {
    /// Explicit node UUID filter.
    #[serde(default)]
    pub node_ids: Option<Vec<Uuid>>,
    /// Optional node type filter.
    #[serde(default)]
    pub type_name: Option<String>,
    /// Metric name.
    pub metric: String,
    /// Window range start (inclusive).
    pub from: DateTime<Utc>,
    /// Window range end (exclusive).
    pub to: DateTime<Utc>,
    /// Window definition.
    pub window: WindowSpec,
    /// Aggregation function.
    pub agg: AggFn,
    /// Grouping dimension (`node`, `type`, or `none`).
    #[serde(default)]
    pub group_by: WindowGroupBy,
}

/// Single aggregated bucket returned by `POST /api/xblue/metrics/window`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowBucketItem {
    /// Window bucket timestamp.
    pub ts: DateTime<Utc>,
    /// Optional group key (`node_id` string, `type_name`, or `"all"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_key: Option<String>,
    /// Aggregated value.
    pub value: Option<f64>,
}

/// Response payload for `POST /api/xblue/metrics/window`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowQueryResponse {
    /// Ordered window aggregation items.
    pub items: Vec<WindowBucketItem>,
}

/// Entry returned by `GET /api/xblue/metrics/catalog`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricCatalogEntry {
    /// Node type name.
    pub type_name: String,
    /// Metric identifier.
    pub metric: String,
    /// Total raw observation count.
    pub point_count: i64,
    /// Earliest observation timestamp.
    pub min_ts: Option<DateTime<Utc>>,
    /// Latest observation timestamp.
    pub max_ts: Option<DateTime<Utc>>,
}

/// Query parameters for `GET /api/xblue/nodes/search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSearchQuery {
    /// Search query string.
    pub q: String,
    /// Optional node type filter.
    #[serde(default)]
    pub type_name: Option<String>,
    /// Number of top RRF-fused hits to return (default 20, max 100).
    #[serde(default = "default_search_k")]
    pub k: u32,
}

const fn default_search_k() -> u32 {
    20
}

/// Hybrid FTS + vector RRF hit for `GET /api/xblue/nodes/search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSearchHit {
    /// Node UUID.
    pub id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Business key.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Node properties.
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Node metrics snapshot.
    pub metrics: BTreeMap<String, f64>,
    /// Optional WGS-84 coordinate.
    pub geo: Option<GeoPoint>,
    /// Reciprocal Rank Fusion score.
    pub score: f64,
    /// Retrieval channels that matched (`"FTS"`, `"VECTOR"`).
    pub channels: Vec<String>,
}

/// Response wrapper for `GET /api/xblue/nodes/search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSearchResponse {
    /// Ranked hybrid search hits.
    pub items: Vec<NodeSearchHit>,
}

/// Hypergraph document mentioning a business node via `xb_entity_links` + `MENTIONS` edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDocumentHit {
    /// Document UUID (`hg_docs.id`).
    pub doc_id: Uuid,
    /// Document title.
    pub title: String,
    /// Source filename or URI.
    pub source_name: String,
    /// Linked hypergraph entity UUID.
    pub entity_id: Uuid,
    /// Linked hypergraph entity name.
    pub entity_name: String,
    /// Matching paragraph excerpts.
    pub snippets: Vec<String>,
}

/// Response wrapper for `GET /api/xblue/nodes/{id}/documents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDocumentsResponse {
    /// Documents linked to the node.
    pub items: Vec<NodeDocumentHit>,
}

/// Matching method in `xb_entity_links`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EntityMatchMethod {
    /// `normalize(entity.name) == normalize(node.node_key)` (`1.0`).
    ExactKey,
    /// `normalize(entity.name) == normalize(node.title)` (`0.95`).
    ExactTitle,
    /// Normalized entity synonym matches node key or title (`0.9`).
    Synonym,
    /// Embedding cosine similarity `>= 0.88` with margin `>= 0.03`.
    Embedding,
    /// Manually created or confirmed link.
    Manual,
}

impl EntityMatchMethod {
    /// Canonical uppercase SQL token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactKey => "EXACT_KEY",
            Self::ExactTitle => "EXACT_TITLE",
            Self::Synonym => "SYNONYM",
            Self::Embedding => "EMBEDDING",
            Self::Manual => "MANUAL",
        }
    }

    /// Parses a SQL token into [`EntityMatchMethod`].
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ENUM`) if unknown.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw {
            "EXACT_KEY" => Ok(Self::ExactKey),
            "EXACT_TITLE" => Ok(Self::ExactTitle),
            "SYNONYM" => Ok(Self::Synonym),
            "EMBEDDING" => Ok(Self::Embedding),
            "MANUAL" => Ok(Self::Manual),
            _ => Err(AppError::bad(
                "INVALID_ENUM",
                format!("unknown EntityMatchMethod: {raw}"),
            )),
        }
    }
}

/// Hypergraph-entity ↔ business-node link record (`xb_entity_links`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityLink {
    /// Hypergraph entity UUID.
    pub entity_id: Uuid,
    /// Hypergraph entity name.
    pub entity_name: String,
    /// Business node UUID.
    pub node_id: Uuid,
    /// Business node key.
    pub node_key: String,
    /// Business node title.
    pub node_title: String,
    /// Resolution method.
    pub match_method: EntityMatchMethod,
    /// Confidence score in `[0.0, 1.0]`.
    pub confidence: f64,
    /// Whether confirmed (`confidence >= 0.9` auto-confirms).
    pub confirmed: bool,
    /// Whether explicitly rejected by an operator.
    pub rejected: bool,
    /// Link creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request payload for `POST /api/xblue/entities/resolve`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveEntitiesRequest {
    /// Optional single node UUID to scope resolution.
    #[serde(default)]
    pub node_id: Option<Uuid>,
}

/// Response payload for `POST /api/xblue/entities/resolve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveEntitiesResponse {
    /// Total links created or updated.
    pub resolved: usize,
    /// Count of links auto-confirmed (`confidence >= 0.9`).
    pub auto_confirmed: usize,
    /// Count of candidate links awaiting manual review (`confidence < 0.9`).
    pub pending_candidates: usize,
}

/// Response wrapper for `GET /api/xblue/entities/candidates`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityCandidatesResponse {
    /// Unconfirmed, non-rejected candidate links.
    pub items: Vec<EntityLink>,
}

/// Edge traversal direction for neighbor queries and degree centrality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TraversalDirection {
    /// Follow outgoing edges (`src -> dst`).
    Out,
    /// Follow incoming edges (`dst <- src`).
    In,
    /// Follow edges in both directions.
    #[default]
    Both,
}

/// Query parameters for `GET /api/xblue/graph/neighbors`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborQuery {
    /// Seed node UUID.
    pub node_id: Uuid,
    /// Traversal hop depth (`1..=4`, default `1`).
    #[serde(default = "default_neighbor_depth")]
    pub depth: u32,
    /// Traversal direction (`OUT`, `IN`, `BOTH`).
    #[serde(default)]
    pub direction: TraversalDirection,
    /// Optional comma-separated `link_types` filter.
    #[serde(default)]
    pub link_types: Option<String>,
    /// Maximum returned nodes (`<= 2000`, default `500`).
    #[serde(default = "default_neighbor_limit")]
    pub limit: u32,
}

const fn default_neighbor_depth() -> u32 {
    1
}

const fn default_neighbor_limit() -> u32 {
    500
}

/// Query parameters for `GET /api/xblue/graph/subgraph`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubgraphQuery {
    /// Optional comma-separated node `type_names`.
    #[serde(default)]
    pub type_names: Option<String>,
    /// Optional comma-separated `link_types`.
    #[serde(default)]
    pub link_types: Option<String>,
    /// Maximum nodes (`<= 2000`, default `2000`).
    #[serde(default = "default_subgraph_limit")]
    pub limit: u32,
}

const fn default_subgraph_limit() -> u32 {
    2_000
}

/// Subgraph response shared by `GET /api/xblue/graph/neighbors` and `GET /api/xblue/graph/subgraph`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubgraphResponse {
    /// Subgraph nodes.
    pub nodes: Vec<BlueNode>,
    /// Subgraph links connecting `nodes`.
    pub links: Vec<LinkRecord>,
    /// `true` if `limit` truncated additional nodes.
    pub truncated: bool,
}

/// Query parameters for `GET /api/xblue/graph/path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathQuery {
    /// Source node UUID.
    pub from: Uuid,
    /// Target node UUID.
    pub to: Uuid,
    /// Maximum hop depth (`1..=6`, default `6`).
    #[serde(default = "default_path_max_depth")]
    pub max_depth: u32,
    /// Whether to compute weighted shortest path (`cost = 1 / weight`) via Dijkstra.
    #[serde(default)]
    pub weighted: bool,
}

const fn default_path_max_depth() -> u32 {
    6
}

/// Shortest-path response for `GET /api/xblue/graph/path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathResponse {
    /// Ordered node UUIDs from `from` to `to` (empty if unreachable within `max_depth`).
    pub path: Vec<Uuid>,
    /// Full node records along `path`.
    pub nodes: Vec<BlueNode>,
    /// Traversed links along `path`.
    pub links: Vec<LinkRecord>,
    /// Total path cost (`hop count` when unweighted, `sum(1 / weight)` when weighted).
    pub total_cost: Option<f64>,
}

/// Request payload for `POST /api/xblue/graph/algorithms/{algo}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlgoRunRequest {
    /// Optional node type filter.
    #[serde(default)]
    pub type_names: Vec<String>,
    /// Optional link type filter.
    #[serde(default)]
    pub link_types: Vec<String>,
    /// Algorithm-specific parameters (`damping`, `max_iter`, `tol`, `direction`, `seed`, `weak`).
    #[serde(default = "default_empty_json_object")]
    pub params: serde_json::Value,
}

fn default_empty_json_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Ranked top entry in [`AlgoRunResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgoTopEntry {
    /// Node UUID.
    pub node_id: Uuid,
    /// Node title.
    pub title: String,
    /// Centrality / `PageRank` score, if applicable.
    pub score: Option<f64>,
    /// Community / component identifier, if applicable.
    pub community: Option<i64>,
}

/// Response payload for `POST /api/xblue/graph/algorithms/{algo}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgoRunResponse {
    /// Executed algorithm.
    pub algo: GraphAlgo,
    /// Deterministic SHA-256 hash of `(type_names, link_types, params)`.
    pub params_hash: String,
    /// Number of nodes in analyzed graph.
    pub node_count: usize,
    /// Number of edges in analyzed graph.
    pub edge_count: usize,
    /// Execution wall time in milliseconds.
    pub duration_ms: u64,
    /// Top 50 nodes ordered by score desc (or community size).
    pub top: Vec<AlgoTopEntry>,
}

/// Persisted row in `xb_graph_algo_results`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgoResultRow {
    /// Algorithm kind.
    pub algo: GraphAlgo,
    /// Parameter hash.
    pub params_hash: String,
    /// Node UUID.
    pub node_id: Uuid,
    /// Node title.
    pub title: String,
    /// Score value.
    pub score: Option<f64>,
    /// Community / component label.
    pub community: Option<i64>,
    /// Computation timestamp.
    pub computed_at: DateTime<Utc>,
}

/// Query parameters for `GET /api/xblue/graph/algorithms/{algo}/results`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlgoResultsQuery {
    /// Optional parameter hash (defaults to latest run for `algo`).
    #[serde(default)]
    pub params_hash: Option<String>,
    /// Page size (default 100, max 1,000).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Paginated response for `GET /api/xblue/graph/algorithms/{algo}/results`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgoResultsResponse {
    /// Algorithm result rows.
    pub items: Vec<AlgoResultRow>,
    /// Next page cursor.
    pub next_cursor: Option<String>,
}

/// Builds a serialized `HyperLogLog++` (`p = 14`) JSON byte vector from string observations.
///
/// # Errors
/// Returns [`AppError::Database`] if sketch construction or JSON serialization fails.
pub fn build_hll_bytes<S: AsRef<str>>(values: &[S]) -> Result<Vec<u8>, AppError> {
    let mut hll = StringHll::new(HLL_PRECISION, DeterministicBuildHasher)
        .map_err(|err| AppError::Database(format!("HLL init failed: {err}")))?;
    for v in values {
        hll.insert(v.as_ref());
    }
    serde_json::to_vec(&hll)
        .map_err(|err| AppError::Database(format!("HLL serialize failed: {err}")))
}

/// Merges two serialized `HyperLogLog++` (`p = 14`) sketches into a single serialized sketch.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if either byte slice cannot be deserialized
/// or has incompatible precision.
pub fn merge_hll(a: &[u8], b: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut sketch_a: StringHll = serde_json::from_slice(a)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid HLL lhs: {err}")))?;
    let sketch_b: StringHll = serde_json::from_slice(b)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid HLL rhs: {err}")))?;
    sketch_a
        .merge(&sketch_b)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("HLL merge failed: {err}")))?;
    serde_json::to_vec(&sketch_a)
        .map_err(|err| AppError::Database(format!("HLL serialize failed: {err}")))
}

/// Estimates distinct cardinality from a serialized `HyperLogLog++` sketch.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `bytes` is not a valid sketch.
pub fn estimate_hll(bytes: &[u8]) -> Result<f64, AppError> {
    let mut sketch: StringHll = serde_json::from_slice(bytes)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid HLL bytes: {err}")))?;
    Ok(sketch.count())
}

/// Truncates a categorical frequency map to the top [`FREQ_TOP_K`] entries, folding any remaining
/// counts into [`FREQ_OTHER_KEY`].
#[must_use]
pub fn truncate_freq_map(counts: BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    let mut other_acc = counts.get(FREQ_OTHER_KEY).copied().unwrap_or(0);
    let mut entries: Vec<(String, u64)> = counts
        .into_iter()
        .filter(|(k, v)| k != FREQ_OTHER_KEY && *v > 0)
        .collect();

    if entries.len() <= FREQ_TOP_K {
        let mut out: BTreeMap<String, u64> = entries.into_iter().collect();
        if other_acc > 0 {
            out.insert(String::from(FREQ_OTHER_KEY), other_acc);
        }
        return out;
    }

    entries.sort_by(|(ka, va), (kb, vb)| vb.cmp(va).then_with(|| ka.cmp(kb)));
    let mut out = BTreeMap::new();
    for (idx, (key, val)) in entries.into_iter().enumerate() {
        if idx < FREQ_TOP_K {
            out.insert(key, val);
        } else {
            other_acc = other_acc.saturating_add(val);
        }
    }
    if other_acc > 0 {
        out.insert(String::from(FREQ_OTHER_KEY), other_acc);
    }
    out
}

/// Builds a top-100 categorical frequency map from string observations.
#[must_use]
pub fn build_freq_map<S: AsRef<str>>(values: &[S]) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for val in values {
        let entry = counts.entry(String::from(val.as_ref())).or_insert(0);
        *entry = entry.saturating_add(1);
    }
    truncate_freq_map(counts)
}

/// Merges two categorical frequency maps and truncates the result to top [`FREQ_TOP_K`] + [`FREQ_OTHER_KEY`].
#[must_use]
pub fn merge_freq_maps(
    a: &BTreeMap<String, u64>,
    b: &BTreeMap<String, u64>,
) -> BTreeMap<String, u64> {
    let mut combined = a.clone();
    for (k, &v) in b {
        let entry = combined.entry(k.clone()).or_insert(0);
        *entry = entry.saturating_add(v);
    }
    truncate_freq_map(combined)
}

/// Merges two lower-grain [`RollupRow`] buckets into a single target-grain bucket (§03.3.2 item 2).
///
/// # Errors
/// Returns [`AppError::BadRequest`] if HLL sketch merging fails.
pub fn merge_rollup_rows(
    a: &RollupRow,
    b: &RollupRow,
    target_grain: RollupGrain,
    bucket_start: DateTime<Utc>,
) -> Result<RollupRow, AppError> {
    let cnt = a.cnt.saturating_add(b.cnt);
    let sum_v = match (a.sum_v, b.sum_v) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    };
    let min_v = match (a.min_v, b.min_v) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    };
    let max_v = match (a.max_v, b.max_v) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    };
    let (last_v, last_text, last_ts) = if b.last_ts >= a.last_ts {
        (
            b.last_v.or(a.last_v),
            b.last_text.clone().or_else(|| a.last_text.clone()),
            b.last_ts,
        )
    } else {
        (
            a.last_v.or(b.last_v),
            a.last_text.clone().or_else(|| b.last_text.clone()),
            a.last_ts,
        )
    };
    let hll = match (&a.hll, &b.hll) {
        (Some(ha), Some(hb)) => Some(merge_hll(ha, hb)?),
        (Some(ha), None) => Some(ha.clone()),
        (None, Some(hb)) => Some(hb.clone()),
        (None, None) => None,
    };
    let freq = match (&a.freq, &b.freq) {
        (Some(fa), Some(fb)) => Some(merge_freq_maps(fa, fb)),
        (Some(fa), None) => Some(fa.clone()),
        (None, Some(fb)) => Some(fb.clone()),
        (None, None) => None,
    };

    Ok(RollupRow {
        node_id: a.node_id,
        metric: a.metric.clone(),
        grain: target_grain,
        bucket_start,
        cnt,
        sum_v,
        min_v,
        max_v,
        last_v,
        last_text,
        last_ts,
        hll,
        freq,
    })
}

/// Normalizes an entity or business node name according to Step 03 §03.5.1:
/// 1. Unicode NFKC normalization (converts full-width characters to half-width).
/// 2. Trim leading/trailing whitespace and collapse consecutive internal whitespace to `' '`.
/// 3. Lowercase conversion.
/// 4. Strip trailing organization suffixes from `normalize_suffixes.txt`.
#[must_use]
pub fn normalize_name(raw: &str) -> String {
    let nfkc: String = raw.nfkc().collect();
    let collapsed = nfkc.split_whitespace().collect::<Vec<&str>>().join(" ");
    let mut lowered = collapsed.to_lowercase();

    // Repeatedly strip trailing punctuation/whitespace + longest matching suffix, provided stem remains non-empty.
    loop {
        let trimmed = lowered
            .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '·'))
            .to_owned();
        let mut stripped_any = false;
        for suffix in ORG_SUFFIXES.iter() {
            if let Some(stem) = trimmed.strip_suffix(suffix.as_str()) {
                let clean_stem =
                    stem.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ',' | '，'));
                if !clean_stem.is_empty() {
                    clean_stem.clone_into(&mut lowered);
                    stripped_any = true;
                    break;
                }
            }
        }
        if !stripped_any {
            return trimmed;
        }
    }
}

/// Builds deterministic embeddable text for a `BlueNode` (§03.4, truncated to 2,000 characters).
#[must_use]
pub fn build_node_embed_text(type_display_name: &str, node: &BlueNode) -> String {
    let mut text = format!(
        "{type_display_name}：{}（{}）",
        node.title.trim(),
        node.node_key.trim()
    );
    for (k, v) in &node.properties {
        if !k.starts_with("__")
            && let Some(s) = v.as_str()
            && !s.trim().is_empty()
        {
            text.push(' ');
            text.push_str(k);
            text.push_str(": ");
            text.push_str(s.trim());
        }
    }
    text.chars().take(2_000).collect()
}

/// Computes a deterministic SHA-256 hex `params_hash` for graph algorithm caching (`xb_graph_algo_results`).
#[must_use]
pub fn compute_params_hash(
    algo: GraphAlgo,
    type_names: &[String],
    link_types: &[String],
    params: &serde_json::Value,
) -> String {
    let mut sorted_types = type_names.to_vec();
    sorted_types.sort();
    sorted_types.dedup();

    let mut sorted_links = link_types.to_vec();
    sorted_links.sort();
    sorted_links.dedup();

    let canonical = serde_json::json!({
        "algo": algo.as_str(),
        "type_names": sorted_types,
        "link_types": sorted_links,
        "params": canonicalize_json(params),
    });
    let encoded = serde_json::to_vec(&canonical).unwrap_or_default();
    let digest = Sha256::digest(&encoded);
    hex::encode(digest)
}

fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted_keys: Vec<&String> = map.keys().collect();
            sorted_keys.sort();
            let mut out = serde_json::Map::new();
            for k in sorted_keys {
                if let Some(v) = map.get(k) {
                    out.insert(k.clone(), canonicalize_json(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize_json).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hll_merge_100k_distinct_within_two_percent() -> Result<(), AppError> {
        let batch_a: Vec<String> = (0..60_000).map(|i| format!("val_{i}")).collect();
        // Overlap 20,000 items (40,000..60,000) + 40,000 new items (60,000..100,000) => 100,000 unique total.
        let batch_b: Vec<String> = (40_000..100_000).map(|i| format!("val_{i}")).collect();

        let bytes_a = build_hll_bytes(&batch_a)?;
        let bytes_b = build_hll_bytes(&batch_b)?;
        let merged = merge_hll(&bytes_a, &bytes_b)?;
        let est = estimate_hll(&merged)?;

        let rel_err = ((est - 100_000.0) / 100_000.0).abs();
        assert!(
            rel_err < 0.02,
            "expected HLL estimate within 2% of 100,000, got {est} (rel_err = {rel_err})"
        );
        Ok(())
    }

    #[test]
    fn freq_map_truncates_to_top_100_and_folds_other() {
        let mut map_a = BTreeMap::new();
        for i in 0..80_u64 {
            map_a.insert(format!("k_{i:03}"), 200_u64.saturating_sub(i));
        }
        let mut map_b = BTreeMap::new();
        for i in 60..130_u64 {
            map_b.insert(format!("k_{i:03}"), 50);
        }

        let merged = merge_freq_maps(&map_a, &map_b);
        // 130 distinct keys -> top 100 retained + "__other__" = 101 entries.
        assert_eq!(merged.len(), 101);
        assert!(merged.contains_key(FREQ_OTHER_KEY));

        let total_before: u64 =
            map_a.values().copied().sum::<u64>() + map_b.values().copied().sum::<u64>();
        let total_after: u64 = merged.values().copied().sum();
        assert_eq!(total_before, total_after);
    }

    #[test]
    fn normalize_name_handles_nfkc_whitespace_and_org_suffixes() {
        assert_eq!(
            normalize_name("　上海华东晶圆制造股份有限公司　"),
            "上海华东晶圆制造"
        );
        assert_eq!(
            normalize_name("Ａｃｍｅ   Semiconductor  Co., Ltd."),
            "acme semiconductor"
        );
        assert_eq!(normalize_name("华东物流集团有限公司"), "华东物流");
        // Pure suffix name should not collapse to empty string.
        assert_eq!(normalize_name("有限公司"), "有限公司");
    }

    #[test]
    fn merge_rollup_rows_combines_aggregates_accurately() -> Result<(), AppError> {
        let node_id = Uuid::new_v4();
        let t0 = DateTime::from_timestamp(1_769_817_600, 0).unwrap_or_else(Utc::now);
        let t1 = t0 + Duration::minutes(1);
        let hll_a = build_hll_bytes(&["OK", "WARN"])?;
        let hll_b = build_hll_bytes(&["WARN", "ERR"])?;

        let row_a = RollupRow {
            node_id,
            metric: String::from("status"),
            grain: RollupGrain::Minute,
            bucket_start: t0,
            cnt: 2,
            sum_v: Some(30.0),
            min_v: Some(10.0),
            max_v: Some(20.0),
            last_v: Some(20.0),
            last_text: Some(String::from("WARN")),
            last_ts: t0,
            hll: Some(hll_a),
            freq: Some(build_freq_map(&["OK", "WARN"])),
        };
        let row_b = RollupRow {
            node_id,
            metric: String::from("status"),
            grain: RollupGrain::Minute,
            bucket_start: t1,
            cnt: 3,
            sum_v: Some(45.0),
            min_v: Some(5.0),
            max_v: Some(25.0),
            last_v: Some(25.0),
            last_text: Some(String::from("ERR")),
            last_ts: t1,
            hll: Some(hll_b),
            freq: Some(build_freq_map(&["WARN", "ERR", "ERR"])),
        };

        let hour_row = merge_rollup_rows(&row_a, &row_b, RollupGrain::Hour, t0)?;
        assert_eq!(hour_row.cnt, 5);
        assert_eq!(hour_row.sum_v, Some(75.0));
        assert_eq!(hour_row.min_v, Some(5.0));
        assert_eq!(hour_row.max_v, Some(25.0));
        assert_eq!(hour_row.last_v, Some(25.0));
        assert_eq!(hour_row.last_text.as_deref(), Some("ERR"));
        let distinct = estimate_hll(hour_row.hll.as_deref().unwrap_or_default())?;
        assert!((distinct - 3.0).abs() < 0.5);
        Ok(())
    }
}
