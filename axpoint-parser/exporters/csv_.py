from __future__ import annotations

import csv
import io
import re
import zipfile
from pathlib import Path
from typing import TextIO

from exporters.document import (
    ExportChart,
    ExportDocument,
    ExportKpi,
    ExportResult,
    ExportTable,
    ExportText,
    atomic_write_export,
    build_download_filename,
    format_csv_cell,
    format_timestamp_tz,
    iter_table_rows,
)

_SAFE_CSV_MEMBER_RE = re.compile(r'[\x00-\x1f\x7f/\\:*?"<>|]+')


def _safe_member_stem(raw_name: str, fallback: str) -> str:
    cleaned = _SAFE_CSV_MEMBER_RE.sub("_", raw_name.strip()).strip(" ._")
    if not cleaned:
        return fallback
    return cleaned[:60].rstrip(" ._") or fallback


def _write_kpi_overview_csv(
    stream: TextIO,
    doc: ExportDocument,
    kpis: list[ExportKpi],
) -> None:
    writer = csv.writer(stream, lineterminator="\n")
    writer.writerow(["报表标题", "指标", "数值", "对比", "生成人", "生成时间"])
    ts_str = format_timestamp_tz(doc.generated_at, doc.timezone)
    if kpis:
        for kpi in kpis:
            writer.writerow(
                [
                    doc.title,
                    kpi.label,
                    kpi.value,
                    kpi.compare or "",
                    doc.generated_by,
                    ts_str,
                ]
            )
    else:
        writer.writerow([doc.title, "", "", "", doc.generated_by, ts_str])


def _write_table_csv(
    stream: TextIO,
    table: ExportTable,
    *,
    tz_name: str,
    data_root: Path | None = None,
) -> None:
    writer = csv.writer(stream, lineterminator="\n")
    columns = table.columns
    num_cols = len(columns)
    writer.writerow([col.name for col in columns])

    for raw_row in iter_table_rows(table, data_root=data_root):
        writer.writerow(
            [
                format_csv_cell(
                    raw_row[col_idx] if col_idx < len(raw_row) else None,
                    columns[col_idx],
                    tz_name=tz_name,
                )
                for col_idx in range(num_cols)
            ]
        )

    if table.totals is not None:
        writer.writerow(
            [
                format_csv_cell(
                    table.totals[col_idx] if col_idx < len(table.totals) else None,
                    columns[col_idx],
                    tz_name=tz_name,
                )
                for col_idx in range(num_cols)
            ]
        )


def _write_text_fallback_csv(stream: TextIO, doc: ExportDocument) -> None:
    writer = csv.writer(stream, lineterminator="\n")
    writer.writerow(["报表标题", "类型", "内容"])
    texts = [sec for sec in doc.sections if isinstance(sec, ExportText)]
    if texts:
        for sec in texts:
            writer.writerow([doc.title, sec.kind, sec.text])
    else:
        writer.writerow([doc.title, "METADATA", doc.subtitle])


def will_package_as_zip(doc: ExportDocument) -> bool:
    """Return `True` when `CSV` export produces multiple `.csv` members inside a `.zip`."""
    kpis = [sec for sec in doc.sections if isinstance(sec, ExportKpi)]
    tables = _collect_tables(doc)
    if len(tables) >= 2:
        return True
    if len(tables) == 1 and len(kpis) >= 1:
        return True
    return False


def _collect_tables(doc: ExportDocument) -> list[ExportTable]:
    tables = [sec for sec in doc.sections if isinstance(sec, ExportTable)]
    if tables:
        return tables
    chart_tables: list[ExportTable] = []
    for sec in doc.sections:
        if (
            isinstance(sec, ExportChart)
            and sec.data_table is not None
            and sec.data_table.columns
        ):
            dt = sec.data_table
            if not dt.title and sec.title:
                dt = dt.model_copy(update={"title": sec.title})
            chart_tables.append(dt)
    return chart_tables


def export(
    doc: ExportDocument,
    out_path: Path,
    *,
    data_root: Path | None = None,
) -> ExportResult:
    """Export `ExportDocument` to a single `.csv` file or multi-table `.zip` archive."""
    encoding = "utf-8-sig" if doc.csv_bom else "utf-8"
    kpis = [sec for sec in doc.sections if isinstance(sec, ExportKpi)]
    tables = _collect_tables(doc)
    use_zip = will_package_as_zip(doc)

    if not use_zip:
        def _write_single(part_path: Path) -> None:
            with part_path.open("w", encoding=encoding, newline="") as fh:
                if len(tables) == 1 and not kpis:
                    _write_table_csv(fh, tables[0], tz_name=doc.timezone, data_root=data_root)
                elif kpis:
                    _write_kpi_overview_csv(fh, doc, kpis)
                else:
                    _write_text_fallback_csv(fh, doc)

        file_size, sha256_hex = atomic_write_export(out_path, _write_single)
        return ExportResult(
            file_path=out_path,
            file_size=file_size,
            sha256=sha256_hex,
            file_name=build_download_filename(doc.title, "csv"),
        )

    def _write_zip(part_path: Path) -> None:
        used_names: set[str] = set()
        with zipfile.ZipFile(part_path, mode="w", compression=zipfile.ZIP_DEFLATED) as zf:
            with zf.open("00_overview.csv", "w") as raw_member:
                with io.TextIOWrapper(raw_member, encoding=encoding, newline="") as txt_out:
                    _write_kpi_overview_csv(txt_out, doc, kpis)
            used_names.add("00_overview.csv")

            for idx, table in enumerate(tables, start=1):
                stem = _safe_member_stem(table.title, f"table_{idx:02d}")
                member_name = f"{idx:02d}_{stem}.csv"
                dedup = 2
                while member_name.lower() in used_names:
                    member_name = f"{idx:02d}_{stem}_{dedup}.csv"
                    dedup += 1
                used_names.add(member_name.lower())

                with zf.open(member_name, "w") as raw_member:
                    with io.TextIOWrapper(
                        raw_member, encoding=encoding, newline=""
                    ) as txt_out:
                        _write_table_csv(
                            txt_out,
                            table,
                            tz_name=doc.timezone,
                            data_root=data_root,
                        )

    file_size, sha256_hex = atomic_write_export(out_path, _write_zip)
    return ExportResult(
        file_path=out_path,
        file_size=file_size,
        sha256=sha256_hex,
        file_name=build_download_filename(doc.title, "zip"),
    )
