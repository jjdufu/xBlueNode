//! Per-tenant hardened `DuckDB` in-memory `SessionPool` and query execution engine (`05-STEP04` §04.2).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use arrow::array::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};
use axpoint_domain::AppError;
use axpoint_domain::analytics::{ExplainResult, QueryRequest, QueryResult};
use axpoint_domain::axhub::{Connection, SealedCredential};
use axpoint_domain::enums::ConnectorType;
use axpoint_domain::ports::LakeInternalQueryPort;
use axpoint_security::CredentialKeyring;
use lru::LruCache;
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::AnalyticsConfig;
use crate::result::{ResultSerializeOptions, SerializedQueryOutput, batches_to_query_result};
use crate::sql_guard::{GuardOptions, GuardedSql, TenantCatalog, guard};

/// Default interactive SQL execution timeout in milliseconds (`30s`).
pub const DEFAULT_QUERY_TIMEOUT_MS: u64 = 30_000;

/// Hard upper bound on interactive SQL execution timeout in milliseconds (`120s`).
pub const MAX_QUERY_TIMEOUT_MS: u64 = 120_000;

/// Live-query external relational connection descriptor mounted as `live_<name>` (`05-STEP04` §04.2).
#[derive(Debug, Clone)]
pub struct LiveConnectionSpec {
    /// Connection UUID.
    pub id: Uuid,
    /// Connection alias stem (mounted as `live_<name>`).
    pub name: String,
    /// Connector type (`POSTGRES`, `MYSQL`, or `SQLITE`).
    pub connector_type: ConnectorType,
    /// Plain connection configuration map.
    pub config: serde_json::Map<String, serde_json::Value>,
    /// Optional AES-256-GCM sealed secret map (`ds_credentials`).
    pub credential: Option<SealedCredential>,
}

impl LiveConnectionSpec {
    /// Filters `AxHub` `(Connection, Option<SealedCredential>)` pairs down to `live_query = true`
    /// relational sources supported by `DuckDB` scanner extensions (`POSTGRES`, `MYSQL`, `SQLITE`).
    #[must_use]
    pub fn from_connections(connections: &[(Connection, Option<SealedCredential>)]) -> Vec<Self> {
        let mut out = Vec::new();
        for (conn, cred) in connections {
            let is_live = conn
                .config
                .get("live_query")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !is_live {
                continue;
            }
            if matches!(
                conn.connector_type,
                ConnectorType::Postgres | ConnectorType::Mysql | ConnectorType::Sqlite
            ) {
                out.push(Self {
                    id: conn.id,
                    name: conn.name.clone(),
                    connector_type: conn.connector_type,
                    config: conn.config.clone(),
                    credential: cred.clone(),
                });
            }
        }
        out
    }
}

/// Hardened per-tenant in-memory `DuckDB` session (`05-STEP04` §04.2).
pub struct TenantDb {
    conn: Arc<StdMutex<duckdb::Connection>>,
    interrupt_handle: Arc<duckdb::InterruptHandle>,
    exec_gate: AsyncMutex<()>,
    tenant_id: String,
    built_at: Instant,
    attached_live: HashSet<String>,
    catalog: StdRwLock<TenantCatalog>,
}

/// Alias for [`TenantDb`] used across `AxAnalytics` pool consumers.
pub type TenantSession = TenantDb;

impl std::fmt::Debug for TenantDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantDb")
            .field("tenant_id", &self.tenant_id)
            .field("built_at", &self.built_at)
            .field("attached_live", &self.attached_live)
            .finish_non_exhaustive()
    }
}

impl TenantDb {
    /// Synchronously builds and locks down a hardened in-memory `DuckDB` instance for `tenant_id`.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] if `tenant_id` is invalid, or [`AppError::Config`] /
    /// [`AppError::Database`] if `DuckDB`, `DuckLake`, or platform `PostgreSQL` attachment fails.
    pub fn build(
        tenant_id: &str,
        cfg: &AnalyticsConfig,
        mut catalog: TenantCatalog,
        live_sources: &[LiveConnectionSpec],
    ) -> Result<Self, AppError> {
        validate_tenant_ident(tenant_id)?;
        catalog.tenant_id = String::from(tenant_id);

        let conn = duckdb::Connection::open_in_memory()
            .map_err(|err| AppError::Config(format!("duckdb open_in_memory failed: {err}")))?;
        conn.execute_batch("SET autoinstall_known_extensions = false;")
            .map_err(|err| AppError::Config(format!("duckdb disable autoinstall failed: {err}")))?;

        let mut allowed_dirs = BTreeSet::new();
        attach_ducklake(&conn, cfg, &mut allowed_dirs)?;

        if let Some(app_db_url) = cfg
            .app_database_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            attach_platform_postgres(&conn, tenant_id, app_db_url)?;
        }

        let attached_live = attach_live_sources(
            &conn,
            tenant_id,
            live_sources,
            cfg.keyring.as_ref(),
            &mut allowed_dirs,
        );
        for live_name in &attached_live {
            catalog.insert_live_connection(live_name);
        }

        lock_duckdb_security(&conn, cfg, &allowed_dirs)?;

