//! `moka`-backed `xBlueView` widget execution cache with principal-scoped keys and event invalidation (`10-STEP11-12` §11.2.3).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use axpoint_domain::enums::Permission;
use axpoint_domain::view::{DrillPathItem, WidgetDataResponse};
use axpoint_security::AuthPrincipal;
use moka::future::Cache;
use sha2::{Digest as _, Sha256};
use tracing::warn;
use uuid::Uuid;

/// Maximum cached widget execution responses across all tenants (`10-STEP11-12` §11.2.3).
pub const VIEW_CACHE_MAX_CAPACITY: u64 = 10_000;

/// Time-to-live in seconds for cached widget responses (`10-STEP11-12` §11.2.3).
pub const VIEW_CACHE_TTL_SECS: u64 = 600;

/// Composite cache key isolating responses by tenant, dashboard, widget, filters, drill stack, and security scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewCacheKey {
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Optional owning dashboard UUID (populated for persisted dashboard widgets).
    pub dashboard_id: Option<Uuid>,
    /// Target widget UUID.
    pub widget_id: Uuid,
    /// Deterministic SHA-256 digest of `(filters, drill_path, principal_fingerprint)`.
    pub scope_hash: String,
}

/// Thread-safe `moka::future::Cache` storing [`WidgetDataResponse`] payloads for `xBlueView`.
#[derive(Clone)]
pub struct ViewCache {
    inner: Cache<ViewCacheKey, WidgetDataResponse>,
}

impl std::fmt::Debug for ViewCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewCache")
            .field("entry_count", &self.inner.entry_count())
            .finish_non_exhaustive()
    }
}

impl Default for ViewCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewCache {
    /// Creates a [`ViewCache`] with capacity `10_000` and `600s` TTL.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity_and_ttl(
            VIEW_CACHE_MAX_CAPACITY,
            Duration::from_secs(VIEW_CACHE_TTL_SECS),
        )
    }

    /// Creates a [`ViewCache`] with explicit capacity and TTL (useful for deterministic tests).
    #[must_use]
    pub fn with_capacity_and_ttl(max_capacity: u64, ttl: Duration) -> Self {
        let inner = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(ttl)
            .support_invalidation_closures()
            .build();
        Self { inner }
    }

    /// Builds a [`ViewCacheKey`] hashing `(filters, drill_path, principal_fingerprint)`.
    #[must_use]
    pub fn build_key(
        &self,
        tenant_id: &str,
        dashboard_id: Option<Uuid>,
        widget_id: Uuid,
        filters: &BTreeMap<String, serde_json::Value>,
        drill_path: &[DrillPathItem],
        principal_fingerprint: &str,
    ) -> ViewCacheKey {
        ViewCacheKey {
            tenant_id: tenant_id.to_owned(),
            dashboard_id,
            widget_id,
            scope_hash: compute_scope_hash(filters, drill_path, principal_fingerprint),
        }
    }

    /// Looks up a cached [`WidgetDataResponse`] by key.
    pub async fn get(&self, key: &ViewCacheKey) -> Option<WidgetDataResponse> {
        self.inner.get(key).await
    }

    /// Inserts or replaces a [`WidgetDataResponse`] entry.
    pub async fn insert(&self, key: ViewCacheKey, value: WidgetDataResponse) {
        self.inner.insert(key, value).await;
    }

    /// Invalidates all cached entries belonging to `(tenant_id, widget_ids)` and flushes pending invalidations.
    pub async fn invalidate_widgets(&self, tenant_id: &str, widget_ids: &[Uuid]) {
        if widget_ids.is_empty() {
            return;
        }
        let tenant = tenant_id.to_owned();
        let id_set: HashSet<Uuid> = widget_ids.iter().copied().collect();
        if let Err(err) = self.inner.invalidate_entries_if(move |k, _v| {
            k.tenant_id == tenant && id_set.contains(&k.widget_id)
        }) {
            warn!(tenant_id = %tenant_id, error = %err, "ViewCache widget invalidation failed");
        }
        self.inner.run_pending_tasks().await;
    }

    /// Invalidates all cached entries belonging to `(tenant_id, dashboard_ids)` and flushes pending invalidations.
    pub async fn invalidate_dashboards(&self, tenant_id: &str, dashboard_ids: &[Uuid]) {
        if dashboard_ids.is_empty() {
            return;
        }
        let tenant = tenant_id.to_owned();
        let id_set: HashSet<Uuid> = dashboard_ids.iter().copied().collect();
        if let Err(err) = self.inner.invalidate_entries_if(move |k, _v| {
            k.tenant_id == tenant && k.dashboard_id.is_some_and(|did| id_set.contains(&did))
        }) {
            warn!(tenant_id = %tenant_id, error = %err, "ViewCache dashboard invalidation failed");
        }
        self.inner.run_pending_tasks().await;
    }

    /// Invalidates all cached entries for `tenant_id`.
    pub fn invalidate_tenant(&self, tenant_id: &str) {
        let tenant = tenant_id.to_owned();
        if let Err(err) = self
            .inner
            .invalidate_entries_if(move |k, _v| k.tenant_id == tenant)
        {
            warn!(tenant_id = %tenant_id, error = %err, "ViewCache tenant invalidation failed");
        }
    }

    /// Flushes pending `moka` maintenance tasks (evictions and invalidations).
    pub async fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks().await;
    }

    /// Returns the approximate number of entries currently stored in the cache.
    #[must_use]
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }
}

