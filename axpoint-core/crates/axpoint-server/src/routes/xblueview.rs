//! `xBlueView` digital-twin dashboard, widget execution, `SSE` stream, spatial binding,
//! and visual asset routes (`/api/xblueview/*`) (`10-STEP11-12` §11.7).

use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AssetKind, Permission, TokenKind};
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
use axum::body::{Body, Bytes};
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_TYPE, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};

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

/// Builds the `/xblueview/*` sub-router mounted under `/api` (`10-STEP11-12` §11.7).
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
        .route(
            "/xblueview/assets/{id}",
            axum::routing::delete(delete_asset_handler),
        )
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
        actor_uuid(&principal),
        req,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn get_dashboard_handler(
    State(state): State<AppState>,
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
    Ok(Json(detail))
}

async fn get_dashboard_by_slug_handler(
    State(state): State<AppState>,
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
            .invalidate_widgets(&principal.tenant_id, &widget_ids);
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
        .invalidate_widgets(&principal.tenant_id, &[widget_id]);
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
        .invalidate_widgets(&principal.tenant_id, &[widget_id]);
    Ok(Json(DeleteStatus { deleted }))
}

async fn widget_data_handler(
    State(state): State<AppState>,
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

    let env = WidgetExecEnv {
        store: &state.store,
        analytics_pool: &state.analytics_pool,
        cache: &state.view.cache,
        principal: &principal,
    };
    let resp = execute_widget_data(&env, &detail.dashboard, &widget, &req).await?;
    Ok(Json(resp))
}

async fn widget_preview_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<WidgetPreviewRequest>,
) -> Result<Json<WidgetDataResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let env = WidgetExecEnv {
        store: &state.store,
        analytics_pool: &state.analytics_pool,
        cache: &state.view.cache,
        principal: &principal,
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
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("multipart error: {err}")))?
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
            let bytes = field.bytes().await.map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("read file failed: {err}"))
            })?;
            file_bytes = Some(bytes.to_vec());
            continue;
        }
        let text = field.text().await.map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid field '{field_name}': {err}"),
            )
        })?;
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
        return Err(AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE"));
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
    if !text.to_ascii_lowercase().contains("<svg") {
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
    let mut cleaned = strip_tag_block(raw, "script");
    cleaned = strip_tag_block(&cleaned, "foreignObject");
    cleaned = strip_tag_block(&cleaned, "iframe");
    cleaned = strip_tag_block(&cleaned, "object");
    cleaned = strip_tag_block(&cleaned, "embed");
    cleaned = strip_svg_event_handlers(&cleaned);

    let lower = cleaned.to_ascii_lowercase();
    for forbidden in ["javascript:", "vbscript:", "data:text/html"] {
        if lower.contains(forbidden) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("SVG contains forbidden URI scheme '{forbidden}'"),
            ));
        }
    }
    for external_ref in [
        "href=\"http://",
        "href=\"https://",
        "href='http://",
        "href='https://",
        "url(http://",
        "url(https://",
    ] {
        if lower.contains(external_ref) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "SVG external URL references are forbidden",
            ));
        }
    }
    if !lower.contains("<svg") {
        return Err(AppError::bad("INVALID_ARGUMENT", "invalid SVG document"));
    }
    Ok(cleaned)
}

fn strip_tag_block(input: &str, tag: &str) -> String {
    let open_pat = format!("<{}", tag.to_ascii_lowercase());
    let close_pat = format!("</{}>", tag.to_ascii_lowercase());
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(pos) = rest.to_ascii_lowercase().find(&open_pat) {
        if let Some(prefix) = rest.get(..pos) {
            out.push_str(prefix);
        }
        let after_open = rest.get(pos..).unwrap_or_default();
        let lower_after = after_open.to_ascii_lowercase();
        if let Some(close_idx) = lower_after.find(&close_pat) {
            let skip = close_idx.saturating_add(close_pat.len());
            rest = after_open.get(skip..).unwrap_or_default();
        } else if let Some(self_close) = after_open.find("/>") {
            let skip = self_close.saturating_add(2);
            rest = after_open.get(skip..).unwrap_or_default();
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

fn strip_svg_event_handlers(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0_usize;
    while i < bytes.len() {
        let remaining = input.get(i..).unwrap_or_default();
        if let Some(ch) = remaining.chars().next() {
            let prev_is_ws = i == 0
                || bytes
                    .get(i.saturating_sub(1))
                    .is_some_and(u8::is_ascii_whitespace);
            if prev_is_ws
                && remaining.len() > 3
                && remaining
                    .get(..2)
                    .is_some_and(|p| p.eq_ignore_ascii_case("on"))
                && let Some(eq_rel) = remaining.find('=')
                && eq_rel <= 24
                && remaining.get(2..eq_rel).is_some_and(|name| {
                    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphabetic())
                })
            {
                let after_eq = remaining
                    .get(eq_rel.saturating_add(1)..)
                    .unwrap_or_default();
                let trimmed_len = after_eq.len().saturating_sub(after_eq.trim_start().len());
                let val_part = after_eq.get(trimmed_len..).unwrap_or_default();
                let val_skip =
                    if let Some(q) = val_part.chars().next().filter(|c| *c == '"' || *c == '\'') {
                        val_part
                            .get(1..)
                            .and_then(|s| s.find(q))
                            .map_or(val_part.len(), |end| end.saturating_add(2))
                    } else {
                        val_part
                            .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                            .unwrap_or(val_part.len())
                    };
                i = i
                    .saturating_add(eq_rel)
                    .saturating_add(1)
                    .saturating_add(trimmed_len)
                    .saturating_add(val_skip);
                continue;
            }
            out.push(ch);
            i = i.saturating_add(ch.len_utf8());
        } else {
            break;
        }
    }
    out
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
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|err| HttpError(AppError::Config(format!("failed to rename asset: {err}"))))?;

    let rel_path = PathBuf::from("assets")
        .join(&principal.tenant_id)
        .join(&file_name)
        .to_string_lossy()
        .into_owned();

    let created = state
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
        .await?;

    Ok((StatusCode::CREATED, Json(created)))
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
    resp.headers_mut().insert(CONTENT_TYPE, content_type);
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
    if let Err(err) = tokio::fs::remove_file(&full_path).await
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %full_path.display(), error = %err, "failed to remove deleted asset file");
    }
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
    Ok(Json(items))
}