        let interrupt_handle = conn.interrupt_handle();
        Ok(Self {
            conn: Arc::new(StdMutex::new(conn)),
            interrupt_handle,
            exec_gate: AsyncMutex::new(()),
            tenant_id: String::from(tenant_id),
            built_at: Instant::now(),
            attached_live,
            catalog: StdRwLock::new(catalog),
        })
    }

    /// Builds a [`TenantDb`] on the blocking threadpool so extension loading and attachment I/O
    /// never block Tokio async worker threads.
    ///
    /// # Errors
    /// Propagates any error from [`TenantDb::build`] or blocking task join failure.
    pub async fn build_async(
        tenant_id: &str,
        cfg: AnalyticsConfig,
        catalog: TenantCatalog,
        live_sources: Vec<LiveConnectionSpec>,
    ) -> Result<Arc<Self>, AppError> {
        let tid = String::from(tenant_id);
        let built =
            tokio::task::spawn_blocking(move || Self::build(&tid, &cfg, catalog, &live_sources))
                .await
                .map_err(|err| {
                    AppError::Database(format!("duckdb session build join failed: {err}"))
                })??;
        Ok(Arc::new(built))
    }

    /// Wraps an already-opened [`duckdb::Connection`] into an isolated [`TenantDb`] handle
    /// (intended for pure in-memory `DuckDB` unit tests without `DuckLake` filesystem bootstrap).
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `tenant_id` is invalid.
    pub fn from_connection(tenant_id: &str, conn: duckdb::Connection) -> Result<Self, AppError> {
        validate_tenant_ident(tenant_id)?;
        let interrupt_handle = conn.interrupt_handle();
        Ok(Self {
            conn: Arc::new(StdMutex::new(conn)),
            interrupt_handle,
            exec_gate: AsyncMutex::new(()),
            tenant_id: String::from(tenant_id),
            built_at: Instant::now(),
            attached_live: HashSet::new(),
            catalog: StdRwLock::new(TenantCatalog::new(tenant_id)),
        })
    }

    /// Returns the owning `tenant_id`.
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Returns the `Instant` when this session was constructed.
    #[must_use]
    pub fn built_at(&self) -> Instant {
        self.built_at
    }

    /// Returns a snapshot clone of the cached [`TenantCatalog`].
    #[must_use]
    pub fn catalog(&self) -> TenantCatalog {
        match self.catalog.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Replaces the cached [`TenantCatalog`] in-place (e.g. upon `dataset.synced`), preserving
    /// live connection aliases attached when this `TenantDb` was constructed.
    pub fn refresh_catalog(&self, mut next_catalog: TenantCatalog) {
        next_catalog.tenant_id.clone_from(&self.tenant_id);
        for live_name in &self.attached_live {
            next_catalog.insert_live_connection(live_name);
        }
        match self.catalog.write() {
            Ok(mut guard) => *guard = next_catalog,
            Err(poisoned) => *poisoned.into_inner() = next_catalog,
        }
    }

    /// Guards and executes a [`QueryRequest`], enforcing timeout interruption, row truncation,
    /// and dynamic PII masking.
    ///
    /// # Errors
    /// Returns `PAYLOAD_TOO_LARGE`, `PERMISSION_DENIED`, `SQL_REJECTED`, `QUERY_TIMEOUT`, or `Database`.
    pub async fn execute_sql(
        &self,
        req: &QueryRequest,
        has_live_permission: bool,
        has_pii_raw_read: bool,
    ) -> Result<SerializedQueryOutput, AppError> {
        if req.reveal_pii && !has_pii_raw_read {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        let mut cat = self.catalog();
        cat.has_live_permission = has_live_permission;

        let guard_opts = GuardOptions {
            max_rows: req.max_rows.unwrap_or(0),
            allow_live: req.allow_live,
            params: req.params.clone(),
        };
        let guarded = guard(&req.sql, &cat, &guard_opts)?;
        self.execute_guarded(
            &guarded,
            &cat,
            req.timeout_ms,
            req.reveal_pii,
            has_pii_raw_read,
        )
        .await
    }

    /// Executes an already-guarded [`GuardedSql`] against this tenant's `DuckDB` connection.
    ///
    /// # Errors
    /// Returns `PERMISSION_DENIED`, `QUERY_TIMEOUT` (`504`), `SQL_REJECTED`, or `Database`.
    pub async fn execute_guarded(
        &self,
        guarded: &GuardedSql,
        catalog: &TenantCatalog,
        timeout_ms: Option<u64>,
        reveal_pii: bool,
        has_pii_raw_read: bool,
    ) -> Result<SerializedQueryOutput, AppError> {
        if reveal_pii && !has_pii_raw_read {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        let started = Instant::now();
        let budget = resolve_timeout_duration(timeout_ms);

        let _gate = tokio::time::timeout(budget, self.exec_gate.lock())
            .await
            .map_err(|_| AppError::Timeout("QUERY_TIMEOUT"))?;

        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(AppError::Timeout("QUERY_TIMEOUT"));
        }

        let sql_to_run = guarded.sql.clone();
        let (schema, batches) = self.run_arrow_with_timeout(sql_to_run, remaining).await?;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        batches_to_query_result(
            &schema,
            &batches,
            ResultSerializeOptions {
                effective_max_rows: guarded.effective_max_rows,
                sql: guarded.sql.clone(),
                duration_ms,
                reveal_pii,
                has_pii_raw_read,
                catalog,
                guarded,
            },
        )
    }

    /// Guards `req.sql` and executes `EXPLAIN <guarded_sql>`, returning [`ExplainResult`].
    ///
    /// # Errors
    /// Returns `PAYLOAD_TOO_LARGE`, `PERMISSION_DENIED`, `SQL_REJECTED`, `QUERY_TIMEOUT`, or `Database`.
    pub async fn explain_sql(
        &self,
        req: &QueryRequest,
        has_live_permission: bool,
    ) -> Result<ExplainResult, AppError> {
        let mut cat = self.catalog();
        cat.has_live_permission = has_live_permission;

        let guard_opts = GuardOptions {
            max_rows: req.max_rows.unwrap_or(0),
            allow_live: req.allow_live,
            params: req.params.clone(),
        };
        let guarded = guard(&req.sql, &cat, &guard_opts)?;

        let started = Instant::now();
        let budget = resolve_timeout_duration(req.timeout_ms);
        let _gate = tokio::time::timeout(budget, self.exec_gate.lock())
            .await
            .map_err(|_| AppError::Timeout("QUERY_TIMEOUT"))?;

        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(AppError::Timeout("QUERY_TIMEOUT"));
        }

        let explain_stmt = format!("EXPLAIN {}", guarded.sql);
        let (_, batches) = self.run_arrow_with_timeout(explain_stmt, remaining).await?;
        let plan = format_explain_batches(&batches)?;

        Ok(ExplainResult {
            sql: guarded.sql,
            plan,
            referenced_datasets: guarded.referenced_datasets,
            uses_platform_views: guarded.uses_platform_views,
        })
    }

    /// Executes `EXPLAIN <guarded.sql>` on an already-validated [`GuardedSql`] (`05-STEP04` §04.6.5, §04.7).
    ///
    /// # Errors
    /// Returns `QUERY_TIMEOUT` (`504`), `SQL_REJECTED` (on `DuckDB` binder/planner errors), or `Database`.
    pub async fn explain_guarded(
        &self,
        guarded: &GuardedSql,
        timeout_ms: Option<u64>,
    ) -> Result<ExplainResult, AppError> {
        let started = Instant::now();
        let budget = resolve_timeout_duration(timeout_ms);
        let _gate = tokio::time::timeout(budget, self.exec_gate.lock())
            .await
            .map_err(|_| AppError::Timeout("QUERY_TIMEOUT"))?;

        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(AppError::Timeout("QUERY_TIMEOUT"));
        }

        let explain_stmt = format!("EXPLAIN {}", guarded.sql);
        let (_, batches) = self.run_arrow_with_timeout(explain_stmt, remaining).await?;
        let plan = format_explain_batches(&batches)?;

        Ok(ExplainResult {
            sql: guarded.sql.clone(),
            plan,
            referenced_datasets: guarded.referenced_datasets.clone(),
            uses_platform_views: guarded.uses_platform_views,
        })
    }

    /// Executes a trusted, code-generated internal `DuckDB` query without external user AST
    /// rewriting or PII masking (`06-STEP05-06` §05.5.1, §05.7, §06.2.1).
    ///
    /// Streams Arrow batches up to `max_rows + 1` rows so `truncated` is accurately reported
    /// without materializing unbounded result sets in memory.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` / `SQL_REJECTED`) on empty,
    /// NUL-containing, or invalid SQL, or [`AppError::Database`] if `DuckDB` execution fails.
    pub fn run_internal(&self, sql: &str, max_rows: usize) -> Result<QueryResult, AppError> {
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "internal SQL query cannot be empty",
            ));
        }
        if trimmed.contains('\0') {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "internal SQL query contains NUL byte",
            ));
        }

        let started = Instant::now();
        let (schema, batches) = {
            let conn_guard = match self.conn.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut stmt = conn_guard
                .prepare(trimmed)
                .map_err(|err| map_duckdb_exec_error(&err.to_string()))?;
            let arrow_stream = stmt
                .query_arrow([])
                .map_err(|err| map_duckdb_exec_error(&err.to_string()))?;
            let schema = arrow_stream.get_schema();
            let stop_after = max_rows.saturating_add(1);
            let mut collected = Vec::new();
            let mut row_acc = 0_usize;
            for batch in arrow_stream {
                row_acc = row_acc.saturating_add(batch.num_rows());
                collected.push(batch);
                if row_acc >= stop_after {
                    break;
                }
            }
            (schema, collected)
        };

        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let effective_max_rows = u32::try_from(max_rows).unwrap_or(u32::MAX);
        let empty_catalog = TenantCatalog::new(&self.tenant_id);
        let empty_guarded = GuardedSql {
            sql: String::from(trimmed),
            effective_max_rows,
            referenced_datasets: Vec::new(),
            referenced_dataset_names: Vec::new(),
            referenced_columns: Vec::new(),
            output_lineage: Vec::new(),
            uses_platform_views: false,
            referenced_live_connections: Vec::new(),
        };

        let serialized = batches_to_query_result(
            &schema,
            &batches,
            ResultSerializeOptions {
                effective_max_rows,
                sql: String::from(trimmed),
                duration_ms,
                reveal_pii: false,
                has_pii_raw_read: false,
                catalog: &empty_catalog,
                guarded: &empty_guarded,
            },
        )?;
        Ok(serialized.result)
    }

    async fn run_arrow_with_timeout(
        &self,
        sql: String,
        timeout: Duration,
    ) -> Result<(arrow::datatypes::SchemaRef, Vec<RecordBatch>), AppError> {
        let timed_out = Arc::new(AtomicBool::new(false));
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

        let ih = Arc::clone(&self.interrupt_handle);
        let flag = Arc::clone(&timed_out);
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(timeout) => {
                    flag.store(true, Ordering::SeqCst);
                    ih.interrupt();
                }
                _ = cancel_rx => {}
            }
        });

        let conn_arc = Arc::clone(&self.conn);
        let blocking_res = tokio::task::spawn_blocking(move || {
            let conn_guard = match conn_arc.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut stmt = conn_guard.prepare(&sql)?;
            let arrow_stream = stmt.query_arrow([])?;
            let schema = arrow_stream.get_schema();
            let batches: Vec<RecordBatch> = arrow_stream.collect();
            Ok::<_, duckdb::Error>((schema, batches))
        })
        .await;

        let _watchdog_stopped = cancel_tx.send(());

        match blocking_res {
            Ok(Ok(pair)) => {
                if timed_out.load(Ordering::SeqCst) {
                    return Err(AppError::Timeout("QUERY_TIMEOUT"));
                }
                Ok(pair)
            }
            Ok(Err(duck_err)) => {
                let msg = duck_err.to_string();
                if timed_out.load(Ordering::SeqCst) || is_interrupt_error(&msg) {
                    Err(AppError::Timeout("QUERY_TIMEOUT"))
                } else {
                    Err(map_duckdb_exec_error(&msg))
                }
            }
            Err(join_err) => Err(AppError::Database(format!(
                "duckdb blocking query task failed: {join_err}"
            ))),
        }
    }
}

