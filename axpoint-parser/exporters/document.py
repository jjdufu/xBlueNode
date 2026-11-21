from __future__ import annotations

import hashlib
import logging
import os
import re
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from datetime import UTC, date, datetime
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any, Literal
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

import orjson
import pyarrow.parquet as pq

from schemas import (
    DataType,
    ExportChart,
    ExportColumnSpec,
    ExportDocument,
    ExportFormatLiteral,
    ExportKpi,
    ExportSectionUnion,
    ExportTable,
    ExportText,
    OrientationLiteral,
    PageSizeLiteral,
    RenderChartRequest,
    SidecarExportRequest,
    SidecarExportResponse,
)
from security import SidecarError, resolve_path_under_root, validate_tenant_id
from settings import get_data_root

logger = logging.getLogger(__name__)

PARSER_ROOT = Path(__file__).resolve().parent.parent

ExportSection = ExportSectionUnion

_JOB_ID_RE = re.compile(r"^[A-Za-z0-9_-]{1,128}$")
_EXT_RE = re.compile(r"^[a-z0-9]{1,16}$")
_UNSAFE_FILENAME_CHARS_RE = re.compile(r'[\x00-\x1f\x7f/\\:*?"<>|]+')
_WHITESPACE_RE = re.compile(r"\s+")
_NUMERIC_KPI_RE = re.compile(r"^[+-]?\d[\d,]*(?:\.\d+)?$")


class ExportHeading(ExportText):
    """Heading section convenience model (`kind = 'HEADING'`)."""

    kind: Literal["HEADING"] = "HEADING"


class ExportParagraph(ExportText):
    """Paragraph section convenience model (`kind = 'PARAGRAPH'`)."""

    kind: Literal["PARAGRAPH"] = "PARAGRAPH"


class ExportMarkdown(ExportText):
    """Markdown section convenience model (`kind = 'MARKDOWN'`)."""

    kind: Literal["MARKDOWN"] = "MARKDOWN"


@dataclass(frozen=True, slots=True)
class ExportResult:
    """Artifact metadata returned by an individual format exporter."""

    file_path: Path
    file_size: int
    sha256: str
    file_name: str


def validate_job_id(job_id: str) -> str:
    """Validate export job identifier against path-safe whitelist."""
    cleaned = job_id.strip()
    if not _JOB_ID_RE.fullmatch(cleaned):
        raise SidecarError(
            400,
            "INVALID_ARGUMENT",
            f"invalid export job_id: {job_id!r}",
        )
    return cleaned


def resolve_export_target_path(
    tenant_id: str,
    job_id: str,
    ext: str,
    *,
    data_root: Path | None = None,
) -> Path:
    """Build safe destination `{AXPOINT_DATA_ROOT}/exports/{tenant_id}/{job_id}.{ext}`."""
    valid_tenant = validate_tenant_id(tenant_id)
    valid_job = validate_job_id(job_id)
    clean_ext = ext.lstrip(".").lower()
    if not _EXT_RE.fullmatch(clean_ext):
        raise SidecarError(400, "INVALID_ARGUMENT", f"invalid export extension: {ext!r}")

    root = (data_root or get_data_root()).resolve()
    out_dir = (root / "exports" / valid_tenant).resolve()
    if not out_dir.is_relative_to(root):
        raise SidecarError(403, "PERMISSION_DENIED", "export path escapes AXPOINT_DATA_ROOT")
    out_dir.mkdir(parents=True, exist_ok=True)
    return out_dir / f"{valid_job}.{clean_ext}"


def build_download_filename(title: str, ext: str) -> str:
    """Build clean UTF-8 filename preserving CJK and alphanumeric characters."""
    clean_ext = ext.lstrip(".").lower()
    stem = _UNSAFE_FILENAME_CHARS_RE.sub("_", title.strip())
    stem = _WHITESPACE_RE.sub(" ", stem).strip(" ._")
    if not stem:
        stem = "export"
    if len(stem) > 100:
        stem = stem[:100].rstrip(" ._") or "export"
    return f"{stem}.{clean_ext}"


