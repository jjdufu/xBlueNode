//! `xBlueView` digital-twin dashboard, widget execution, `SSE` stream, spatial binding,
//! visual asset, full-format export, and `axs_` external share routes (`/api/xblueview/*`) (`10-STEP11-12` §11.7 & §12.6).

use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{
    AssetKind, AuditAction, ExportFormat, JobStatus, Permission, TokenKind,
};
use axpoint_domain::export::{
    CreateExportJobParams, CreateExportRequest, CreateExportTemplateParams, CreateShareRequest,
    CreateShareResponse, ExportJobRecord, ExportListQuery, ExportTemplateRecord, ShareLinkRecord,
    ShareListQuery, SharePublicResponse, ShareUnlockRequest, ShareUnlockResponse,
};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::ViewRepository as _;
use axpoint_domain::rules::SimplePageQuery;
use axpoint_domain::view::{
    AiGenerateDashboardRequest, AssetRecord, CreateAssetParams, CreateDashboardRequest,
    CreateWidgetRecordParams, CreateWidgetRequest, DashboardDetail, DashboardRecord,
    FloorPlacementRecord, InvokeWidgetActionRequest, InvokeWidgetActionResponse,
    PutFloorPlacementsRequest, PutScene3dBindingsRequest, Scene3dBindingRecord,
    UpdateDashboardRequest, UpdateLayoutRequest, UpdateWidgetRecordParams, UpdateWidgetRequest,
    ViewEvent, WidgetDataRequest, WidgetDataResponse, WidgetPreviewRequest, WidgetRecord,
};
use axpoint_security::AuthPrincipal;
use axpoint_server::view::action::invoke_widget_action;
use axpoint_server::view::ai_gen::{
    generate_ai_dashboard, validate_dashboard_slug, validate_widget_position,
};
use axpoint_server::view::data::{WidgetExecEnv, execute_widget_data, execute_widget_preview};
use axpoint_server::view::deps::compute_widget_deps;
use axpoint_server::view::export::{
    build_content_disposition, cleanup_file_quietly, content_type_for_export,
    extract_ooxml_placeholders, resolve_data_root_path, validate_create_export_request,
};
use axpoint_server::view::share::{
    PublicShareEnv, ShareRequestMeta, create_share, execute_shared_widget_data, get_public_share,
    prepare_shared_download, resolve_shared_dashboard_asset, unlock_public_share,
};
use axum::body::{Body, Bytes};
use axum::extract::{Multipart, State};
use axum::http::header::{
    CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderValue, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::broadcast;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};
use crate::telemetry::current_trace_id;

const MAX_VISUAL_ASSET_BYTES: usize = 20 * 1024 * 1024;
const MAX_MODEL_3D_BYTES: usize = 50 * 1024 * 1024;
const GLB_MAGIC_LE: u32 = 0x4654_6C67;
const GLB_VERSION_2: u32 = 2;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DuplicateDashboardRequest {
    name: Option<String>,
    slug: Option<String>,
}

/// Builds the `/xblueview/*` sub-router mounted under `/api` (`10-STEP11-12` §11.7 & §12.6).
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/xblueview/dashboards",
            get(list_dashboards_handler).post(create_dashboard_handler),
        )
        .route(
            "/xblueview/dashboards/ai-generate",
            post(ai_generate_dashboard_handler),
        )
        .route(
            "/xblueview/dashboards/by-slug/{slug}",
            get(get_dashboard_by_slug_handler),
        )
        .route(
            "/xblueview/dashboards/{id}",
            get(get_dashboard_handler)
                .put(update_dashboard_handler)
                .delete(delete_dashboard_handler),
        )
        .route(
            "/xblueview/dashboards/{id}/publish",
            post(publish_dashboard_handler),
        )
        .route(
            "/xblueview/dashboards/{id}/duplicate",
            post(duplicate_dashboard_handler),
        )
        .route(
            "/xblueview/dashboards/{id}/layout",
            put(update_layout_handler),
        )
        .route(
            "/xblueview/dashboards/{id}/widgets",
            post(create_widget_handler),
        )
        .route(
            "/xblueview/dashboards/{id}/stream",
            get(stream_dashboard_handler),
        )
        .merge(widget_and_asset_router())
        .merge(export_and_share_router())
}

fn widget_and_asset_router() -> Router<AppState> {
    Router::new()
        .route("/xblueview/widgets/preview", post(widget_preview_handler))
        .route(
            "/xblueview/widgets/{id}",
            put(update_widget_handler).delete(delete_widget_handler),
        )
        .route("/xblueview/widgets/{id}/data", post(widget_data_handler))
        .route(
            "/xblueview/widgets/{id}/invoke",
            post(invoke_widget_action_handler),
        )
        .route(
            "/xblueview/assets",
            get(list_assets_handler).post(upload_asset_handler),
        )
        .route("/xblueview/assets/{id}", delete(delete_asset_handler))
        .route(
            "/xblueview/assets/{id}/content",
            get(get_asset_content_handler),
        )
        .route(
            "/xblueview/assets/{id}/placements",
            get(list_floor_placements_handler).put(replace_floor_placements_handler),
        )
        .route(
            "/xblueview/assets/{id}/bindings3d",
            get(list_scene3d_bindings_handler).put(replace_scene3d_bindings_handler),
        )
}

#[derive(Debug, Serialize)]
struct DeleteStatus {
    deleted: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListDashboardsQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListAssetsQuery {
    asset_kind: Option<AssetKind>,
    limit: Option<u32>,
    cursor: Option<String>,
}

fn validate_page_limit(limit: Option<u32>) -> Result<(), AppError> {
    if let Some(val) = limit
        && !(1..=500).contains(&val)
    {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=500, got {val}"),
        ));
    }
    Ok(())
}

fn validate_refresh_seconds(secs: i32) -> Result<(), AppError> {
    if !(5..=86_400).contains(&secs) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("refresh_seconds must be in 5..=86400, got {secs}"),
        ));
    }
    Ok(())
}

fn actor_uuid(principal: &AuthPrincipal) -> Option<Uuid> {
    (!principal.user_id.is_nil()).then_some(principal.user_id)
}

fn parse_optional_json<T: Default + serde::de::DeserializeOwned>(
    body: &Bytes,
) -> Result<T, AppError> {
    if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice::<T>(body)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid JSON payload: {err}")))
}

// ============================================================================
// Dashboards
// ============================================================================

async fn list_dashboards_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ListDashboardsQuery>,
) -> Result<Json<PaginatedList<DashboardRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let only_published = !principal.has(Permission::ViewEdit);
    let page = state
        .store
        .list_dashboards(
            &principal.tenant_id,
            only_published,
            SimplePageQuery {
                limit: query.limit,
                cursor: query.cursor,
            },
        )
        .await?;
    Ok(Json(page))
}

async fn create_dashboard_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(mut req): ApiJson<CreateDashboardRequest>,
) -> Result<(StatusCode, Json<DashboardDetail>), HttpError> {
    principal.require(Permission::ViewEdit)?;
    let trimmed_name = req.name.trim();
    if trimmed_name.is_empty() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "dashboard name cannot be empty",
        )));
    }
    req.name = trimmed_name.to_owned();
    req.slug = req.slug.trim().to_owned();
    validate_dashboard_slug(&req.slug)?;

    let created = state
        .store
        .create_dashboard(&principal.tenant_id, actor_uuid(&principal), req)
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn ai_generate_dashboard_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<AiGenerateDashboardRequest>,
) -> Result<(StatusCode, Json<DashboardDetail>), HttpError> {
    principal.require(Permission::ViewEdit)?;
    let detail = generate_ai_dashboard(
        &state.store,
        &state.view.llm_router,
        &principal.tenant_id,
        Some(principal.user_id),
        actor_uuid(&principal),
        req,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn get_dashboard_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<DashboardDetail>, HttpError> {
    principal.require(Permission::DataRead)?;
    let require_published = !principal.has(Permission::ViewEdit);
    let detail = state
        .store
        .get_dashboard(&principal.tenant_id, id, require_published)
        .await?
        .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::Dashboard,
        &detail.dashboard.id.to_string(),
        &client_ip,
    )
    .await?;
    Ok(Json(detail))
}

async fn get_dashboard_by_slug_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
) -> Result<Json<DashboardDetail>, HttpError> {
    principal.require(Permission::DataRead)?;
    let trimmed = slug.trim();
    validate_dashboard_slug(trimmed)?;
    let require_published = !principal.has(Permission::ViewEdit);
    let detail = state
        .store
        .get_dashboard_by_slug(&principal.tenant_id, trimmed, require_published)
        .await?
        .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::Dashboard,
        &detail.dashboard.id.to_string(),
        &client_ip,
    )
    .await?;
    Ok(Json(detail))
}

async fn update_dashboard_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(mut req): ApiJson<UpdateDashboardRequest>,
) -> Result<Json<DashboardRecord>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    if let Some(ref name) = req.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "dashboard name cannot be empty",
            )));
        }
        req.name = Some(trimmed.to_owned());
    }
    if let Some(ref slug) = req.slug {
        let trimmed = slug.trim();
        validate_dashboard_slug(trimmed)?;
        req.slug = Some(trimmed.to_owned());
    }
    let updated = state
        .store
        .update_dashboard(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(updated))
}

async fn delete_dashboard_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    if let Some(detail) = state
        .store
        .get_dashboard(&principal.tenant_id, id, false)
        .await?
    {
        let widget_ids: Vec<Uuid> = detail.widgets.iter().map(|w| w.id).collect();
        state
            .view
            .cache
            .invalidate_widgets(&principal.tenant_id, &widget_ids)
            .await;
        state
            .view
            .cache
            .invalidate_dashboards(&principal.tenant_id, &[id])
            .await;
    }
    let deleted = state
        .store
        .delete_dashboard(&principal.tenant_id, id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("DASHBOARD_NOT_FOUND")));
    }
    Ok(Json(DeleteStatus { deleted }))
}

async fn publish_dashboard_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<DashboardRecord>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    let published = state
        .store
        .publish_dashboard(&principal.tenant_id, id)
        .await?;
    Ok(Json(published))
}