/// LRU cache of per-tenant hardened [`TenantDb`] instances (`05-STEP04` §04.2).
pub struct SessionPool {
    inner: AsyncMutex<LruCache<String, Arc<TenantDb>>>,
    cfg: AnalyticsConfig,
}

impl std::fmt::Debug for SessionPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionPool")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl SessionPool {
    /// Creates a new [`SessionPool`] with LRU capacity `cfg.tenant_cache_capacity` (minimum `1`).
    #[must_use]
    pub fn new(cfg: AnalyticsConfig) -> Self {
        let cap_num = cfg.tenant_cache_capacity.max(1);
        let cap = NonZeroUsize::new(cap_num).unwrap_or(NonZeroUsize::MIN);
        Self {
            inner: AsyncMutex::new(LruCache::new(cap)),
            cfg,
        }
    }

    /// Returns the pool's [`AnalyticsConfig`].
    #[must_use]
    pub const fn config(&self) -> &AnalyticsConfig {
        &self.cfg
    }

    /// Returns an existing cached [`TenantDb`] for `tenant_id` without constructing one.
    pub async fn get_cached(&self, tenant_id: &str) -> Option<Arc<TenantDb>> {
        let mut cache = self.inner.lock().await;
        cache.get(tenant_id).cloned()
    }

    /// Returns a cached [`TenantDb`] for `tenant_id` (refreshing its `catalog`) or builds and caches a new one.
    ///
    /// # Errors
    /// Propagates any error from [`TenantDb::build_async`].
    pub async fn get_or_try_init(
        &self,
        tenant_id: &str,
        catalog: TenantCatalog,
        live_sources: &[LiveConnectionSpec],
    ) -> Result<Arc<TenantDb>, AppError> {
        {
            let mut cache = self.inner.lock().await;
            if let Some(existing) = cache.get(tenant_id).cloned() {
                let missing_live = !live_sources.is_empty() && existing.attached_live.is_empty();
                if !missing_live {
                    existing.refresh_catalog(catalog);
                    return Ok(existing);
                }
                let _ = cache.pop(tenant_id);
            }
        }

        let built =
            TenantDb::build_async(tenant_id, self.cfg.clone(), catalog, live_sources.to_vec())
                .await?;

        let mut cache = self.inner.lock().await;
        if let Some(raced) = cache.get(tenant_id).cloned() {
            return Ok(raced);
        }
        cache.put(String::from(tenant_id), Arc::clone(&built));
        debug!(tenant_id = %tenant_id, "initialized hardened DuckDB TenantDb session");
        Ok(built)
    }

    /// Acquires the cached [`TenantDb`] for `tenant_id` or opens a hardened `DuckLake`-backed
    /// session with an empty user SQL catalog (for internal `profiling` and `MAP_MATERIALIZE` callers).
    ///
    /// # Errors
    /// Propagates any error from [`TenantDb::build_async`].
    pub async fn get_or_open(&self, tenant_id: &str) -> Result<Arc<TenantDb>, AppError> {
        {
            let mut cache = self.inner.lock().await;
            if let Some(existing) = cache.get(tenant_id).cloned() {
                return Ok(existing);
            }
        }

        let built = TenantDb::build_async(
            tenant_id,
            self.cfg.clone(),
            TenantCatalog::new(tenant_id),
            Vec::new(),
        )
        .await?;

        let mut cache = self.inner.lock().await;
        if let Some(raced) = cache.get(tenant_id).cloned() {
            return Ok(raced);
        }
        cache.put(String::from(tenant_id), Arc::clone(&built));
        debug!(tenant_id = %tenant_id, "opened hardened DuckDB TenantDb session for internal query");
        Ok(built)
    }

    /// Executes a trusted internal `DuckDB` query for `tenant_id` on the blocking threadpool
    /// (`06-STEP05-06` §05.5.1, §05.7, §06.2.1).
    ///
    /// # Errors
    /// Propagates any error from [`SessionPool::get_or_open`], [`TenantDb::run_internal`], or task join.
    pub async fn run_internal(
        &self,
        tenant_id: &str,
        sql: &str,
        max_rows: usize,
    ) -> Result<QueryResult, AppError> {
        let session = self.get_or_open(tenant_id).await?;
        let sql_owned = String::from(sql);
        tokio::task::spawn_blocking(move || session.run_internal(&sql_owned, max_rows))
            .await
            .map_err(|err| {
                AppError::Database(format!("duckdb internal query task failed: {err}"))
            })?
    }

    /// Evicts `tenant_id` from the LRU session pool (used when live connections or DSNs change).
    pub async fn invalidate_tenant(&self, tenant_id: &str) -> bool {
        let mut cache = self.inner.lock().await;
        cache.pop(tenant_id).is_some()
    }

    /// Clears all cached tenant sessions.
    pub async fn invalidate_all(&self) {
        let mut cache = self.inner.lock().await;
        cache.clear();
    }

    /// Refreshes the cached [`TenantCatalog`] for `tenant_id` if currently resident in the pool.
    pub async fn refresh_catalog(&self, tenant_id: &str, catalog: TenantCatalog) -> bool {
        let cache = self.inner.lock().await;
        if let Some(session) = cache.peek(tenant_id) {
            session.refresh_catalog(catalog);
            true
        } else {
            false
        }
    }

    /// Convenience entrypoint acquiring/initializing `tenant_id`'s session and executing `req`.
    ///
    /// # Errors
    /// Returns `PAYLOAD_TOO_LARGE`, `PERMISSION_DENIED`, `SQL_REJECTED`, `QUERY_TIMEOUT`, or `Database`.
    pub async fn query(
        &self,
        tenant_id: &str,
        catalog: TenantCatalog,
        live_sources: &[LiveConnectionSpec],
        req: &QueryRequest,
        has_live_permission: bool,
        has_pii_raw_read: bool,
    ) -> Result<SerializedQueryOutput, AppError> {
        let session = self
            .get_or_try_init(tenant_id, catalog, live_sources)
            .await?;
        session
            .execute_sql(req, has_live_permission, has_pii_raw_read)
            .await
    }

    /// Convenience entrypoint acquiring/initializing `tenant_id`'s session and running `EXPLAIN`.
    ///
    /// # Errors
    /// Returns `PAYLOAD_TOO_LARGE`, `PERMISSION_DENIED`, `SQL_REJECTED`, `QUERY_TIMEOUT`, or `Database`.
    pub async fn explain(
        &self,
        tenant_id: &str,
        catalog: TenantCatalog,
        live_sources: &[LiveConnectionSpec],
        req: &QueryRequest,
        has_live_permission: bool,
    ) -> Result<ExplainResult, AppError> {
        let session = self
            .get_or_try_init(tenant_id, catalog, live_sources)
            .await?;
        session.explain_sql(req, has_live_permission).await
    }
}

