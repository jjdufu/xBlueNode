from __future__ import annotations

import hashlib
import os
import tempfile
import uuid
from pathlib import Path

from parsers.base import ParseContext
from parsers.registry import resolve_parser
from schemas import UnifiedParseResponse
from security import SidecarError, safe_tenant_dir, validate_tenant_id
from settings import get_data_root

DEFAULT_MODEL_PATH = os.getenv("AXPOINT_MODEL_PATH", "/var/lib/axpoint/models")


def parse_unified(
    filename: str,
    content: bytes,
    *,
    tenant_id: str = "default",
    timezone: str = "Asia/Shanghai",
    options: dict[str, str] | None = None,
    data_root: Path | None = None,
) -> UnifiedParseResponse:
    """Execute full dual-track parsing (Markdown + staged Parquet tables)."""
    if not content:
        raise SidecarError(400, "INVALID_ARGUMENT", "Uploaded file is empty")

    valid_tenant = validate_tenant_id(tenant_id)
    parse_id = str(uuid.uuid4())
    staging_dir = safe_tenant_dir(
        valid_tenant,
        "staging",
        parse_id,
        data_root=data_root or get_data_root(),
        create=True,
    )
    sha256_hex = hashlib.sha256(content).hexdigest()

    ctx = ParseContext(
        tenant_id=valid_tenant,
        parse_id=parse_id,
        staging_dir=staging_dir,
        filename=filename or "document.txt",
        options=dict(options or {}),
        timezone=timezone or "Asia/Shanghai",
        write_staging=True,
    )
    parser = resolve_parser(content, ctx)
    result = parser.parse(content, ctx)

    return UnifiedParseResponse(
        parse_id=parse_id,
        title=result.title,
        source_format=result.source_format,
        engine=result.engine,
        sha256=sha256_hex,
        markdown=result.markdown,
        tables=result.tables,
        warnings=ctx.warnings,
    )


def parse_bytes_to_markdown(filename: str, content: bytes) -> tuple[str, str, str]:
    """Convert uploaded file bytes into (title, markdown, engine_used) without staging Parquet."""
    path = Path(filename or "document.txt")
    suffix = path.suffix.lower()
    title = path.stem or "untitled"

    # Preserve Step 00 lightweight plain-utf8 behavior for text-like suffixes
    if suffix in {".md", ".markdown", ".txt", ".csv", ".json"} and not content.startswith(
        (b"PK\x03\x04", b"%PDF-")
    ):
        text = content.decode("utf-8", errors="replace").strip()
        return title, text, "plain-utf8"

    with tempfile.TemporaryDirectory() as tmp_str:
        ctx = ParseContext(
            tenant_id="default",
            parse_id=str(uuid.uuid4()),
            staging_dir=Path(tmp_str),
            filename=filename or "document.txt",
            options={},
            timezone="Asia/Shanghai",
            write_staging=False,
        )
        parser = resolve_parser(content, ctx)
        res = parser.parse(content, ctx)
        return res.title, res.markdown, res.engine