async fn duplicate_dashboard_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<(StatusCode, Json<DashboardDetail>), HttpError> {
    principal.require(Permission::ViewEdit)?;
    let req: DuplicateDashboardRequest = parse_optional_json(&body)?;
    if let Some(ref slug) = req.slug {
        validate_dashboard_slug(slug.trim())?;
    }
    let duplicated = state
        .store
        .duplicate_dashboard(
            &principal.tenant_id,
            id,
            req.name.map(|n| n.trim().to_owned()),
            req.slug.map(|s| s.trim().to_owned()),
            actor_uuid(&principal),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(duplicated)))
}

async fn update_layout_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateLayoutRequest>,
) -> Result<Json<Vec<WidgetRecord>>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    for item in &req.widgets {
        validate_widget_position(&item.position)?;
    }
    let widgets = state
        .store
        .update_dashboard_layout(&principal.tenant_id, id, &req.widgets)
        .await?;
    Ok(Json(widgets))
}

// ============================================================================
// Widgets & Execution
// ============================================================================

async fn create_widget_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(dashboard_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateWidgetRequest>,
) -> Result<(StatusCode, Json<WidgetRecord>), HttpError> {
    principal.require(Permission::ViewEdit)?;
    validate_widget_position(&req.position)?;
    validate_refresh_seconds(req.refresh_seconds)?;

    let deps = compute_widget_deps(&state.store, &principal.tenant_id, &req.data_source).await?;
    let widget = state
        .store
        .create_widget(
            &principal.tenant_id,
            CreateWidgetRecordParams {
                dashboard_id,
                widget_kind: req.widget_kind,
                title: req.title,
                position: req.position,
                data_source: req.data_source,
                encoding: req.encoding,
                options: req.options,
                drill: req.drill,
                refresh_mode: req.refresh_mode,
                refresh_seconds: req.refresh_seconds,
                deps,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(widget)))
}

async fn update_widget_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(widget_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateWidgetRequest>,
) -> Result<Json<WidgetRecord>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    if let Some(ref pos) = req.position {
        validate_widget_position(pos)?;
    }
    if let Some(secs) = req.refresh_seconds {
        validate_refresh_seconds(secs)?;
    }
    let deps = match req.data_source {
        Some(ref ds) => Some(compute_widget_deps(&state.store, &principal.tenant_id, ds).await?),
        None => None,
    };

    let updated = state
        .store
        .update_widget(
            &principal.tenant_id,
            widget_id,
            UpdateWidgetRecordParams {
                widget_kind: req.widget_kind,
                title: req.title,
                position: req.position,
                data_source: req.data_source,
                encoding: req.encoding,
                options: req.options,
                drill: req.drill,
                refresh_mode: req.refresh_mode,
                refresh_seconds: req.refresh_seconds,
                deps,
            },
        )
        .await?;
    state
        .view
        .cache
        .invalidate_widgets(&principal.tenant_id, &[widget_id])
        .await;
    Ok(Json(updated))
}

async fn delete_widget_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(widget_id): ApiPath<Uuid>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    let deleted = state
        .store
        .delete_widget(&principal.tenant_id, widget_id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("WIDGET_NOT_FOUND")));
    }
    state
        .view
        .cache
        .invalidate_widgets(&principal.tenant_id, &[widget_id])
        .await;
    Ok(Json(DeleteStatus { deleted }))
}

async fn widget_data_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(widget_id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<Json<WidgetDataResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let req: WidgetDataRequest = parse_optional_json(&body)?;

    let widget = state
        .store
        .get_widget(&principal.tenant_id, widget_id)
        .await?
        .ok_or(AppError::NotFound("WIDGET_NOT_FOUND"))?;

    let require_published = !principal.has(Permission::ViewEdit);
    let detail = state
        .store
        .get_dashboard(&principal.tenant_id, widget.dashboard_id, require_published)
        .await?
        .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;

    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::Dashboard,
        &detail.dashboard.id.to_string(),
        &client_ip,
    )
    .await?;
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;

    let env = WidgetExecEnv {
        store: &state.store,
        analytics_pool: &state.analytics_pool,
        cache: &state.view.cache,
        principal: &principal,
        policy: Some(&policy),
    };
    let resp = execute_widget_data(&env, &detail.dashboard, &widget, &req).await?;
    Ok(Json(resp))
}

async fn widget_preview_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<WidgetPreviewRequest>,
) -> Result<Json<WidgetDataResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let env = WidgetExecEnv {
        store: &state.store,
        analytics_pool: &state.analytics_pool,
        cache: &state.view.cache,
        principal: &principal,
        policy: Some(&policy),
    };
    let resp = execute_widget_preview(&env, &req).await?;
    Ok(Json(resp))
}

async fn invoke_widget_action_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(widget_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<InvokeWidgetActionRequest>,
) -> Result<Json<InvokeWidgetActionResponse>, HttpError> {
    if principal.token_kind == TokenKind::Share {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    principal.require(Permission::FlowRun)?;
    let resp =
        invoke_widget_action(&state.store, &state.view.cache, &principal, widget_id, req).await?;
    Ok(Json(resp))
}

// ============================================================================
// Real-time SSE Stream
// ============================================================================

struct SseStreamState {
    rx: broadcast::Receiver<ViewEvent>,
    tenant_id: String,
    dashboard_id: Uuid,
    widget_ids: HashSet<Uuid>,
    pending: VecDeque<Event>,
}

async fn stream_dashboard_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(dashboard_id): ApiPath<Uuid>,
) -> Result<Response, HttpError> {
    principal.require(Permission::DataRead)?;
    let require_published = !principal.has(Permission::ViewEdit);
    let detail = state
        .store
        .get_dashboard(&principal.tenant_id, dashboard_id, require_published)
        .await?
        .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::Dashboard,
        &detail.dashboard.id.to_string(),
        &client_ip,
    )
    .await?;

    let widget_ids: HashSet<Uuid> = detail.widgets.iter().map(|w| w.id).collect();
    let stream_state = SseStreamState {
        rx: state.view.event_bus.subscribe(),
        tenant_id: principal.tenant_id,
        dashboard_id,
        widget_ids,
        pending: VecDeque::new(),
    };

    let stream = futures::stream::unfold(stream_state, |mut st| async move {
        loop {
            if let Some(ev) = st.pending.pop_front() {
                let item: Result<Event, Infallible> = Ok(ev);
                return Some((item, st));
            }
            match st.rx.recv().await {
                Ok(view_ev) => enqueue_matching_sse_events(&mut st, view_ev),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}

fn enqueue_matching_sse_events(st: &mut SseStreamState, ev: ViewEvent) {
    if ev.tenant_id != st.tenant_id {
        return;
    }
    let dash_matched = ev.dashboard_ids.contains(&st.dashboard_id);
    let matching_widgets: Vec<Uuid> = if ev.widget_ids.is_empty() && dash_matched {
        st.widget_ids.iter().copied().collect()
    } else if dash_matched {
        ev.widget_ids.clone()
    } else {
        ev.widget_ids
            .iter()
            .copied()
            .filter(|wid| st.widget_ids.contains(wid))
            .collect()
    };

    if !dash_matched && matching_widgets.is_empty() {
        return;
    }

    if !matching_widgets.is_empty() {
        let data = serde_json::json!({ "widget_ids": matching_widgets }).to_string();
        st.pending
            .push_back(Event::default().event("invalidate").data(data));
    }
    if let Some(alert) = ev.alert {
        st.pending
            .push_back(Event::default().event("alert").data(alert.to_string()));
    }
}

// ============================================================================
// Visual Assets, Floor Placements, and 3D Bindings
// ============================================================================

async fn list_assets_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ListAssetsQuery>,
) -> Result<Json<PaginatedList<AssetRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let page = state
        .store
        .list_assets(
            &principal.tenant_id,
            query.asset_kind,
            SimplePageQuery {
                limit: query.limit,
                cursor: query.cursor,
            },
        )
        .await?;
    Ok(Json(page))
}

struct ParsedAssetUpload {
    asset_kind: AssetKind,
    name: String,
    raw_bytes: Vec<u8>,
}

async fn read_asset_multipart(mut multipart: Multipart) -> Result<ParsedAssetUpload, AppError> {
    let mut asset_kind_raw: Option<String> = None;
    let mut name: Option<String> = None;
    let mut file_name: Option<String> = None;
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| map_multipart_error(&e))?
    {
        let field_name = field.name().unwrap_or_default().to_owned();
        if field_name == "tenant_id" {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "tenant_id parameter is forbidden",
            ));
        }
        if field_name == "file" {
            if let Some(orig) = field.file_name() {
                let clean = orig.rsplit(['/', '\\']).next().unwrap_or(orig).trim();
                if !clean.is_empty() {
                    file_name = Some(clean.to_owned());
                }
            }
            let bytes = field.bytes().await.map_err(|e| map_multipart_error(&e))?;
            file_bytes = Some(bytes.to_vec());
            continue;
        }
        let text = field.text().await.map_err(|e| map_multipart_error(&e))?;
        match field_name.as_str() {
            "asset_kind" | "kind" => asset_kind_raw = Some(text.trim().to_owned()),
            "name" => name = Some(text.trim().to_owned()),
            _ => {}
        }
    }

    let kind_str = asset_kind_raw
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "missing 'asset_kind' field"))?;
    let asset_kind: AssetKind = kind_str.parse()?;
    let raw_bytes =
        file_bytes.ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "missing 'file' field"))?;
    if raw_bytes.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "uploaded asset file cannot be empty",
        ));
    }

    let resolved_name = name
        .filter(|s| !s.is_empty())
        .or(file_name)
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "missing asset 'name'"))?;

    Ok(ParsedAssetUpload {
        asset_kind,
        name: resolved_name,
        raw_bytes,
    })
}

fn map_multipart_error(err: &axum::extract::multipart::MultipartError) -> AppError {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        AppError::PayloadTooLarge("UPLOAD_TOO_LARGE")
    } else {
        AppError::bad("INVALID_ARGUMENT", format!("multipart error: {err}"))
    }
}

struct ValidatedAsset {
    bytes: Vec<u8>,
    mime_type: &'static str,
    ext: &'static str,
    width_px: Option<i32>,
    height_px: Option<i32>,
}

