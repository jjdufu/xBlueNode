from __future__ import annotations

import hmac
import io
import ipaddress
import logging
import re
import socket
import zipfile
from pathlib import Path
from urllib.parse import urljoin, urlparse

import defusedxml.ElementTree as DefusedET
import httpx
from defusedxml.common import DefusedXmlException
from fastapi import Request, UploadFile
from fastapi.responses import JSONResponse
from lxml import etree
from pydantic import ValidationError
from starlette.middleware.base import BaseHTTPMiddleware, RequestResponseEndpoint
from starlette.responses import Response

from settings import (
    SecretFilter,
    configure_logging,
    get_data_root,
    get_upload_max_bytes,
    settings,
)

logger = logging.getLogger(__name__)

PUBLIC_PATHS: set[str] = {"/health"}

TENANT_ID_RE = re.compile(r"^[a-z0-9_-]{1,64}$")
LAKE_TABLE_RE = re.compile(r"^[a-z][a-z0-9_]{0,62}$")
SQL_IDENT_RE = re.compile(r"^[A-Za-z_\u4e00-\u9fff][A-Za-z0-9_\u4e00-\u9fff$]{0,127}$")

_DOCTYPE_ENTITY_RE = re.compile(rb"<!DOCTYPE\b|<!ENTITY\b", re.IGNORECASE)

_CGNAT_NET = ipaddress.ip_network("100.64.0.0/10")
_METADATA_NET = ipaddress.ip_network("169.254.0.0/16")

DEFAULT_ZIP_MAX_ENTRIES = 10_000
DEFAULT_ZIP_MAX_UNCOMPRESSED_BYTES = 512 * 1024 * 1024
DEFAULT_ZIP_MAX_RATIO = 100.0


class SidecarError(Exception):
    """Structured Sidecar error carrying HTTP status and canonical error_code."""

    def __init__(self, status_code: int, error_code: str, message: str) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.error_code = error_code
        self.message = message


class OutboundBlocked(SidecarError):
    """Raised when an outbound host or URL resolves to a forbidden network address."""

    def __init__(self, message: str) -> None:
        super().__init__(status_code=403, error_code="PERMISSION_DENIED", message=message)


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


def validate_tenant_id(tenant_id: str) -> str:
    """Validate tenant_id against ^[a-z0-9_-]{1,64}$."""
    cleaned = tenant_id.strip()
    if not TENANT_ID_RE.fullmatch(cleaned):
        raise SidecarError(
            status_code=400,
            error_code="INVALID_ARGUMENT",
            message=f"invalid tenant_id: {tenant_id!r}",
        )
    return cleaned


def validate_lake_table(table_name: str) -> str:
    """Validate lake table identifier against ^[a-z][a-z0-9_]{0,62}$."""
    cleaned = table_name.strip()
    if not LAKE_TABLE_RE.fullmatch(cleaned):
        raise SidecarError(
            status_code=400,
            error_code="INVALID_ARGUMENT",
            message=f"invalid lake table name: {table_name!r}",
        )
    return cleaned


def validate_sql_ident(ident: str) -> str:
    """Validate SQL identifier against strict whitelist before quoting."""
    if not ident or not SQL_IDENT_RE.fullmatch(ident):
        raise SidecarError(
            status_code=400,
            error_code="SQL_REJECTED",
            message=f"invalid SQL identifier: {ident!r}",
        )
    return ident


def quote_sql_ident(ident: str, dialect: str = "postgres") -> str:
    """Validate and wrap SQL identifier in dialect-specific quotes."""
    valid = validate_sql_ident(ident)
    d = dialect.lower()
    if d in {"mysql", "mariadb", "tidb", "oceanbase", "doris", "starrocks", "hive"}:
        return "`" + valid.replace("`", "``") + "`"
    if d in {"mssql", "tsql"}:
        return "[" + valid.replace("]", "]]") + "]"
    return '"' + valid.replace('"', '""') + '"'


