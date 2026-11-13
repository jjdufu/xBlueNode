from __future__ import annotations

import hashlib
import subprocess
import sys
from pathlib import Path

from tests.fixtures.make_fixtures import (
    MANIFEST_PATH,
    PARSER_ROOT,
    compute_sha256_map,
    generate_all,
)


def _parse_manifest(manifest_path: Path) -> dict[str, str]:
    entries: dict[str, str] = {}
    for raw_line in manifest_path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line:
            continue
        digest, sep, rel_path = line.partition("  ")
        assert sep == "  ", f"Malformed manifest line: {raw_line!r}"
        assert len(digest) == 64, f"Invalid SHA-256 hex digest in line: {raw_line!r}"
        entries[rel_path] = digest
    return entries


def test_fixtures_reproducible(tmp_path: Path) -> None:
    assert MANIFEST_PATH.is_file(), f"Missing manifest: {MANIFEST_PATH}"
    expected = _parse_manifest(MANIFEST_PATH)
    assert len(expected) == 28

    for legacy_excluded in (
        "tests/fixtures/early_format.xls",
        "tests/fixtures/early_format.ppt",
        "tests/fixtures/early_format.doc",
        "tests/fixtures/report.xlsb",
    ):
        assert legacy_excluded not in expected

    # Run 1: execute make_fixtures.py CLI directly in axpoint-parser root
    script_path = PARSER_ROOT / "tests" / "fixtures" / "make_fixtures.py"
    proc = subprocess.run(
        [sys.executable, str(script_path)],
        cwd=str(PARSER_ROOT),
        capture_output=True,
        text=True,
        check=False,
    )
    assert proc.returncode == 0, f"make_fixtures.py failed:\n{proc.stderr}"

    run1_digests: dict[str, str] = {}
    for rel_path in sorted(expected):
        target = PARSER_ROOT / rel_path
        assert target.is_file(), f"Expected generated fixture missing: {rel_path}"
        run1_digests[rel_path] = hashlib.sha256(target.read_bytes()).hexdigest()

    assert run1_digests == expected

    # Run 2: generate into an isolated temporary root and compare byte-for-byte
    run2_paths = generate_all(root_dir=tmp_path, convert_legacy=False)
    run2_digests = compute_sha256_map(run2_paths, root_dir=tmp_path)

    assert run2_digests == expected
    assert run1_digests == run2_digests