fn validate_and_normalize_asset(
    kind: AssetKind,
    raw_bytes: Vec<u8>,
) -> Result<ValidatedAsset, AppError> {
    let max_bytes = match kind {
        AssetKind::Model3d => MAX_MODEL_3D_BYTES,
        AssetKind::FloorPlanImage | AssetKind::Geojson | AssetKind::Icon => MAX_VISUAL_ASSET_BYTES,
    };
    if raw_bytes.len() > max_bytes {
        return Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"));
    }

    match kind {
        AssetKind::FloorPlanImage | AssetKind::Icon => validate_image_or_svg(raw_bytes),
        AssetKind::Geojson => validate_geojson(raw_bytes),
        AssetKind::Model3d => validate_glb(raw_bytes),
    }
}

fn validate_image_or_svg(raw_bytes: Vec<u8>) -> Result<ValidatedAsset, AppError> {
    if raw_bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let (width_px, height_px) = parse_png_dimensions(&raw_bytes);
        return Ok(ValidatedAsset {
            bytes: raw_bytes,
            mime_type: "image/png",
            ext: "png",
            width_px,
            height_px,
        });
    }
    if raw_bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok(ValidatedAsset {
            bytes: raw_bytes,
            mime_type: "image/jpeg",
            ext: "jpg",
            width_px: None,
            height_px: None,
        });
    }
    if raw_bytes.get(0..4) == Some(b"RIFF") && raw_bytes.get(8..12) == Some(b"WEBP") {
        return Ok(ValidatedAsset {
            bytes: raw_bytes,
            mime_type: "image/webp",
            ext: "webp",
            width_px: None,
            height_px: None,
        });
    }
    let text =
        std::str::from_utf8(&raw_bytes).map_err(|_| AppError::Unsupported("UNSUPPORTED_FORMAT"))?;
    let trimmed_lower = text.trim_start().to_ascii_lowercase();
    let looks_like_xml_or_svg = trimmed_lower.contains("<svg")
        || trimmed_lower.starts_with("<!doctype")
        || trimmed_lower.starts_with("<?xml");
    if !looks_like_xml_or_svg {
        return Err(AppError::Unsupported("UNSUPPORTED_FORMAT"));
    }
    let sanitized = sanitize_svg(text)?;
    Ok(ValidatedAsset {
        bytes: sanitized.into_bytes(),
        mime_type: "image/svg+xml",
        ext: "svg",
        width_px: None,
        height_px: None,
    })
}

fn parse_png_dimensions(bytes: &[u8]) -> (Option<i32>, Option<i32>) {
    if bytes.get(12..16) != Some(b"IHDR") {
        return (None, None);
    }
    let Some(w_bytes) = bytes.get(16..20) else {
        return (None, None);
    };
    let Some(h_bytes) = bytes.get(20..24) else {
        return (None, None);
    };
    let Ok(w_arr) = <[u8; 4]>::try_from(w_bytes) else {
        return (None, None);
    };
    let Ok(h_arr) = <[u8; 4]>::try_from(h_bytes) else {
        return (None, None);
    };
    let w = i32::try_from(u32::from_be_bytes(w_arr)).ok();
    let h = i32::try_from(u32::from_be_bytes(h_arr)).ok();
    (w, h)
}

fn sanitize_svg(raw: &str) -> Result<String, AppError> {
    svg_sanitizer::sanitize_svg_document(raw)
}

mod svg_sanitizer {
    use axpoint_domain::AppError;

    const ALLOWED_SVG_TAGS: &[&str] = &[
        "svg",
        "g",
        "defs",
        "symbol",
        "use",
        "path",
        "rect",
        "circle",
        "ellipse",
        "line",
        "polyline",
        "polygon",
        "text",
        "tspan",
        "title",
        "desc",
        "linearGradient",
        "radialGradient",
        "stop",
        "clipPath",
        "mask",
        "pattern",
        "filter",
        "feGaussianBlur",
        "feOffset",
        "feBlend",
        "feColorMatrix",
        "feMerge",
        "feMergeNode",
        "style",
        "image",
    ];

    const ALLOWED_SVG_ATTRS: &[&str] = &[
        "id",
        "class",
        "style",
        "xmlns",
        "xmlns:xlink",
        "xml:space",
        "version",
        "viewBox",
        "preserveAspectRatio",
        "lang",
        "x",
        "y",
        "x1",
        "y1",
        "x2",
        "y2",
        "cx",
        "cy",
        "r",
        "rx",
        "ry",
        "fx",
        "fy",
        "fr",
        "width",
        "height",
        "d",
        "points",
        "dx",
        "dy",
        "rotate",
        "pathLength",
        "transform",
        "gradientUnits",
        "gradientTransform",
        "patternUnits",
        "patternContentUnits",
        "patternTransform",
        "clipPathUnits",
        "maskUnits",
        "maskContentUnits",
        "filterUnits",
        "primitiveUnits",
        "spreadMethod",
        "offset",
        "fill",
        "fill-opacity",
        "fill-rule",
        "stroke",
        "stroke-width",
        "stroke-opacity",
        "stroke-linecap",
        "stroke-linejoin",
        "stroke-dasharray",
        "stroke-dashoffset",
        "stroke-miterlimit",
        "opacity",
        "color",
        "stop-color",
        "stop-opacity",
        "flood-color",
        "flood-opacity",
        "lighting-color",
        "display",
        "visibility",
        "overflow",
        "clip-path",
        "clip-rule",
        "mask",
        "filter",
        "marker-start",
        "marker-mid",
        "marker-end",
        "vector-effect",
        "shape-rendering",
        "image-rendering",
        "color-interpolation",
        "color-interpolation-filters",
        "paint-order",
        "mix-blend-mode",
        "isolation",
        "font-family",
        "font-size",
        "font-weight",
        "font-style",
        "font-variant",
        "text-anchor",
        "dominant-baseline",
        "alignment-baseline",
        "letter-spacing",
        "word-spacing",
        "text-decoration",
        "textLength",
        "lengthAdjust",
        "writing-mode",
        "in",
        "in2",
        "result",
        "stdDeviation",
        "mode",
        "type",
        "values",
        "operator",
        "k1",
        "k2",
        "k3",
        "k4",
        "edgeMode",
        "href",
        "xlink:href",
        "src",
        "role",
        "aria-label",
        "aria-hidden",
    ];

