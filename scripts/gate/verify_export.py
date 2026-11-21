#!/usr/bin/env python3
"""Verifies exported files for Step 12 gate (`scripts/gate/step12.sh`, blueprint §12.7).

Usage:
    python3 scripts/gate/verify_export.py <FORMAT> <PATH> <EXPECTED_PAID>

Asserts:
- Extracted text/cells contain the report title "供应概览";
- KPI paid amount matches EXPECTED_PAID (plain `1234.56` or comma-grouped `1,234.56`,
  or numeric float within 0.01);
- PDF, DOCX, and PPTX artifacts contain watermark substring "dev".
"""
from __future__ import annotations

import csv
from decimal import Decimal
import io
from pathlib import Path
import sys
import zipfile


def _extract_xlsx_text(path: Path) -> tuple[str, list[float]]:
    import openpyxl

    wb = openpyxl.load_workbook(path, data_only=True, read_only=True)
    chunks: list[str] = []
    nums: list[float] = []
    try:
        for name in wb.sheetnames:
            chunks.append(name)
            ws = wb[name]
            for row in ws.iter_rows(values_only=True):
                for cell in row:
                    if cell is None:
                        continue
                    chunks.append(str(cell))
                    if isinstance(cell, (int, float, Decimal)):
                        nums.append(float(cell))
    finally:
        wb.close()
    return "\n".join(chunks), nums


def _extract_csv_text(path: Path) -> tuple[str, list[float]]:
    raw_texts: list[str] = []
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path, "r") as zf:
            for member in zf.namelist():
                if member.lower().endswith(".csv") or member.lower().endswith(".txt"):
                    raw_texts.append(zf.read(member).decode("utf-8-sig"))
    else:
        raw_texts.append(path.read_bytes().decode("utf-8-sig"))

    chunks: list[str] = []
    nums: list[float] = []
    for content in raw_texts:
        reader = csv.reader(io.StringIO(content))
        for row in reader:
            for cell in row:
                chunks.append(cell)
                cleaned = cell.replace(",", "").strip()
                try:
                    nums.append(float(cleaned))
                except ValueError:
                    pass
    return "\n".join(chunks), nums


def _extract_pdf_text(path: Path) -> str:
    from pypdf import PdfReader

    reader = PdfReader(str(path))
    return "\n".join((page.extract_text() or "") for page in reader.pages)


def _extract_docx_text(path: Path) -> str:
    from docx import Document

    doc = Document(str(path))
    parts: list[str] = [p.text for p in doc.paragraphs]
    for tbl in doc.tables:
        for row in tbl.rows:
            for cell in row.cells:
                parts.append(cell.text)
    for section in doc.sections:
        for hdr_p in section.header.paragraphs:
            parts.append(hdr_p.text)
        for ftr_p in section.footer.paragraphs:
            parts.append(ftr_p.text)
    return "\n".join(parts)


def _extract_pptx_text(path: Path) -> str:
    from pptx import Presentation

    prs = Presentation(str(path))
    parts: list[str] = []
    for slide in prs.slides:
        for shape in slide.shapes:
            if shape.has_text_frame:
                parts.append(shape.text_frame.text)
            if shape.has_table:
                for row in shape.table.rows:
                    for cell in row.cells:
                        parts.append(cell.text)
            if shape.has_chart:
                chart = shape.chart
                if chart.has_title and chart.chart_title.has_text_frame:
                    parts.append(chart.chart_title.text_frame.text)
                for series in chart.series:
                    parts.extend(str(v) for v in series.values if v is not None)
    return "\n".join(parts)


def _extract_markdown_text(path: Path) -> str:
    if zipfile.is_zipfile(path):
        parts: list[str] = []
        with zipfile.ZipFile(path, "r") as zf:
            for member in zf.namelist():
                if member.lower().endswith(".md"):
                    parts.append(zf.read(member).decode("utf-8"))
        return "\n".join(parts)
    return path.read_text(encoding="utf-8")


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: verify_export.py <FORMAT> <PATH> <EXPECTED_PAID>", file=sys.stderr)
        return 2

    fmt = sys.argv[1].strip().upper()
    path = Path(sys.argv[2])
    expected_dec = Decimal(sys.argv[3].strip() or "0")
    expected_float = float(expected_dec)
    plain_two = f"{expected_dec:.2f}"
    comma_two = f"{expected_dec:,.2f}"
    plain_int = str(int(expected_dec)) if expected_dec == int(expected_dec) else plain_two

    if not path.is_file() or path.stat().st_size == 0:
        print(f"GATE step12 FAIL: export-{fmt} missing or empty file {path}", file=sys.stderr)
        return 1

    nums: list[float] = []
    if fmt == "XLSX":
        text, nums = _extract_xlsx_text(path)
    elif fmt == "CSV":
        text, nums = _extract_csv_text(path)
    elif fmt == "PDF":
        text = _extract_pdf_text(path)
    elif fmt == "DOCX":
        text = _extract_docx_text(path)
    elif fmt == "PPTX":
        text = _extract_pptx_text(path)
    elif fmt == "MARKDOWN":
        text = _extract_markdown_text(path)
    else:
        print(f"GATE step12 FAIL: unknown format {fmt}", file=sys.stderr)
        return 2

    if fmt != "CSV" and "供应概览" not in text:
        # Note: CSV exports tabular data + KPI overview sheet/file; ensure title or KPI label is present
        print(f"GATE step12 FAIL: export-{fmt} missing title '供应概览'", file=sys.stderr)
        return 1
    if fmt == "CSV" and ("供应概览" not in text and "已支付金额" not in text):
        print(f"GATE step12 FAIL: export-CSV missing title/KPI label", file=sys.stderr)
        return 1

    matched_amount = (
        plain_two in text
        or comma_two in text
        or plain_int in text
        or any(abs(v - expected_float) < 0.01 for v in nums)
    )
    if not matched_amount:
        print(
            f"GATE step12 FAIL: export-{fmt} missing expected paid amount {plain_two} / {comma_two}",
            file=sys.stderr,
        )
        return 1

    if fmt in {"PDF", "DOCX", "PPTX"} and "dev" not in text:
        print(f"GATE step12 FAIL: export-{fmt} missing watermark 'dev'", file=sys.stderr)
        return 1

    print(f"ok  export-{fmt}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