impl LakeInternalQueryPort for SessionPool {
    async fn query_internal(
        &self,
        tenant_id: &str,
        sql: &str,
        max_rows: usize,
    ) -> Result<QueryResult, AppError> {
        self.run_internal(tenant_id, sql, max_rows).await
    }

    async fn invalidate_tenant_session(&self, tenant_id: &str) -> bool {
        self.invalidate_tenant(tenant_id).await
    }
}

fn resolve_timeout_duration(timeout_ms: Option<u64>) -> Duration {
    let raw = match timeout_ms {
        None | Some(0) => DEFAULT_QUERY_TIMEOUT_MS,
        Some(ms) => ms.min(MAX_QUERY_TIMEOUT_MS),
    };
    Duration::from_millis(raw)
}

fn is_interrupt_error(msg: &str) -> bool {
    msg.to_ascii_lowercase().contains("interrupt")
}

fn map_duckdb_exec_error(msg: &str) -> AppError {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("syntax error")
        || lower.contains("binder error")
        || lower.contains("catalog error")
        || lower.contains("conversion error")
        || lower.contains("parser error")
    {
        AppError::bad("SQL_REJECTED", format!("query execution failed: {msg}"))
    } else {
        AppError::Database(format!("duckdb query failed: {msg}"))
    }
}

fn format_explain_batches(batches: &[RecordBatch]) -> Result<String, AppError> {
    let opts = FormatOptions::default();
    let mut parts = Vec::new();
    for batch in batches {
        let mut formatters = Vec::with_capacity(batch.num_columns());
        for col in batch.columns() {
            let fmt = ArrayFormatter::try_new(col.as_ref(), &opts)
                .map_err(|err| AppError::Database(format!("EXPLAIN format failed: {err}")))?;
            formatters.push(fmt);
        }
        for row_idx in 0..batch.num_rows() {
            let mut row_cells = Vec::with_capacity(formatters.len());
            for fmt in &formatters {
                let text = fmt.value(row_idx).to_string();
                if !text.trim().is_empty() {
                    row_cells.push(text);
                }
            }
            if !row_cells.is_empty() {
                parts.push(row_cells.join("\n"));
            }
        }
    }
    Ok(parts.join("\n"))
}

fn validate_tenant_ident(tenant_id: &str) -> Result<(), AppError> {
    let bytes = tenant_id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "invalid tenant_id length",
        ));
    }
    let valid = bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !valid {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "invalid tenant_id format",
        ));
    }
    Ok(())
}