    pub(super) fn sanitize_svg_document(raw: &str) -> Result<String, AppError> {
        let mut scanner = SvgScanner {
            src: raw,
            pos: 0,
            out: String::with_capacity(raw.len()),
            open_stack: Vec::new(),
            skip_stack: Vec::new(),
            style_buf: String::new(),
            saw_svg_root: false,
        };
        scanner.run()?;
        if !scanner.open_stack.is_empty() || !scanner.skip_stack.is_empty() || !scanner.saw_svg_root
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "invalid or unclosed SVG XML structure",
            ));
        }
        Ok(scanner.out)
    }

    struct SvgScanner<'a> {
        src: &'a str,
        pos: usize,
        out: String,
        open_stack: Vec<&'static str>,
        skip_stack: Vec<String>,
        style_buf: String,
        saw_svg_root: bool,
    }

    impl<'a> SvgScanner<'a> {
        fn run(&mut self) -> Result<(), AppError> {
            while self.pos < self.src.len() {
                if self.peek_byte() == Some(b'<') {
                    self.handle_markup()?;
                } else {
                    self.handle_text();
                }
            }
            Ok(())
        }

        fn rest(&self) -> &'a str {
            self.src.get(self.pos..).unwrap_or_default()
        }

        fn peek_byte(&self) -> Option<u8> {
            self.src.as_bytes().get(self.pos).copied()
        }

        fn consume_byte(&mut self, expected: u8) -> bool {
            if self.peek_byte() == Some(expected) {
                self.pos = self.pos.saturating_add(1);
                true
            } else {
                false
            }
        }

        fn skip_ws(&mut self) {
            while self.peek_byte().is_some_and(|b| b.is_ascii_whitespace()) {
                self.pos = self.pos.saturating_add(1);
            }
        }

        fn starts_with_ci(&self, pat: &str) -> bool {
            self.rest()
                .get(..pat.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(pat))
        }

        fn in_style_tag(&self) -> bool {
            self.open_stack.last().copied() == Some("style")
        }

        fn allows_text_content(&self) -> bool {
            matches!(
                self.open_stack.last().copied(),
                Some("text" | "tspan" | "title" | "desc")
            )
        }

        fn handle_markup(&mut self) -> Result<(), AppError> {
            let rem = self.rest();
            if rem.starts_with("<!--") {
                let end = rem
                    .get(4..)
                    .and_then(|s| s.find("-->"))
                    .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "unclosed XML comment"))?;
                self.pos = self
                    .pos
                    .saturating_add(4)
                    .saturating_add(end)
                    .saturating_add(3);
                return Ok(());
            }
            if self.starts_with_ci("<![CDATA[") {
                let after = rem.get(9..).unwrap_or_default();
                let end = after
                    .find("]]>")
                    .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "unclosed CDATA section"))?;
                let cdata = after.get(..end).unwrap_or_default();
                if self.skip_stack.is_empty() {
                    if self.in_style_tag() {
                        self.style_buf.push_str(cdata);
                    } else if self.allows_text_content() {
                        push_xml_escaped(&mut self.out, cdata);
                    }
                }
                self.pos = self
                    .pos
                    .saturating_add(9)
                    .saturating_add(end)
                    .saturating_add(3);
                return Ok(());
            }
            if rem.starts_with("<!") {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "SVG DOCTYPE and ENTITY declarations are forbidden",
                ));
            }
            if rem.starts_with("<?") {
                let end = rem.get(2..).and_then(|s| s.find("?>")).ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "unclosed XML processing instruction")
                })?;
                self.pos = self
                    .pos
                    .saturating_add(2)
                    .saturating_add(end)
                    .saturating_add(2);
                return Ok(());
            }
            if rem.starts_with("</") {
                return self.handle_close_tag();
            }
            self.handle_open_tag()
        }

        fn handle_text(&mut self) {
            let rem = self.rest();
            let len = rem.find('<').unwrap_or(rem.len());
            let chunk = rem.get(..len).unwrap_or_default();
            self.pos = self.pos.saturating_add(len);
            if !self.skip_stack.is_empty() {
                return;
            }
            if self.in_style_tag() {
                self.style_buf.push_str(chunk);
            } else if self.allows_text_content() {
                let decoded = decode_html_entities(chunk);
                push_xml_escaped(&mut self.out, &decoded);
            }
        }

        fn handle_close_tag(&mut self) -> Result<(), AppError> {
            self.pos = self.pos.saturating_add(2);
            let raw_name = self.read_xml_name();
            if raw_name.is_empty() {
                return Err(AppError::bad("INVALID_ARGUMENT", "empty closing tag name"));
            }
            self.skip_ws();
            if !self.consume_byte(b'>') {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "malformed closing SVG tag",
                ));
            }
            let lower = raw_name.to_ascii_lowercase();
            if !self.skip_stack.is_empty() {
                if let Some(idx) = self.skip_stack.iter().rposition(|s| s == &lower) {
                    self.skip_stack.truncate(idx);
                }
                return Ok(());
            }
            let canon = canonical_svg_tag(&raw_name).ok_or_else(|| {
                AppError::bad("INVALID_ARGUMENT", "closing non-whitelisted SVG tag")
            })?;
            if self.open_stack.pop() != Some(canon) {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "mismatched closing SVG tag",
                ));
            }
            if canon == "style" {
                let clean_css = sanitize_css_stylesheet(&self.style_buf);
                self.style_buf.clear();
                self.out.push_str(&clean_css);
            }
            self.out.push_str("</");
            self.out.push_str(canon);
            self.out.push('>');
            Ok(())
        }

        fn handle_open_tag(&mut self) -> Result<(), AppError> {
            self.pos = self.pos.saturating_add(1);
            let raw_name = self.read_xml_name();
            if raw_name.is_empty() {
                return Err(AppError::bad("INVALID_ARGUMENT", "malformed SVG tag name"));
            }
            let (attrs, self_closing) = self.read_tag_attributes()?;

            if !self.skip_stack.is_empty() {
                if !self_closing {
                    self.skip_stack.push(raw_name.to_ascii_lowercase());
                }
                return Ok(());
            }

            let Some(canon_tag) = canonical_svg_tag(&raw_name) else {
                if !self_closing {
                    self.skip_stack.push(raw_name.to_ascii_lowercase());
                }
                return Ok(());
            };

            if canon_tag == "svg" {
                self.saw_svg_root = true;
            }

            self.out.push('<');
            self.out.push_str(canon_tag);
            for (attr_name, attr_val) in attrs {
                if let Some((canon_attr, clean_val)) =
                    sanitize_svg_attribute(canon_tag, &attr_name, &attr_val)?
                {
                    self.out.push(' ');
                    self.out.push_str(canon_attr);
                    self.out.push_str("=\"");
                    push_xml_escaped(&mut self.out, &clean_val);
                    self.out.push('"');
                }
            }
            if self_closing && canon_tag != "svg" && canon_tag != "style" {
                self.out.push_str("/>");
            } else {
                self.out.push('>');
                if self_closing {
                    self.out.push_str("</");
                    self.out.push_str(canon_tag);
                    self.out.push('>');
                } else {
                    self.open_stack.push(canon_tag);
                }
            }
            Ok(())
        }

        fn read_xml_name(&mut self) -> String {
            let rem = self.rest();
            let mut end = 0_usize;
            for (idx, ch) in rem.char_indices() {
                let ok = if idx == 0 {
                    ch.is_ascii_alphabetic() || ch == '_' || ch == ':'
                } else {
                    ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '-' | '.')
                };
                if ok {
                    end = idx.saturating_add(ch.len_utf8());
                } else {
                    break;
                }
            }
            let out = rem.get(..end).unwrap_or_default().to_owned();
            self.pos = self.pos.saturating_add(end);
            out
        }

        fn read_tag_attributes(&mut self) -> Result<(Vec<(String, String)>, bool), AppError> {
            let mut attrs = Vec::new();
            loop {
                self.skip_ws();
                if self.rest().starts_with("/>") {
                    self.pos = self.pos.saturating_add(2);
                    return Ok((attrs, true));
                }
                if self.consume_byte(b'>') {
                    return Ok((attrs, false));
                }
                let name = self.read_xml_name();
                if name.is_empty() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "malformed attribute token inside SVG tag",
                    ));
                }
                self.skip_ws();
                let val = if self.consume_byte(b'=') {
                    self.skip_ws();
                    self.read_attr_value()?
                } else {
                    String::new()
                };
                attrs.push((name, val));
            }
        }

        fn read_attr_value(&mut self) -> Result<String, AppError> {
            let Some(first) = self.peek_byte() else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "unexpected EOF reading SVG attribute value",
                ));
            };
            if first == b'"' || first == b'\'' {
                let quote = char::from(first);
                self.pos = self.pos.saturating_add(1);
                let rem = self.rest();
                let end = rem.find(quote).ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "unclosed quote in SVG attribute")
                })?;
                let val = rem.get(..end).unwrap_or_default().to_owned();
                self.pos = self.pos.saturating_add(end).saturating_add(1);
                return Ok(val);
            }
            let rem = self.rest();
            let end = rem
                .find(|c: char| {
                    c.is_ascii_whitespace() || matches!(c, '>' | '/' | '"' | '\'' | '<')
                })
                .unwrap_or(rem.len());
            if end == 0 {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "empty unquoted SVG attribute value",
                ));
            }
            let val = rem.get(..end).unwrap_or_default().to_owned();
            self.pos = self.pos.saturating_add(end);
            Ok(val)
        }
    }

    fn canonical_svg_tag(raw: &str) -> Option<&'static str> {
        ALLOWED_SVG_TAGS
            .iter()
            .copied()
            .find(|allowed| allowed.eq_ignore_ascii_case(raw))
    }

    fn canonical_svg_attr(raw: &str) -> Option<&'static str> {
        if raw.len() >= 2 && raw.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("on")) {
            return None;
        }
        ALLOWED_SVG_ATTRS
            .iter()
            .copied()
            .find(|allowed| allowed.eq_ignore_ascii_case(raw))
    }

    fn sanitize_svg_attribute<'a>(
        canon_tag: &str,
        raw_attr: &'a str,
        raw_val: &str,
    ) -> Result<Option<(&'a str, String)>, AppError> {
        let resolved_name: Option<&str> = if let Some(canon) = canonical_svg_attr(raw_attr) {
            Some(canon)
        } else if is_safe_data_attr(raw_attr) {
            Some(raw_attr)
        } else {
            None
        };
        let Some(attr_name) = resolved_name else {
            return Ok(None);
        };

        let decoded = decode_html_entities(raw_val);
        let trimmed = decoded.trim();

        if attr_name.eq_ignore_ascii_case("xmlns") {
            return Ok((trimmed == "http://www.w3.org/2000/svg")
                .then(|| (attr_name, String::from("http://www.w3.org/2000/svg"))));
        }
        if attr_name.eq_ignore_ascii_case("xmlns:xlink") {
            return Ok((trimmed == "http://www.w3.org/1999/xlink")
                .then(|| (attr_name, String::from("http://www.w3.org/1999/xlink"))));
        }
        if matches!(attr_name, "href" | "xlink:href" | "src") {
            validate_svg_reference_uri(canon_tag, trimmed)?;
            return Ok(Some((attr_name, trimmed.to_owned())));
        }
        if attr_name == "style" {
            let clean_inline = sanitize_css_inline(trimmed);
            if clean_inline.is_empty() {
                return Ok(None);
            }
            return Ok(Some((attr_name, clean_inline)));
        }

        validate_presentation_value(trimmed)?;
        Ok(Some((attr_name, trimmed.to_owned())))
    }

    fn is_safe_data_attr(raw: &str) -> bool {
        let Some(suffix) = raw.strip_prefix("data-") else {
            return false;
        };
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    }

    fn validate_svg_reference_uri(canon_tag: &str, decoded_trimmed: &str) -> Result<(), AppError> {
        if is_safe_fragment_ref(decoded_trimmed) {
            return Ok(());
        }
        if canon_tag == "image" && is_safe_raster_data_uri(decoded_trimmed) {
            return Ok(());
        }
        Err(AppError::bad(
            "INVALID_ARGUMENT",
            "SVG href/xlink:href/src allows only #fragment or safe raster data: URIs",
        ))
    }

    fn is_safe_fragment_ref(val: &str) -> bool {
        let Some(rest) = val.strip_prefix('#') else {
            return false;
        };
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
    }

    fn is_safe_raster_data_uri(val: &str) -> bool {
        let lower = val.to_ascii_lowercase();
        let payload_len = [
            "data:image/png;base64,",
            "data:image/jpeg;base64,",
            "data:image/webp;base64,",
        ]
        .iter()
        .find_map(|prefix| lower.starts_with(prefix).then_some(prefix.len()));
        let Some(prefix_len) = payload_len else {
            return false;
        };
        let b64 = val.get(prefix_len..).unwrap_or_default().trim();
        !b64.is_empty()
            && b64
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    }

    fn validate_presentation_value(val: &str) -> Result<(), AppError> {
        let unescaped = decode_css_escapes(val);
        let compact: String = unescaped
            .chars()
            .filter(|c| !c.is_control() && !c.is_ascii_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        for bad in [
            "javascript:",
            "vbscript:",
            "data:text/html",
            "expression(",
            "@import",
        ] {
            if compact.contains(bad) {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("SVG attribute contains forbidden token '{bad}'"),
                ));
            }
        }
        if compact.contains("url(") && !all_css_urls_are_fragments(&unescaped) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "SVG presentation attribute contains non-fragment url(...)",
            ));
        }
        Ok(())
    }

    fn sanitize_css_stylesheet(raw_css: &str) -> String {
        let without_comments = strip_css_comments(raw_css);
        let decoded = decode_css_escapes(&decode_html_entities(&without_comments));
        let mut out = String::new();
        let mut idx = 0_usize;

        while idx < decoded.len() {
            let rem = decoded.get(idx..).unwrap_or_default();
            let ws_skip = rem.len().saturating_sub(rem.trim_start().len());
            idx = idx.saturating_add(ws_skip);
            let rem = decoded.get(idx..).unwrap_or_default();
            if rem.is_empty() {
                break;
            }
            if rem.starts_with('@') {
                idx = idx.saturating_add(skip_at_rule(rem));
                continue;
            }
            let Some(open_brace) = rem.find('{') else {
                break;
            };
            let selector = rem.get(..open_brace).unwrap_or_default().trim();
            let after_open = rem.get(open_brace.saturating_add(1)..).unwrap_or_default();
            let Some(close_brace) = after_open.find('}') else {
                break;
            };
            let body = after_open.get(..close_brace).unwrap_or_default();
            idx = idx
                .saturating_add(open_brace)
                .saturating_add(1)
                .saturating_add(close_brace)
                .saturating_add(1);

            if is_safe_css_selector(selector) {
                let clean_decls = sanitize_css_inline(body);
                if !clean_decls.is_empty() {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(selector);
                    out.push_str(" { ");
                    out.push_str(&clean_decls);
                    out.push_str(" }");
                }
            }
        }
        out
    }

    fn skip_at_rule(rem: &str) -> usize {
        let semi = rem.find(';');
        let brace = rem.find('{');
        match (semi, brace) {
            (Some(s), None) => s.saturating_add(1),
            (Some(s), Some(b)) if s < b => s.saturating_add(1),
            (_, Some(b)) => {
                let mut depth = 0_usize;
                for (i, ch) in rem.char_indices() {
                    if i < b {
                        continue;
                    }
                    if ch == '{' {
                        depth = depth.saturating_add(1);
                    } else if ch == '}' {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return i.saturating_add(1);
                        }
                    }
                }
                rem.len()
            }
            (None, None) => rem.len(),
        }
    }

    fn is_safe_css_selector(sel: &str) -> bool {
        !sel.is_empty()
            && !sel.contains('<')
            && !sel.contains('>')
            && !sel.contains('@')
            && !sel.contains('\\')
    }

    fn sanitize_css_inline(raw_inline: &str) -> String {
        let no_comments = strip_css_comments(raw_inline);
        let decoded = decode_css_escapes(&decode_html_entities(&no_comments));
        let mut kept = Vec::new();
        for decl in decoded.split(';') {
            let trimmed = decl.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((prop_raw, val_raw)) = trimmed.split_once(':') else {
                continue;
            };
            let prop = prop_raw.trim();
            let val = val_raw.trim();
            if !is_safe_css_prop(prop) || val.is_empty() || val.contains('<') || val.contains('>') {
                continue;
            }
            let compact: String = val
                .chars()
                .filter(|c| !c.is_control() && !c.is_ascii_whitespace())
                .flat_map(char::to_lowercase)
                .collect();
            if compact.contains("@import")
                || compact.contains("expression(")
                || compact.contains("javascript:")
                || compact.contains("vbscript:")
                || compact.contains("behavior:")
                || compact.contains("-moz-binding")
            {
                continue;
            }
            if compact.contains("url(") && !all_css_urls_are_fragments(val) {
                continue;
            }
            kept.push(format!("{prop}: {val}"));
        }
        kept.join("; ")
    }

    fn is_safe_css_prop(prop: &str) -> bool {
        !prop.is_empty()
            && !prop.eq_ignore_ascii_case("behavior")
            && !prop.eq_ignore_ascii_case("-moz-binding")
            && prop
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    fn all_css_urls_are_fragments(val: &str) -> bool {
        let lower = val.to_ascii_lowercase();
        let mut offset = 0_usize;
        while let Some(rel) = lower.get(offset..).and_then(|s| s.find("url(")) {
            let start = offset.saturating_add(rel).saturating_add(4);
            let Some(after) = val.get(start..) else {
                return false;
            };
            let Some(close_rel) = after.find(')') else {
                return false;
            };
            let inner = after
                .get(..close_rel)
                .unwrap_or_default()
                .trim()
                .trim_matches(['"', '\''])
                .trim();
            if !is_safe_fragment_ref(inner) {
                return false;
            }
            offset = start.saturating_add(close_rel).saturating_add(1);
        }
        true
    }

    fn strip_css_comments(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find("/*") {
            if let Some(prefix) = rest.get(..start) {
                out.push_str(prefix);
            }
            let after = rest.get(start.saturating_add(2)..).unwrap_or_default();
            if let Some(end) = after.find("*/") {
                rest = after.get(end.saturating_add(2)..).unwrap_or_default();
            } else {
                return out;
            }
        }
        out.push_str(rest);
        out
    }

    fn decode_css_escapes(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut idx = 0_usize;
        while idx < input.len() {
            let rem = input.get(idx..).unwrap_or_default();
            let Some(ch) = rem.chars().next() else {
                break;
            };
            if ch != '\\' {
                out.push(ch);
                idx = idx.saturating_add(ch.len_utf8());
                continue;
            }
            let after_bs = rem.get(1..).unwrap_or_default();
            let hex_len = after_bs
                .bytes()
                .take(6)
                .take_while(u8::is_ascii_hexdigit)
                .count();
            if hex_len > 0 {
                let hex_str = after_bs.get(..hex_len).unwrap_or_default();
                if let Ok(code) = u32::from_str_radix(hex_str, 16)
                    && let Some(decoded_ch) = char::from_u32(code)
                {
                    out.push(decoded_ch);
                }
                let mut skip = 1_usize.saturating_add(hex_len);
                if after_bs.as_bytes().get(hex_len) == Some(&b' ') {
                    skip = skip.saturating_add(1);
                }
                idx = idx.saturating_add(skip);
            } else if let Some(escaped_ch) = after_bs.chars().next() {
                out.push(escaped_ch);
                idx = idx.saturating_add(1).saturating_add(escaped_ch.len_utf8());
            } else {
                idx = idx.saturating_add(1);
            }
        }
        out
    }

    fn decode_html_entities(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut idx = 0_usize;
        while idx < input.len() {
            let rem = input.get(idx..).unwrap_or_default();
            let Some(ch) = rem.chars().next() else {
                break;
            };
            if ch != '&' {
                out.push(ch);
                idx = idx.saturating_add(ch.len_utf8());
                continue;
            }
            if let Some((decoded_ch, consumed)) = parse_entity_at(rem) {
                out.push(decoded_ch);
                idx = idx.saturating_add(consumed);
            } else {
                out.push('&');
                idx = idx.saturating_add(1);
            }
        }
        out
    }

    fn parse_entity_at(rem: &str) -> Option<(char, usize)> {
        let after_amp = rem.strip_prefix('&')?;
        if let Some(num_part) = after_amp.strip_prefix('#') {
            return parse_numeric_entity(num_part);
        }
        let semi_pos = after_amp.bytes().take(12).position(|b| b == b';')?;
        let name = after_amp.get(..semi_pos)?;
        let decoded = match name.to_ascii_lowercase().as_str() {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "colon" => ':',
            "tab" => '\t',
            "newline" => '\n',
            _ => return None,
        };
        Some((decoded, semi_pos.saturating_add(2)))
    }

    fn parse_numeric_entity(num_part: &str) -> Option<(char, usize)> {
        let (is_hex, digits_src, prefix_extra) = if let Some(rest) = num_part
            .strip_prefix('x')
            .or_else(|| num_part.strip_prefix('X'))
        {
            (true, rest, 1_usize)
        } else {
            (false, num_part, 0_usize)
        };
        let digit_len = digits_src
            .bytes()
            .take(8)
            .take_while(|b| {
                if is_hex {
                    b.is_ascii_hexdigit()
                } else {
                    b.is_ascii_digit()
                }
            })
            .count();
        if digit_len == 0 {
            return None;
        }
        let raw_digits = digits_src.get(..digit_len)?;
        let radix = if is_hex { 16 } else { 10 };
        let code = u32::from_str_radix(raw_digits, radix).ok()?;
        let decoded = char::from_u32(code)?;
        let has_semi = digits_src.as_bytes().get(digit_len) == Some(&b';');
        let consumed = 2_usize
            .saturating_add(prefix_extra)
            .saturating_add(digit_len)
            .saturating_add(usize::from(has_semi));
        Some((decoded, consumed))
    }

    fn push_xml_escaped(out: &mut String, text: &str) {
        for ch in text.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                _ => out.push(ch),
            }
        }
    }
}