def safe_tenant_dir(
    tenant_id: str,
    *subparts: str,
    data_root: Path | None = None,
    create: bool = True,
) -> Path:
    """Resolve a directory inside {AXPOINT_DATA_ROOT}/tenants/{tenant_id}/ safely."""
    valid_tenant = validate_tenant_id(tenant_id)
    root = (data_root or get_data_root()).resolve()
    for part in subparts:
        if not part or part in {".", ".."} or "/" in part or "\\" in part or "\x00" in part:
            raise SidecarError(
                status_code=400,
                error_code="INVALID_ARGUMENT",
                message=f"unsafe path segment: {part!r}",
            )
    target = root.joinpath("tenants", valid_tenant, *subparts).resolve()
    if not target.is_relative_to(root):
        raise SidecarError(
            status_code=400,
            error_code="INVALID_ARGUMENT",
            message="resolved path escapes AXPOINT_DATA_ROOT",
        )
    if create:
        target.mkdir(parents=True, exist_ok=True)
    return target


def resolve_path_under_root(
    raw_path: str | Path,
    *,
    data_root: Path | None = None,
    required_prefix: Path | None = None,
) -> Path:
    """Ensure a relative or absolute path resolves strictly inside AXPOINT_DATA_ROOT."""
    path_str = str(raw_path).strip()
    if not path_str or "\x00" in path_str:
        raise SidecarError(400, "INVALID_ARGUMENT", "empty or NUL path")
    root = (data_root or get_data_root()).resolve()
    p = Path(path_str)
    candidate = p.resolve() if p.is_absolute() else (root / p).resolve()
    if not candidate.is_relative_to(root):
        raise SidecarError(
            status_code=403,
            error_code="PERMISSION_DENIED",
            message="path escapes AXPOINT_DATA_ROOT",
        )
    if required_prefix is not None:
        req = required_prefix.resolve()
        if not candidate.is_relative_to(req):
            raise SidecarError(
                status_code=403,
                error_code="PERMISSION_DENIED",
                message="path is outside permitted tenant subdirectory",
            )
    return candidate


async def read_limited(upload: UploadFile, max_bytes: int | None = None) -> bytes:
    """Stream upload in chunks, aborting with 413 UPLOAD_TOO_LARGE if limit exceeded."""
    limit = max_bytes if max_bytes is not None else get_upload_max_bytes()
    buf = bytearray()
    chunk_size = 65_536
    while True:
        chunk = await upload.read(chunk_size)
        if not chunk:
            break
        buf.extend(chunk)
        if len(buf) > limit:
            raise SidecarError(
                status_code=413,
                error_code="UPLOAD_TOO_LARGE",
                message=f"upload exceeds maximum allowed size of {limit} bytes",
            )
    return bytes(buf)


def check_zip(
    data: bytes,
    *,
    max_entries: int = DEFAULT_ZIP_MAX_ENTRIES,
    max_total_uncompressed: int = DEFAULT_ZIP_MAX_UNCOMPRESSED_BYTES,
    max_ratio: float = DEFAULT_ZIP_MAX_RATIO,
) -> None:
    """Validate ZIP/OOXML/ODF archive against entry count, total size, and compression ratio."""
    try:
        zf = zipfile.ZipFile(io.BytesIO(data), "r")
    except zipfile.BadZipFile as exc:
        raise SidecarError(
            status_code=422,
            error_code="PARSE_MALFORMED",
            message=f"corrupted archive: {exc}",
        ) from exc

    with zf:
        infos = zf.infolist()
        if len(infos) > max_entries:
            raise SidecarError(
                status_code=413,
                error_code="ARCHIVE_BOMB",
                message=f"archive entry count {len(infos)} exceeds limit {max_entries}",
            )

        total_uncompressed = 0
        for info in infos:
            if ".." in Path(info.filename).parts or info.filename.startswith(("/", "\\")):
                raise SidecarError(
                    status_code=422,
                    error_code="PARSE_MALFORMED",
                    message=f"unsafe archive entry path: {info.filename}",
                )
            uncompressed = max(info.file_size, 0)
            compressed = max(info.compress_size, 1)
            total_uncompressed += uncompressed
            if total_uncompressed > max_total_uncompressed:
                raise SidecarError(
                    status_code=413,
                    error_code="ARCHIVE_BOMB",
                    message=(
                        f"archive uncompressed size {total_uncompressed} exceeds "
                        f"limit {max_total_uncompressed}"
                    ),
                )
            if uncompressed > 1024 and (uncompressed / compressed) > max_ratio:
                raise SidecarError(
                    status_code=413,
                    error_code="ARCHIVE_BOMB",
                    message=(
                        f"archive entry {info.filename!r} compression ratio "
                        f"{uncompressed / compressed:.1f} exceeds {max_ratio}"
                    ),
                )


