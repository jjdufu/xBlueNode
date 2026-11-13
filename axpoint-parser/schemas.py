"""Shared Pydantic v2 schemas for AxPoint Parser & Connector Sidecar."""

from __future__ import annotations

from pydantic import BaseModel


class ParseResponse(BaseModel):
    title: str
    markdown: str
    engine: str
