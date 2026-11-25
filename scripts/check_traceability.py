#!/usr/bin/env python3
"""Verify A7-REQUIREMENTS-TRACEABILITY.md coverage and gate assertion names (A7 §3)."""

from __future__ import annotations

import argparse
import re
from pathlib import Path

_CAP_ROW_RE = re.compile(r"^\|\s*(C\d{2})\s*\|")
_DS_ROW_RE = re.compile(r"^\|\s*(D\d{2})\s*\|")
_STEP_RE = re.compile(r"\bstep(\d{2})\b")
_CURLY_QUOTE_RE = re.compile(r"“([^”]+)”")


def _extract_step_assertions(cell: str) -> list[tuple[str, str]]:
    """Extract (stepNN, assertion_name) pairs from the '验收' column of an A7 table row."""
    pairs: list[tuple[str, str]] = []
    for clause in re.split(r"[；;]", cell):
        step_match = _STEP_RE.search(clause)
        if not step_match:
            continue
        step_name = f"step{step_match.group(1)}"
        for quoted in _CURLY_QUOTE_RE.findall(clause):
            name = quoted.strip()
            if name:
                pairs.append((step_name, name))
    return pairs


def _assertion_present_in_gate(script_text: str, assertion_name: str) -> bool:
    escaped = re.escape(assertion_name)
    patterns = (
        rf'"({escaped})"',
        rf"'({escaped})'",
        rf"\b{escaped}\b",
    )
    return any(re.search(pat, script_text) for pat in patterns)


def verify_traceability(matrix_path: Path, gate_dir: Path) -> list[str]:
    errors: list[str] = []
    if not matrix_path.is_file():
        return [f"missing traceability matrix file: {matrix_path}"]
    if not gate_dir.is_dir():
        return [f"missing gate directory: {gate_dir}"]

    cap_ids: list[str] = []
    ds_ids: list[str] = []
    required_assertions: list[tuple[str, str, str]] = []

    for raw_line in matrix_path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        cap_m = _CAP_ROW_RE.match(line)
        ds_m = _DS_ROW_RE.match(line)
        if not cap_m and not ds_m:
            continue
        item_id = cap_m.group(1) if cap_m else ds_m.group(1)  # type: ignore[union-attr]
        if cap_m:
            cap_ids.append(item_id)
        else:
            ds_ids.append(item_id)

        cols = [c.strip() for c in line.strip("|").split("|")]
        if len(cols) < 5:
            errors.append(f"{item_id}: malformed table row (expected >= 5 columns)")
            continue
        acceptance_cell = cols[-1]
        for step_name, assertion_name in _extract_step_assertions(acceptance_cell):
            required_assertions.append((item_id, step_name, assertion_name))

    expected_caps = {f"C{i:02d}" for i in range(1, 99)}
    missing_caps = sorted(expected_caps - set(cap_ids))
    if len(cap_ids) < 98 or len(set(cap_ids)) != len(cap_ids) or missing_caps:
        errors.append(
            f"capability count/uniqueness/contiguity mismatch: total={len(cap_ids)} "
            f"unique={len(set(cap_ids))} missing={missing_caps} "
            "(expected >= 98 unique contiguous items C01..C98)"
        )
    if len(ds_ids) < 26 or len(set(ds_ids)) != len(ds_ids):
        errors.append(
            f"datasource count/uniqueness mismatch: total={len(ds_ids)} "
            f"unique={len(set(ds_ids))} (expected >= 26 unique items)"
        )

    gate_cache: dict[str, str | None] = {}
    for item_id, step_name, assertion_name in required_assertions:
        if step_name not in gate_cache:
            script_path = gate_dir / f"{step_name}.sh"
            gate_cache[step_name] = (
                script_path.read_text(encoding="utf-8") if script_path.is_file() else None
            )
        script_text = gate_cache[step_name]
        if script_text is None:
            errors.append(f"{item_id}: missing gate script {gate_dir / f'{step_name}.sh'}")
            continue
        if not _assertion_present_in_gate(script_text, assertion_name):
            errors.append(
                f"{item_id}: missing assertion '{assertion_name}' in {step_name}.sh"
            )

    return errors


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check A7 requirements traceability assertions against scripts/gate/stepNN.sh."
    )
    parser.add_argument(
        "matrix",
        nargs="?",
        default="docs/standards/A7-REQUIREMENTS-TRACEABILITY.md",
        help="Path to A7-REQUIREMENTS-TRACEABILITY.md",
    )
    parser.add_argument(
        "gate_dir",
        nargs="?",
        default="scripts/gate",
        help="Directory containing stepNN.sh gate scripts",
    )
    args = parser.parse_args()

    missing = verify_traceability(Path(args.matrix), Path(args.gate_dir))
    if missing:
        for line in missing:
            print(line)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