fn validate_geojson(raw_bytes: Vec<u8>) -> Result<ValidatedAsset, AppError> {
    let val: serde_json::Value = serde_json::from_slice(&raw_bytes)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid GeoJSON: {err}")))?;
    let is_fc = val
        .get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|t| t == "FeatureCollection");
    let has_features = val.get("features").is_some_and(serde_json::Value::is_array);
    if !is_fc || !has_features {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "GeoJSON root must be a FeatureCollection with a features array",
        ));
    }
    Ok(ValidatedAsset {
        bytes: raw_bytes,
        mime_type: "application/geo+json",
        ext: "geojson",
        width_px: None,
        height_px: None,
    })
}

fn validate_glb(raw_bytes: Vec<u8>) -> Result<ValidatedAsset, AppError> {
    let (Some(b0), Some(b1), Some(b2)) = (
        raw_bytes.get(0..4),
        raw_bytes.get(4..8),
        raw_bytes.get(8..12),
    ) else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "GLB file shorter than 12-byte glTF 2.0 header",
        ));
    };
    let Ok(magic_arr) = <[u8; 4]>::try_from(b0) else {
        return Err(AppError::bad("INVALID_ARGUMENT", "invalid GLB header"));
    };
    let Ok(ver_arr) = <[u8; 4]>::try_from(b1) else {
        return Err(AppError::bad("INVALID_ARGUMENT", "invalid GLB header"));
    };
    let Ok(len_arr) = <[u8; 4]>::try_from(b2) else {
        return Err(AppError::bad("INVALID_ARGUMENT", "invalid GLB header"));
    };

    let magic = u32::from_le_bytes(magic_arr);
    let version = u32::from_le_bytes(ver_arr);
    let total_len = u32::from_le_bytes(len_arr);

    if magic != GLB_MAGIC_LE || version != GLB_VERSION_2 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "GLB header must have glTF magic 0x46546C67 and version 2",
        ));
    }
    if usize::try_from(total_len).ok() != Some(raw_bytes.len()) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!(
                "GLB header length {total_len} does not match payload size {}",
                raw_bytes.len()
            ),
        ));
    }

    Ok(ValidatedAsset {
        bytes: raw_bytes,
        mime_type: "model/gltf-binary",
        ext: "glb",
        width_px: None,
        height_px: None,
    })
}