/// Computes a deterministic SHA-256 fingerprint from sorted roles, department, clearance, and permissions.
#[must_use]
pub fn compute_principal_fingerprint(
    roles: &[String],
    dept_id: &str,
    clearance: Option<&str>,
    permissions: &BTreeSet<Permission>,
) -> String {
    let mut sorted_roles: Vec<&str> = roles.iter().map(String::as_str).collect();
    sorted_roles.sort_unstable();
    sorted_roles.dedup();

    let mut perm_strs: Vec<&str> = permissions.iter().map(|p| p.as_str()).collect();
    perm_strs.sort_unstable();

    let mut hasher = Sha256::new();
    hasher.update(b"roles:");
    for role in sorted_roles {
        hasher.update(role.as_bytes());
        hasher.update(b"\x1f");
    }
    hasher.update(b"|dept:");
    hasher.update(dept_id.as_bytes());
    hasher.update(b"|clearance:");
    hasher.update(clearance.unwrap_or_default().as_bytes());
    hasher.update(b"|perms:");
    for perm in perm_strs {
        hasher.update(perm.as_bytes());
        hasher.update(b"\x1f");
    }
    hex::encode(hasher.finalize())
}

/// Computes the security scope fingerprint for an authenticated [`AuthPrincipal`].
#[must_use]
pub fn principal_fingerprint_from_auth(principal: &AuthPrincipal) -> String {
    compute_principal_fingerprint(
        &principal.roles,
        &principal.dept_id,
        None,
        &principal.permissions,
    )
}

/// Computes the deterministic SHA-256 digest of `(filters, drill_path, principal_fingerprint)`.
#[must_use]
pub fn compute_scope_hash(
    filters: &BTreeMap<String, serde_json::Value>,
    drill_path: &[DrillPathItem],
    principal_fingerprint: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"filters:");
    for (k, v) in filters {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(canonical_json_string(v).as_bytes());
        hasher.update(b"\x1e");
    }
    hasher.update(b"|drill:");
    for item in drill_path {
        hasher.update(item.field.as_bytes());
        hasher.update(b"=");
        hasher.update(canonical_json_string(&item.value).as_bytes());
        hasher.update(b"\x1e");
    }
    hasher.update(b"|principal:");
    hasher.update(principal_fingerprint.as_bytes());
    hex::encode(hasher.finalize())
}

