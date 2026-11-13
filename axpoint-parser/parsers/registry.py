from __future__ import annotations

import io
import re
import zipfile
from pathlib import Path

from parsers.base import BaseParser, ParseContext
from parsers.csv_parser import CsvParser
from parsers.docx_parser import DocxParser
from parsers.excel_parser import ExcelParser
from parsers.json_parser import JsonParser
from parsers.log_parser import LogParser
from parsers.pdf_parser import PdfAndImageParser
from parsers.pptx_parser import PptxParser
from parsers.text_parser import TextParser
from parsers.xml_parser import XmlParser
from security import SidecarError, check_zip

_CSV = CsvParser()
_TEXT = TextParser()
_EXCEL = ExcelParser()
_PPTX = PptxParser()
_DOCX = DocxParser()
_PDF_IMG = PdfAndImageParser()
_JSON = JsonParser()
_XML = XmlParser()
_LOG = LogParser()

_SUFFIX_MAP: dict[str, BaseParser] = {}
for _parser in (_CSV, _TEXT, _EXCEL, _PPTX, _DOCX, _PDF_IMG, _JSON, _XML, _LOG):
    for _sfx in _parser.suffixes:
        _SUFFIX_MAP[_sfx] = _parser

_LOG_HEAD_RE = re.compile(
    r"^(?:\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}"
    r"|<\d{1,3}>[A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})"
)


def _detect_zip_container(data: bytes, suffix: str) -> BaseParser | None:
    """Inspect PK archive [Content_Types].xml or mimetype to distinguish xlsx/pptx/docx/ods."""
    check_zip(data)
    with zipfile.ZipFile(io.BytesIO(data), "r") as zf:
        names = set(zf.namelist())
        if "mimetype" in names:
            mime = zf.read("mimetype").decode("utf-8", errors="ignore").strip()
            if "opendocument.spreadsheet" in mime:
                return _EXCEL
            if "opendocument.presentation" in mime:
                return _PPTX
            if "opendocument.text" in mime:
                return _DOCX

        if (
            "xl/workbook.xml" in names
            or "xl/workbook.bin" in names
            or any(n.startswith("xl/worksheets/") for n in names)
        ):
            return _EXCEL
        if "ppt/presentation.xml" in names or any(n.startswith("ppt/slides/") for n in names):
            return _PPTX
        if "word/document.xml" in names:
            return _DOCX

        if "[Content_Types].xml" in names:
            ct_txt = zf.read("[Content_Types].xml").decode("utf-8", errors="ignore").lower()
            if "spreadsheetml" in ct_txt:
                return _EXCEL
            if "presentationml" in ct_txt:
                return _PPTX
            if "wordprocessingml" in ct_txt:
                return _DOCX

    return _SUFFIX_MAP.get(suffix)


def _detect_magic_parser(data: bytes, suffix: str) -> BaseParser | None:
    if data.startswith(b"PK\x03\x04"):
        return _detect_zip_container(data, suffix)
    if data.startswith(b"%PDF-"):
        return _PDF_IMG
    if data.startswith(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"):
        if suffix == ".ppt":
            return _PPTX
        if suffix == ".doc":
            return _DOCX
        return _EXCEL
    if (
        data.startswith(b"\x89PNG\r\n\x1a\n")
        or data.startswith(b"\xff\xd8\xff")
        or data.startswith((b"II*\x00", b"MM\x00*"))
    ):
        return _PDF_IMG
    return None


def resolve_parser(data: bytes, ctx: ParseContext) -> BaseParser:
    """Resolve format parser using (1) magic bytes, (2) suffix, (3) content sniffing."""
    suffix = Path(ctx.filename).suffix.lower()
    magic_parser = _detect_magic_parser(data, suffix)
    suffix_parser = _SUFFIX_MAP.get(suffix)

    if magic_parser is not None:
        return magic_parser
    if suffix_parser is not None:
        return suffix_parser

    # Content sniffing fallback for extensionless files
    head = data[:4096].lstrip()
    if head.startswith((b"{", b"[")):
        return _JSON
    if head.startswith(b"<?xml") or (
        head.startswith(b"<") and not head.lower().startswith(b"<!doctype html")
    ):
        return _XML
    head_str = head.decode("utf-8", errors="ignore")
    if _LOG_HEAD_RE.match(head_str):
        return _LOG
    if head:
        return _TEXT

    raise SidecarError(
        415, "UNSUPPORTED_FORMAT", f"unable to determine format for {ctx.filename!r}"
    )
