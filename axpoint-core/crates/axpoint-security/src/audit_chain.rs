//! Tamper-evident `SHA-256` hash-chain computation and verification for `sec_audit_logs`
//! (`11-STEP13` §13.9.2).

use axpoint_domain::guard::{AuditLogEntry, AuditVerifyResponse};
use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

/// Borrowed audit log row fields (excluding `prev_hash` and `row_hash`) serialized into canonical
/// JSON for `SHA-256` chain hashing (`§13.9.2`).
#[derive(Debug, Clone)]
pub struct AuditHashFields<'a> {
    /// Monotonic `sec_audit_logs.id`.
    pub id: i64,
    /// Tenant identifier.
    pub tenant_id: &'a str,
    /// Optional authenticated user UUID.
    pub user_id: Option<Uuid>,
    /// Username or anonymous marker.
    pub username: &'a str,
    /// Canonical `AuditAction` string.
    pub action: &'a str,
    /// Target resource reference.
    pub resource_ref: &'a str,
    /// HTTP status code.
    pub status_code: i32,
    /// Client IP string.
    pub client_ip: &'a str,
    /// Distributed trace / request identifier.
    pub trace_id: &'a str,
    /// Structured audit detail JSON payload.
    pub detail_json: &'a serde_json::Value,
    /// Event timestamp (normalized to microsecond resolution matching `PostgreSQL` `TIMESTAMPTZ`).
    pub created_at: DateTime<Utc>,
}

/// Truncates `ts` to whole microseconds so `Rust` nanosecond `DateTime<Utc>` values and
/// round-tripped `PostgreSQL` `TIMESTAMPTZ` values hash identically (`§13.9.2`).
#[must_use]
pub fn truncate_to_micros(ts: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(ts.timestamp_micros()).unwrap_or(ts)
}

/// Computes `row_hash = hex(SHA-256(prev_hash || canonical_json(fields)))` (`§13.9.2`).
#[must_use]
pub fn compute_audit_row_hash(prev_hash: &str, fields: &AuditHashFields<'_>) -> String {
    let micros_ts = truncate_to_micros(fields.created_at);
    let ts_rfc3339 = micros_ts.to_rfc3339_opts(SecondsFormat::Micros, true);
    let canonical_detail = canonicalize_json(fields.detail_json);

    // Keys in `serde_json::Map` (backed by `BTreeMap`) serialize in deterministic lexicographical order.
    let mut obj = serde_json::Map::new();
    obj.insert(
        String::from("action"),
        serde_json::Value::String(String::from(fields.action)),
    );
    obj.insert(
        String::from("client_ip"),
        serde_json::Value::String(String::from(fields.client_ip)),
    );
    obj.insert(
        String::from("created_at"),
        serde_json::Value::String(ts_rfc3339),
    );
    obj.insert(String::from("detail_json"), canonical_detail);
    obj.insert(
        String::from("id"),
        serde_json::Value::Number(serde_json::Number::from(fields.id)),
    );
    obj.insert(
        String::from("resource_ref"),
        serde_json::Value::String(String::from(fields.resource_ref)),
    );
    obj.insert(
        String::from("status_code"),
        serde_json::Value::Number(serde_json::Number::from(fields.status_code)),
    );
    obj.insert(
        String::from("tenant_id"),
        serde_json::Value::String(String::from(fields.tenant_id)),
    );
    obj.insert(
        String::from("trace_id"),
        serde_json::Value::String(String::from(fields.trace_id)),
    );
    obj.insert(
        String::from("user_id"),
        match fields.user_id {
            Some(uid) => serde_json::Value::String(uid.to_string()),
            None => serde_json::Value::Null,
        },
    );
    obj.insert(
        String::from("username"),
        serde_json::Value::String(String::from(fields.username)),
    );

    let canonical_json = serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(canonical_json.as_bytes());
    hex::encode(hasher.finalize())
}

/// Verifies an ascending sequence of [`AuditLogEntry`] rows starting from `expected_prev_hash`
/// (`""` at genesis or the preceding row's `row_hash`), reporting the first broken `id` if any
/// (`GET /api/axguard/audit/verify`, `§13.9.2`).
#[must_use]
pub fn verify_audit_chain(
    expected_prev_hash: &str,
    entries: &[AuditLogEntry],
) -> AuditVerifyResponse {
    let mut expected_prev = expected_prev_hash;

    for (idx, entry) in entries.iter().enumerate() {
        let prev_matches = constant_time_str_eq(&entry.prev_hash, expected_prev);
        let recomputed = compute_audit_row_hash(
            &entry.prev_hash,
            &AuditHashFields {
                id: entry.id,
                tenant_id: &entry.tenant_id,
                user_id: entry.user_id,
                username: &entry.username,
                action: entry.action.as_str(),
                resource_ref: &entry.resource_ref,
                status_code: entry.status_code,
                client_ip: &entry.client_ip,
                trace_id: &entry.trace_id,
                detail_json: &entry.detail_json,
                created_at: entry.created_at,
            },
        );
        let row_matches =
            !entry.row_hash.is_empty() && constant_time_str_eq(&entry.row_hash, &recomputed);

        if !prev_matches || !row_matches {
            return AuditVerifyResponse {
                ok: false,
                checked_count: u64::try_from(idx.saturating_add(1)).unwrap_or(u64::MAX),
                first_broken_id: Some(entry.id),
            };
        }
        expected_prev = &entry.row_hash;
    }

    AuditVerifyResponse {
        ok: true,
        checked_count: u64::try_from(entries.len()).unwrap_or(u64::MAX),
        first_broken_id: None,
    }
}

fn canonicalize_json(val: &serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::Object(map) => {
            let mut sorted = serde_json::Map::with_capacity(map.len());
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                if let Some(v) = map.get(k) {
                    sorted.insert(k.clone(), canonicalize_json(v));
                }
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize_json).collect())
        }
        other => other.clone(),
    }
}

fn constant_time_str_eq(a: &str, b: &str) -> bool {
    let da = Sha256::digest(a.as_bytes());
    let db = Sha256::digest(b.as_bytes());
    bool::from(da.as_slice().ct_eq(db.as_slice()))
}
