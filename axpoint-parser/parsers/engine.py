from __future__ import annotations

import io
import os
import tempfile
from pathlib import Path

from settings import get_model_path

DEFAULT_MODEL_PATH = os.getenv("AXPOINT_MODEL_PATH", "/var/lib/axpoint/models")


def parse_bytes_to_markdown(filename: str, content: bytes) -> tuple[str, str, str]:
    """Convert uploaded file bytes into (title, markdown, engine_used)."""
    path = Path(filename)
    suffix = path.suffix.lower()
    title = path.stem or "untitled"

    if suffix in {".md", ".markdown", ".txt", ".csv", ".json"}:
        text = content.decode("utf-8", errors="replace").strip()
        return title, text, "plain-utf8"

    # Try Docling + EasyOCR if installed
    try:
        markdown = _parse_with_docling(suffix, content)
        if markdown.strip():
            return title, markdown, "docling-easyocr"
    except ImportError:
        pass

    # Lightweight fallback for text-based PDFs
    if suffix == ".pdf":
        markdown = _parse_pdf_pypdf(content)
        if markdown.strip():
            return title, markdown, "pypdf-fallback"

    raise RuntimeError(
        f"Unsupported or unparseable file '{filename}'. "
        "Install optional `docling` + `easyocr` extras for full OCR/layout support."
    )


def _parse_with_docling(suffix: str, content: bytes) -> str:
    from docling.datamodel.base_models import InputFormat
    from docling.datamodel.pipeline_options import EasyOcrOptions, PdfPipelineOptions
    from docling.document_converter import (
        DocumentConverter,
        ImageFormatOption,
        PdfFormatOption,
        WordFormatOption,
    )

    model_root = get_model_path()
    easyocr_dir = model_root / "easyocr"
    doc2md_dir = model_root / "doc2md"

    easyocr_options = EasyOcrOptions()
    if easyocr_dir.is_dir():
        easyocr_options.model_storage_directory = str(easyocr_dir)

    pdf_options = PdfPipelineOptions(
        artifacts_path=str(doc2md_dir) if doc2md_dir.is_dir() else None
    )
    pdf_options.ocr_options = easyocr_options
    pdf_options.do_ocr = True
    pdf_options.do_table_structure = True

    converter = DocumentConverter(
        format_options={
            InputFormat.PDF: PdfFormatOption(pipeline_options=pdf_options),
            InputFormat.IMAGE: ImageFormatOption(pipeline_options=pdf_options),
            InputFormat.DOCX: WordFormatOption(pipeline_options=pdf_options),
        }
    )

    with tempfile.NamedTemporaryFile(suffix=suffix, delete=True) as tmp:
        tmp.write(content)
        tmp.flush()
        result = converter.convert(tmp.name)
        return str(result.document.export_to_markdown())


def _parse_pdf_pypdf(content: bytes) -> str:
    from pypdf import PdfReader

    reader = PdfReader(io.BytesIO(content))
    pages: list[str] = []
    for idx, page in enumerate(reader.pages, start=1):
        txt = (page.extract_text() or "").strip()
        if txt:
            pages.append(f"## Page {idx}\n\n{txt}")
    return "\n\n".join(pages)
