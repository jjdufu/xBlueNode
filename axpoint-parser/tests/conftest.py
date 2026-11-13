from __future__ import annotations

import pytest

from tests.fixtures.make_fixtures import PARSER_ROOT, generate_all


@pytest.fixture(scope="session", autouse=True)
def ensure_step01_fixtures() -> None:
    """Auto-generate deterministic test fixtures (including gitignored *.log) before tests run."""
    fixtures_dir = PARSER_ROOT / "tests" / "fixtures"
    required = (
        fixtures_dir / "supply_chain.xlsx",
        fixtures_dir / "nginx_access.log",
        fixtures_dir / "xxe.xml",
    )
    if not all(p.is_file() for p in required):
        generate_all(root_dir=PARSER_ROOT, convert_legacy=False)
