from __future__ import annotations

import asyncio
import logging
import time
from collections.abc import AsyncIterator, Mapping
from contextlib import asynccontextmanager
from typing import Annotated

import orjson
from fastapi import FastAPI, File, Form, Query, Request, UploadFile
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse, Response
from starlette.exceptions import HTTPException as StarletteHTTPException

from charts import render_chart_bytes, shutdown_chart_pool
from connectors.base import arrow_table_to_preview
from connectors.registry import create_connector, list_connector_specs
from exporters import export_document, normalize_export_format
from lake.maintenance import run_maintenance
from lake.writer import (
    commit_staging,
    drop_lake_table,
    list_snapshots,
    materialize_derived_view,
    preview_lake,
    sync_dataset,
)
from parsers.engine import parse_bytes_to_markdown, parse_unified
from pii import analyze_texts, anonymize_texts
from schemas import (
    CommitStagingRequest,
    ConnectorActionRequest,
    ConnectorPreviewRequest,
    ConnectorSpecModel,
    DiscoveredObject,
    DropLakeTableRequest,
    LakeMaintenanceRequest,
    LakeMaintenanceResult,
    LakePreviewRequest,
    LakeSnapshotInfo,
    MaterializeRequest,
    ParseResponse,
    PiiAnalyzeRequest,
    PiiAnalyzeResponse,
    PiiAnonymizeRequest,
    PiiAnonymizeResponse,
    PreviewResponse,
    RenderChartRequest,
    SidecarExportRequest,
    SidecarExportResponse,
    SyncRequest,
    SyncResult,
    TestResult,
    UnifiedParseResponse,
)
from security import (
    InternalTokenMiddleware,
    SidecarError,
    configure_logging,
    error_json_response,
    read_limited,
    validate_tenant_id,
)
from settings import get_model_path

configure_logging()
logger = logging.getLogger(__name__)

_STATUS_ERROR_CODES: dict[int, str] = {
    400: "INVALID_ARGUMENT",
    401: "TOKEN_INVALID",
    403: "PERMISSION_DENIED",
    404: "ROUTE_NOT_FOUND",
    405: "ROUTE_NOT_FOUND",
    409: "CONFLICT",
    413: "UPLOAD_TOO_LARGE",
    415: "UNSUPPORTED_FORMAT",
    422: "PARSE_FAILED",
    429: "RATE_LIMITED",
    502: "UPSTREAM_PARSER",
    504: "UPSTREAM_TIMEOUT",
}


@asynccontextmanager
async def _lifespan(_app: FastAPI) -> AsyncIterator[None]:
    yield
    await asyncio.to_thread(shutdown_chart_pool)


app = FastAPI(
    title="AxPoint Document & Data Parser Sidecar",
    version="0.2.0",
    lifespan=_lifespan,
)
app.add_middleware(InternalTokenMiddleware)


@app.exception_handler(SidecarError)
async def handle_sidecar_error(request: Request, exc: SidecarError) -> JSONResponse:
    return error_json_response(
        status_code=exc.status_code,
        error_code=exc.error_code,
        message=exc.message,
        request=request,
    )


@app.exception_handler(StarletteHTTPException)
async def handle_http_exception(request: Request, exc: StarletteHTTPException) -> JSONResponse:
    if isinstance(exc.detail, Mapping) and "error_code" in exc.detail:
        error_code = str(exc.detail["error_code"])
        message = str(exc.detail.get("message", ""))
    else:
        error_code = _STATUS_ERROR_CODES.get(
            exc.status_code, "INTERNAL" if exc.status_code >= 500 else "INVALID_ARGUMENT"
        )
        message = "internal error" if exc.status_code >= 500 else str(exc.detail)
    return error_json_response(
        status_code=exc.status_code,
        error_code=error_code,
        message=message,
        request=request,
    )


@app.exception_handler(RequestValidationError)
async def handle_validation_error(request: Request, exc: RequestValidationError) -> JSONResponse:
    errors = exc.errors()
    summary = "; ".join(
        f"{'.'.join(str(loc) for loc in err.get('loc', ()))}: {err.get('msg', 'invalid')}"
        for err in errors
    )
    return error_json_response(
        status_code=400,
        error_code="INVALID_ARGUMENT",
        message=summary or "invalid request parameters",
        request=request,
    )


@app.exception_handler(Exception)
async def handle_unexpected_exception(request: Request, exc: Exception) -> JSONResponse:
    logger.exception("Unhandled sidecar exception on %s: %s", request.url.path, type(exc).__name__)
    return error_json_response(
        status_code=500,
        error_code="INTERNAL",
        message="internal error",
        request=request,
    )


def _header_tenant(request: Request) -> str:
    raw = request.headers.get("x-axpoint-tenant", "default").strip() or "default"
    return validate_tenant_id(raw)


def _header_timezone(request: Request) -> str:
    return request.headers.get("x-axpoint-timezone", "Asia/Shanghai").strip() or "Asia/Shanghai"


