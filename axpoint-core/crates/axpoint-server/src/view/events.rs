//! Real-time `xBlueView` event invalidation, `pg_notify('ax_view_events')`, and multi-instance `PgListener` bus (`10-STEP11-12` §11.5).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::enums::OutboxTopic;
use axpoint_domain::platform::{BoxFuture, OutboxEvent, OutboxSubscriber};
use axpoint_domain::ports::{GraphStorePort, ViewRepository};
use axpoint_domain::view::{ViewEvent, WidgetDeps};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::{broadcast, watch};
use tracing::warn;
use uuid::Uuid;

use super::cache::ViewCache;
use super::deps::normalize_deps;

/// PostgreSQL `LISTEN`/`NOTIFY` channel carrying cross-instance `xBlueView` invalidation events.
pub const VIEW_EVENTS_CHANNEL: &str = "ax_view_events";

/// Capacity of the in-process `ViewEvent` broadcast channel (`10-STEP11-12` §11.5).
pub const VIEW_EVENT_BUS_CAPACITY: usize = 1_024;

/// Maximum JSON byte size emitted on `pg_notify('ax_view_events', ...)` before compacting to `dashboard_ids`.
pub const PG_NOTIFY_MAX_BYTES: usize = 7_000;

const MAX_NODE_LOOKUPS_PER_EVENT: usize = 50;

/// In-process broadcast bus fanning out [`ViewEvent`]s to active dashboard `SSE` streams.
#[derive(Debug, Clone)]
pub struct ViewEventBus {
    sender: broadcast::Sender<ViewEvent>,
}

impl Default for ViewEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewEventBus {
    /// Creates a new [`ViewEventBus`] with capacity `1_024`.
    #[must_use]
    pub fn new() -> Self {
        let (sender, _rx) = broadcast::channel(VIEW_EVENT_BUS_CAPACITY);
        Self { sender }
    }

    /// Publishes a [`ViewEvent`] to all currently subscribed `SSE` streams.
    #[must_use]
    pub fn publish(&self, event: ViewEvent) -> usize {
        self.sender.send(event).unwrap_or(0)
    }

    /// Subscribes a new receiver to the [`ViewEventBus`].
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ViewEvent> {
        self.sender.subscribe()
    }
}

/// Outbox subscriber invalidating [`ViewCache`] and notifying all Core instances via `pg_notify('ax_view_events', ...)`.
pub struct ViewEventSubscriber<S> {
    store: S,
    pg_pool: Option<PgPool>,
    cache: Arc<ViewCache>,
    fallback_bus: Option<Arc<ViewEventBus>>,
}

impl<S> ViewEventSubscriber<S> {
    /// Constructs a production [`ViewEventSubscriber`] backed by `PgPool` `NOTIFY`.
    #[must_use]
    pub const fn new(store: S, pg_pool: PgPool, cache: Arc<ViewCache>) -> Self {
        Self {
            store,
            pg_pool: Some(pg_pool),
            cache,
            fallback_bus: None,
        }
    }

    /// Attaches an in-process [`ViewEventBus`] used when `pg_pool` is absent (for offline unit tests).
    #[must_use]
    pub fn with_offline_bus(store: S, cache: Arc<ViewCache>, bus: Arc<ViewEventBus>) -> Self {
        Self {
            store,
            pg_pool: None,
            cache,
            fallback_bus: Some(bus),
        }
    }
}

impl<S> OutboxSubscriber for ViewEventSubscriber<S>
where
    S: ViewRepository + GraphStorePort + Send + Sync + 'static,
{
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.process_outbox_event(event).await })
    }
}

