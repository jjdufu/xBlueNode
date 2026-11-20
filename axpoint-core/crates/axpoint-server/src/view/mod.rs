//! `xBlueView` digital-twin dashboard execution, caching, dependencies, real-time events,
//! AI generation, and interactive node/flow action services (`10-STEP11-12` §11.2–§11.6).

pub mod action;
pub mod ai_gen;
pub mod cache;
pub mod data;
pub mod deps;
pub mod events;

use std::path::PathBuf;
use std::sync::Arc;

use axpoint_analytics::SessionPool;
use axpoint_llm::LlmRouter;

use self::cache::ViewCache;
use self::events::ViewEventBus;

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
    /// Root filesystem directory (`AXPOINT_DATA_ROOT`) for uploaded visual assets.
    pub data_root: PathBuf,
}

impl<S> ViewServices<S> {
    /// Constructs [`ViewServices`] with a fresh [`ViewCache`] and [`ViewEventBus`].
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
            data_root,
        }
    }
}
