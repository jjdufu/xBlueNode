from __future__ import annotations

import logging
from collections.abc import Mapping

from fastapi import FastAPI, File, HTTPException, Request, UploadFile
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from starlette.exceptions import HTTPException as StarletteHTTPException

from parsers.engine import parse_bytes_to_markdown
from schemas import ParseResponse
from security import InternalTokenMiddleware, error_json_response
from settings import configure_logging, get_model_path

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

app = FastAPI(
    title="AxPoint Parser Sidecar",
    version="0.1.0",
    description="Stateless PDF/OCR/Office to Markdown microservice for xBlueNode / AxPoint",
)

app.add_middleware(InternalTokenMiddleware)


@app.exception_handler(StarletteHTTPException)
async def handle_http_exception(
    request: Request, exc: StarletteHTTPException
) -> JSONResponse:
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
async def handle_validation_error(
    request: Request, exc: RequestValidationError
) -> JSONResponse:
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


@app.get("/health")
def health() -> dict[str, object]:
    model_path = get_model_path()
    easyocr_ready = (model_path / "easyocr").is_dir()
    return {
        "status": "ok",
        "service": "axpoint-parser",
        "model_path": str(model_path),
        "easyocr_models_present": easyocr_ready,
    }


@app.post("/parse", response_model=ParseResponse)
async def parse_document(file: UploadFile = File(...)) -> ParseResponse:  # noqa: B008
    raw = await file.read()
    if not raw:
        raise HTTPException(status_code=400, detail="Uploaded file is empty")

    filename = file.filename or "document.txt"
    try:
        title, markdown, engine = parse_bytes_to_markdown(filename, raw)
    except Exception as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    return ParseResponse(title=title, markdown=markdown, engine=engine)
