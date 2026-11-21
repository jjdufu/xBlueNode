from __future__ import annotations

import csv
import hashlib
import io
import subprocess
import sys
import zipfile
from datetime import UTC, date, datetime
from pathlib import Path
from typing import Any

import openpyxl
import pyarrow as pa
import pyarrow.parquet as pq
import pytest
from docx import Document as DocxDocument
from fastapi.testclient import TestClient
from pptx import Presentation
from pptx.enum.chart import XL_CHART_TYPE
from pypdf import PdfReader

from exporters import (
    ExportChart,
    ExportColumnSpec,
    ExportDocument,
    ExportHeading,
    ExportKpi,
    ExportMarkdown,
    ExportParagraph,
    ExportTable,
    SidecarExportRequest,
    export_document,
)
from exporters import xlsx as xlsx_exporter
from main import app
from security import SidecarError
from tests.fixtures.make_fixtures import PARSER_ROOT, make_templates

TEST_TOKEN = "step12-export-internal-token-0123456789abcdef"
VERIFY_SCRIPT = PARSER_ROOT.parent / "scripts" / "gate" / "verify_export.py"


@pytest.fixture(autouse=True)
def _configure_env(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    monkeypatch.setenv("AXPOINT_CHART_WORKERS", "2")


@pytest.fixture(scope="module")
def template_paths() -> tuple[Path, Path]:
    templates_dir = PARSER_ROOT / "tests" / "data" / "templates"
    docx_tmpl = templates_dir / "report_template.docx"
    pptx_tmpl = templates_dir / "deck_template.pptx"
    if not docx_tmpl.is_file() or not pptx_tmpl.is_file():
        make_templates(templates_dir)
    return docx_tmpl, pptx_tmpl


def _build_rich_dashboard_document(*, row_count: int = 10_000) -> ExportDocument:
    bar_table = ExportTable(
        title="按状态订单金额",
        columns=[
            ExportColumnSpec(name="状态", data_type="STRING"),
            ExportColumnSpec(name="订单金额", data_type="DECIMAL", format="#,##0.00"),
        ],
        rows=[
            ["已支付", "12345.67"],
            ["待发货", "4320.00"],
            ["已完成", "8900.50"],
        ],
    )
    bar_chart = ExportChart(
        title="按状态订单金额",
        widget_kind="BAR_CHART",
        echarts_option={
            "title": {"text": "按状态订单金额"},
            "xAxis": {"type": "category", "data": ["已支付", "待发货", "已完成"]},
            "yAxis": {"type": "value"},
            "series": [{"name": "订单金额", "type": "bar", "data": [12345.67, 4320.0, 8900.5]}],
        },
        data_table=bar_table,
        width_px=800,
        height_px=450,
    )

    sankey_chart = ExportChart(
        title="供应链物流流向",
        widget_kind="SANKEY",
        echarts_option={
            "title": {"text": "供应链物流流向"},
            "series": [
                {
                    "type": "sankey",
                    "data": [{"name": "晶圆一厂"}, {"name": "华东仓"}, {"name": "终端客户"}],
                    "links": [
                        {"source": "晶圆一厂", "target": "华东仓", "value": 120},
                        {"source": "华东仓", "target": "终端客户", "value": 100},
                    ],
                }
            ],
        },
        data_table=None,
        width_px=800,
        height_px=450,
    )

    detail_rows: list[list[Any]] = [
        [
            f"ORD-{idx:05d}",
            "华东仓" if idx % 2 == 0 else "华南仓",
            f"{(idx % 97) * 10 + 12.34:.2f}",
            f"2026-01-{(idx % 28) + 1:02d}",
            idx + 1,
        ]
        for idx in range(row_count)
    ]

    detail_table = ExportTable(
        title="订单履约明细",
        columns=[
            ExportColumnSpec(name="订单号", data_type="STRING"),
            ExportColumnSpec(name="履约仓库", data_type="STRING"),
            ExportColumnSpec(name="金额", data_type="DECIMAL", format="#,##0.00"),
            ExportColumnSpec(name="订单日期", data_type="DATE", format="yyyy-mm-dd"),
            ExportColumnSpec(name="件数", data_type="INT64", format="#,##0"),
        ],
        rows=detail_rows,
        totals=["合计", "", "12345.67", "", row_count],
    )

    return ExportDocument(
        title="供应概览",
        subtitle="华东供应链全景运营报告",
        generated_at=datetime(2026, 1, 31, 8, 0, 0, tzinfo=UTC),
        generated_by="dev",
        timezone="Asia/Shanghai",
        watermark="dev 2026-01-31 16:00",
        csv_bom=True,
        page_size="A4",
        orientation="portrait",
        sections=[
            ExportKpi(label="已支付金额", value="12,345.67", compare="环比 +12.5%"),
            ExportHeading(text="一、运营态势摘要", level=1),
            ExportParagraph(text="本期华东仓与晶圆一厂整体履约稳定，关键订单准时交付率保持高位。"),
            ExportMarkdown(text="- 核心仓库：**华东仓**\n- 重点监控指标：已支付金额"),
            bar_chart,
            sankey_chart,
            detail_table,
        ],
    )


def test_all_six_formats_end_to_end_and_gate_verifier() -> None:
    doc = _build_rich_dashboard_document(row_count=10_000)
    payload_doc = doc.model_dump(mode="json")
    headers = {
        "X-AxPoint-Internal-Token": TEST_TOKEN,
        "X-AxPoint-Tenant": "default",
    }

    exported: dict[str, dict[str, Any]] = {}
    with TestClient(app) as client:
        for fmt in ("XLSX", "CSV", "PDF", "DOCX", "PPTX", "MARKDOWN"):
            resp = client.post(
                f"/export/{fmt.lower()}",
                headers=headers,
                json={
                    "tenant_id": "default",
                    "job_id": f"job-{fmt.lower()}-10k",
                    "document": payload_doc,
                },
            )
            assert resp.status_code == 200, resp.text
            body = resp.json()
            out_file = Path(body["file_path"])
            assert out_file.is_file()
            assert not out_file.with_name(f"{out_file.name}.part").exists()
            raw_bytes = out_file.read_bytes()
            assert body["file_size"] == len(raw_bytes)
            assert body["sha256"] == hashlib.sha256(raw_bytes).hexdigest()
            assert "供应概览" in body["file_name"]
            exported[fmt] = body

            proc = subprocess.run(
                [sys.executable, str(VERIFY_SCRIPT), fmt, str(out_file), "12345.67"],
                capture_output=True,
                text=True,
                check=False,
            )
            assert proc.returncode == 0, (
                f"verify_export.py failed for {fmt}: stdout={proc.stdout!r} stderr={proc.stderr!r}"
            )
            assert f"ok  export-{fmt}" in proc.stdout

        # Unsupported format -> 400 INVALID_ARGUMENT
        bad_fmt = client.post(
            "/export/html",
            headers=headers,
            json={"tenant_id": "default", "job_id": "job-bad", "document": payload_doc},
        )
        assert bad_fmt.status_code == 400
        assert bad_fmt.json()["error_code"] == "INVALID_ARGUMENT"

        # Mismatched tenant header -> 403 PERMISSION_DENIED
        mismatch = client.post(
            "/export/xlsx",
            headers={**headers, "X-AxPoint-Tenant": "other-tenant"},
            json={"tenant_id": "default", "job_id": "job-mismatch", "document": payload_doc},
        )
        assert mismatch.status_code == 403
        assert mismatch.json()["error_code"] == "PERMISSION_DENIED"

    # --- Deep format-specific inspections ---
    # 1. XLSX: sheet names, embedded images, Decimal/Date types & number_format, 10,000+2 rows
    wb = openpyxl.load_workbook(exported["XLSX"]["file_path"], data_only=False)
    try:
        assert "概览" in wb.sheetnames
        assert "订单履约明细" in wb.sheetnames
        overview_ws = wb["概览"]
        assert len(overview_ws._images) == 2

        detail_ws = wb["订单履约明细"]
        assert detail_ws.freeze_panes == "A2"
        assert detail_ws.max_row == 10_002  # 1 header + 10,000 rows + 1 totals
        amt_cell = detail_ws["C2"]
        assert amt_cell.number_format == "#,##0.00"
        assert isinstance(amt_cell.value, (int, float))
        assert abs(float(amt_cell.value) - 12.34) < 1e-6
        date_cell = detail_ws["D2"]
        assert date_cell.number_format == "yyyy-mm-dd"
        assert isinstance(date_cell.value, (date, datetime))
    finally:
        wb.close()

    # 2. CSV: ZIP with UTF-8 BOM, 00_overview.csv + 01_订单履约明细.csv (10,002 rows)
    csv_zip_path = Path(exported["CSV"]["file_path"])
    assert exported["CSV"]["file_name"].endswith(".zip")
    assert zipfile.is_zipfile(csv_zip_path)
    with zipfile.ZipFile(csv_zip_path, "r") as zf:
        names = sorted(zf.namelist())
        assert "00_overview.csv" in names
        detail_member = next(n for n in names if "订单履约明细" in n)
        raw_detail = zf.read(detail_member)
        assert raw_detail.startswith(b"\xef\xbb\xbf")
        parsed_rows = list(csv.reader(io.StringIO(raw_detail.decode("utf-8-sig"))))
        assert len(parsed_rows) == 10_002
        assert parsed_rows[0] == ["订单号", "履约仓库", "金额", "订单日期", "件数"]
        assert parsed_rows[1][0] == "ORD-00000"
        assert parsed_rows[-1][0] == "合计"

    # 3. PDF: pypdf text has title, KPI, watermark, and 5,000-row truncation notice
    pdf_reader = PdfReader(exported["PDF"]["file_path"])
    pdf_text = "\n".join((p.extract_text() or "") for p in pdf_reader.pages)
    assert "供应概览" in pdf_text
    assert "12,345.67" in pdf_text
    assert "dev 2026-01-31 16:00" in pdf_text
    assert "完整数据请导出 XLSX" in pdf_text

    # 4. DOCX: paragraphs, tables, embedded chart blips, and footer watermark
    docx_doc = DocxDocument(exported["DOCX"]["file_path"])
    assert len(docx_doc.inline_shapes) == 2
    assert any("供应概览" in p.text for p in docx_doc.paragraphs)
    assert any(
        tbl.rows[0].cells[0].text == "订单号" and len(tbl.rows) == 10_002
        for tbl in docx_doc.tables
    )
    footer_text = "\n".join(p.text for s in docx_doc.sections for p in s.footer.paragraphs)
    assert "dev 2026-01-31 16:00" in footer_text

    # 5. PPTX: native editable BAR_CHART + PNG fallback SANKEY + 15-row pagination
    prs = Presentation(exported["PPTX"]["file_path"])
    chart_shapes = [
        shape.chart
        for slide in prs.slides
        for shape in slide.shapes
        if shape.has_chart
    ]
    assert len(chart_shapes) == 1
    native_chart = chart_shapes[0]
    assert native_chart.chart_type == XL_CHART_TYPE.COLUMN_CLUSTERED
    cats = [str(c) for c in native_chart.plots[0].categories]
    assert cats == ["已支付", "待发货", "已完成"]
    series_vals = list(native_chart.series[0].values)
    assert abs(series_vals[0] - 12345.67) < 0.01

    table_slides = [
        slide
        for slide in prs.slides
        if any(shape.has_table for shape in slide.shapes)
    ]
    # 10,000 data rows + 1 totals row = 10,001 rows -> ceil(10001 / 15) = 667 slides
    assert len(table_slides) == 667

    # 6. MARKDOWN: ZIP containing report.md (truncated at 500 rows) + 2 SVG charts
    md_zip_path = Path(exported["MARKDOWN"]["file_path"])
    assert exported["MARKDOWN"]["file_name"].endswith(".zip")
    assert zipfile.is_zipfile(md_zip_path)
    with zipfile.ZipFile(md_zip_path, "r") as zf:
        md_members = set(zf.namelist())
        assert "report.md" in md_members
        assert "charts/chart_01.svg" in md_members
        assert "charts/chart_02.svg" in md_members
        md_text = zf.read("report.md").decode("utf-8")
        assert "# 供应概览" in md_text
        assert "12,345.67" in md_text
        assert "![按状态订单金额](charts/chart_01.svg)" in md_text
        assert "Markdown 仅展示前 500 行" in md_text


def test_parquet_streaming_and_cleanup_plus_single_file_csv_and_md(tmp_path: Path) -> None:
    staging_dir = tmp_path / "staging"
    staging_dir.mkdir(parents=True, exist_ok=True)
    pq_file = staging_dir / "saved_query_60k.parquet"

    n_rows = 60_000
    pa_tbl = pa.table(
        {
            "order_id": [f"SQ-{i:06d}" for i in range(n_rows)],
            "amount": [f"{(i % 50) + 0.50:.2f}" for i in range(n_rows)],
        }
    )
    pq.write_table(pa_tbl, pq_file)
    assert pq_file.is_file()

    single_table_doc = ExportDocument(
        title="保存查询导出",
        generated_at=datetime(2026, 2, 1, 0, 0, 0, tzinfo=UTC),
        generated_by="dev",
        timezone="UTC",
        csv_bom=False,
        sections=[
            ExportTable(
                title="订单流式导出",
                columns=[
                    ExportColumnSpec(name="order_id", data_type="STRING"),
                    ExportColumnSpec(name="amount", data_type="DECIMAL"),
                ],
                rows_parquet_path="staging/saved_query_60k.parquet",
            )
        ],
    )

    csv_res = export_document(
        "CSV",
        SidecarExportRequest(
            tenant_id="default",
            job_id="job-pq-csv",
            document=single_table_doc,
        ),
        data_root=tmp_path,
    )
    # Single table with no KPIs -> plain .csv, no BOM, and Parquet temp file unlinked!
    assert csv_res.file_name == "保存查询导出.csv"
    assert not pq_file.exists()
    raw_csv = Path(csv_res.file_path).read_bytes()
    assert not raw_csv.startswith(b"\xef\xbb\xbf")
    assert raw_csv.count(b"\n") == n_rows + 1

    # Markdown without charts -> plain .md file
    md_doc = ExportDocument(
        title="无图表简报",
        generated_at=datetime(2026, 2, 1, 0, 0, 0, tzinfo=UTC),
        generated_by="dev",
        timezone="UTC",
        sections=[
            ExportKpi(label="订单总数", value="60,000"),
            ExportParagraph(text="全部订单已完成对账。"),
        ],
    )
    md_res = export_document(
        "MARKDOWN",
        SidecarExportRequest(tenant_id="default", job_id="job-plain-md", document=md_doc),
        data_root=tmp_path,
    )
    assert md_res.file_name == "无图表简报.md"
    assert Path(md_res.file_path).read_text(encoding="utf-8").startswith("# 无图表简报")

    # Path traversal in rows_parquet_path -> 403 PERMISSION_DENIED
    bad_pq_doc = ExportDocument(
        title="越权测试",
        generated_at=datetime(2026, 2, 1, 0, 0, 0, tzinfo=UTC),
        generated_by="dev",
        timezone="UTC",
        sections=[
            ExportTable(
                title="非法路径",
                columns=[ExportColumnSpec(name="x", data_type="STRING")],
                rows_parquet_path="/etc/passwd",
            )
        ],
    )
    with pytest.raises(SidecarError) as exc_info:
        export_document(
            "CSV",
            SidecarExportRequest(tenant_id="default", job_id="job-bad-pq", document=bad_pq_doc),
            data_root=tmp_path,
        )
    assert exc_info.value.status_code == 403
    assert exc_info.value.error_code == "PERMISSION_DENIED"


def test_xlsx_pivot_merging_and_sheet_auto_splitting(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(xlsx_exporter, "XLSX_MAX_DATA_ROWS", 6)
    pivot_doc = ExportDocument(
        title="透视与分表测试",
        generated_at=datetime(2026, 2, 1, 0, 0, 0, tzinfo=UTC),
        generated_by="dev",
        timezone="UTC",
        sections=[
            ExportTable(
                title="区域透视表",
                columns=[
                    ExportColumnSpec(name="大区", data_type="STRING"),
                    ExportColumnSpec(name="仓库", data_type="STRING"),
                    ExportColumnSpec(name="销售额", data_type="DECIMAL"),
                ],
                rows=[
                    ["华东", "上海仓", "100.00"],
                    ["华东", "苏州仓", "200.00"],
                    ["华东小计", "", "300.00"],
                    ["华南", "深圳仓", "150.00"],
                    ["华南", "广州仓", "250.00"],
                    ["华南小计", "", "400.00"],
                    ["华北", "北京仓", "500.00"],
                    ["总计", "", "1200.00"],
                ],
                row_kinds=[
                    "DETAIL",
                    "DETAIL",
                    "SUBTOTAL",
                    "DETAIL",
                    "DETAIL",
                    "SUBTOTAL",
                    "DETAIL",
                    "TOTAL",
                ],
            )
        ],
    )
    res = export_document(
        "XLSX",
        SidecarExportRequest(tenant_id="default", job_id="job-pivot-split", document=pivot_doc),
        data_root=tmp_path,
    )
    wb = openpyxl.load_workbook(res.file_path)
    try:
        assert wb.sheetnames == ["概览", "区域透视表", "区域透视表 (2)"]
        ws1 = wb["区域透视表"]
        merged_strs = {str(rng) for rng in ws1.merged_cells.ranges}
        assert "A2:A3" in merged_strs
        assert "A5:A6" in merged_strs
        assert ws1["A4"].font.bold is True
        ws2 = wb["区域透视表 (2)"]
        assert ws2["A3"].value == "总计"
        assert ws2["A3"].font.bold is True
    finally:
        wb.close()


def test_docx_and_pptx_templates_replace_all_placeholders(
    template_paths: tuple[Path, Path], tmp_path: Path
) -> None:
    docx_tmpl, pptx_tmpl = template_paths

    # 1. Official fixture report_template.docx
    doc_with_docx_tmpl = ExportDocument(
        title="供应概览",
        subtitle="季度经营模板出品",
        generated_at=datetime(2026, 1, 31, 8, 0, 0, tzinfo=UTC),
        generated_by="dev",
        timezone="Asia/Shanghai",
        watermark="dev 2026-01-31 16:00",
        template_path=str(docx_tmpl),
        placeholders={"department": "供应链管理部"},
        sections=[
            ExportKpi(label="已支付金额", value="12,345.67"),
            ExportTable(
                title="核心仓库库存",
                columns=[
                    ExportColumnSpec(name="仓库", data_type="STRING"),
                    ExportColumnSpec(name="库存金额", data_type="DECIMAL"),
                ],
                rows=[["华东仓", "12345.67"]],
            ),
        ],
    )
    docx_res = export_document(
        "DOCX",
        SidecarExportRequest(
            tenant_id="default", job_id="job-tmpl-docx", document=doc_with_docx_tmpl
        ),
        data_root=tmp_path,
    )
    loaded_docx = DocxDocument(docx_res.file_path)
    all_docx_text_parts = [p.text for p in loaded_docx.paragraphs]
    for tbl in loaded_docx.tables:
        assert tbl.style is not None and tbl.style.name == "AxTable"
        for row in tbl.rows:
            for cell in row.cells:
                all_docx_text_parts.append(cell.text)
    for sec in loaded_docx.sections:
        all_docx_text_parts.extend(p.text for p in sec.header.paragraphs)
        all_docx_text_parts.extend(p.text for p in sec.footer.paragraphs)
    combined_docx_text = "\n".join(all_docx_text_parts)
    assert "{{" not in combined_docx_text
    assert "供应概览" in combined_docx_text
    assert "季度经营模板出品" in combined_docx_text
    assert "12,345.67" in combined_docx_text

    # 2. Split-run DOCX template ({{ + report_ + title + }} across separate w:r runs)
    split_tmpl_path = tmp_path / "split_runs_template.docx"
    split_doc = DocxDocument()
    p_split = split_doc.add_paragraph()
    p_split.add_run("报告名：{{")
    p_split.add_run("report_")
    p_split.add_run("title}} - 部门：{{")
    p_split.add_run("dept}}")
    p_sec = split_doc.add_paragraph()
    p_sec.add_run("{{sec")
    p_sec.add_run("tions}}")
    split_doc.save(str(split_tmpl_path))

    split_export_doc = doc_with_docx_tmpl.model_copy(
        update={
            "template_path": str(split_tmpl_path),
            "placeholders": {"dept": "华东运营中心"},
        }
    )
    split_res = export_document(
        "DOCX",
        SidecarExportRequest(
            tenant_id="default", job_id="job-split-docx", document=split_export_doc
        ),
        data_root=tmp_path,
    )
    loaded_split = DocxDocument(split_res.file_path)
    split_text = "\n".join(p.text for p in loaded_split.paragraphs)
    assert "{{" not in split_text
    assert "报告名：供应概览 - 部门：华东运营中心" in split_text

    # 3. Official fixture deck_template.pptx
    doc_with_pptx_tmpl = doc_with_docx_tmpl.model_copy(
        update={"template_path": str(pptx_tmpl)}
    )
    pptx_res = export_document(
        "PPTX",
        SidecarExportRequest(
            tenant_id="default", job_id="job-tmpl-pptx", document=doc_with_pptx_tmpl
        ),
        data_root=tmp_path,
    )
    loaded_prs = Presentation(pptx_res.file_path)
    pptx_texts: list[str] = []
    for slide in loaded_prs.slides:
        for shape in slide.shapes:
            if shape.has_text_frame:
                pptx_texts.append(shape.text_frame.text)
            if shape.has_table:
                for r in shape.table.rows:
                    for c in r.cells:
                        pptx_texts.append(c.text)
    combined_pptx_text = "\n".join(pptx_texts)
    assert "{{" not in combined_pptx_text
    assert "供应概览" in combined_pptx_text
    assert "季度经营模板出品" in combined_pptx_text
    assert "12,345.67" in combined_pptx_text