fn validate_live_ident(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 58 {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn sql_escape_single_quoted(s: &str) -> String {
    s.replace('\'', "''")
}

fn sql_escape_double_quoted(s: &str) -> String {
    s.replace('"', "\"\"")
}

fn canonical_or_owned(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn attach_ducklake(
    conn: &duckdb::Connection,
    cfg: &AnalyticsConfig,
    allowed_dirs: &mut BTreeSet<String>,
) -> Result<(), AppError> {
    let raw_lake_dir = cfg.data_root.join("lake");
    std::fs::create_dir_all(&raw_lake_dir).map_err(|err| {
        AppError::Config(format!(
            "failed to create lake directory '{}': {err}",
            raw_lake_dir.display()
        ))
    })?;
    let lake_dir = canonical_or_owned(&raw_lake_dir);
    let lake_dir_str = lake_dir.to_string_lossy().into_owned();
    let lake_data_path = if lake_dir_str.ends_with('/') {
        lake_dir_str.clone()
    } else {
        format!("{lake_dir_str}/")
    };
    allowed_dirs.insert(lake_dir_str);

    let dsn_opt = cfg
        .lake_catalog_dsn
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match dsn_opt {
        Some(dsn) if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") => {
            let libpq = postgres_dsn_to_libpq(dsn, None)?;
            let target = format!("ducklake:postgres:{libpq}");
            attach_postgres_ducklake(conn, &target, &lake_data_path)
        }
        Some(dsn) if dsn.starts_with("ducklake:postgres:") => {
            attach_postgres_ducklake(conn, dsn, &lake_data_path)
        }
        Some(dsn) => {
            let file_part = dsn.strip_prefix("ducklake:").unwrap_or(dsn);
            attach_local_ducklake_file(conn, Path::new(file_part), &lake_data_path, allowed_dirs)
        }
        None => {
            let default_file = cfg.data_root.join("lake_catalog.ducklake");
            attach_local_ducklake_file(conn, &default_file, &lake_data_path, allowed_dirs)
        }
    }
}

fn attach_postgres_ducklake(
    conn: &duckdb::Connection,
    ducklake_target: &str,
    lake_data_path: &str,
) -> Result<(), AppError> {
    conn.execute_batch("LOAD ducklake; LOAD postgres;")
        .map_err(|err| AppError::Config(format!("failed to LOAD ducklake/postgres: {err}")))?;

    let escaped_target = sql_escape_single_quoted(ducklake_target);
    let escaped_data = sql_escape_single_quoted(lake_data_path);
    let ro_sql = format!(
        "ATTACH '{escaped_target}' AS lake (DATA_PATH '{escaped_data}', METADATA_SCHEMA 'lake_catalog', READ_ONLY);"
    );

    if conn.execute_batch(&ro_sql).is_err() {
        let init_sql = format!(
            "ATTACH '{escaped_target}' AS lake_init (DATA_PATH '{escaped_data}', METADATA_SCHEMA 'lake_catalog'); DETACH lake_init;"
        );
        conn.execute_batch(&init_sql).map_err(|err| {
            AppError::Config(format!(
                "failed to initialize postgres DuckLake catalog: {err}"
            ))
        })?;
        conn.execute_batch(&ro_sql).map_err(|err| {
            AppError::Config(format!(
                "failed to ATTACH postgres DuckLake READ_ONLY: {err}"
            ))
        })?;
    }
    Ok(())
}

fn attach_local_ducklake_file(
    conn: &duckdb::Connection,
    catalog_file: &Path,
    lake_data_path: &str,
    allowed_dirs: &mut BTreeSet<String>,
) -> Result<(), AppError> {
    if let Some(parent) = catalog_file.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| {
            AppError::Config(format!(
                "failed to create DuckLake catalog parent '{}': {err}",
                parent.display()
            ))
        })?;
        let canon_parent = canonical_or_owned(parent);
        allowed_dirs.insert(canon_parent.to_string_lossy().into_owned());
    }

    let resolved_file = if let Some(parent) = catalog_file.parent()
        && let Some(fname) = catalog_file.file_name()
    {
        canonical_or_owned(parent).join(fname)
    } else {
        catalog_file.to_path_buf()
    };

    conn.execute_batch("LOAD ducklake;")
        .map_err(|err| AppError::Config(format!("failed to LOAD ducklake: {err}")))?;

    let escaped_file = sql_escape_single_quoted(&resolved_file.to_string_lossy());
    let escaped_data = sql_escape_single_quoted(lake_data_path);
    let init_sql = format!(
        "ATTACH 'ducklake:{escaped_file}' AS lake_init (DATA_PATH '{escaped_data}'); DETACH lake_init;"
    );
    let ro_sql = format!(
        "ATTACH 'ducklake:{escaped_file}' AS lake (DATA_PATH '{escaped_data}', READ_ONLY);"
    );

    if !resolved_file.exists() {
        conn.execute_batch(&init_sql).map_err(|err| {
            AppError::Config(format!(
                "failed to initialize local DuckLake catalog: {err}"
            ))
        })?;
    }

    if conn.execute_batch(&ro_sql).is_err() {
        conn.execute_batch(&init_sql).map_err(|err| {
            AppError::Config(format!("failed to bootstrap local DuckLake catalog: {err}"))
        })?;
        conn.execute_batch(&ro_sql).map_err(|err| {
            AppError::Config(format!("failed to ATTACH local DuckLake READ_ONLY: {err}"))
        })?;
    }
    Ok(())
}

fn attach_platform_postgres(
    conn: &duckdb::Connection,
    tenant_id: &str,
    app_database_url: &str,
) -> Result<(), AppError> {
    let guc_opt = format!("-c app.tenant_id={tenant_id}");
    let libpq = postgres_dsn_to_libpq(app_database_url, Some(&guc_opt))?;
    let escaped_libpq = sql_escape_single_quoted(&libpq);
    let escaped_tid = sql_escape_single_quoted(tenant_id);

    let batch_sql = format!(
        "LOAD postgres;
         ATTACH '{escaped_libpq}' AS ax_pg (TYPE postgres, READ_ONLY);
         CREATE SCHEMA IF NOT EXISTS ax;
         CREATE OR REPLACE VIEW ax.nodes AS
             SELECT id::VARCHAR AS id, type_name, node_key, title,
                    properties::VARCHAR AS properties, metrics::VARCHAR AS metrics,
                    geo_lng, geo_lat, updated_at
               FROM ax_pg.public.xb_nodes
              WHERE tenant_id = '{escaped_tid}';
         CREATE OR REPLACE VIEW ax.links AS
             SELECT id::VARCHAR AS id, link_type,
                    src_node_id::VARCHAR AS src_node_id, dst_node_id::VARCHAR AS dst_node_id,
                    weight, properties::VARCHAR AS properties
               FROM ax_pg.public.xb_links
              WHERE tenant_id = '{escaped_tid}';
         CREATE OR REPLACE VIEW ax.metric_points AS
             SELECT node_id::VARCHAR AS node_id, metric, ts, value, value_text, source_tag
               FROM ax_pg.public.xb_metric_series
              WHERE tenant_id = '{escaped_tid}';
         CREATE OR REPLACE VIEW ax.metric_series AS
             SELECT node_id::VARCHAR AS node_id, metric, ts, value, value_text, source_tag
               FROM ax_pg.public.xb_metric_series
              WHERE tenant_id = '{escaped_tid}';
         CREATE OR REPLACE VIEW ax.metric_rollups AS
             SELECT node_id::VARCHAR AS node_id, metric, grain, bucket_start,
                    cnt, sum_v, min_v, max_v, last_v
               FROM ax_pg.public.xb_metric_rollups
              WHERE tenant_id = '{escaped_tid}';"
    );

    conn.execute_batch(&batch_sql).map_err(|err| {
        AppError::Config(format!(
            "failed to ATTACH platform PostgreSQL and create ax.* views: {err}"
        ))
    })
}