async fn upload_asset_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    multipart: Multipart,
) -> Result<(StatusCode, Json<AssetRecord>), HttpError> {
    principal.require(Permission::ViewEdit)?;
    let parsed = read_asset_multipart(multipart).await?;
    let validated = validate_and_normalize_asset(parsed.asset_kind, parsed.raw_bytes)?;
    let sha256 = hex::encode(Sha256::digest(&validated.bytes));

    let asset_id = Uuid::new_v4();
    let dir = state.data_root.join("assets").join(&principal.tenant_id);
    tokio::fs::create_dir_all(&dir).await.map_err(|err| {
        HttpError(AppError::Config(format!(
            "failed to create asset directory: {err}"
        )))
    })?;
    let file_name = format!("{asset_id}.{}", validated.ext);
    let final_path = dir.join(&file_name);
    let part_path = dir.join(format!("{asset_id}.{}.part", validated.ext));

    tokio::fs::write(&part_path, &validated.bytes)
        .await
        .map_err(|err| HttpError(AppError::Config(format!("failed to write asset: {err}"))))?;
    if let Err(err) = tokio::fs::rename(&part_path, &final_path).await {
        cleanup_orphan_file(&part_path).await;
        return Err(HttpError(AppError::Config(format!(
            "failed to rename asset: {err}"
        ))));
    }

    let rel_path = PathBuf::from("assets")
        .join(&principal.tenant_id)
        .join(&file_name)
        .to_string_lossy()
        .into_owned();

    let created = match state
        .store
        .create_asset(
            &principal.tenant_id,
            CreateAssetParams {
                id: asset_id,
                asset_kind: parsed.asset_kind,
                name: parsed.name,
                file_path: rel_path,
                mime_type: String::from(validated.mime_type),
                sha256,
                width_px: validated.width_px,
                height_px: validated.height_px,
            },
        )
        .await
    {
        Ok(rec) => rec,
        Err(db_err) => {
            cleanup_orphan_file(&final_path).await;
            return Err(HttpError(db_err));
        }
    };

    record_asset_upload_audit(&state, &principal, &created);
    Ok((StatusCode::CREATED, Json(created)))
}

async fn cleanup_orphan_file(path: &std::path::Path) {
    if let Err(err) = tokio::fs::remove_file(path).await
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %err, "failed to remove orphan asset file");
    }
}

fn record_asset_upload_audit(state: &AppState, principal: &AuthPrincipal, asset: &AssetRecord) {
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: actor_uuid(principal),
        username: principal.username.clone(),
        action: AuditAction::FileUpload,
        resource_ref: format!("asset:{}", asset.id),
        status_code: 201,
        client_ip: String::new(),
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "asset_id": asset.id,
            "asset_kind": asset.asset_kind,
            "name": asset.name,
            "mime_type": asset.mime_type,
            "sha256": asset.sha256,
        }),
        created_at: Utc::now(),
    });
}

async fn get_asset_content_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(asset_id): ApiPath<Uuid>,
) -> Result<Response, HttpError> {
    principal.require(Permission::DataRead)?;
    let asset = state
        .store
        .get_asset(&principal.tenant_id, asset_id)
        .await?
        .ok_or(AppError::NotFound("ASSET_NOT_FOUND"))?;

    let full_path = resolve_asset_disk_path(&state.data_root, &asset.file_path);
    let bytes = tokio::fs::read(&full_path)
        .await
        .map_err(|_| HttpError(AppError::NotFound("ASSET_NOT_FOUND")))?;

    let content_type = HeaderValue::from_str(&asset.mime_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let mut resp = Response::new(Body::from(bytes));
    let headers = resp.headers_mut();
    headers.insert(CONTENT_TYPE, content_type);
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if asset.mime_type.eq_ignore_ascii_case("image/svg+xml") {
        headers.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; img-src data:; style-src 'unsafe-inline'; sandbox",
            ),
        );
    }
    Ok(resp)
}

async fn delete_asset_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(asset_id): ApiPath<Uuid>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    let removed = state
        .store
        .delete_asset(&principal.tenant_id, asset_id)
        .await?
        .ok_or(AppError::NotFound("ASSET_NOT_FOUND"))?;

    let full_path = resolve_asset_disk_path(&state.data_root, &removed.file_path);
    cleanup_orphan_file(&full_path).await;
    Ok(Json(DeleteStatus { deleted: true }))
}

fn resolve_asset_disk_path(data_root: &std::path::Path, stored_path: &str) -> PathBuf {
    let p = PathBuf::from(stored_path);
    if p.is_absolute() {
        p
    } else {
        data_root.join(p)
    }
}

async fn refresh_asset_bound_widgets(
    state: &AppState,
    tenant_id: &str,
    asset_id: Uuid,
) -> Result<(), AppError> {
    let affected = state
        .store
        .list_widgets_by_asset(tenant_id, asset_id)
        .await?;
    if affected.is_empty() {
        return Ok(());
    }
    let mut affected_ids = Vec::with_capacity(affected.len());
    for widget in affected {
        let deps = compute_widget_deps(&state.store, tenant_id, &widget.data_source).await?;
        state
            .store
            .update_widget(
                tenant_id,
                widget.id,
                UpdateWidgetRecordParams {
                    deps: Some(deps),
                    ..UpdateWidgetRecordParams::default()
                },
            )
            .await?;
        affected_ids.push(widget.id);
    }
    state
        .view
        .cache
        .invalidate_widgets(tenant_id, &affected_ids)
        .await;
    Ok(())
}

async fn list_floor_placements_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(asset_id): ApiPath<Uuid>,
) -> Result<Json<Vec<FloorPlacementRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state
        .store
        .list_floor_placements(&principal.tenant_id, asset_id)
        .await?;
    Ok(Json(items))
}

async fn replace_floor_placements_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(asset_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PutFloorPlacementsRequest>,
) -> Result<Json<Vec<FloorPlacementRecord>>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    for p in &req.placements {
        if !p.x_ratio.is_finite()
            || !p.y_ratio.is_finite()
            || !(0.0..=1.0).contains(&p.x_ratio)
            || !(0.0..=1.0).contains(&p.y_ratio)
        {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "floor placement coordinates must be within [0.0, 1.0]",
            )));
        }
    }
    let items = state
        .store
        .replace_floor_placements(&principal.tenant_id, asset_id, &req.placements)
        .await?;
    refresh_asset_bound_widgets(&state, &principal.tenant_id, asset_id).await?;
    Ok(Json(items))
}

async fn list_scene3d_bindings_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(asset_id): ApiPath<Uuid>,
) -> Result<Json<Vec<Scene3dBindingRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state
        .store
        .list_scene3d_bindings(&principal.tenant_id, asset_id)
        .await?;
    Ok(Json(items))
}

async fn replace_scene3d_bindings_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(asset_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PutScene3dBindingsRequest>,
) -> Result<Json<Vec<Scene3dBindingRecord>>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    for b in &req.bindings {
        if !b.pos_x.is_finite()
            || !b.pos_y.is_finite()
            || !b.pos_z.is_finite()
            || !b.scale.is_finite()
            || b.scale <= 0.0
        {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "3D scene binding coordinates must be finite and scale > 0",
            )));
        }
    }
    let items = state
        .store
        .replace_scene3d_bindings(&principal.tenant_id, asset_id, &req.bindings)
        .await?;
    refresh_asset_bound_widgets(&state, &principal.tenant_id, asset_id).await?;
    Ok(Json(items))
}

// ============================================================================
// Step 12: Export Jobs, OOXML Templates, and External `axs_` Share Links
// ============================================================================

fn export_and_share_router() -> Router<AppState> {
    Router::new()
        .route(
            "/xblueview/exports",
            get(list_exports_handler).post(create_export_handler),
        )
        .route(
            "/xblueview/exports/{id}",
            get(get_export_handler).delete(delete_export_handler),
        )
        .route(
            "/xblueview/exports/{id}/download",
            get(download_export_handler),
        )
        .route(
            "/xblueview/export-templates",
            get(list_export_templates_handler).post(upload_export_template_handler),
        )
        .route(
            "/xblueview/export-templates/{id}",
            delete(delete_export_template_handler),
        )
        .route(
            "/xblueview/shares",
            get(list_shares_handler).post(create_share_handler),
        )
        .route("/xblueview/shares/{id}", delete(revoke_share_handler))
        .merge(public_share_router())
}

