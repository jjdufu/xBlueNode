from __future__ import annotations

import re
from collections.abc import Sequence
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from typing import Any

import polars as pl

from schemas import QualityReport, QualityRule, QualityRuleOutcome


def _to_comparable_float(val: Any) -> float | None:
    if val is None or isinstance(val, bool):
        return None
    if isinstance(val, (int, float, Decimal)):
        return float(val)
    if isinstance(val, str):
        try:
            return float(val.strip())
        except ValueError:
            return None
    return None


def _to_utc_dt(val: Any) -> datetime | None:
    if val is None:
        return None
    if isinstance(val, datetime):
        return val.astimezone(UTC) if val.tzinfo else val.replace(tzinfo=UTC)
    if isinstance(val, str):
        s = val.strip()
        if not s:
            return None
        norm = s[:-1] + "+00:00" if s.endswith("Z") else s
        try:
            dt = datetime.fromisoformat(norm)
            return dt.astimezone(UTC) if dt.tzinfo else dt.replace(tzinfo=UTC)
        except ValueError:
            return None
    return None


def evaluate_quality(
    df: pl.DataFrame,
    rules: Sequence[QualityRule],
    *,
    now_utc: datetime | None = None,
) -> QualityReport:
    """Evaluate all configured QualityRules over a batch Polars DataFrame."""
    if not rules:
        return QualityReport(passed=True, blocked=False, rules=[])

    ref_now = now_utc or datetime.now(UTC)
    outcomes: list[QualityRuleOutcome] = []
    all_passed = True
    any_blocked = False

    for idx, rule in enumerate(rules):
        r_id = rule.id or f"rule_{idx + 1}"
        kind = rule.rule_kind
        col = rule.column_name
        params = rule.params or {}
        passed = True
        detail = "ok"

        if kind == "ROW_COUNT_MIN":
            min_rows = int(params.get("min_rows", params.get("min", 1)))
            actual = df.height
            if actual < min_rows:
                passed = False
                detail = f"row_count {actual} < min {min_rows}"

        elif not col or col not in df.columns:
            passed = False
            detail = f"column {col!r} missing from batch"

        elif kind == "NOT_NULL":
            null_cnt = int(df[col].null_count())
            if null_cnt > 0:
                passed = False
                detail = f"{null_cnt} NULL values in column {col!r}"

        elif kind == "UNIQUE":
            non_null = df[col].drop_nulls()
            dup_cnt = non_null.len() - int(non_null.n_unique())
            if dup_cnt > 0:
                passed = False
                detail = f"{dup_cnt} duplicate values in column {col!r}"

        elif kind == "RANGE":
            min_v = params.get("min")
            max_v = params.get("max")
            min_f = float(min_v) if min_v is not None else None
            max_f = float(max_v) if max_v is not None else None
            violations = 0
            for raw_v in df[col].drop_nulls().to_list():
                fv = _to_comparable_float(raw_v)
                if fv is None:
                    violations += 1
                    continue
                if (min_f is not None and fv < min_f) or (max_f is not None and fv > max_f):
                    violations += 1
            if violations > 0:
                passed = False
                detail = f"{violations} out-of-range values in column {col!r} ([{min_v}, {max_v}])"

        elif kind == "REGEX":
            pat_str = str(params.get("pattern") or "")
            compiled = re.compile(pat_str)
            violations = 0
            for raw_v in df[col].drop_nulls().to_list():
                if compiled.search(str(raw_v)) is None:
                    violations += 1
            if violations > 0:
                passed = False
                detail = f"{violations} values in {col!r} failed regex {pat_str!r}"

        elif kind in {"ENUM", "ALLOWED_VALUES", "ENUM_SET"}:
            raw_allowed = params.get("values") or params.get("allowed") or []
            allowed_str = {str(x) for x in raw_allowed}
            violations = 0
            for raw_v in df[col].drop_nulls().to_list():
                if str(raw_v) not in allowed_str:
                    violations += 1
            if violations > 0:
                passed = False
                detail = f"{violations} values in {col!r} outside allowed set"

        elif kind in {"FRESHNESS", "FRESHNESS_MINUTES"}:
            max_lag = float(params.get("max_lag_minutes") or params.get("minutes") or 60.0)
            dts = [_to_utc_dt(v) for v in df[col].drop_nulls().to_list()]
            valid_dts = [d for d in dts if d is not None]
            if not valid_dts:
                passed = False
                detail = f"no valid timestamps in {col!r} for freshness check"
            else:
                latest = max(valid_dts)
                cutoff = ref_now - timedelta(minutes=max_lag)
                if latest < cutoff:
                    passed = False
                    detail = f"latest timestamp {latest.isoformat()} older than {max_lag}m cutoff"

        if not passed:
            all_passed = False
            if rule.on_fail == "BLOCK":
                any_blocked = True

        outcomes.append(
            QualityRuleOutcome(
                rule_id=r_id,
                rule_kind=kind,
                column_name=col,
                passed=passed,
                on_fail=rule.on_fail,
                detail=detail,
            )
        )

    return QualityReport(
        passed=all_passed,
        blocked=any_blocked,
        rules=outcomes,
    )