fn attach_live_sources(
    conn: &duckdb::Connection,
    tenant_id: &str,
    live_sources: &[LiveConnectionSpec],
    keyring: Option<&CredentialKeyring>,
    allowed_dirs: &mut BTreeSet<String>,
) -> HashSet<String> {
    let mut attached = HashSet::new();
    for spec in live_sources {
        let norm_name = spec.name.trim().to_ascii_lowercase();
        let norm_name = norm_name.strip_prefix("live_").unwrap_or(&norm_name);
        if !validate_live_ident(norm_name) {
            warn!(
                tenant_id = %tenant_id,
                connection_id = %spec.id,
                name = %spec.name,
                "skipping live connection with invalid SQL identifier name"
            );
            continue;
        }

        let secrets = match decrypt_live_secrets(tenant_id, spec.credential.as_ref(), keyring) {
            Ok(map) => map,
            Err(err) => {
                warn!(
                    tenant_id = %tenant_id,
                    connection_id = %spec.id,
                    error = %err,
                    "failed to decrypt live connection credential; skipping mount"
                );
                continue;
            }
        };

        let alias_quoted = sql_escape_double_quoted(&format!("live_{norm_name}"));
        let attach_res = match spec.connector_type {
            ConnectorType::Postgres => {
                build_live_postgres_sql(&spec.config, &secrets, &alias_quoted).and_then(|sql| {
                    conn.execute_batch(&sql)
                        .map_err(|e| AppError::Upstream(e.to_string()))
                })
            }
            ConnectorType::Mysql => build_live_mysql_sql(&spec.config, &secrets, &alias_quoted)
                .and_then(|sql| {
                    conn.execute_batch(&sql)
                        .map_err(|e| AppError::Upstream(e.to_string()))
                }),
            ConnectorType::Sqlite => {
                build_live_sqlite_sql(&spec.config, &alias_quoted, allowed_dirs).and_then(|sql| {
                    conn.execute_batch(&sql)
                        .map_err(|e| AppError::Upstream(e.to_string()))
                })
            }
            _ => continue,
        };

        match attach_res {
            Ok(()) => {
                attached.insert(String::from(norm_name));
            }
            Err(err) => {
                warn!(
                    tenant_id = %tenant_id,
                    connection_id = %spec.id,
                    alias = %norm_name,
                    error = %err,
                    "live relational source offline or failed to attach; continuing session startup"
                );
            }
        }
    }
    attached
}

fn decrypt_live_secrets(
    tenant_id: &str,
    sealed: Option<&SealedCredential>,
    keyring: Option<&CredentialKeyring>,
) -> Result<BTreeMap<String, String>, AppError> {
    let Some(cred) = sealed else {
        return Ok(BTreeMap::new());
    };
    let Some(kr) = keyring else {
        return Err(AppError::Config(String::from(
            "keyring required to decrypt live connection credentials",
        )));
    };
    let aad = CredentialKeyring::aad_for_credential(tenant_id, cred.id);
    let plain = kr.open_credential(&aad, cred)?;
    serde_json::from_slice(plain.as_slice())
        .map_err(|_| AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
}

fn cfg_str<'a>(
    config: &'a serde_json::Map<String, serde_json::Value>,
    secrets: &'a BTreeMap<String, String>,
    keys: &[&str],
) -> Option<&'a str> {
    for &k in keys {
        if let Some(s) = secrets.get(k).map(String::as_str).filter(|v| !v.is_empty()) {
            return Some(s);
        }
        if let Some(s) = config
            .get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
        {
            return Some(s);
        }
    }
    None
}

fn cfg_port(config: &serde_json::Map<String, serde_json::Value>, default_port: u16) -> String {
    if let Some(val) = config.get("port") {
        if let Some(n) = val.as_u64() {
            return n.to_string();
        }
        if let Some(s) = val.as_str().filter(|s| !s.is_empty()) {
            return String::from(s);
        }
    }
    default_port.to_string()
}

fn build_live_postgres_sql(
    config: &serde_json::Map<String, serde_json::Value>,
    secrets: &BTreeMap<String, String>,
    alias_quoted: &str,
) -> Result<String, AppError> {
    let libpq = if let Some(dsn) = cfg_str(config, secrets, &["dsn", "url", "connection_string"]) {
        postgres_dsn_to_libpq(dsn, None)?
    } else {
        let host = cfg_str(config, secrets, &["host"]).unwrap_or("localhost");
        let port = cfg_port(config, 5432);
        let dbname = cfg_str(config, secrets, &["database", "dbname"]).ok_or_else(|| {
            AppError::Config(String::from("postgres live connection missing database"))
        })?;
        let mut pairs = vec![
            format!("host={}", libpq_quote(host)),
            format!("port={}", libpq_quote(&port)),
            format!("dbname={}", libpq_quote(dbname)),
        ];
        if let Some(user) = cfg_str(config, secrets, &["username", "user"]) {
            pairs.push(format!("user={}", libpq_quote(user)));
        }
        if let Some(pw) = cfg_str(config, secrets, &["password"]) {
            pairs.push(format!("password={}", libpq_quote(pw)));
        }
        if let Some(ssl) = cfg_str(config, secrets, &["sslmode", "ssl_mode"]) {
            pairs.push(format!("sslmode={}", libpq_quote(ssl)));
        }
        pairs.join(" ")
    };
    let escaped = sql_escape_single_quoted(&libpq);
    Ok(format!(
        "LOAD postgres; ATTACH '{escaped}' AS \"{alias_quoted}\" (TYPE postgres, READ_ONLY);"
    ))
}

fn build_live_mysql_sql(
    config: &serde_json::Map<String, serde_json::Value>,
    secrets: &BTreeMap<String, String>,
    alias_quoted: &str,
) -> Result<String, AppError> {
    let host = cfg_str(config, secrets, &["host"]).unwrap_or("localhost");
    let port = cfg_port(config, 3306);
    let database = cfg_str(config, secrets, &["database", "dbname"])
        .ok_or_else(|| AppError::Config(String::from("mysql live connection missing database")))?;
    let mut pairs = vec![
        format!("host={}", libpq_quote(host)),
        format!("port={}", libpq_quote(&port)),
        format!("database={}", libpq_quote(database)),
    ];
    if let Some(user) = cfg_str(config, secrets, &["username", "user"]) {
        pairs.push(format!("user={}", libpq_quote(user)));
    }
    if let Some(pw) = cfg_str(config, secrets, &["password"]) {
        pairs.push(format!("password={}", libpq_quote(pw)));
    }
    let conn_str = sql_escape_single_quoted(&pairs.join(" "));
    Ok(format!(
        "LOAD mysql_scanner; ATTACH '{conn_str}' AS \"{alias_quoted}\" (TYPE mysql, READ_ONLY);"
    ))
}