@app.get("/health")
def health() -> dict[str, object]:
    model_path = get_model_path()
    easyocr_ready = (model_path / "easyocr").is_dir()
    return {
        "status": "ok",
        "service": "axpoint-parser",
        "model_path": str(model_path),
        "model_dir": str(model_path),
        "easyocr_models_present": easyocr_ready,
        "model_dir_ready": model_path.exists(),
    }


@app.post("/parse", response_model=ParseResponse)
async def parse_document(
    file: Annotated[UploadFile, File(...)],
) -> ParseResponse:
    raw = await read_limited(file)
    if not raw:
        raise SidecarError(400, "INVALID_ARGUMENT", "Uploaded file is empty")
    title, markdown, engine = parse_bytes_to_markdown(file.filename or "document.txt", raw)
    return ParseResponse(title=title, markdown=markdown, engine=engine)


@app.post("/parse/unified", response_model=UnifiedParseResponse)
async def parse_unified_endpoint(
    request: Request,
    file: Annotated[UploadFile, File(...)],
    options: Annotated[str | None, Form()] = None,
    log_format: Annotated[str | None, Form()] = None,
    json_explode: Annotated[str | None, Form()] = None,
    ocr: Annotated[str | None, Form()] = None,
    record_tag: Annotated[str | None, Form()] = None,
) -> UnifiedParseResponse:
    tenant_id = _header_tenant(request)
    tz = _header_timezone(request)
    raw = await read_limited(file)
    if not raw:
        raise SidecarError(400, "INVALID_ARGUMENT", "Uploaded file is empty")

    merged_opts: dict[str, str] = {}
    if options:
        try:
            parsed_opts = orjson.loads(options)
        except orjson.JSONDecodeError as exc:
            raise SidecarError(400, "INVALID_ARGUMENT", f"invalid options JSON: {exc}") from exc
        if not isinstance(parsed_opts, dict):
            raise SidecarError(400, "INVALID_ARGUMENT", "options must be a JSON object")
        for k, v in parsed_opts.items():
            if v is not None:
                merged_opts[str(k)] = str(v)

    for key, val in (
        ("log_format", log_format),
        ("json_explode", json_explode),
        ("ocr", ocr),
        ("record_tag", record_tag),
    ):
        if val is not None and val != "":
            merged_opts[key] = val

    return parse_unified(
        file.filename or "document.txt",
        raw,
        tenant_id=tenant_id,
        timezone=tz,
        options=merged_opts,
    )


@app.get("/connectors/types", response_model=list[ConnectorSpecModel])
async def get_connector_types(request: Request) -> list[ConnectorSpecModel]:
    _header_tenant(request)
    return list_connector_specs()


@app.post("/connectors/test", response_model=TestResult)
async def post_connector_test(request: Request, req: ConnectorActionRequest) -> TestResult:
    tenant_id = _header_tenant(request)
    t0 = time.perf_counter()
    conn = create_connector(
        req.connector_type,
        req.config,
        req.secrets,
        allow_private_network=req.allow_private_network,
        tenant_id=tenant_id,
    )
    try:
        ok, msg = conn.test()
    finally:
        conn.close()
    latency_ms = max(0, int((time.perf_counter() - t0) * 1000))
    return TestResult(ok=ok, message=msg, latency_ms=latency_ms)


@app.post("/connectors/discover", response_model=list[DiscoveredObject])
async def post_connector_discover(
    request: Request, req: ConnectorActionRequest
) -> list[DiscoveredObject]:
    tenant_id = _header_tenant(request)
    conn = create_connector(
        req.connector_type,
        req.config,
        req.secrets,
        allow_private_network=req.allow_private_network,
        tenant_id=tenant_id,
    )
    try:
        return conn.discover()
    finally:
        conn.close()


@app.post("/connectors/preview", response_model=PreviewResponse)
async def post_connector_preview(request: Request, req: ConnectorPreviewRequest) -> PreviewResponse:
    tenant_id = _header_tenant(request)
    tz = _header_timezone(request)
    conn = create_connector(
        req.connector_type,
        req.config,
        req.secrets,
        allow_private_network=req.allow_private_network,
        tenant_id=tenant_id,
    )
    try:
        arrow_tbl = conn.preview(req.ref, req.limit)
    finally:
        conn.close()
    return arrow_table_to_preview(arrow_tbl, limit=req.limit, timezone_str=tz)


@app.post(
    "/connectors/sync",
    response_model=SyncResult,
    response_model_by_alias=True,
)
async def post_connector_sync(request: Request, req: SyncRequest) -> SyncResult:
    tz = _header_timezone(request)
    return sync_dataset(req, timezone_str=tz, raise_on_quality_block=True)


@app.post(
    "/lake/commit_staging",
    response_model=list[SyncResult],
    response_model_by_alias=True,
)
async def post_lake_commit_staging(request: Request, req: CommitStagingRequest) -> list[SyncResult]:
    tz = _header_timezone(request)
    return commit_staging(req, timezone_str=tz)