def safe_xml_parser() -> etree.XMLParser:
    """Build hardened lxml XMLParser disabling entity expansion, network, DTD, and huge trees."""
    return etree.XMLParser(
        resolve_entities=False,
        no_network=True,
        load_dtd=False,
        dtd_validation=False,
        huge_tree=False,
        recover=False,
    )


def check_xml_safe(data: bytes) -> None:
    """Reject DTD/ENTITY payloads (XXE, Billion Laughs) via pre-scan and defusedxml."""
    head_scan = data[: min(len(data), 1024 * 1024)]
    if _DOCTYPE_ENTITY_RE.search(head_scan):
        raise SidecarError(
            status_code=422,
            error_code="XML_UNSAFE",
            message="XML DOCTYPE and ENTITY declarations are forbidden",
        )
    if len(data) <= 16 * 1024 * 1024:
        try:
            for _event, _elem in DefusedET.iterparse(
                io.BytesIO(data),
                events=("start",),
                forbid_dtd=True,
                forbid_entities=True,
                forbid_external=True,
            ):
                _elem.clear()
        except DefusedXmlException as exc:
            raise SidecarError(
                status_code=422,
                error_code="XML_UNSAFE",
                message=f"unsafe XML construct rejected: {type(exc).__name__}",
            ) from exc
        except Exception as exc:
            raise SidecarError(
                status_code=422,
                error_code="PARSE_MALFORMED",
                message=f"malformed XML document: {exc}",
            ) from exc


def parse_xml_bytes_safe(data: bytes) -> etree._Element:
    """Validate and parse XML bytes into a hardened lxml Element."""
    check_xml_safe(data)
    parser = safe_xml_parser()
    try:
        return etree.fromstring(data, parser=parser)
    except etree.XMLSyntaxError as exc:
        msg = str(exc)
        if "entity" in msg.lower() or "doctype" in msg.lower():
            raise SidecarError(422, "XML_UNSAFE", "unsafe XML entity or DTD") from exc
        raise SidecarError(422, "PARSE_MALFORMED", f"XML syntax error: {msg}") from exc


def _is_forbidden_ip(
    ip_str: str,
    *,
    allow_private_network: bool,
) -> tuple[bool, str]:
    addr = ipaddress.ip_address(ip_str)
    if isinstance(addr, ipaddress.IPv6Address) and addr.ipv4_mapped is not None:
        addr = addr.ipv4_mapped

    # Cloud metadata / link-local is ALWAYS blocked regardless of allow_private_network
    if addr.is_link_local or (isinstance(addr, ipaddress.IPv4Address) and addr in _METADATA_NET):
        return True, f"link-local / cloud metadata address forbidden: {addr}"

    if addr.is_unspecified or addr.is_multicast:
        return True, f"unspecified or multicast address forbidden: {addr}"

    if not allow_private_network:
        if addr.is_loopback:
            return True, f"loopback address forbidden: {addr}"
        if isinstance(addr, ipaddress.IPv4Address) and addr in _CGNAT_NET:
            return True, f"carrier-grade NAT address forbidden: {addr}"
        if addr.is_private or addr.is_reserved:
            return True, f"private/reserved network address forbidden: {addr}"

    return False, ""