fn build_live_sqlite_sql(
    config: &serde_json::Map<String, serde_json::Value>,
    alias_quoted: &str,
    allowed_dirs: &mut BTreeSet<String>,
) -> Result<String, AppError> {
    let empty = BTreeMap::new();
    let raw_path = cfg_str(config, &empty, &["path", "database", "file"]).ok_or_else(|| {
        AppError::Config(String::from("sqlite live connection missing file path"))
    })?;
    let p = canonical_or_owned(Path::new(raw_path));
    if let Some(parent) = p.parent()
        && !parent.as_os_str().is_empty()
    {
        allowed_dirs.insert(parent.to_string_lossy().into_owned());
    }
    let escaped = sql_escape_single_quoted(&p.to_string_lossy());
    Ok(format!(
        "LOAD sqlite_scanner; ATTACH '{escaped}' AS \"{alias_quoted}\" (TYPE sqlite, READ_ONLY);"
    ))
}

fn lock_duckdb_security(
    conn: &duckdb::Connection,
    cfg: &AnalyticsConfig,
    allowed_dirs: &BTreeSet<String>,
) -> Result<(), AppError> {
    let dirs_literal: Vec<String> = allowed_dirs
        .iter()
        .map(|d| format!("'{}'", sql_escape_single_quoted(d)))
        .collect();
    let dirs_sql = dirs_literal.join(", ");

    let threads = cfg.threads.max(1);
    let mem = sanitize_size_setting(&cfg.memory_limit, "1GB");

    let lockdown_sql = format!(
        "SET allowed_directories = [{dirs_sql}];
         SET threads = {threads};
         SET memory_limit = '{mem}';
         SET max_temp_directory_size = '10GB';
         SET enable_external_access = false;
         SET lock_configuration = true;"
    );

    conn.execute_batch(&lockdown_sql)
        .map_err(|err| AppError::Config(format!("failed to lock DuckDB configuration: {err}")))
}

fn sanitize_size_setting(raw: &str, fallback: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 16 {
        return String::from(fallback);
    }
    let ok = trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b' ');
    if ok {
        String::from(trimmed)
    } else {
        String::from(fallback)
    }
}

/// Converts a `postgres://` / `postgresql://` URI (or existing libpq key-value string) into a
/// properly escaped libpq connection string, percent-decoding credentials and query options.
///
/// # Errors
/// Returns [`AppError::Config`] if the URI is malformed.
pub fn postgres_dsn_to_libpq(dsn: &str, extra_options: Option<&str>) -> Result<String, AppError> {
    let trimmed = dsn.trim();
    let Some(rest) = trimmed
        .strip_prefix("postgresql://")
        .or_else(|| trimmed.strip_prefix("postgres://"))
    else {
        return Ok(match extra_options {
            Some(opt) if !trimmed.contains("options=") => {
                format!("{trimmed} options={}", libpq_quote(opt))
            }
            _ => String::from(trimmed),
        });
    };

    let (before_query, query_opt) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };
    let (authority, path_part) = match before_query.split_once('/') {
        Some((auth, p)) => (auth, p),
        None => (before_query, ""),
    };

    let (userinfo_opt, hostport) = match authority.rsplit_once('@') {
        Some((u, hp)) => (Some(u), hp),
        None => (None, authority),
    };

    let mut kv: Vec<(String, String)> = Vec::new();
    let (host, port_opt) = parse_host_port(hostport)?;
    if !host.is_empty() {
        kv.push((String::from("host"), percent_decode(&host)));
    }
    if let Some(port) = port_opt
        && !port.is_empty()
    {
        kv.push((String::from("port"), port));
    }

    if !path_part.is_empty() {
        kv.push((String::from("dbname"), percent_decode(path_part)));
    }

    if let Some(userinfo) = userinfo_opt {
        match userinfo.split_once(':') {
            Some((user, pass)) => {
                if !user.is_empty() {
                    kv.push((String::from("user"), percent_decode(user)));
                }
                kv.push((String::from("password"), percent_decode(pass)));
            }
            None if !userinfo.is_empty() => {
                kv.push((String::from("user"), percent_decode(userinfo)));
            }
            None => {}
        }
    }

    let mut existing_options: Option<String> = None;
    if let Some(query) = query_opt {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (raw_k, raw_v) = pair.split_once('=').unwrap_or((pair, ""));
            let k = percent_decode(raw_k);
            let v = percent_decode(raw_v);
            if k == "options" {
                existing_options = Some(v);
            } else if !k.is_empty() {
                kv.push((k, v));
            }
        }
    }

    match (existing_options, extra_options) {
        (Some(mut ex), Some(extra)) => {
            ex.push(' ');
            ex.push_str(extra);
            kv.push((String::from("options"), ex));
        }
        (Some(ex), None) => kv.push((String::from("options"), ex)),
        (None, Some(extra)) => kv.push((String::from("options"), String::from(extra))),
        (None, None) => {}
    }

    let rendered: Vec<String> = kv
        .into_iter()
        .map(|(k, v)| format!("{k}={}", libpq_quote(&v)))
        .collect();
    Ok(rendered.join(" "))
}

