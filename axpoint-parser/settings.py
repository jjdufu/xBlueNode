from __future__ import annotations

import logging
import os
import re
from collections.abc import Mapping
from pathlib import Path
from urllib.parse import unquote, urlparse

from pydantic import Field, ValidationError
from pydantic_settings import BaseSettings, SettingsConfigDict

REDACTED_VALUE = "***REDACTED***"

_SECRET_KEY_RE = re.compile(
    r"(?i)(password|passwd|secret|token|authorization|api[_-]?key)"
)
_JSON_KV_SECRET_RE = re.compile(
    r"""(?i)(["'](?:[^"']*?(?:password|passwd|secret|token|authorization|api[_-]?key)[^"']*?)["']\s*:\s*["'])([^"']*)(["'])"""
)
_ASSIGN_SECRET_RE = re.compile(
    r"""(?i)(\b(?:[a-z0-9_-]*?(?:password|passwd|secret|token|authorization|api[_-]?key)[a-z0-9_-]*)\b\s*(?:=|:)\s*(?:bearer\s+)?)([^\s"',;}\]]+)"""
)
_DSN_SECRET_RE = re.compile(r"(://[^:/@\s]+:)([^@\s]+)(@)")

_STANDARD_LOG_ATTRS = frozenset(
    logging.LogRecord("", 0, "", 0, "", (), None).__dict__.keys()
)


def _scrub_text(value: str) -> str:
    scrubbed = _JSON_KV_SECRET_RE.sub(rf"\1{REDACTED_VALUE}\3", value)
    scrubbed = _DSN_SECRET_RE.sub(rf"\1{REDACTED_VALUE}\3", scrubbed)
    scrubbed = _ASSIGN_SECRET_RE.sub(rf"\1{REDACTED_VALUE}", scrubbed)
    return scrubbed


def _scrub_obj(obj: object) -> object:
    if isinstance(obj, str):
        return _scrub_text(obj)
    if isinstance(obj, Mapping):
        cleaned: dict[object, object] = {}
        for k, v in obj.items():
            if isinstance(k, str) and _SECRET_KEY_RE.search(k):
                cleaned[k] = REDACTED_VALUE
            else:
                cleaned[k] = _scrub_obj(v)
        return cleaned
    if isinstance(obj, tuple):
        return tuple(_scrub_obj(item) for item in obj)
    if isinstance(obj, list):
        return [_scrub_obj(item) for item in obj]
    return obj


class SecretFilter(logging.Filter):
    """Mask sensitive credentials, tokens, and Authorization values in log records."""

    def filter(self, record: logging.LogRecord) -> bool:
        if isinstance(record.msg, str):
            record.msg = _scrub_text(record.msg)
        elif isinstance(record.msg, Mapping):
            record.msg = _scrub_obj(record.msg)

        if record.args:
            if isinstance(record.args, Mapping):
                scrubbed_map = _scrub_obj(record.args)
                if isinstance(scrubbed_map, Mapping):
                    record.args = dict(scrubbed_map)
            elif isinstance(record.args, tuple):
                record.args = tuple(_scrub_obj(arg) for arg in record.args)

        for attr_key in list(record.__dict__.keys()):
            if attr_key in _STANDARD_LOG_ATTRS:
                continue
            if _SECRET_KEY_RE.search(attr_key):
                record.__dict__[attr_key] = REDACTED_VALUE
            else:
                record.__dict__[attr_key] = _scrub_obj(record.__dict__[attr_key])

        return True


def configure_logging() -> None:
    """Attach SecretFilter idempotently to root and framework loggers."""
    secret_filter = SecretFilter()
    logger_names = ("", "uvicorn", "uvicorn.error", "uvicorn.access", "fastapi", "httpx")
    for name in logger_names:
        target = logging.getLogger(name)
        if not any(isinstance(flt, SecretFilter) for flt in target.filters):
            target.addFilter(secret_filter)
        for handler in target.handlers:
            if not any(isinstance(flt, SecretFilter) for flt in handler.filters):
                handler.addFilter(secret_filter)


class Settings(BaseSettings):
    """Runtime configuration for AxPoint Parser Sidecar."""

    model_config = SettingsConfigDict(extra="ignore")

    axpoint_env: str = "development"
    axpoint_internal_token: str = Field(min_length=32)
    axpoint_data_root: Path = Path("/var/lib/axpoint")
    axpoint_model_path: Path = Path("/var/lib/axpoint/models")
    axpoint_max_upload_mb: int = 512
    axpoint_lake_catalog_dsn: str = ""

    def lake_catalog_libpq(self) -> str:
        u = urlparse(self.axpoint_lake_catalog_dsn)
        parts = [
            f"dbname={u.path.lstrip('/')}",
            f"host={u.hostname or '127.0.0.1'}",
            f"port={u.port or 5432}",
        ]
        if u.username:
            parts.append(f"user={unquote(u.username)}")
        if u.password:
            parts.append(f"password={unquote(u.password)}")
        return " ".join(parts)


def get_settings() -> Settings:
    """Load and validate current runtime Settings from environment variables."""
    return Settings()  # type: ignore[call-arg]


def get_model_path() -> Path:
    """Resolve AXPOINT_MODEL_PATH via Settings, even if token is not yet configured."""
    try:
        return settings.axpoint_model_path
    except ValidationError:
        return Path(os.getenv("AXPOINT_MODEL_PATH", "/var/lib/axpoint/models"))


class _SettingsProxy:
    """Lazy proxy delegating attribute lookup to validated Settings."""

    def __getattr__(self, name: str) -> object:
        return getattr(get_settings(), name)


settings: Settings = _SettingsProxy()  # type: ignore[assignment]
