from __future__ import annotations

import re
import time
from collections.abc import Callable, Iterator
from typing import Any
from urllib.parse import urljoin

import httpx
import pyarrow as pa
from jsonpath_ng import parse as parse_jsonpath  # type: ignore[attr-defined]
from tenacity import Retrying, retry_if_exception_type, stop_after_attempt, wait_exponential

from connectors.base import (
    BaseConnector,
    ReadBatch,
    format_cursor_value,
    records_to_arrow_table,
)
from connectors.webhook import extract_records_by_jsonpath
from parsers.json_parser import flatten_json_records
from schemas import ConnectorSpecModel, DiscoveredObject
from security import SidecarError, check_outbound_url

_LINK_NEXT_RE = re.compile(r'<([^>]+)>\s*;\s*rel="?next"?', re.IGNORECASE)

REST_API_SPEC = ConnectorSpecModel(
    connector_type="REST_API",
    display_name="REST HTTP API",
    category="API",
    config_schema={
        "type": "object",
        "required": ["base_url"],
        "properties": {
            "base_url": {"type": "string"},
            "auth_type": {
                "type": "string",
                "enum": [
                    "NONE",
                    "BEARER",
                    "API_KEY_HEADER",
                    "API_KEY_QUERY",
                    "BASIC",
                    "OAUTH2_CLIENT_CREDENTIALS",
                ],
                "default": "NONE",
            },
            "api_key_name": {"type": "string", "default": "X-API-Key"},
            "token_url": {"type": "string"},
            "scopes": {"type": "string"},
            "headers": {"type": "object"},
            "timeout_s": {"type": "number", "default": 30.0},
            "max_pages": {"type": "integer", "default": 1000},
            "openapi_url": {"type": "string"},
        },
    },
    secret_fields=["username", "password", "token", "api_key", "client_id", "client_secret"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)


class _TransientUpstreamError(Exception):
    """Internal marker triggering tenacity retry on 429 / 502 / 503 / 504."""


class RestApiConnector(BaseConnector):
    """Declarative REST HTTP connector with OAuth2, 5 pagination modes, retry, and SSRF guard."""

    spec = REST_API_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
        sleep_fn: Callable[[float], None] = time.sleep,
    ) -> None:
        super().__init__(
            config, secrets, allow_private_network=allow_private_network, tenant_id=tenant_id
        )
        base_url = str(self.config.get("base_url") or "").strip().rstrip("/")
        if not base_url:
            raise SidecarError(400, "INVALID_ARGUMENT", "REST_API config missing 'base_url'")
        check_outbound_url(base_url, allow_private_network=allow_private_network)
        self._base_url = base_url
        self._sleep = sleep_fn
        self._oauth_token: str | None = None
        self._oauth_expires_at: float = 0.0
        self._client = httpx.Client(
            timeout=min(max(float(self.config.get("timeout_s") or 30.0), 1.0), 300.0),
            follow_redirects=False,
            trust_env=False,
        )

    def _fetch_oauth2_token(self, *, force_refresh: bool = False) -> str:
        now = time.monotonic()
        if not force_refresh and self._oauth_token and now < self._oauth_expires_at:
            return self._oauth_token

        token_url = str(self.config.get("token_url") or "").strip()
        if not token_url:
            raise SidecarError(
                400, "INVALID_ARGUMENT", "OAUTH2_CLIENT_CREDENTIALS requires 'token_url'"
            )
        check_outbound_url(token_url, allow_private_network=self.allow_private_network)

        form_data: dict[str, str] = {
            "grant_type": "client_credentials",
            "client_id": str(self.secrets.get("client_id") or self.config.get("client_id") or ""),
            "client_secret": str(self.secrets.get("client_secret") or ""),
        }
        if self.config.get("scopes"):
            form_data["scope"] = str(self.config["scopes"])

        resp = self._send_ssrf_safe("POST", token_url, data=form_data, skip_auth=True)
        if resp.status_code >= 400:
            raise SidecarError(
                401, "TOKEN_INVALID", f"OAuth2 token request failed with HTTP {resp.status_code}"
            )
        body = resp.json()
        tok = str(body.get("access_token") or "")
        if not tok:
            raise SidecarError(401, "TOKEN_INVALID", "OAuth2 response missing access_token")
        expires_in = float(body.get("expires_in") or 3600.0)
        self._oauth_token = tok
        self._oauth_expires_at = now + max(expires_in - 60.0, 5.0)
        return tok

    def _apply_auth(
        self, headers: dict[str, str], params: dict[str, Any], *, force_refresh: bool = False
    ) -> tuple[str, str] | None:
        auth_type = str(self.config.get("auth_type") or "NONE").upper()
        if auth_type == "BEARER":
            tok = str(self.secrets.get("token") or "")
            headers["Authorization"] = f"Bearer {tok}"
        elif auth_type == "API_KEY_HEADER":
            key_name = str(self.config.get("api_key_name") or "X-API-Key")
            headers[key_name] = str(self.secrets.get("api_key") or "")
        elif auth_type == "API_KEY_QUERY":
            key_name = str(self.config.get("api_key_name") or "api_key")
            params[key_name] = str(self.secrets.get("api_key") or "")
        elif auth_type == "BASIC":
            user = str(self.secrets.get("username") or self.config.get("username") or "")
            pw = str(self.secrets.get("password") or "")
            return (user, pw)
        elif auth_type == "OAUTH2_CLIENT_CREDENTIALS":
            tok = self._fetch_oauth2_token(force_refresh=force_refresh)
            headers["Authorization"] = f"Bearer {tok}"
        return None

    def _send_ssrf_safe(
        self,
        method: str,
        url: str,
        *,
        params: dict[str, Any] | None = None,
        json_body: Any = None,
        data: dict[str, str] | None = None,
        extra_headers: dict[str, str] | None = None,
        skip_auth: bool = False,
    ) -> httpx.Response:
        """Send HTTP request with SSRF check on every redirect hop and retry on 429/502/503/504."""
        req_headers: dict[str, str] = {}
        cfg_headers = self.config.get("headers")
        if isinstance(cfg_headers, dict):
            for k, v in cfg_headers.items():
                req_headers[str(k)] = str(v)
        if extra_headers:
            req_headers.update(extra_headers)

        req_params: dict[str, Any] = dict(params or {})
        basic_auth = None if skip_auth else self._apply_auth(req_headers, req_params)

        def _attempt_once(
            cur_headers: dict[str, str], cur_params: dict[str, Any]
        ) -> httpx.Response:
            current_url = url
            current_method = method.upper()
            current_params: dict[str, Any] | None = cur_params
            for _hop in range(6):
                check_outbound_url(current_url, allow_private_network=self.allow_private_network)
                resp = self._client.request(
                    current_method,
                    current_url,
                    params=current_params,
                    json=json_body if current_method == "POST" else None,
                    data=data if current_method == "POST" else None,
                    headers=cur_headers,
                    auth=basic_auth,
                )
                if resp.status_code in {301, 302, 303, 307, 308} and "location" in resp.headers:
                    current_url = urljoin(str(resp.request.url), resp.headers["location"])
                    current_params = None
                    if resp.status_code == 303:
                        current_method = "GET"
                    continue
                if resp.status_code in {429, 502, 503, 504}:
                    retry_after = resp.headers.get("retry-after", "").strip()
                    if retry_after:
                        try:
                            wait_s = min(max(float(retry_after), 0.0), 10.0)
                            self._sleep(wait_s)
                        except ValueError:
                            pass
                    raise _TransientUpstreamError(f"HTTP {resp.status_code}")
                return resp
            raise SidecarError(502, "UPSTREAM_CONNECTOR", "exceeded maximum HTTP redirects (5)")

        retrying = Retrying(
            stop=stop_after_attempt(4),
            wait=wait_exponential(multiplier=0.05, min=0.05, max=1.0),
            retry=retry_if_exception_type((_TransientUpstreamError, httpx.TransportError)),
            sleep=self._sleep,
            reraise=True,
        )
        try:
            resp = retrying(_attempt_once, req_headers, req_params)
        except _TransientUpstreamError as exc:
            raise SidecarError(
                502, "UPSTREAM_CONNECTOR", f"upstream API exhausted retries: {exc}"
            ) from exc
        except httpx.HTTPError as exc:
            raise SidecarError(502, "UPSTREAM_CONNECTOR", f"HTTP transport error: {exc}") from exc

        # Single refresh retry if OAuth2 access token expired mid-flight
        auth_type = str(self.config.get("auth_type") or "NONE").upper()
        if not skip_auth and resp.status_code == 401 and auth_type == "OAUTH2_CLIENT_CREDENTIALS":
            self._apply_auth(req_headers, req_params, force_refresh=True)
            resp = _attempt_once(req_headers, req_params)

        return resp

    def test(self) -> tuple[bool, str]:
        probe_path = str(self.config.get("health_path") or "").lstrip("/")
        target = f"{self._base_url}/{probe_path}" if probe_path else self._base_url
        try:
            resp = self._send_ssrf_safe("GET", target)
            if resp.status_code < 400:
                return True, f"HTTP {resp.status_code}"
            return False, f"HTTP {resp.status_code}"
        except SidecarError as exc:
            if exc.error_code == "PERMISSION_DENIED":
                raise
            return False, exc.message
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        openapi_url = str(self.config.get("openapi_url") or "").strip()
        if not openapi_url:
            return []
        full_url = (
            openapi_url
            if openapi_url.startswith(("http://", "https://"))
            else f"{self._base_url}/{openapi_url.lstrip('/')}"
        )
        resp = self._send_ssrf_safe("GET", full_url)
        if resp.status_code >= 400:
            return []
        spec_doc = resp.json()
        paths = spec_doc.get("paths") or {}
        out: list[DiscoveredObject] = []
        for path_key, methods in sorted(paths.items()):
            if isinstance(methods, dict) and "get" in methods:
                op = methods["get"]
                summary = str(op.get("summary") or path_key)
                out.append(
                    DiscoveredObject(
                        ref={"path": str(path_key), "method": "GET", "record_path": "$"},
                        display_name=summary,
                        source_kind="API_RESOURCE",
                        columns=[],
                    )
                )
        return out

    def _resolve_endpoint_url(self, ref: dict[str, Any]) -> str:
        raw_path = str(ref.get("path") or "").strip()
        if raw_path.startswith(("http://", "https://")):
            return raw_path
        if raw_path:
            return f"{self._base_url}/{raw_path.lstrip('/')}"
        return self._base_url

    def _iter_pages(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        max_records: int | None = None,
    ) -> Iterator[list[dict[str, Any]]]:
        method = str(ref.get("method") or "GET").upper()
        if method not in {"GET", "POST"}:
            raise SidecarError(400, "INVALID_ARGUMENT", f"REST_API method {method!r} not permitted")

        url = self._resolve_endpoint_url(ref)
        record_path = str(ref.get("record_path") or self.config.get("record_path") or "$")
        base_params: dict[str, Any] = dict(ref.get("params") or {})
        body_template = ref.get("body")

        # Incremental cursor parameter injection if configured
        cursor_param_in_query = ref.get("since_param") or self.config.get("since_param")
        if mode.upper() in {"APPEND", "UPSERT"} and cursor_from and cursor_param_in_query:
            base_params[str(cursor_param_in_query)] = cursor_from

        pag_cfg: dict[str, Any] = dict(
            ref.get("pagination") or self.config.get("pagination") or {"type": "NONE"}
        )
        pag_type = str(pag_cfg.get("type") or "NONE").upper()
        max_pages = min(max(int(self.config.get("max_pages") or 1000), 1), 10_000)

        emitted = 0
        page_num = int(pag_cfg.get("start_page", 1))
        page_size = int(pag_cfg.get("page_size") or pag_cfg.get("limit") or 100)
        offset = int(pag_cfg.get("start_offset", 0))
        next_cursor: str | None = None
        next_link_url: str | None = url

        for page_idx in range(max_pages):
            params = dict(base_params)
            req_url = url

            if pag_type == "PAGE_NUMBER":
                p_param = str(pag_cfg.get("page_param") or "page")
                s_param = str(pag_cfg.get("size_param") or "page_size")
                params[p_param] = page_num
                params[s_param] = page_size
            elif pag_type == "OFFSET":
                o_param = str(pag_cfg.get("offset_param") or "offset")
                l_param = str(pag_cfg.get("limit_param") or "limit")
                params[o_param] = offset
                params[l_param] = page_size
            elif pag_type == "CURSOR":
                if page_idx > 0:
                    if not next_cursor:
                        break
                    c_param = str(pag_cfg.get("cursor_param") or "cursor")
                    params[c_param] = next_cursor
            elif pag_type == "LINK_HEADER":
                if not next_link_url:
                    break
                req_url = next_link_url
                if page_idx > 0:
                    params = {}

            resp = self._send_ssrf_safe(method, req_url, params=params, json_body=body_template)
            if resp.status_code >= 400:
                raise SidecarError(
                    502,
                    "UPSTREAM_CONNECTOR",
                    f"REST endpoint returned HTTP {resp.status_code}",
                )
            payload = resp.json()
            raw_records = extract_records_by_jsonpath(payload, record_path)
            if not raw_records:
                break

            # Client-side cursor filter when API has no server-side since_param
            if (
                mode.upper() in {"APPEND", "UPSERT"}
                and cursor_column
                and cursor_from is not None
                and not cursor_param_in_query
            ):
                filtered: list[dict[str, Any]] = []
                for r in raw_records:
                    rv = r.get(cursor_column)
                    if rv is None:
                        continue
                    rv_s = str(rv)
                    if (mode.upper() == "UPSERT" and rv_s >= cursor_from) or (
                        mode.upper() == "APPEND" and rv_s > cursor_from
                    ):
                        filtered.append(r)
                page_out = filtered
            else:
                page_out = raw_records

            if page_out:
                if max_records is not None and emitted + len(page_out) > max_records:
                    yield page_out[: max_records - emitted]
                    return
                emitted += len(page_out)
                yield page_out

            if pag_type == "NONE":
                break
            if pag_type == "PAGE_NUMBER":
                if len(raw_records) < page_size:
                    break
                page_num += 1
            elif pag_type == "OFFSET":
                if len(raw_records) < page_size:
                    break
                offset += len(raw_records)
            elif pag_type == "CURSOR":
                c_path = str(pag_cfg.get("next_cursor_path") or "$.next_cursor")
                matches = [m.value for m in parse_jsonpath(c_path).find(payload)]
                cand = str(matches[0]) if matches and matches[0] is not None else ""
                if not cand or cand == next_cursor:
                    break
                next_cursor = cand
            elif pag_type == "LINK_HEADER":
                link_hdr = resp.headers.get("link", "")
                m_link = _LINK_NEXT_RE.search(link_hdr)
                if not m_link:
                    break
                next_link_url = urljoin(str(resp.request.url), m_link.group(1))

    def _flatten_to_arrow(self, docs: list[dict[str, Any]]) -> pa.Table:
        if not docs:
            return pa.table({})
        flat = flatten_json_records(docs, json_explode="NONE")
        row_dicts = [dict(zip(flat.main_columns, r, strict=False)) for r in flat.main_rows]
        return records_to_arrow_table(row_dicts, column_order=flat.main_columns)

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        capped = min(max(limit, 1), 1000)
        collected: list[dict[str, Any]] = []
        for page in self._iter_pages(ref, "SNAPSHOT", None, None, max_records=capped):
            collected.extend(page)
            if len(collected) >= capped:
                break
        return self._flatten_to_arrow(collected[:capped])

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        step = max(batch_rows, 1)
        buf: list[dict[str, Any]] = []
        max_cur: str | None = None

        for page in self._iter_pages(ref, mode, cursor_column, cursor_from):
            buf.extend(page)
            if cursor_column:
                for r in page:
                    v = r.get(cursor_column)
                    if v is not None:
                        cand = format_cursor_value(v)
                        if cand and (max_cur is None or cand > max_cur):
                            max_cur = cand
            if len(buf) >= step:
                tbl = self._flatten_to_arrow(buf)
                for rb in tbl.to_batches(max_chunksize=step):
                    yield ReadBatch(batch=rb, cursor_to=max_cur)
                buf.clear()

        if buf:
            tbl = self._flatten_to_arrow(buf)
            for rb in tbl.to_batches(max_chunksize=step):
                yield ReadBatch(batch=rb, cursor_to=max_cur)

    def close(self) -> None:
        self._client.close()