fn parse_host_port(hostport: &str) -> Result<(String, Option<String>), AppError> {
    if let Some(after_bracket) = hostport.strip_prefix('[') {
        let Some((ipv6, remainder)) = after_bracket.split_once(']') else {
            return Err(AppError::Config(String::from(
                "unclosed IPv6 bracket in PostgreSQL DSN",
            )));
        };
        let port = remainder.strip_prefix(':').map(String::from);
        return Ok((String::from(ipv6), port));
    }
    if let Some((h, p)) = hostport.rsplit_once(':') {
        Ok((String::from(h), Some(String::from(p))))
    } else {
        Ok((String::from(hostport), None))
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0_usize;
    while let Some(&b) = bytes.get(idx) {
        if b == b'%'
            && let Some(&h1) = bytes.get(idx.saturating_add(1))
            && let Some(&h2) = bytes.get(idx.saturating_add(2))
            && let (Some(v1), Some(v2)) = (hex_nibble(h1), hex_nibble(h2))
        {
            out.push((v1 << 4) | v2);
            idx = idx.saturating_add(3);
            continue;
        }
        out.push(b);
        idx = idx.saturating_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn libpq_quote(val: &str) -> String {
    let needs_quotes = val.is_empty()
        || val
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '\\');
    if !needs_quotes {
        return String::from(val);
    }
    let mut escaped = String::with_capacity(val.len().saturating_add(4));
    escaped.push('\'');
    for ch in val.chars() {
        if ch == '\\' || ch == '\'' {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped.push('\'');
    escaped
}

#[cfg(test)]
mod tests {
    use axpoint_domain::axhub::ColumnMeta;
    use axpoint_domain::enums::{DataType, PiiCategory};
    use serde_json::json;

    use super::*;
    use crate::sql_guard::CatalogDatasetEntry;

    fn temp_analytics_config(test_name: &str) -> AnalyticsConfig {
        let dir =
            std::env::temp_dir().join(format!("axpoint-analytics-{test_name}-{}", Uuid::new_v4()));
        AnalyticsConfig {
            data_root: dir,
            lake_catalog_dsn: None,
            app_database_url: None,
            tenant_cache_capacity: 2,
            memory_limit: String::from("256MB"),
            threads: 2,
            keyring: None,
        }
    }

    #[test]
    fn postgres_uri_converts_to_escaped_libpq_with_percent_decoding() -> Result<(), AppError> {
        let libpq = postgres_dsn_to_libpq(
            "postgres://axpoint_app:p%40ss%20w%27rd@db.internal:5433/xbluenode?sslmode=disable",
            Some("-c app.tenant_id=default"),
        )?;
        assert!(libpq.contains("host=db.internal"));
        assert!(libpq.contains("port=5433"));
        assert!(libpq.contains("dbname=xbluenode"));
        assert!(libpq.contains("user=axpoint_app"));
        assert!(libpq.contains("password='p@ss w\\'rd'"));
        assert!(libpq.contains("sslmode=disable"));
        assert!(libpq.contains("options='-c app.tenant_id=default'"));
        Ok(())
    }

    #[tokio::test]
    async fn session_pool_truncates_range_queries_and_locks_duckdb_config() -> Result<(), AppError>
    {
        let cfg = temp_analytics_config("range");
        let data_root = cfg.data_root.clone();
        let pool = SessionPool::new(cfg);
        let catalog = TenantCatalog::new("default");

        let req = QueryRequest {
            sql: String::from("SELECT * FROM range(20)"),
            params: BTreeMap::new(),
            max_rows: Some(10),
            timeout_ms: Some(5_000),
            allow_live: false,
            reveal_pii: false,
        };
        let out = pool
            .query("default", catalog.clone(), &[], &req, false, false)
            .await?;
        assert!(out.result.truncated);
        assert_eq!(out.result.row_count, 10);
        assert_eq!(out.result.rows.len(), 10);

        let explain = pool
            .explain("default", catalog.clone(), &[], &req, false)
            .await?;
        assert_ne!(explain.plan, "");

        // Verify DuckDB lock_configuration=true & enable_external_access=false at engine level too
        let session = pool.get_or_try_init("default", catalog, &[]).await?;
        let raw_check = tokio::task::spawn_blocking(move || {
            let guard_conn = match session.conn.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard_conn
                .execute_batch("SET enable_external_access = true;")
                .is_err()
        })
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;
        assert!(
            raw_check,
            "DuckDB lock_configuration must block SET changes"
        );

        std::fs::remove_dir_all(data_root).map_err(|e| AppError::Config(e.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn ducklake_dataset_query_masks_pii_and_supports_lru_invalidation() -> Result<(), AppError>
    {
        let cfg = temp_analytics_config("ducklake");
        let data_root = cfg.data_root.clone();
        let lake_dir = data_root.join("lake");
        std::fs::create_dir_all(&lake_dir).map_err(|e| AppError::Config(e.to_string()))?;
        let canon_lake = canonical_or_owned(&lake_dir);
        let catalog_file = canonical_or_owned(&data_root).join("lake_catalog.ducklake");

        {
            let seed_conn = duckdb::Connection::open_in_memory()
                .map_err(|e| AppError::Config(e.to_string()))?;
            let batch = format!(
                "LOAD ducklake;
                 ATTACH 'ducklake:{}' AS lake_seed (DATA_PATH '{}/');
                 CREATE SCHEMA IF NOT EXISTS lake_seed.\"tenant_default\";
                 CREATE TABLE lake_seed.\"tenant_default\".\"demo_orders\" AS
                     SELECT 1 AS id, '13812345678' AS phone, 125.50::DECIMAL(10,2) AS amount
                     UNION ALL
                     SELECT 2 AS id, '13987654321' AS phone, 200.00::DECIMAL(10,2) AS amount;
                 DETACH lake_seed;",
                sql_escape_single_quoted(&catalog_file.to_string_lossy()),
                sql_escape_single_quoted(&canon_lake.to_string_lossy()),
            );
            seed_conn
                .execute_batch(&batch)
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        let ds_id = Uuid::new_v4();
        let mut ds = CatalogDatasetEntry::simple(ds_id, "demo_orders", "demo_orders");
        ds.columns.insert(
            String::from("phone"),
            ColumnMeta {
                dataset_id: ds_id,
                column_name: String::from("phone"),
                data_type: DataType::String,
                pii_category: PiiCategory::Phone,
                pii_confirmed: true,
                description: String::new(),
                business_term: String::new(),
                unit: String::new(),
                ordinal: 1,
            },
        );
        let catalog = TenantCatalog::new("default").with_dataset(ds);

        let pool = SessionPool::new(cfg);
        let req = QueryRequest {
            sql: String::from("SELECT id, phone, amount FROM demo_orders ORDER BY id"),
            params: BTreeMap::new(),
            max_rows: Some(100),
            timeout_ms: Some(5_000),
            allow_live: false,
            reveal_pii: false,
        };
        let out = pool
            .query("default", catalog.clone(), &[], &req, false, true)
            .await?;
        assert!(!out.revealed_pii);
        assert_eq!(out.result.row_count, 2);
        let Some(first) = out.result.rows.first() else {
            return Err(AppError::bad("TEST_FAIL", "expected first row"));
        };
        assert_eq!(first.get("id"), Some(&json!(1)));
        assert_eq!(first.get("phone"), Some(&json!("138****5678")));
        assert_eq!(first.get("amount"), Some(&json!(125.5)));

        assert!(pool.refresh_catalog("default", catalog).await);
        assert!(pool.invalidate_tenant("default").await);
        assert!(pool.get_cached("default").await.is_none());

        std::fs::remove_dir_all(data_root).map_err(|e| AppError::Config(e.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn runaway_cartesian_query_is_interrupted_with_query_timeout() -> Result<(), AppError> {
        let cfg = temp_analytics_config("timeout");
        let data_root = cfg.data_root.clone();
        let pool = SessionPool::new(cfg);
        let catalog = TenantCatalog::new("default");

        let warmup = QueryRequest {
            sql: String::from("SELECT 1 AS ok"),
            params: BTreeMap::new(),
            max_rows: Some(10),
            timeout_ms: Some(5_000),
            allow_live: false,
            reveal_pii: false,
        };
        let _warmup_out = pool
            .query("default", catalog.clone(), &[], &warmup, false, false)
            .await?;

        let runaway = QueryRequest {
            sql: String::from(
                "SELECT count(*) AS n FROM range(10000000) a CROSS JOIN range(10000000) b",
            ),
            params: BTreeMap::new(),
            max_rows: Some(10),
            timeout_ms: Some(60),
            allow_live: false,
            reveal_pii: false,
        };
        let res = pool
            .query("default", catalog.clone(), &[], &runaway, false, false)
            .await;
        assert!(
            matches!(res, Err(AppError::Timeout("QUERY_TIMEOUT"))),
            "expected QUERY_TIMEOUT, got: {res:?}"
        );

        let after = pool
            .query("default", catalog, &[], &warmup, false, false)
            .await?;
        assert_eq!(after.result.row_count, 1);

        std::fs::remove_dir_all(data_root).map_err(|e| AppError::Config(e.to_string()))?;
        Ok(())
    }
}
