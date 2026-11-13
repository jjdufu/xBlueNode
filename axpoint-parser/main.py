"""Stateless FastAPI sidecar for AxPoint document parsing, connectors, and export."""

from __future__ import annotations

import os
from fastapi import FastAPI, File, HTTPException, UploadFile

from parsers.engine import DEFAULT_MODEL_PATH, parse_bytes_to_markdown
from schemas import ParseResponse

app = FastAPI(
    title="AxPoint Parser Sidecar",
    version="0.1.0",
    description="Stateless PDF/OCR/Office to Markdown microservice for xBlueNode / AxPoint",
)


@app.get("/health")
def health() -> dict[str, object]:
    easyocr_ready = os.path.isdir(os.path.join(DEFAULT_MODEL_PATH, "easyocr"))
    return {
        "status": "ok",
        "service": "axpoint-parser",
        "model_path": DEFAULT_MODEL_PATH,
        "easyocr_models_present": easyocr_ready,
    }


@app.post("/parse", response_model=ParseResponse)
async def parse_document(file: UploadFile = File(...)) -> ParseResponse:
    raw = await file.read()
    if not raw:
        raise HTTPException(status_code=400, detail="Uploaded file is empty")

    filename = file.filename or "document.txt"
    try:
        title, markdown, engine = parse_bytes_to_markdown(filename, raw)
    except Exception as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    return ParseResponse(title=title, markdown=markdown, engine=engine)