fn canonical_json_string(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Null => String::from("null"),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(canonical_json_string).collect();
            format!("[{}]", items.join(","))
        }
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(&str, String)> = map
                .iter()
                .map(|(k, v)| (k.as_str(), canonical_json_string(v)))
                .collect();
            entries.sort_by_key(|(a, _)| *a);
            let parts: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{}:{v}", serde_json::to_string(k).unwrap_or_default()))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use axpoint_domain::enums::DataType;
    use axpoint_domain::view::WidgetColumn;
    use chrono::Utc;

    use super::*;

    fn sample_response(tag: &str) -> WidgetDataResponse {
        WidgetDataResponse {
            columns: vec![WidgetColumn {
                name: String::from("v"),
                data_type: DataType::String,
            }],
            rows: vec![vec![serde_json::Value::String(tag.to_owned())]],
            row_count: 1,
            truncated: false,
            generated_at: Utc::now(),
            data_version: tag.to_owned(),
            drill_level: 0,
        }
    }

    #[test]
    fn principal_fingerprint_is_order_independent_for_roles_and_sensitive_to_scope() {
        let perms = BTreeSet::from([Permission::DataRead]);
        let fp1 = compute_principal_fingerprint(
            &[String::from("ANALYST"), String::from("OPERATOR")],
            "east",
            Some("INTERNAL"),
            &perms,
        );
        let fp2 = compute_principal_fingerprint(
            &[String::from("OPERATOR"), String::from("ANALYST")],
            "east",
            Some("INTERNAL"),
            &perms,
        );
        assert_eq!(fp1, fp2);

        let diff_dept = compute_principal_fingerprint(
            &[String::from("ANALYST"), String::from("OPERATOR")],
            "south",
            Some("INTERNAL"),
            &perms,
        );
        assert_ne!(fp1, diff_dept);

        let diff_clearance = compute_principal_fingerprint(
            &[String::from("ANALYST"), String::from("OPERATOR")],
            "east",
            Some("CONFIDENTIAL"),
            &perms,
        );
        assert_ne!(fp1, diff_clearance);
    }

    #[tokio::test]
    async fn cache_isolates_security_scopes_and_invalidates_target_widgets() {
        let cache = ViewCache::new();
        let d1 = Uuid::from_u128(10);
        let d2 = Uuid::from_u128(20);
        let w1 = Uuid::from_u128(1);
        let w2 = Uuid::from_u128(2);
        let w3 = Uuid::from_u128(3);
        let filters = BTreeMap::from([(String::from("status"), serde_json::json!(["PAID"]))]);
        let drill = vec![DrillPathItem {
            field: String::from("status"),
            value: serde_json::json!("PAID"),
        }];

        let fp_east = compute_principal_fingerprint(
            &[String::from("ANALYST")],
            "east",
            Some("INTERNAL"),
            &BTreeSet::from([Permission::DataRead]),
        );
        let fp_south = compute_principal_fingerprint(
            &[String::from("ANALYST")],
            "south",
            Some("INTERNAL"),
            &BTreeSet::from([Permission::DataRead]),
        );

        let k1_east = cache.build_key("t1", Some(d1), w1, &filters, &drill, &fp_east);
        let k1_south = cache.build_key("t1", Some(d1), w1, &filters, &drill, &fp_south);
        let k2_east = cache.build_key("t1", Some(d1), w2, &filters, &drill, &fp_east);
        let k3_other_dash = cache.build_key("t1", Some(d2), w3, &filters, &drill, &fp_east);
        let k1_other_tenant = cache.build_key("t2", Some(d1), w1, &filters, &drill, &fp_east);

        cache
            .insert(k1_east.clone(), sample_response("east-w1"))
            .await;
        cache
            .insert(k1_south.clone(), sample_response("south-w1"))
            .await;
        cache
            .insert(k2_east.clone(), sample_response("east-w2"))
            .await;
        cache
            .insert(k3_other_dash.clone(), sample_response("east-w3"))
            .await;
        cache
            .insert(k1_other_tenant.clone(), sample_response("t2-w1"))
            .await;

        assert_eq!(
            cache.get(&k1_east).await.map(|r| r.data_version),
            Some(String::from("east-w1"))
        );
        assert_eq!(
            cache.get(&k1_south).await.map(|r| r.data_version),
            Some(String::from("south-w1"))
        );

        cache.invalidate_widgets("t1", &[w1]).await;

        assert!(cache.get(&k1_east).await.is_none());
        assert!(cache.get(&k1_south).await.is_none());
        assert!(cache.get(&k2_east).await.is_some());
        assert!(cache.get(&k3_other_dash).await.is_some());
        assert!(cache.get(&k1_other_tenant).await.is_some());

        cache.invalidate_dashboards("t1", &[d1]).await;
        assert!(cache.get(&k2_east).await.is_none());
        assert!(cache.get(&k3_other_dash).await.is_some());
        assert!(cache.get(&k1_other_tenant).await.is_some());
    }
}