def compute_file_size_and_sha256(path: Path) -> tuple[int, str]:
    """Stream file in 1 MiB chunks to compute `(file_size, sha256_hex)`."""
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as fh:
        while True:
            chunk = fh.read(1024 * 1024)
            if not chunk:
                break
            size += len(chunk)
            digest.update(chunk)
    return size, digest.hexdigest()


def atomic_write_export(
    out_path: Path,
    writer_fn: Callable[[Path], None],
) -> tuple[int, str]:
    """Write via `<out_path>.part`, compute size & sha256, and atomically `os.replace`."""
    out_path.parent.mkdir(parents=True, exist_ok=True)
    part_path = out_path.with_name(f"{out_path.name}.part")
    part_path.unlink(missing_ok=True)
    try:
        writer_fn(part_path)
        if not part_path.is_file():
            raise SidecarError(500, "INTERNAL", "exporter failed to materialize output file")
        file_size, sha256_hex = compute_file_size_and_sha256(part_path)
        os.replace(part_path, out_path)
        return file_size, sha256_hex
    except Exception:
        part_path.unlink(missing_ok=True)
        raise


def resolve_template_file(raw_path: str, *, expected_suffix: str) -> Path:
    """Validate and resolve a `.docx` or `.pptx` user/test template path."""
    cleaned = raw_path.strip()
    if not cleaned or "\x00" in cleaned:
        raise SidecarError(400, "INVALID_ARGUMENT", "empty or NUL template_path")

    raw_p = Path(cleaned)
    if not raw_p.is_absolute() and ".." in raw_p.parts:
        raise SidecarError(403, "PERMISSION_DENIED", "path traversal forbidden in template_path")

    if raw_p.is_absolute():
        candidate = raw_p.resolve()
    else:
        data_candidate = (get_data_root().resolve() / raw_p).resolve()
        repo_candidate = (PARSER_ROOT / raw_p).resolve()
        candidate = data_candidate if data_candidate.is_file() else repo_candidate

    if candidate.suffix.lower() != expected_suffix.lower():
        raise SidecarError(
            400,
            "INVALID_ARGUMENT",
            f"template_path must end with {expected_suffix}, got {candidate.name!r}",
        )
    if not candidate.is_file():
        raise SidecarError(
            400,
            "INVALID_ARGUMENT",
            f"template file does not exist: {raw_path!r}",
        )
    return candidate


def iter_table_rows(
    table: ExportTable,
    *,
    data_root: Path | None = None,
    batch_size: int = 10_000,
) -> Iterator[list[Any]]:
    """Yield rows lazily from inline `table.rows` or a Parquet spill file under data_root."""
    if table.rows_parquet_path:
        pq_path = resolve_path_under_root(table.rows_parquet_path, data_root=data_root)
        if not pq_path.is_file():
            raise SidecarError(
                400,
                "INVALID_ARGUMENT",
                f"Parquet staging file not found: {table.rows_parquet_path!r}",
            )
        pf = pq.ParquetFile(pq_path)
        for batch in pf.iter_batches(batch_size=batch_size):
            num_cols = batch.num_columns
            num_rows = batch.num_rows
            if num_cols == 0 or num_rows == 0:
                continue
            col_lists = [col.to_pylist() for col in batch.columns]
            for row_idx in range(num_rows):
                yield [col_lists[col_idx][row_idx] for col_idx in range(num_cols)]
        return

    for row in table.rows:
        yield list(row)


def cleanup_document_parquet_files(
    doc: ExportDocument,
    *,
    data_root: Path | None = None,
) -> None:
    """Best-effort cleanup of temporary `rows_parquet_path` files inside `AXPOINT_DATA_ROOT`."""
    candidates: list[str] = []
    for section in doc.sections:
        if isinstance(section, ExportTable) and section.rows_parquet_path:
            candidates.append(section.rows_parquet_path)
        elif (
            isinstance(section, ExportChart)
            and section.data_table is not None
            and section.data_table.rows_parquet_path
        ):
            candidates.append(section.data_table.rows_parquet_path)

    for raw_path in candidates:
        try:
            resolved = resolve_path_under_root(raw_path, data_root=data_root)
            resolved.unlink(missing_ok=True)
        except SidecarError:
            logger.warning("Skipped unlinking out-of-root parquet path: %s", raw_path)
        except OSError as exc:
            logger.warning("Failed to unlink temporary parquet file %s: %s", raw_path, exc)


