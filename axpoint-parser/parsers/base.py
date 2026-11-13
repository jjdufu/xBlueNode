from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

from schemas import ExtractedTable, ParseWarning, ParseWarningCode


@dataclass
class ParseContext:
    """Per-request execution context passed to format parsers."""

    tenant_id: str
    parse_id: str
    staging_dir: Path
    filename: str
    options: dict[str, str] = field(default_factory=dict)
    timezone: str = "Asia/Shanghai"
    write_staging: bool = True
    warnings: list[ParseWarning] = field(default_factory=list)
    used_table_names: set[str] = field(default_factory=set)

    def add_warning(self, code: ParseWarningCode, detail: str) -> None:
        self.warnings.append(ParseWarning(code=code, detail=detail))


@dataclass
class ParseResult:
    """Dual-track output produced by a concrete BaseParser."""

    title: str
    source_format: str
    engine: str
    markdown: str
    tables: list[ExtractedTable]


class BaseParser(Protocol):
    """Protocol implemented by all AxParser format handlers."""

    suffixes: tuple[str, ...]

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult: ...