impl<S> ViewEventSubscriber<S>
where
    S: ViewRepository + GraphStorePort + Send + Sync,
{
    /// Processes a single claimed [`OutboxEvent`], invalidating affected widgets and broadcasting `NOTIFY`.
    ///
    /// # Errors
    /// Propagates [`AppError`] if GIN dependency lookup or `pg_notify` fails.
    pub async fn process_outbox_event(&self, event: &OutboxEvent) -> Result<(), AppError> {
        let Some((filter_deps, alert_payload)) =
            resolve_filter_deps_for_event(&self.store, event).await?
        else {
            return Ok(());
        };

        let affected = self
            .store
            .find_widgets_by_deps(&event.tenant_id, &filter_deps)
            .await?;

        let widget_ids: Vec<Uuid> = affected
            .iter()
            .map(|r| r.widget_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let dashboard_ids: Vec<Uuid> = affected
            .iter()
            .map(|r| r.dashboard_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        if widget_ids.is_empty() && alert_payload.is_none() {
            return Ok(());
        }

        self.cache.invalidate_widgets(&event.tenant_id, &widget_ids);

        let view_event = ViewEvent {
            tenant_id: event.tenant_id.clone(),
            topic: String::from(event.topic.as_str()),
            widget_ids,
            dashboard_ids,
            alert: alert_payload,
        };

        if let Some(ref pool) = self.pg_pool {
            let payload_str = serialize_compact_view_event(&view_event)?;
            sqlx::query("SELECT pg_notify($1, $2)")
                .bind(VIEW_EVENTS_CHANNEL)
                .bind(&payload_str)
                .execute(pool)
                .await
                .map_err(|err| {
                    AppError::Database(format!("pg_notify {VIEW_EVENTS_CHANNEL} failed: {err}"))
                })?;
        } else if let Some(ref bus) = self.fallback_bus {
            let _receivers = bus.publish(view_event);
        }

        Ok(())
    }
}

/// Serializes a [`ViewEvent`] for `pg_notify`, compacting `widget_ids` when exceeding `7_000` bytes.
///
/// # Errors
/// Returns [`AppError::Database`] if JSON serialization fails.
pub fn serialize_compact_view_event(event: &ViewEvent) -> Result<String, AppError> {
    let full = serde_json::to_string(event)
        .map_err(|err| AppError::Database(format!("serialize ViewEvent failed: {err}")))?;
    if full.len() <= PG_NOTIFY_MAX_BYTES {
        return Ok(full);
    }

    let compacted = ViewEvent {
        tenant_id: event.tenant_id.clone(),
        topic: event.topic.clone(),
        widget_ids: Vec::new(),
        dashboard_ids: event.dashboard_ids.clone(),
        alert: event.alert.clone(),
    };
    let compact_json = serde_json::to_string(&compacted)
        .map_err(|err| AppError::Database(format!("serialize compact ViewEvent failed: {err}")))?;
    if compact_json.len() <= PG_NOTIFY_MAX_BYTES {
        return Ok(compact_json);
    }

    let minimal = ViewEvent {
        tenant_id: event.tenant_id.clone(),
        topic: event.topic.clone(),
        widget_ids: Vec::new(),
        dashboard_ids: event.dashboard_ids.clone(),
        alert: None,
    };
    serde_json::to_string(&minimal)
        .map_err(|err| AppError::Database(format!("serialize minimal ViewEvent failed: {err}")))
}

/// Starts the background `PgListener` forwarding `ax_view_events` notifications onto `bus`
/// until `shutdown_rx` signals `true`.
pub async fn run_view_event_listener(
    pool: PgPool,
    bus: Arc<ViewEventBus>,
    shutdown_rx: watch::Receiver<bool>,
) {
    run_view_event_listener_with_cache(pool, bus, None, shutdown_rx).await;
}

/// Starts the background `PgListener` forwarding `ax_view_events` notifications onto `bus`
/// and optionally invalidating `cache` entries for cross-instance consistency.
pub async fn run_view_event_listener_with_cache(
    pool: PgPool,
    bus: Arc<ViewEventBus>,
    cache: Option<Arc<ViewCache>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut listener = connect_view_listener(&pool).await;

    while !*shutdown_rx.borrow_and_update() {
        let Some(ref mut pg_listener) = listener else {
            tokio::select! {
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
                _ = shutdown_rx.changed() => break,
            }
            listener = connect_view_listener(&pool).await;
            continue;
        };

        tokio::select! {
            _ = shutdown_rx.changed() => break,
            recv_res = pg_listener.recv() => {
                match recv_res {
                    Ok(notif) => {
                        dispatch_pg_notification(notif.payload(), &bus, cache.as_ref());
                    }
                    Err(err) => {
                        warn!(error = %err, "PgListener ax_view_events interrupted; reconnecting");
                        listener = None;
                    }
                }
            }
        }
    }
}

async fn connect_view_listener(pool: &PgPool) -> Option<PgListener> {
    match PgListener::connect_with(pool).await {
        Ok(mut listener) => match listener.listen(VIEW_EVENTS_CHANNEL).await {
            Ok(()) => Some(listener),
            Err(err) => {
                warn!(error = %err, "PgListener failed to LISTEN ax_view_events");
                None
            }
        },
        Err(err) => {
            warn!(error = %err, "failed to connect PgListener for ax_view_events");
            None
        }
    }
}

fn dispatch_pg_notification(raw_payload: &str, bus: &ViewEventBus, cache: Option<&Arc<ViewCache>>) {
    match serde_json::from_str::<ViewEvent>(raw_payload) {
        Ok(event) => {
            if let Some(view_cache) = cache
                && !event.widget_ids.is_empty()
            {
                view_cache.invalidate_widgets(&event.tenant_id, &event.widget_ids);
            }
            let _receivers = bus.publish(event);
        }
        Err(err) => {
            warn!(error = %err, "ignoring malformed ViewEvent notification payload");
        }
    }
}

async fn resolve_filter_deps_for_event<G: GraphStorePort + ?Sized>(
    store: &G,
    event: &OutboxEvent,
) -> Result<Option<(WidgetDeps, Option<serde_json::Value>)>, AppError> {
    match event.topic {
        OutboxTopic::DatasetSynced => {
            let Some(ds_id) = event
                .payload
                .get("dataset_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                return Ok(None);
            };
            Ok(Some((
                WidgetDeps {
                    dataset_ids: vec![ds_id],
                    ..WidgetDeps::default()
                },
                None,
            )))
        }
        OutboxTopic::NodeMetricsChanged | OutboxTopic::NodeUpserted => {
            let mut type_names = extract_string_list(&event.payload, "type_names");
            if let Some(single) = event
                .payload
                .get("type_name")
                .and_then(serde_json::Value::as_str)
            {
                type_names.push(single.to_owned());
            }
            let mut metrics = extract_string_list(&event.payload, "metrics");
            if let Some(single_m) = event
                .payload
                .get("metric")
                .and_then(serde_json::Value::as_str)
            {
                metrics.push(single_m.to_owned());
            }

            let node_ids = extract_node_ids(&event.payload);
            if type_names.is_empty()
                || (event.topic == OutboxTopic::NodeMetricsChanged && metrics.is_empty())
            {
                hydrate_node_deps_from_store(
                    store,
                    &event.tenant_id,
                    &node_ids,
                    &mut type_names,
                    &mut metrics,
                )
                .await?;
            }

            let deps = normalize_deps(WidgetDeps {
                type_names,
                metrics,
                ..WidgetDeps::default()
            });
            if deps.type_names.is_empty() && deps.metrics.is_empty() {
                return Ok(None);
            }
            Ok(Some((deps, None)))
        }
        OutboxTopic::AlertOpened | OutboxTopic::AlertResolved => Ok(Some((
            WidgetDeps {
                alerts: true,
                ..WidgetDeps::default()
            },
            Some(event.payload.clone()),
        ))),
        OutboxTopic::SimApplied => {
            let mut scenario_ids = Vec::new();
            if let Some(sc_id) = event
                .payload
                .get("scenario_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                scenario_ids.push(sc_id);
            }
            let mut type_names = Vec::new();
            let mut metrics = Vec::new();
            let node_ids = extract_node_ids(&event.payload);
            if !node_ids.is_empty() {
                hydrate_node_deps_from_store(
                    store,
                    &event.tenant_id,
                    &node_ids,
                    &mut type_names,
                    &mut metrics,
                )
                .await?;
            }
            let deps = normalize_deps(WidgetDeps {
                type_names,
                metrics,
                scenario_ids,
                ..WidgetDeps::default()
            });
            Ok(Some((deps, None)))
        }
        OutboxTopic::ApprovalDecided | OutboxTopic::ExportFinished => Ok(None),
    }
}

async fn hydrate_node_deps_from_store<G: GraphStorePort + ?Sized>(
    store: &G,
    tenant_id: &str,
    node_ids: &[Uuid],
    type_names: &mut Vec<String>,
    metrics: &mut Vec<String>,
) -> Result<(), AppError> {
    let need_types = type_names.is_empty();
    let need_metrics = metrics.is_empty();
    for nid in node_ids.iter().take(MAX_NODE_LOOKUPS_PER_EVENT) {
        if let Some(node) = store.get_node(tenant_id, *nid).await? {
            if need_types {
                type_names.push(node.type_name);
            }
            if need_metrics {
                metrics.extend(node.metrics.keys().cloned());
            }
        }
    }
    Ok(())
}

fn extract_node_ids(payload: &serde_json::Value) -> Vec<Uuid> {
    let mut ids = Vec::new();
    if let Some(arr) = payload
        .get("node_ids")
        .and_then(serde_json::Value::as_array)
    {
        for item in arr {
            if let Some(id) = item.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                ids.push(id);
            }
        }
    }
    for key in ["node_id", "deleted_node_id"] {
        if let Some(id) = payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

fn extract_string_list(payload: &serde_json::Value, key: &str) -> Vec<String> {
    payload
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map_or_else(Vec::new, |arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(String::from)
                .collect()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_compact_view_event_drops_widget_ids_over_7000_bytes() -> Result<(), AppError> {
        let many_widgets: Vec<Uuid> = (1..=300_u128).map(Uuid::from_u128).collect();
        let dash_id = Uuid::from_u128(999);
        let ev = ViewEvent {
            tenant_id: String::from("default"),
            topic: String::from("dataset.synced"),
            widget_ids: many_widgets,
            dashboard_ids: vec![dash_id],
            alert: None,
        };
        let full_len = serde_json::to_string(&ev)
            .map_err(|err| AppError::Database(err.to_string()))?
            .len();
        assert!(full_len > PG_NOTIFY_MAX_BYTES);

        let compact_str = serialize_compact_view_event(&ev)?;
        assert!(compact_str.len() <= PG_NOTIFY_MAX_BYTES);
        let decoded: ViewEvent = serde_json::from_str(&compact_str)
            .map_err(|err| AppError::Database(err.to_string()))?;
        assert_eq!(decoded.widget_ids, Vec::<Uuid>::new());
        assert_eq!(decoded.dashboard_ids, vec![dash_id]);
        Ok(())
    }
}