def resolve_zoneinfo(tz_name: str) -> ZoneInfo:
    """Resolve IANA timezone name, falling back safely to UTC."""
    cleaned = tz_name.strip() or "UTC"
    try:
        return ZoneInfo(cleaned)
    except ZoneInfoNotFoundError:
        return ZoneInfo("UTC")


def format_timestamp_tz(dt: datetime, tz_name: str = "UTC") -> str:
    """Format `datetime` in target timezone as `YYYY-MM-DD HH:MM:SS`."""
    tz = resolve_zoneinfo(tz_name)
    aware = dt.replace(tzinfo=UTC) if dt.tzinfo is None else dt
    return aware.astimezone(tz).strftime("%Y-%m-%d %H:%M:%S")


def build_placeholder_map(doc: ExportDocument) -> dict[str, str]:
    """Build merged `{{key}}` replacement map (explicit `doc.placeholders` override defaults)."""
    ts_str = format_timestamp_tz(doc.generated_at, doc.timezone)
    merged: dict[str, str] = {
        "title": doc.title,
        "report_title": doc.title,
        "subtitle": doc.subtitle,
        "generated_by": doc.generated_by,
        "author": doc.generated_by,
        "generated_at": ts_str,
        "date": ts_str[:10],
        "timezone": doc.timezone,
        "watermark": doc.watermark or "",
    }
    for key, val in doc.placeholders.items():
        merged[str(key)] = str(val)
    return merged


def parse_decimal_safe(value: Any) -> Decimal | None:
    """Parse int/float/Decimal or numeric string (optional comma grouping) into `Decimal`."""
    if value is None or isinstance(value, bool):
        return None
    if isinstance(value, Decimal):
        return value if value.is_finite() else None
    if isinstance(value, int):
        return Decimal(value)
    if isinstance(value, float):
        dec = Decimal(str(value))
        return dec if dec.is_finite() else None
    if isinstance(value, str):
        cleaned = value.strip()
        if not cleaned:
            return None
        normalized = cleaned.replace(",", "")
        try:
            dec = Decimal(normalized)
        except InvalidOperation:
            return None
        return dec if dec.is_finite() else None
    return None


def parse_kpi_numeric(raw_value: str) -> Decimal | None:
    """Return `Decimal` if `raw_value` is a plain or comma-grouped decimal string."""
    cleaned = raw_value.strip()
    if not _NUMERIC_KPI_RE.fullmatch(cleaned):
        return None
    return parse_decimal_safe(cleaned)


def coerce_excel_cell_value(
    value: Any,
    col: ExportColumnSpec,
    *,
    tz_name: str = "UTC",
) -> int | float | Decimal | bool | date | datetime | str | None:
    """Convert raw JSON/Arrow cell value into native Python value accepted by openpyxl."""
    if value is None:
        return None

    dtype = col.data_type
    if dtype == "DECIMAL":
        dec = parse_decimal_safe(value)
        return dec if dec is not None else str(value)

    if dtype == "INT64":
        if isinstance(value, bool):
            return int(value)
        if isinstance(value, int):
            return value
        try:
            return int(str(value).replace(",", "").strip())
        except ValueError:
            return str(value)

    if dtype == "FLOAT64":
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            return float(value)
        try:
            return float(str(value).replace(",", "").strip())
        except ValueError:
            return str(value)

    if dtype == "BOOLEAN":
        if isinstance(value, bool):
            return value
        lowered = str(value).strip().lower()
        if lowered in {"true", "t", "1"}:
            return True
        if lowered in {"false", "f", "0"}:
            return False
        return str(value)

    if dtype == "DATE":
        if isinstance(value, datetime):
            return value.date()
        if isinstance(value, date):
            return value
        raw_s = str(value).strip()
        try:
            return date.fromisoformat(raw_s[:10])
        except ValueError:
            return raw_s

    if dtype == "TIMESTAMP":
        tz = resolve_zoneinfo(tz_name)
        if isinstance(value, datetime):
            if value.tzinfo is not None:
                return value.astimezone(tz).replace(tzinfo=None)
            return value
        if isinstance(value, date):
            return datetime(value.year, value.month, value.day)
        raw_s = str(value).strip()
        iso_candidate = raw_s[:-1] + "+00:00" if raw_s.endswith("Z") else raw_s
        try:
            parsed_dt = datetime.fromisoformat(iso_candidate)
        except ValueError:
            return raw_s
        if parsed_dt.tzinfo is not None:
            return parsed_dt.astimezone(tz).replace(tzinfo=None)
        return parsed_dt

    if isinstance(value, (dict, list)):
        return orjson.dumps(value).decode("utf-8")
    if isinstance(value, bytes):
        return value.hex()
    return str(value)


