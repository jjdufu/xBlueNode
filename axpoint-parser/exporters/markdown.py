from __future__ import annotations

import zipfile
from pathlib import Path

from charts import render_chart_bytes
from exporters.document import (
    ExportChart,
    ExportDocument,
    ExportKpi,
    ExportResult,
    ExportTable,
    ExportText,
    RenderChartRequest,
    atomic_write_export,
    build_download_filename,
    format_display_cell,
    format_timestamp_tz,
    iter_table_rows,
)

MARKDOWN_MAX_TABLE_ROWS = 500


def _escape_md_cell(raw: str) -> str:
    return raw.replace("\\", "\\\\").replace("|", "\\|").replace("\r", " ").replace("\n", "<br>")


def has_charts(doc: ExportDocument) -> bool:
    """Return `True` if `doc` contains at least one `ExportChart` section."""
    return any(isinstance(sec, ExportChart) for sec in doc.sections)


def _render_markdown_and_charts(
    doc: ExportDocument,
    *,
    data_root: Path | None = None,
) -> tuple[str, list[tuple[str, bytes]]]:
    lines: list[str] = [f"# {doc.title}", ""]
    if doc.subtitle:
        lines.append(f"_{doc.subtitle}_")
        lines.append("")

    ts_str = format_timestamp_tz(doc.generated_at, doc.timezone)
    lines.append(f"> 生成人：{doc.generated_by} | 生成时间：{ts_str} ({doc.timezone})")
    if doc.watermark:
        lines.append(f"> 水印：{doc.watermark}")
    lines.append("")

    kpis = [sec for sec in doc.sections if isinstance(sec, ExportKpi)]
    if kpis:
        lines.append("## 核心指标")
        lines.append("")
        lines.append("| 指标 | 数值 | 对比 |")
        lines.append("| :--- | :--- | :--- |")
        for kpi in kpis:
            lines.append(
                f"| {_escape_md_cell(kpi.label)} "
                f"| {_escape_md_cell(kpi.value)} "
                f"| {_escape_md_cell(kpi.compare or '')} |"
            )
        lines.append("")

    chart_files: list[tuple[str, bytes]] = []
    chart_counter = 0

    for section in doc.sections:
        if isinstance(section, ExportKpi):
            continue

        if isinstance(section, ExportText):
            if section.kind == "HEADING":
                hashes = "#" * min(max(section.level + 1, 2), 6)
                lines.append(f"{hashes} {section.text}")
                lines.append("")
            else:
                lines.append(section.text)
                lines.append("")

        elif isinstance(section, ExportChart):
            chart_counter += 1
            rel_svg_path = f"charts/chart_{chart_counter:02d}.svg"
            svg_bytes, _ = render_chart_bytes(
                RenderChartRequest(
                    option=section.echarts_option,
                    width=section.width_px,
                    height=section.height_px,
                    format="svg",
                )
            )
            chart_files.append((rel_svg_path, svg_bytes))
            chart_title = section.title.strip() or f"图表 {chart_counter}"
            lines.append(f"## {chart_title}")
            lines.append("")
            lines.append(f"![{_escape_md_cell(chart_title)}]({rel_svg_path})")
            lines.append("")

        elif isinstance(section, ExportTable):
            if section.title:
                lines.append(f"## {section.title}")
                lines.append("")
            columns = section.columns
            num_cols = len(columns)
            if num_cols == 0:
                continue

            headers = " | ".join(_escape_md_cell(col.name) for col in columns)
            separators = " | ".join(
                "---:" if col.data_type in {"INT64", "FLOAT64", "DECIMAL"} else ":---"
                for col in columns
            )
            lines.append(f"| {headers} |")
            lines.append(f"| {separators} |")

            total_rows = 0
            for raw_row in iter_table_rows(section, data_root=data_root):
                if total_rows < MARKDOWN_MAX_TABLE_ROWS:
                    cells_md = " | ".join(
                        _escape_md_cell(
                            format_display_cell(
                                raw_row[c_idx] if c_idx < len(raw_row) else None,
                                columns[c_idx],
                                tz_name=doc.timezone,
                            )
                        )
                        for c_idx in range(num_cols)
                    )
                    lines.append(f"| {cells_md} |")
                total_rows += 1

            if section.totals is not None and total_rows <= MARKDOWN_MAX_TABLE_ROWS:
                totals_cells: list[str] = []
                for c_idx in range(num_cols):
                    raw_tot = section.totals[c_idx] if c_idx < len(section.totals) else None
                    disp = format_display_cell(raw_tot, columns[c_idx], tz_name=doc.timezone)
                    totals_cells.append(f"**{_escape_md_cell(disp)}**")
                totals_md = " | ".join(totals_cells)
                lines.append(f"| {totals_md} |")

            lines.append("")
            if total_rows > MARKDOWN_MAX_TABLE_ROWS:
                lines.append(
                    f"> （共 {total_rows:,} 行，Markdown 仅展示前 "
                    f"{MARKDOWN_MAX_TABLE_ROWS:,} 行，完整数据请导出 XLSX 或 CSV）"
                )
                lines.append("")

    markdown_str = "\n".join(lines).rstrip() + "\n"
    return markdown_str, chart_files


def export(
    doc: ExportDocument,
    out_path: Path,
    *,
    data_root: Path | None = None,
) -> ExportResult:
    """Export `ExportDocument` to `.md` (no charts) or `.zip` (`report.md` + `charts/*.svg`)."""
    markdown_str, chart_files = _render_markdown_and_charts(doc, data_root=data_root)

    if not chart_files:
        def _write_md(part_path: Path) -> None:
            part_path.write_text(markdown_str, encoding="utf-8")

        file_size, sha256_hex = atomic_write_export(out_path, _write_md)
        return ExportResult(
            file_path=out_path,
            file_size=file_size,
            sha256=sha256_hex,
            file_name=build_download_filename(doc.title, "md"),
        )

    def _write_zip(part_path: Path) -> None:
        with zipfile.ZipFile(part_path, mode="w", compression=zipfile.ZIP_DEFLATED) as zf:
            zf.writestr("report.md", markdown_str.encode("utf-8"))
            for rel_path, svg_bytes in chart_files:
                zf.writestr(rel_path, svg_bytes)

    file_size, sha256_hex = atomic_write_export(out_path, _write_zip)
    return ExportResult(
        file_path=out_path,
        file_size=file_size,
        sha256=sha256_hex,
        file_name=build_download_filename(doc.title, "zip"),
    )
