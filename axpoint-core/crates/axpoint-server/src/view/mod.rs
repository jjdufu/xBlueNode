//! `xBlueView` digital-twin dashboard execution, caching, dependencies, real-time events,
//! AI generation, interactive actions, full-format exports, and `axs_` share services (`10-STEP11-12`).

pub mod action;
pub mod ai_gen;
pub mod cache;
pub mod data;
pub mod deps;
pub mod events;
pub mod export;
pub mod share;

use std::path::PathBuf;
use std::sync::Arc;

use axpoint_analytics::SessionPool;
use axpoint_llm::LlmRouter;

use self::cache::ViewCache;
use self::events::ViewEventBus;
use self::share::ShareRateLimiters;

/// Shared `xBlueView` services bundle injected into HTTP routes and background workers.
#[derive(Clone)]
pub struct ViewServices<S> {
    /// Persistence store implementing `ViewRepository` and supporting domain ports.
    pub store: S,
    /// Per-tenant hardened `DuckDB` session pool.
    pub analytics_pool: Arc<SessionPool>,
    /// Multi-model LLM router used for AI dashboard generation.
    pub llm_router: Arc<LlmRouter>,
    /// `moka` widget response cache (`capacity = 10_000`, `ttl = 600s`).
    pub cache: Arc<ViewCache>,
    /// In-process real-time `ViewEvent` broadcast bus (`capacity = 1_024`).
    pub event_bus: Arc<ViewEventBus>,
    /// Per-IP token-bucket rate limiters (`10 rpm` unlock, `120 rpm` access) for `/api/xblueview/share/*`.
    pub share_limiters: Arc<ShareRateLimiters>,
    /// Root filesystem directory (`AXPOINT_DATA_ROOT`) for uploaded visual assets and exports.
    pub data_root: PathBuf,
}

impl<S> ViewServices<S> {
    /// Constructs [`ViewServices`] with fresh [`ViewCache`], [`ViewEventBus`], and [`ShareRateLimiters`].
    #[must_use]
    pub fn new(
        store: S,
        analytics_pool: Arc<SessionPool>,
        llm_router: Arc<LlmRouter>,
        data_root: PathBuf,
    ) -> Self {
        Self {
            store,
            analytics_pool,
            llm_router,
            cache: Arc::new(ViewCache::new()),
            event_bus: Arc::new(ViewEventBus::new()),
            share_limiters: Arc::new(ShareRateLimiters::default()),
            data_root,
        }
    }
}