fn public_share_router() -> Router<AppState> {
    Router::new()
        .route("/xblueview/share/{token}", get(get_public_share_handler))
        .route(
            "/xblueview/share/{token}/unlock",
            post(unlock_public_share_handler),
        )
        .route(
            "/xblueview/share/{token}/widgets/{widget_id}/data",
            post(shared_widget_data_handler),
        )
        .route(
            "/xblueview/share/{token}/assets/{asset_id}/content",
            get(shared_asset_raw_handler),
        )
        .route(
            "/xblueview/share/{token}/download",
            get(shared_download_handler),
        )
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListTemplatesQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedDownloadQuery {
    #[serde(default)]
    format: Option<ExportFormat>,
}

fn explicit_export_dataset_ref(
    principal: &AuthPrincipal,
    id: String,
) -> axpoint_domain::guard::ResourceRef {
    axpoint_domain::guard::ResourceRef {
        kind: axpoint_domain::guard::GrantResourceKind::Dataset,
        id,
        owner_dept: principal.dept_id.clone(),
        sensitivity: axpoint_domain::enums::Sensitivity::Internal,
    }
}

async fn authorize_export_source(
    state: &AppState,
    principal: &AuthPrincipal,
    source: &axpoint_domain::export::ExportSource,
    client_ip: &str,
) -> Result<(), AppError> {
    use axpoint_domain::export::ExportSource;
    use axpoint_domain::guard::GrantResourceKind;

    match source {
        ExportSource::Dashboard { dashboard_id, .. } => {
            let _ = axpoint_server::policy::authorize_resource_for_store(
                &state.store,
                &state.cedar,
                principal,
                Permission::ExportData,
                GrantResourceKind::Dashboard,
                &dashboard_id.to_string(),
                client_ip,
            )
            .await?;
        }
        ExportSource::Widget { widget_id, .. } => {
            let widget = state
                .store
                .get_widget(&principal.tenant_id, *widget_id)
                .await?
                .ok_or(AppError::NotFound("WIDGET_NOT_FOUND"))?;
            let _ = axpoint_server::policy::authorize_resource_for_store(
                &state.store,
                &state.cedar,
                principal,
                Permission::ExportData,
                GrantResourceKind::Dashboard,
                &widget.dashboard_id.to_string(),
                client_ip,
            )
            .await?;
        }
        ExportSource::Pivot { spec } => {
            let ds_ref = spec
                .get("dataset")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("pivot");
            let _ = axpoint_server::policy::authorize_resource_for_store(
                &state.store,
                &state.cedar,
                principal,
                Permission::ExportData,
                GrantResourceKind::Dataset,
                ds_ref,
                client_ip,
            )
            .await?;
        }
        ExportSource::SavedQuery { saved_query_id } => {
            let resource = explicit_export_dataset_ref(principal, saved_query_id.to_string());
            axpoint_server::policy::authorize_explicit_resource_for_store(
                &state.store,
                &state.cedar,
                principal,
                Permission::ExportData,
                &resource,
                client_ip,
            )
            .await?;
        }
        ExportSource::Scenario { scenario_ids } => {
            let id_str = scenario_ids
                .first()
                .map_or_else(|| String::from("scenario"), ToString::to_string);
            let resource = explicit_export_dataset_ref(principal, id_str);
            axpoint_server::policy::authorize_explicit_resource_for_store(
                &state.store,
                &state.cedar,
                principal,
                Permission::ExportData,
                &resource,
                client_ip,
            )
            .await?;
        }
        ExportSource::Conversation { conversation_id } => {
            let resource = explicit_export_dataset_ref(principal, conversation_id.to_string());
            axpoint_server::policy::authorize_explicit_resource_for_store(
                &state.store,
                &state.cedar,
                principal,
                Permission::ExportData,
                &resource,
                client_ip,
            )
            .await?;
        }
    }
    Ok(())
}

async fn create_export_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateExportRequest>,
) -> Result<(StatusCode, Json<ExportJobRecord>), HttpError> {
    validate_create_export_request(&state.store, &principal, &req).await?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    authorize_export_source(&state, &principal, &req.source, &client_ip).await?;
    let created = state
        .store
        .create_export_job(
            &principal.tenant_id,
            CreateExportJobParams {
                export_format: req.export_format,
                source: req.source,
                options: req.options,
                template_id: req.template_id,
                created_by: principal.user_id,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn list_exports_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ExportListQuery>,
) -> Result<Json<PaginatedList<ExportJobRecord>>, HttpError> {
    principal.require(Permission::ExportData)?;
    let show_all = query.mine == Some(false) && principal.has(Permission::DataAdmin);
    let created_by_filter = (!show_all).then_some(principal.user_id);
    let page = state
        .store
        .list_export_jobs(&principal.tenant_id, created_by_filter, query)
        .await?;
    Ok(Json(page))
}

async fn get_export_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(export_id): ApiPath<Uuid>,
) -> Result<Json<ExportJobRecord>, HttpError> {
    principal.require(Permission::ExportData)?;
    let job = state
        .store
        .get_export_job(&principal.tenant_id, export_id)
        .await?
        .ok_or(AppError::NotFound("EXPORT_NOT_FOUND"))?;
    if job.created_by != principal.user_id && !principal.has(Permission::DataAdmin) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    Ok(Json(job))
}

async fn delete_export_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(export_id): ApiPath<Uuid>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::ExportData)?;
    let job = state
        .store
        .get_export_job(&principal.tenant_id, export_id)
        .await?
        .ok_or(AppError::NotFound("EXPORT_NOT_FOUND"))?;
    if job.created_by != principal.user_id && !principal.has(Permission::DataAdmin) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    let removed = state
        .store
        .delete_export_job(&principal.tenant_id, export_id)
        .await?
        .ok_or(AppError::NotFound("EXPORT_NOT_FOUND"))?;
    if let Some(ref rel_path) = removed.file_path
        && let Ok(full_path) = resolve_data_root_path(&state.data_root, rel_path)
    {
        cleanup_file_quietly(&full_path).await;
    }
    Ok(Json(DeleteStatus { deleted: true }))
}

async fn download_export_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(export_id): ApiPath<Uuid>,
) -> Result<Response, HttpError> {
    principal.require(Permission::ExportData)?;
    let job = state
        .store
        .get_export_job(&principal.tenant_id, export_id)
        .await?
        .ok_or(AppError::NotFound("EXPORT_NOT_FOUND"))?;
    if job.created_by != principal.user_id && !principal.has(Permission::DataAdmin) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    if job.status != JobStatus::Succeeded {
        return Err(HttpError(AppError::NotFound("EXPORT_NOT_FOUND")));
    }
    let rel_path = job
        .file_path
        .as_deref()
        .ok_or(AppError::NotFound("EXPORT_NOT_FOUND"))?;
    let disk_path = resolve_data_root_path(&state.data_root, rel_path)?;
    let file_name = job
        .file_name
        .unwrap_or_else(|| format!("export-{export_id}"));
    stream_file_attachment(
        &disk_path,
        &file_name,
        job.export_format,
        "EXPORT_NOT_FOUND",
    )
    .await
}

async fn stream_file_attachment(
    disk_path: &std::path::Path,
    file_name: &str,
    fallback_format: ExportFormat,
    not_found_code: &'static str,
) -> Result<Response, HttpError> {
    let file = tokio::fs::File::open(disk_path)
        .await
        .map_err(|_| HttpError(AppError::NotFound(not_found_code)))?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    build_attachment_response(body, file_name, fallback_format)
}

fn build_attachment_response(
    body: Body,
    file_name: &str,
    fallback_format: ExportFormat,
) -> Result<Response, HttpError> {
    let mime_type = content_type_for_export(file_name, fallback_format);
    let disposition_raw = build_content_disposition(file_name);
    let disposition_val = HeaderValue::from_str(&disposition_raw).map_err(|err| {
        HttpError(AppError::Config(format!(
            "invalid Content-Disposition: {err}"
        )))
    })?;

    let mut resp = Response::new(body);
    let headers = resp.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime_type));
    headers.insert(CONTENT_DISPOSITION, disposition_val);
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(resp)
}

struct ParsedTemplateUpload {
    export_format: ExportFormat,
    name: String,
    raw_bytes: Vec<u8>,
}

async fn read_template_multipart(
    mut multipart: Multipart,
) -> Result<ParsedTemplateUpload, AppError> {
    let mut format_raw: Option<String> = None;
    let mut name: Option<String> = None;
    let mut file_name: Option<String> = None;
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| map_multipart_error(&e))?
    {
        let field_name = field.name().unwrap_or_default().to_owned();
        if field_name == "tenant_id" {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "tenant_id parameter is forbidden",
            ));
        }
        if field_name == "file" {
            if let Some(orig) = field.file_name() {
                let clean = orig.rsplit(['/', '\\']).next().unwrap_or(orig).trim();
                if !clean.is_empty() {
                    file_name = Some(clean.to_owned());
                }
            }
            let bytes = field.bytes().await.map_err(|e| map_multipart_error(&e))?;
            file_bytes = Some(bytes.to_vec());
            continue;
        }
        let text = field.text().await.map_err(|e| map_multipart_error(&e))?;
        match field_name.as_str() {
            "export_format" | "format" => format_raw = Some(text.trim().to_owned()),
            "name" => name = Some(text.trim().to_owned()),
            _ => {}
        }
    }

    let fmt_str = format_raw
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "missing 'export_format' field"))?;
    let export_format: ExportFormat = fmt_str.parse()?;
    if !matches!(export_format, ExportFormat::Docx | ExportFormat::Pptx) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "export_format for template must be DOCX or PPTX",
        ));
    }
    let raw_bytes =
        file_bytes.ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "missing 'file' field"))?;
    let resolved_name = name
        .filter(|s| !s.is_empty())
        .or(file_name)
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "missing template 'name'"))?;

    Ok(ParsedTemplateUpload {
        export_format,
        name: resolved_name,
        raw_bytes,
    })
}

fn require_template_write_perm(principal: &AuthPrincipal) -> Result<(), AppError> {
    if principal.has(Permission::ExportData) || principal.has(Permission::ViewEdit) {
        Ok(())
    } else {
        Err(AppError::Forbidden("PERMISSION_DENIED"))
    }
}

async fn list_export_templates_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ListTemplatesQuery>,
) -> Result<Json<PaginatedList<ExportTemplateRecord>>, HttpError> {
    if !principal.has(Permission::ExportData)
        && !principal.has(Permission::ViewEdit)
        && !principal.has(Permission::DataRead)
    {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    validate_page_limit(query.limit)?;
    let page = state
        .store
        .list_export_templates(
            &principal.tenant_id,
            SimplePageQuery {
                limit: query.limit,
                cursor: query.cursor,
            },
        )
        .await?;
    Ok(Json(page))
}

async fn upload_export_template_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    multipart: Multipart,
) -> Result<(StatusCode, Json<ExportTemplateRecord>), HttpError> {
    require_template_write_perm(&principal)?;
    let parsed = read_template_multipart(multipart).await?;
    let placeholders = extract_ooxml_placeholders(parsed.export_format, &parsed.raw_bytes)?;
    let sha256 = hex::encode(Sha256::digest(&parsed.raw_bytes));

    let tpl_id = Uuid::new_v4();
    let ext = match parsed.export_format {
        ExportFormat::Pptx => "pptx",
        _ => "docx",
    };
    let dir = state
        .data_root
        .join("export_templates")
        .join(&principal.tenant_id);
    tokio::fs::create_dir_all(&dir).await.map_err(|err| {
        HttpError(AppError::Config(format!(
            "failed to create export_templates directory: {err}"
        )))
    })?;
    let file_name = format!("{tpl_id}.{ext}");
    let final_path = dir.join(&file_name);
    let part_path = dir.join(format!("{tpl_id}.{ext}.part"));

    tokio::fs::write(&part_path, &parsed.raw_bytes)
        .await
        .map_err(|err| HttpError(AppError::Config(format!("failed to write template: {err}"))))?;
    if let Err(err) = tokio::fs::rename(&part_path, &final_path).await {
        cleanup_orphan_file(&part_path).await;
        return Err(HttpError(AppError::Config(format!(
            "failed to rename template: {err}"
        ))));
    }

    let rel_path = PathBuf::from("export_templates")
        .join(&principal.tenant_id)
        .join(&file_name)
        .to_string_lossy()
        .into_owned();

    let created = match state
        .store
        .create_export_template(
            &principal.tenant_id,
            CreateExportTemplateParams {
                id: tpl_id,
                name: parsed.name,
                export_format: parsed.export_format,
                file_path: rel_path,
                sha256,
                placeholders,
                created_by: actor_uuid(&principal),
            },
        )
        .await
    {
        Ok(rec) => rec,
        Err(db_err) => {
            cleanup_orphan_file(&final_path).await;
            return Err(HttpError(db_err));
        }
    };

    Ok((StatusCode::CREATED, Json(created)))
}

