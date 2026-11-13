from __future__ import annotations

import io
from pathlib import Path

import pypdf
import pytest

from parsers.engine import parse_unified
from security import SidecarError

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_annual_report_pdf_headings_and_table(tmp_path: Path) -> None:
    res = parse_unified(
        "annual_report.pdf",
        (FIXTURES_DIR / "annual_report.pdf").read_bytes(),
        data_root=tmp_path,
    )
    assert "第一章" in res.markdown
    assert "第二章" in res.markdown
    assert "第三章" in res.markdown

    assert len(res.tables) == 1
    tbl = res.tables[0]
    assert tbl.row_count == 6
    assert len(tbl.columns) == 4
    assert tbl.origin.startswith("page=4;")


def test_scanned_pdf_without_ocr_handles_gracefully(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("AXPOINT_OCR_ENABLED", "false")
    buf = io.BytesIO()
    writer = pypdf.PdfWriter()
    writer.add_blank_page(width=300, height=400)
    writer.write(buf)

    res = parse_unified(
        "scanned_sample.pdf",
        buf.getvalue(),
        data_root=tmp_path,
    )
    assert res.source_format == "pdf"
    assert res.tables == []


def test_image_without_ocr_returns_415(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("AXPOINT_OCR_ENABLED", "false")
    with pytest.raises(SidecarError) as exc_info:
        parse_unified(
            "scanned_invoice.png",
            (FIXTURES_DIR / "scanned_invoice.png").read_bytes(),
            data_root=tmp_path,
        )
    assert exc_info.value.status_code == 415
    assert exc_info.value.error_code == "UNSUPPORTED_FORMAT"