def format_csv_cell(value: Any, col: ExportColumnSpec, *, tz_name: str = "UTC") -> str:
    """Serialize cell value cleanly for CSV output without losing Decimal precision."""
    if value is None:
        return ""
    if col.data_type == "DECIMAL":
        dec = parse_decimal_safe(value)
        return format(dec, "f") if dec is not None else str(value)
    if col.data_type == "DATE":
        if isinstance(value, datetime):
            return value.date().isoformat()
        if isinstance(value, date):
            return value.isoformat()
        return str(value)
    if col.data_type == "TIMESTAMP":
        if isinstance(value, datetime):
            return format_timestamp_tz(value, tz_name)
        return str(value)
    if isinstance(value, (dict, list)):
        return orjson.dumps(value).decode("utf-8")
    if isinstance(value, bytes):
        return value.hex()
    return str(value)


def format_display_cell(value: Any, col: ExportColumnSpec, *, tz_name: str = "UTC") -> str:
    """Format cell value for human-readable report formats (PDF, DOCX, PPTX, Markdown)."""
    if value is None:
        return ""
    fmt = (col.format or "").strip()

    if col.data_type in {"DECIMAL", "FLOAT64"}:
        dec = parse_decimal_safe(value)
        if dec is not None:
            if fmt.endswith("%"):
                return f"{dec * Decimal(100):,.1f}%"
            return f"{dec:,.2f}"
        return str(value)

    if col.data_type == "INT64":
        if isinstance(value, int) and not isinstance(value, bool):
            return f"{value:,}" if "," in fmt else str(value)
        try:
            iv = int(str(value).replace(",", "").strip())
            return f"{iv:,}" if "," in fmt else str(iv)
        except ValueError:
            return str(value)

    if col.data_type == "DATE":
        if isinstance(value, datetime):
            return value.date().isoformat()
        if isinstance(value, date):
            return value.isoformat()
        return str(value)[:10] if len(str(value)) >= 10 else str(value)

    if col.data_type == "TIMESTAMP":
        if isinstance(value, datetime):
            return format_timestamp_tz(value, tz_name)
        return str(value)

    if isinstance(value, (dict, list)):
        return orjson.dumps(value).decode("utf-8")
    if isinstance(value, bytes):
        return value.hex()
    return str(value)


__all__ = [
    "DataType",
    "ExportChart",
    "ExportColumnSpec",
    "ExportDocument",
    "ExportFormatLiteral",
    "ExportHeading",
    "ExportKpi",
    "ExportMarkdown",
    "ExportParagraph",
    "ExportResult",
    "ExportSection",
    "ExportSectionUnion",
    "ExportTable",
    "ExportText",
    "OrientationLiteral",
    "PARSER_ROOT",
    "PageSizeLiteral",
    "RenderChartRequest",
    "SidecarExportRequest",
    "SidecarExportResponse",
    "atomic_write_export",
    "build_download_filename",
    "build_placeholder_map",
    "cleanup_document_parquet_files",
    "coerce_excel_cell_value",
    "compute_file_size_and_sha256",
    "format_csv_cell",
    "format_display_cell",
    "format_timestamp_tz",
    "iter_table_rows",
    "parse_decimal_safe",
    "parse_kpi_numeric",
    "resolve_export_target_path",
    "resolve_template_file",
    "resolve_zoneinfo",
    "validate_job_id",
]