def check_outbound_host(
    host: str,
    port: int | None = None,
    *,
    allow_private_network: bool = False,
) -> list[str]:
    """Resolve hostname via DNS and block private/loopback/link-local targets unless permitted."""
    cleaned_host = host.strip().strip("[]")
    if not cleaned_host:
        raise OutboundBlocked("empty outbound host")

    # Fast check if host is already a literal IP
    try:
        ipaddress.ip_address(cleaned_host)
    except ValueError:
        pass
    else:
        blocked, reason = _is_forbidden_ip(
            cleaned_host, allow_private_network=allow_private_network
        )
        if blocked:
            raise OutboundBlocked(reason)
        return [cleaned_host]

    if cleaned_host.lower() == "localhost" and not allow_private_network:
        raise OutboundBlocked("localhost is forbidden when allow_private_network=false")

    try:
        infos = socket.getaddrinfo(cleaned_host, port or 80, proto=socket.IPPROTO_TCP)
    except socket.gaierror as exc:
        raise OutboundBlocked(f"DNS resolution failed for {cleaned_host!r}: {exc}") from exc

    if not infos:
        raise OutboundBlocked(f"DNS returned no addresses for {cleaned_host!r}")

    resolved_ips: list[str] = []
    for info in infos:
        sockaddr = info[4]
        ip_str = str(sockaddr[0])
        blocked, reason = _is_forbidden_ip(ip_str, allow_private_network=allow_private_network)
        if blocked:
            raise OutboundBlocked(f"host {cleaned_host!r} resolved to forbidden IP ({reason})")
        if ip_str not in resolved_ips:
            resolved_ips.append(ip_str)
    return resolved_ips


def check_outbound_url(
    url: str,
    *,
    allow_private_network: bool = False,
    allowed_schemes: tuple[str, ...] = ("http", "https"),
) -> str:
    """Validate URL scheme and resolved target IP addresses against SSRF policy."""
    parsed = urlparse(url)
    scheme = parsed.scheme.lower()
    if scheme not in allowed_schemes:
        raise OutboundBlocked(f"disallowed URL scheme: {parsed.scheme!r}")
    if not parsed.hostname:
        raise OutboundBlocked("URL is missing hostname")
    default_port = 443 if scheme == "https" else 80
    try:
        port = parsed.port or default_port
    except ValueError as exc:
        raise OutboundBlocked(f"invalid URL port: {exc}") from exc

    check_outbound_host(
        parsed.hostname,
        port,
        allow_private_network=allow_private_network,
    )
    return url


class SafeOutboundTransport(httpx.BaseTransport):
    """HTTP transport verifying SSRF rules on initial request and every redirect hop."""

    def __init__(
        self,
        *,
        allow_private_network: bool = False,
        verify_tls: bool = True,
    ) -> None:
        self._allow_private_network = allow_private_network
        self._inner = httpx.HTTPTransport(verify=verify_tls)

    def handle_request(self, request: httpx.Request) -> httpx.Response:
        check_outbound_url(
            str(request.url),
            allow_private_network=self._allow_private_network,
        )
        response = self._inner.handle_request(request)
        if response.is_redirect and "location" in response.headers:
            next_url = urljoin(str(request.url), response.headers["location"])
            check_outbound_url(
                next_url,
                allow_private_network=self._allow_private_network,
            )
        return response

    def close(self) -> None:
        self._inner.close()


class InternalTokenMiddleware(BaseHTTPMiddleware):
    """Enforce constant-time X-AxPoint-Internal-Token check on all non-public routes."""

    async def dispatch(self, request: Request, call_next: RequestResponseEndpoint) -> Response:
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
        except SidecarError as exc:
            return error_json_response(
                status_code=exc.status_code,
                error_code=exc.error_code,
                message=exc.message,
                request=request,
            )
        except Exception:
            logger.exception("Unhandled exception in sidecar request %s", request.url.path)
            return error_json_response(
                status_code=500,
                error_code="INTERNAL",
                message="internal error",
                request=request,
            )


__all__ = [
    "DEFAULT_ZIP_MAX_ENTRIES",
    "DEFAULT_ZIP_MAX_RATIO",
    "DEFAULT_ZIP_MAX_UNCOMPRESSED_BYTES",
    "InternalTokenMiddleware",
    "LAKE_TABLE_RE",
    "OutboundBlocked",
    "PUBLIC_PATHS",
    "SQL_IDENT_RE",
    "SafeOutboundTransport",
    "SecretFilter",
    "SidecarError",
    "TENANT_ID_RE",
    "check_outbound_host",
    "check_outbound_url",
    "check_xml_safe",
    "check_zip",
    "configure_logging",
    "error_json_response",
    "extract_trace_id",
    "parse_xml_bytes_safe",
    "quote_sql_ident",
    "read_limited",
    "resolve_path_under_root",
    "safe_tenant_dir",
    "safe_xml_parser",
    "validate_lake_table",
    "validate_sql_ident",
    "validate_tenant_id",
]