async fn delete_export_template_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(template_id): ApiPath<Uuid>,
) -> Result<Json<DeleteStatus>, HttpError> {
    require_template_write_perm(&principal)?;
    let removed = state
        .store
        .delete_export_template(&principal.tenant_id, template_id)
        .await?
        .ok_or(AppError::NotFound("TEMPLATE_NOT_FOUND"))?;
    if let Ok(full_path) = resolve_data_root_path(&state.data_root, &removed.file_path) {
        cleanup_file_quietly(&full_path).await;
    }
    Ok(Json(DeleteStatus { deleted: true }))
}

async fn create_share_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateShareRequest>,
) -> Result<(StatusCode, Json<CreateShareResponse>), HttpError> {
    let client_ip = extract_share_client_ip(&headers);
    let trace_id = current_trace_id();
    let created = create_share(
        &state.store,
        &state.audit,
        &principal,
        req,
        &client_ip,
        &trace_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn list_shares_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ShareListQuery>,
) -> Result<Json<PaginatedList<ShareLinkRecord>>, HttpError> {
    if !principal.has(Permission::ViewShare) && !principal.has(Permission::DataRead) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    let page = state
        .store
        .list_share_links(&principal.tenant_id, query)
        .await?;
    Ok(Json(page))
}

async fn revoke_share_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(share_id): ApiPath<Uuid>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::ViewShare)?;
    let revoked = state
        .store
        .revoke_share_link(&principal.tenant_id, share_id)
        .await?;
    if !revoked {
        return Err(HttpError(AppError::NotFound("SHARE_NOT_FOUND")));
    }
    Ok(Json(DeleteStatus { deleted: true }))
}

fn extract_share_client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| String::from("127.0.0.1"), String::from)
}

fn extract_share_meta(headers: &HeaderMap) -> ShareRequestMeta {
    let share_session = headers
        .get("x-share-session")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    ShareRequestMeta {
        share_session,
        client_ip: extract_share_client_ip(headers),
        trace_id: current_trace_id(),
    }
}

fn build_public_share_env(state: &AppState) -> PublicShareEnv<'_, axpoint_db::PgStore> {
    PublicShareEnv {
        store: &state.store,
        analytics_pool: &state.analytics_pool,
        cache: &state.view.cache,
        cedar: &state.cedar,
        keyring: &state.keyring,
        audit: &state.audit,
        limiters: &state.view.share_limiters,
        data_root: &state.data_root,
    }
}

async fn get_public_share_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
) -> Result<Json<SharePublicResponse>, HttpError> {
    let meta = extract_share_meta(&headers);
    let env = build_public_share_env(&state);
    let resp = get_public_share(&env, &token, &meta).await?;
    Ok(Json(resp))
}

async fn unlock_public_share_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
    ApiJson(req): ApiJson<ShareUnlockRequest>,
) -> Result<Json<ShareUnlockResponse>, HttpError> {
    let client_ip = extract_share_client_ip(&headers);
    let resp = unlock_public_share(
        &state.store,
        &state.keyring,
        &state.view.share_limiters,
        &token,
        &req.password,
        &client_ip,
    )
    .await?;
    Ok(Json(resp))
}

async fn shared_widget_data_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiPath((token, widget_id)): ApiPath<(String, Uuid)>,
    body: Bytes,
) -> Result<Json<WidgetDataResponse>, HttpError> {
    let req: WidgetDataRequest = parse_optional_json(&body)?;
    let meta = extract_share_meta(&headers);
    let env = build_public_share_env(&state);
    let resp = execute_shared_widget_data(&env, &token, widget_id, req, &meta).await?;
    Ok(Json(resp))
}

async fn shared_asset_raw_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiPath((token, asset_id)): ApiPath<(String, Uuid)>,
) -> Result<Response, HttpError> {
    let meta = extract_share_meta(&headers);
    let env = build_public_share_env(&state);
    let (asset, disk_path) = resolve_shared_dashboard_asset(&env, &token, asset_id, &meta).await?;
    let bytes = tokio::fs::read(&disk_path)
        .await
        .map_err(|_| HttpError(AppError::NotFound("ASSET_NOT_FOUND")))?;

    let content_type = HeaderValue::from_str(&asset.mime_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let mut resp = Response::new(Body::from(bytes));
    let resp_headers = resp.headers_mut();
    resp_headers.insert(CONTENT_TYPE, content_type);
    resp_headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if asset.mime_type.eq_ignore_ascii_case("image/svg+xml") {
        resp_headers.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; img-src data:; style-src 'unsafe-inline'; sandbox",
            ),
        );
    }
    Ok(resp)
}

async fn shared_download_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
    ApiQuery(query): ApiQuery<SharedDownloadQuery>,
) -> Result<Response, HttpError> {
    let meta = extract_share_meta(&headers);
    let env = build_public_share_env(&state);
    let artifact =
        prepare_shared_download(&env, state.sidecar.as_ref(), &token, query.format, &meta).await?;

    if artifact.ephemeral {
        let read_res = tokio::fs::read(&artifact.disk_path).await;
        cleanup_file_quietly(&artifact.disk_path).await;
        let bytes = read_res.map_err(|_| HttpError(AppError::NotFound("SHARE_NOT_FOUND")))?;
        return build_attachment_response(
            Body::from(bytes),
            &artifact.file_name,
            artifact.export_format,
        );
    }

    stream_file_attachment(
        &artifact.disk_path,
        &artifact.file_name,
        artifact.export_format,
        "SHARE_NOT_FOUND",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_svg_strips_adjacent_event_handlers_and_smil_tags() -> Result<(), AppError> {
        let bypass_no_space = r#"<svg id="a"onload="alert(1)" xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="10" height="10"/><set attributeName="onload" to="alert(1)"/><animate attributeName="href" values="javascript:alert(1)"/><script>alert(1)</script><foreignObject><iframe src="https://evil.example"/></foreignObject></svg>"#;
        let cleaned = sanitize_svg(bypass_no_space)?;
        assert!(cleaned.contains(r#"<svg id="a""#));
        assert!(cleaned.contains(r#"<rect x="0" y="0" width="10" height="10"/>"#));
        assert!(!cleaned.to_ascii_lowercase().contains("onload"));
        assert!(!cleaned.to_ascii_lowercase().contains("<set"));
        assert!(!cleaned.to_ascii_lowercase().contains("<animate"));
        assert!(!cleaned.to_ascii_lowercase().contains("<script"));
        assert!(!cleaned.to_ascii_lowercase().contains("foreignobject"));
        Ok(())
    }

    #[test]
    fn sanitize_svg_rejects_entity_encoded_js_protocol_relative_and_doctype() {
        for payload in [
            r#"<svg><use href="&#106;avascript:alert(1)"/></svg>"#,
            r#"<svg><use href="&#x6a;avascript:alert(1)"/></svg>"#,
            r#"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="//evil.example/a.svg"/></svg>"#,
            r#"<svg><image href="data:image/svg+xml;base64,PHN2Zy8+"/></svg>"#,
            r#"<!DOCTYPE svg SYSTEM "http://evil.example/xxe.dtd"><svg/>"#,
            r#"<!DOCTYPE svg [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><svg>&xxe;</svg>"#,
            r#"<svg><rect fill="url(https://evil.example/p.svg#g)"/></svg>"#,
        ] {
            let res = sanitize_svg(payload);
            assert!(
                matches!(
                    res,
                    Err(AppError::BadRequest {
                        code: "INVALID_ARGUMENT",
                        ..
                    })
                ),
                "expected INVALID_ARGUMENT for payload: {payload}"
            );
        }
    }

    #[test]
    fn sanitize_svg_cleans_css_and_preserves_safe_fragments_and_raster_data_uri()
    -> Result<(), AppError> {
        let valid = r##"<?xml version="1.0"?><svg viewBox="0 0 100 100"><defs><linearGradient id="g1"><stop offset="0%" stop-color="#fff"/></linearGradient></defs><style>@import url("https://evil.example/x.css"); .box { fill: url(#g1); background: url(https://evil.example/bg.png); width: expression(alert(1)) }</style><rect class="box" style="stroke: #000; background: url(//evil.example/x); filter: url(#f1)" width="50" height="50"/><use href="#g1"/><image href="data:image/png;base64,iVBORw0KGgo=" width="10" height="10"/></svg>"##;
        let cleaned = sanitize_svg(valid)?;
        assert!(!cleaned.contains("<?xml"));
        assert!(!cleaned.contains("@import"));
        assert!(!cleaned.contains("evil.example"));
        assert!(!cleaned.contains("expression("));
        assert!(cleaned.contains("fill: url(#g1)"));
        assert!(cleaned.contains("href=\"#g1\""));
        assert!(cleaned.contains("data:image/png;base64,iVBORw0KGgo="));
        Ok(())
    }

    #[test]
    fn oversized_asset_returns_upload_too_large() {
        let oversized = vec![0_u8; MAX_VISUAL_ASSET_BYTES.saturating_add(1)];
        let res = validate_and_normalize_asset(AssetKind::FloorPlanImage, oversized);
        assert!(matches!(
            res,
            Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"))
        ));
    }
}