@app.post(
    "/lake/materialize",
    response_model=SyncResult,
    response_model_by_alias=True,
)
async def post_lake_materialize(request: Request, req: MaterializeRequest) -> SyncResult:
    raw_hdr_tenant = request.headers.get("x-axpoint-tenant", "").strip()
    body_tenant = validate_tenant_id(req.tenant_id)
    if raw_hdr_tenant:
        hdr_tenant = validate_tenant_id(raw_hdr_tenant)
        if hdr_tenant != body_tenant:
            raise SidecarError(
                403,
                "PERMISSION_DENIED",
                "x-axpoint-tenant header does not match request tenant_id",
            )
    tz = _header_timezone(request)
    return materialize_derived_view(req, timezone_str=tz)


@app.post("/lake/preview", response_model=PreviewResponse)
async def post_lake_preview(request: Request, req: LakePreviewRequest) -> PreviewResponse:
    tz = _header_timezone(request)
    return preview_lake(req, timezone_str=tz)


@app.get("/lake/snapshots", response_model=list[LakeSnapshotInfo])
async def get_lake_snapshots(
    request: Request,
    lake_table: Annotated[str, Query()],
    tenant_id: Annotated[str | None, Query()] = None,
) -> list[LakeSnapshotInfo]:
    eff_tenant = validate_tenant_id(tenant_id) if tenant_id else _header_tenant(request)
    return list_snapshots(eff_tenant, lake_table)


@app.post("/lake/maintenance", response_model=LakeMaintenanceResult)
async def post_lake_maintenance(
    request: Request, req: LakeMaintenanceRequest
) -> LakeMaintenanceResult:
    _header_tenant(request)
    return run_maintenance(req)


@app.post("/lake/drop_table")
async def post_lake_drop_table(request: Request, req: DropLakeTableRequest) -> dict[str, bool]:
    _header_tenant(request)
    dropped = drop_lake_table(req.tenant_id, req.lake_table)
    return {"dropped": dropped}


@app.delete("/lake/drop_table")
async def delete_lake_drop_table(
    request: Request,
    lake_table: Annotated[str, Query()],
    tenant_id: Annotated[str | None, Query()] = None,
) -> dict[str, bool]:
    eff_tenant = validate_tenant_id(tenant_id) if tenant_id else _header_tenant(request)
    dropped = drop_lake_table(eff_tenant, lake_table)
    return {"dropped": dropped}


@app.delete("/lake/tables/{lake_table}")
async def delete_lake_table_by_path(
    request: Request,
    lake_table: str,
    tenant_id: Annotated[str | None, Query()] = None,
) -> dict[str, bool]:
    eff_tenant = validate_tenant_id(tenant_id) if tenant_id else _header_tenant(request)
    dropped = drop_lake_table(eff_tenant, lake_table)
    return {"dropped": dropped}


@app.post("/render/chart")
async def post_render_chart(request: Request, req: RenderChartRequest) -> Response:
    _header_tenant(request)
    img_bytes, mime = await asyncio.to_thread(render_chart_bytes, req)
    return Response(content=img_bytes, media_type=mime)


@app.post("/export/{export_format}", response_model=SidecarExportResponse)
async def post_export_document(
    export_format: str,
    request: Request,
    req: SidecarExportRequest,
) -> SidecarExportResponse:
    fmt = normalize_export_format(export_format)
    raw_hdr_tenant = request.headers.get("x-axpoint-tenant", "").strip()
    body_tenant = validate_tenant_id(req.tenant_id)
    if raw_hdr_tenant:
        hdr_tenant = validate_tenant_id(raw_hdr_tenant)
        if hdr_tenant != body_tenant:
            raise SidecarError(
                403,
                "PERMISSION_DENIED",
                "x-axpoint-tenant header does not match request tenant_id",
            )
    return await asyncio.to_thread(export_document, fmt, req)


@app.post("/pii/analyze", response_model=PiiAnalyzeResponse)
async def post_pii_analyze(request: Request, req: PiiAnalyzeRequest) -> PiiAnalyzeResponse:
    _header_tenant(request)
    return await asyncio.to_thread(analyze_texts, req)


@app.post("/pii/anonymize", response_model=PiiAnonymizeResponse)
async def post_pii_anonymize(request: Request, req: PiiAnonymizeRequest) -> PiiAnonymizeResponse:
    raw_hdr_tenant = request.headers.get("x-axpoint-tenant", "").strip()
    if raw_hdr_tenant:
        hdr_tenant = validate_tenant_id(raw_hdr_tenant)
        if "tenant_id" in req.model_fields_set:
            body_tenant = validate_tenant_id(req.tenant_id)
            if hdr_tenant != body_tenant:
                raise SidecarError(
                    403,
                    "PERMISSION_DENIED",
                    "x-axpoint-tenant header does not match request tenant_id",
                )
        eff_tenant = hdr_tenant
    else:
        eff_tenant = validate_tenant_id(req.tenant_id)
    return await asyncio.to_thread(anonymize_texts, req, eff_tenant)

