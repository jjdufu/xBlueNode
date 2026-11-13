from __future__ import annotations

import hmac
import logging

from fastapi import Request
from fastapi.responses import JSONResponse
from pydantic import ValidationError
from starlette.middleware.base import BaseHTTPMiddleware, RequestResponseEndpoint
from starlette.responses import Response

from settings import SecretFilter, configure_logging, settings

logger = logging.getLogger(__name__)

PUBLIC_PATHS: set[str] = {"/health"}


def extract_trace_id(request: Request) -> str:
    """Extract W3C traceparent or x-request-id for unified error correlation."""
    return request.headers.get("traceparent") or request.headers.get("x-request-id", "")


def error_json_response(
    status_code: int, error_code: str, message: str, request: Request
) -> JSONResponse:
    """Build unified 3-field error response body per A1 §5."""
    return JSONResponse(
        status_code=status_code,
        content={
            "error_code": error_code,
            "message": message,
            "trace_id": extract_trace_id(request),
        },
    )


class InternalTokenMiddleware(BaseHTTPMiddleware):
    """Enforce constant-time X-AxPoint-Internal-Token check on all non-public routes."""

    async def dispatch(
        self, request: Request, call_next: RequestResponseEndpoint
    ) -> Response:
        if request.url.path in PUBLIC_PATHS:
            return await call_next(request)

        try:
            expected = settings.axpoint_internal_token
        except ValidationError:
            expected = ""

        supplied = request.headers.get("x-axpoint-internal-token", "")
        if (
            len(expected) < 32
            or not supplied
            or not hmac.compare_digest(supplied.encode("utf-8"), expected.encode("utf-8"))
        ):
            return error_json_response(
                status_code=401,
                error_code="TOKEN_INVALID",
                message="internal token required",
                request=request,
            )

        try:
            return await call_next(request)
        except Exception:
            logger.exception("Unhandled exception in sidecar request %s", request.url.path)
            return error_json_response(
                status_code=500,
                error_code="INTERNAL",
                message="internal error",
                request=request,
            )


__all__ = [
    "InternalTokenMiddleware",
    "PUBLIC_PATHS",
    "SecretFilter",
    "configure_logging",
    "error_json_response",
    "extract_trace_id",
]
