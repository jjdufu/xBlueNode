from __future__ import annotations

import math
import re
from collections.abc import Sequence
from datetime import UTC, date, datetime
from decimal import Decimal, InvalidOperation
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

import orjson
import polars as pl

from schemas import ColumnProfile, DataType, PiiCategory

_BOOL_TRUE = frozenset({"true", "yes"})
_BOOL_FALSE = frozenset({"false", "no"})

_CURRENCY_PREFIX_RE = re.compile(
    r"^\s*(?:[¥￥$€£]|(?:RMB|CNY|USD|EUR)\s*)\s*([+-]?[\d,]+(?:\.\d+)?)\s*$",
    re.IGNORECASE,
)
_THOUSANDS_RE = re.compile(r"^[+-]?\d{1,3}(?:,\d{3})+(?:\.\d+)?$")
_PERCENT_RE = re.compile(r"^([+-]?\d+(?:\.\d+)?)%$")
_INT_STR_RE = re.compile(r"^[+-]?(?:0|[1-9]\d*)$")
_DECIMAL_STR_RE = re.compile(r"^[+-]?(?:0|[1-9]\d*)\.\d+$")
_FLOAT_STR_RE = re.compile(r"^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$")

_DATE_YMD_RE = re.compile(r"^(\d{4})[-/](\d{1,2})[-/](\d{1,2})$")
_DATE_CN_RE = re.compile(r"^(\d{4})年(\d{1,2})月(\d{1,2})日?$")
_DATE_8DIGIT_RE = re.compile(r"^(\d{4})(\d{2})(\d{2})$")
_DATE_DMY_RE = re.compile(r"^(\d{1,2})/(\d{1,2})/(\d{4})$")

_TS_ISO_RE = re.compile(
    r"^(\d{4})[-/](\d{1,2})[-/](\d{1,2})[T ](\d{1,2}):(\d{2}):(\d{2})"
    r"(?:[.,](\d{1,9}))?(?:\s*(Z|[+-]\d{2}:?\d{2}))?$"
)
_TS_CN_RE = re.compile(r"^(\d{4})年(\d{1,2})月(\d{1,2})日?\s+(\d{1,2}):(\d{2})(?::(\d{2}))?$")

_PHONE_CLEAN_RE = re.compile(r"[\s\-()]")
_PHONE_RE = re.compile(r"^1[3-9]\d{9}$")
_EMAIL_RE = re.compile(r"^[^@\s]+@[^@\s]+\.[A-Za-z]{2,}$")
_SECRET_COL_RE = re.compile(
    r"(?i)(?:^|_)(?:password|passwd|secret|token|api[_-]?key|private[_-]?key)(?:$|_)"
)
_SECRET_VAL_RE = re.compile(r"^(?:sk|ak|ghp|xox[bp])-[A-Za-z0-9_-]{16,}$")
_NAME_COL_RE = re.compile(
    r"(?i)^(?:name|full_name|person_name|customer_name|contact_name|real_name|姓名|客户姓名|联系人|收货人)$"
)
_ADDRESS_COL_RE = re.compile(
    r"(?i)^(?:address|street_address|home_address|shipping_address|地址|详细地址|收货地址|联系地址|家庭住址)$"
)

_ID_WEIGHTS = (7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2)
_ID_CHECK_MAP = "10X98765432"

_COMPOUND_SURNAMES = frozenset(
    {
        "欧阳",
        "太史",
        "端木",
        "上官",
        "司马",
        "东方",
        "独孤",
        "南宫",
        "万俟",
        "闻人",
        "夏侯",
        "诸葛",
        "尉迟",
        "公羊",
        "赫连",
        "澹台",
        "皇甫",
        "宗政",
        "濮阳",
        "公冶",
        "太叔",
        "申屠",
        "公孙",
        "慕容",
        "仲孙",
        "钟离",
        "长孙",
        "宇文",
        "司徒",
        "鲜于",
        "司空",
        "闾丘",
        "子车",
        "亓官",
        "司寇",
        "巫马",
        "公西",
        "颛孙",
        "壤驷",
        "公良",
        "漆雕",
        "乐正",
        "宰父",
        "谷梁",
        "拓跋",
        "夹谷",
        "轩辕",
        "令狐",
        "段干",
        "百里",
        "呼延",
        "东郭",
        "南门",
        "羊舌",
        "微生",
        "公户",
        "公玉",
        "公仪",
        "梁丘",
        "公仲",
        "公上",
        "公门",
        "公山",
        "公坚",
        "左丘",
        "公伯",
        "西门",
        "公祖",
        "第五",
        "公乘",
        "贯丘",
        "公皙",
        "南荣",
        "东里",
        "东宫",
        "仲长",
        "子书",
        "子桑",
        "即墨",
        "达奚",
        "褚师",
    }
)

_ADDR_ADMIN_RE = re.compile(r"^(.+?(?:省|自治区|市|区|县|旗))(.*)$")


def _resolve_tz(tz_name: str) -> ZoneInfo:
    try:
        return ZoneInfo(tz_name)
    except (ZoneInfoNotFoundError, KeyError, ValueError):
        return ZoneInfo("Asia/Shanghai")


def is_valid_cn_phone(raw: str) -> bool:
    """Validate mainland China 11-digit mobile number."""
    s = _PHONE_CLEAN_RE.sub("", raw.strip())
    if s.startswith("+86"):
        s = s[3:]
    elif s.startswith("0086"):
        s = s[4:]
    return bool(_PHONE_RE.fullmatch(s))


def is_valid_cn_id_card(raw: str) -> bool:
    """Validate 18-digit GB 11643-1999 resident ID number including checksum and birth date."""
    s = raw.strip().upper()
    if len(s) != 18 or not s[:17].isdigit():
        return False
    try:
        year = int(s[6:10])
        month = int(s[10:12])
        day = int(s[12:14])
        if not (1900 <= year <= 2099):
            return False
        date(year, month, day)
    except ValueError:
        return False

    checksum = sum(int(s[i]) * _ID_WEIGHTS[i] for i in range(17)) % 11
    return s[17] == _ID_CHECK_MAP[checksum]


def is_valid_bank_card(raw: str) -> bool:
    """Validate 13..19 digit payment card via BIN prefix and Luhn algorithm."""
    s = re.sub(r"[\s\-]", "", raw.strip())
    if not (13 <= len(s) <= 19) or not s.isdigit():
        return False
    if not (s.startswith("62") or s[0] in {"3", "4", "5"}):
        return False
    total = 0
    reverse_digits = s[::-1]
    for idx, ch in enumerate(reverse_digits):
        d = ord(ch) - 48
        if idx % 2 == 1:
            d *= 2
            if d > 9:
                d -= 9
        total += d
    return total % 10 == 0


def is_valid_email(raw: str) -> bool:
    """Validate email address via simplified RFC 5322 pattern."""
    s = raw.strip()
    return len(s) <= 254 and bool(_EMAIL_RE.fullmatch(s))


def detect_pii_category(
    col_name: str, non_null_samples: Sequence[str]
) -> tuple[PiiCategory, float]:
    """Detect column PII category and hit ratio over up to 1,000 non-null samples."""
    if _SECRET_COL_RE.search(col_name):
        return "SECRET", 1.0

    sample = list(non_null_samples[:1000])
    n = len(sample)
    if n > 0:
        secret_hits = sum(1 for v in sample if _SECRET_VAL_RE.fullmatch(v.strip()))
        if secret_hits / n >= 0.30:
            return "SECRET", round(secret_hits / n, 4)

        id_hits = sum(1 for v in sample if is_valid_cn_id_card(v))
        if id_hits / n >= 0.30:
            return "ID_CARD", round(id_hits / n, 4)

        bank_hits = sum(1 for v in sample if is_valid_bank_card(v))
        if bank_hits / n >= 0.30:
            return "BANK_CARD", round(bank_hits / n, 4)

        phone_hits = sum(1 for v in sample if is_valid_cn_phone(v))
        if phone_hits / n >= 0.30:
            return "PHONE", round(phone_hits / n, 4)

        email_hits = sum(1 for v in sample if is_valid_email(v))
        if email_hits / n >= 0.30:
            return "EMAIL", round(email_hits / n, 4)

    if _NAME_COL_RE.fullmatch(col_name.strip()):
        return "NAME", 1.0 if n > 0 else 0.0
    if _ADDRESS_COL_RE.fullmatch(col_name.strip()):
        return "ADDRESS", 1.0 if n > 0 else 0.0

    return "NONE", 0.0


def mask_pii_value(value: str, category: PiiCategory) -> str:
    """Mask a single string value according to Step 13 §13.7 MaskRule conventions."""
    if not value or category == "NONE":
        return value

    s = value.strip()
    if category == "SECRET":
        return "[REDACTED]"

    if category == "PHONE":
        prefix = ""
        cleaned = _PHONE_CLEAN_RE.sub("", s)
        if cleaned.startswith("+86"):
            prefix = "+86"
            cleaned = cleaned[3:]
        elif cleaned.startswith("0086"):
            prefix = "0086"
            cleaned = cleaned[4:]
        if len(cleaned) >= 7:
            return f"{prefix}{cleaned[:3]}****{cleaned[-4:]}"
        return "****"

    if category == "ID_CARD":
        if len(s) >= 10:
            return f"{s[:6]}{'*' * (len(s) - 10)}{s[-4:]}"
        return "********"

    if category == "EMAIL":
        at_idx = s.find("@")
        if at_idx > 0:
            local = s[:at_idx]
            domain = s[at_idx:]
            masked_local = local[0] + ("*" * max(len(local) - 1, 1))
            return f"{masked_local}{domain}"
        return "*@***"

    if category == "BANK_CARD":
        digits = re.sub(r"[\s\-]", "", s)
        if len(digits) > 4:
            return f"{'*' * (len(digits) - 4)}{digits[-4:]}"
        return "****"

    if category == "NAME":
        if len(s) >= 3 and s[:2] in _COMPOUND_SURNAMES:
            return s[:2] + ("*" * (len(s) - 2))
        if any("\u4e00" <= ch <= "\u9fff" for ch in s):
            return s[0] + ("*" * max(len(s) - 1, 1))
        parts = s.split()
        if parts:
            return " ".join(p[0] + ("*" * max(len(p) - 1, 1)) for p in parts)
        return s[0] + "*"

    if category == "ADDRESS":
        m = _ADDR_ADMIN_RE.match(s)
        if m and m.group(2):
            return f"{m.group(1)}****"
        if len(s) > 6:
            return f"{s[:6]}****"
        return "****"

    return value


def _clean_numeric_str(s: str) -> tuple[str, bool] | None:
    """Clean currency symbols, thousand commas, and percentages.

    Returns (normalized_number_str, is_percentage) or None if not numeric-looking.
    """
    txt = s.strip()
    if not txt:
        return None
    if re.fullmatch(r"[+-]?0\d+", txt):
        return None
    m_pct = _PERCENT_RE.fullmatch(txt)
    if m_pct:
        return m_pct.group(1), True
    m_curr = _CURRENCY_PREFIX_RE.fullmatch(txt)
    if m_curr:
        return m_curr.group(1).replace(",", ""), False
    if _THOUSANDS_RE.fullmatch(txt):
        return txt.replace(",", ""), False
    return txt, False


def _parse_bool(val: Any) -> bool | None:
    if isinstance(val, bool):
        return val
    if isinstance(val, str):
        low = val.strip().lower()
        if low in _BOOL_TRUE:
            return True
        if low in _BOOL_FALSE:
            return False
    return None


def _parse_int64(val: Any) -> int | None:
    if isinstance(val, bool):
        return None
    if isinstance(val, int):
        if -(1 << 63) <= val <= (1 << 63) - 1:
            return val
        return None
    if isinstance(val, float):
        if math.isfinite(val) and val.is_integer():
            iv = int(val)
            if -(1 << 63) <= iv <= (1 << 63) - 1:
                return iv
        return None
    if isinstance(val, str):
        cleaned = _clean_numeric_str(val)
        if cleaned is None or cleaned[1]:
            return None
        num_s = cleaned[0]
        if _INT_STR_RE.fullmatch(num_s):
            iv = int(num_s)
            if -(1 << 63) <= iv <= (1 << 63) - 1:
                return iv
    return None


def _parse_decimal_with_scale(val: Any) -> tuple[Decimal, int, int] | None:
    """Return (Decimal_value, total_precision, scale) if valid finite decimal <= 38 digits."""
    if isinstance(val, bool):
        return None
    if isinstance(val, Decimal):
        if not val.is_finite():
            return None
        tup = val.as_tuple()
        scale = -int(tup.exponent) if isinstance(tup.exponent, int) and tup.exponent < 0 else 0
        prec = max(len(tup.digits), scale)
        if prec <= 38:
            return val, prec, scale
        return None
    if isinstance(val, int):
        s = str(abs(val))
        if len(s) <= 38:
            return Decimal(val), len(s), 0
        return None
    if isinstance(val, str):
        cleaned = _clean_numeric_str(val)
        if cleaned is None:
            return None
        num_s, is_pct = cleaned
        if "e" in num_s.lower():
            return None
        if not (_DECIMAL_STR_RE.fullmatch(num_s) or _INT_STR_RE.fullmatch(num_s)):
            return None
        try:
            dec = Decimal(num_s)
        except InvalidOperation:
            return None
        if is_pct:
            dec = dec / Decimal(100)
        tup = dec.as_tuple()
        scale = -int(tup.exponent) if isinstance(tup.exponent, int) and tup.exponent < 0 else 0
        prec = max(len(tup.digits), scale)
        if prec <= 38:
            return dec, prec, scale
    return None


def _parse_float64(val: Any) -> float | None:
    if isinstance(val, bool):
        return None
    if isinstance(val, (int, float, Decimal)):
        fv = float(val)
        return fv if math.isfinite(fv) else None
    if isinstance(val, str):
        cleaned = _clean_numeric_str(val)
        if cleaned is None:
            return None
        num_s, is_pct = cleaned
        if not _FLOAT_STR_RE.fullmatch(num_s):
            return None
        try:
            fv = float(num_s)
        except ValueError:
            return None
        if not math.isfinite(fv):
            return None
        return fv / 100.0 if is_pct else fv
    return None


def _parse_date(val: Any, *, allow_dmy: bool = False, allow_8digit: bool = False) -> date | None:
    if isinstance(val, datetime):
        return None
    if isinstance(val, date):
        return val
    if not isinstance(val, str):
        return None
    txt = val.strip()
    m = _DATE_YMD_RE.fullmatch(txt) or _DATE_CN_RE.fullmatch(txt)
    if m:
        try:
            return date(int(m.group(1)), int(m.group(2)), int(m.group(3)))
        except ValueError:
            return None
    if allow_8digit:
        m8 = _DATE_8DIGIT_RE.fullmatch(txt)
        if m8:
            yr = int(m8.group(1))
            if 1900 <= yr <= 2100:
                try:
                    return date(yr, int(m8.group(2)), int(m8.group(3)))
                except ValueError:
                    return None
    if allow_dmy:
        md = _DATE_DMY_RE.fullmatch(txt)
        if md:
            try:
                return date(int(md.group(3)), int(md.group(2)), int(md.group(1)))
            except ValueError:
                return None
    return None


def _parse_timestamp(val: Any, tz: ZoneInfo) -> datetime | None:
    if isinstance(val, datetime):
        if val.tzinfo is None:
            return val.replace(tzinfo=tz).astimezone(UTC)
        return val.astimezone(UTC)
    if not isinstance(val, str):
        return None
    txt = val.strip()
    m = _TS_ISO_RE.fullmatch(txt)
    if m:
        yr, mo, dy = int(m.group(1)), int(m.group(2)), int(m.group(3))
        hh, mm, ss = int(m.group(4)), int(m.group(5)), int(m.group(6))
        frac = m.group(7) or ""
        us = int(frac[:6].ljust(6, "0")) if frac else 0
        tz_part = m.group(8)
        try:
            if tz_part is None:
                dt = datetime(yr, mo, dy, hh, mm, ss, us, tzinfo=tz)
                return dt.astimezone(UTC)
            if tz_part == "Z":
                return datetime(yr, mo, dy, hh, mm, ss, us, tzinfo=UTC)
            iso_norm = f"{yr:04d}-{mo:02d}-{dy:02d}T{hh:02d}:{mm:02d}:{ss:02d}.{us:06d}"
            tz_norm = tz_part if ":" in tz_part else f"{tz_part[:3]}:{tz_part[3:]}"
            return datetime.fromisoformat(iso_norm + tz_norm).astimezone(UTC)
        except ValueError:
            return None

    m_cn = _TS_CN_RE.fullmatch(txt)
    if m_cn:
        try:
            dt = datetime(
                int(m_cn.group(1)),
                int(m_cn.group(2)),
                int(m_cn.group(3)),
                int(m_cn.group(4)),
                int(m_cn.group(5)),
                int(m_cn.group(6) or "0"),
                tzinfo=tz,
            )
            return dt.astimezone(UTC)
        except ValueError:
            return None
    return None


def _parse_json_str(val: Any) -> str | None:
    if isinstance(val, (dict, list)):
        return orjson.dumps(val).decode("utf-8")
    if isinstance(val, str):
        txt = val.strip()
        if (txt.startswith("{") and txt.endswith("}")) or (
            txt.startswith("[") and txt.endswith("]")
        ):
            try:
                orjson.loads(txt)
                return txt
            except orjson.JSONDecodeError:
                return None
    return None


def _stringify_for_sample(val: Any) -> str:
    if val is None:
        return ""
    if isinstance(val, bytes):
        return f"<binary:{len(val)}B>"
    if isinstance(val, datetime):
        return val.astimezone(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    if isinstance(val, date):
        return val.isoformat()
    if isinstance(val, (dict, list)):
        return orjson.dumps(val).decode("utf-8")
    if isinstance(val, float) and math.isfinite(val) and val.is_integer():
        return str(int(val))
    return str(val)


def infer_and_convert_column(
    col_name: str,
    ordinal: int,
    values: Sequence[Any],
    *,
    timezone_str: str = "Asia/Shanghai",
    source_comment: str = "",
) -> tuple[pl.Series, ColumnProfile, int]:
    """Infer canonical DataType, convert values into a typed Polars Series, and build ColumnProfile.

    Returns (typed_series, profile, malformed_coercion_count).
    """
    total = len(values)
    non_null_indices: list[int] = []
    non_null_vals: list[Any] = []
    for idx, v in enumerate(values):
        if v is None:
            continue
        if isinstance(v, str) and v.strip() == "":
            continue
        if isinstance(v, float) and math.isnan(v):
            continue
        non_null_indices.append(idx)
        non_null_vals.append(v)

    # Uniform sampling when > 100_000 non-null values
    n_non_null = len(non_null_vals)
    if n_non_null > 100_000:
        step = max(1, n_non_null // 100_000)
        probe_vals = non_null_vals[::step][:100_000]
    else:
        probe_vals = non_null_vals

    str_samples = [_stringify_for_sample(v) for v in probe_vals[:1000]]
    pii_cat, pii_ratio = detect_pii_category(col_name, str_samples)

    tz = _resolve_tz(timezone_str)
    threshold = 0.98
    malformed_count = 0

    if n_non_null == 0:
        series = pl.Series(col_name, [None] * total, dtype=pl.String)
        profile = ColumnProfile(
            name=col_name,
            ordinal=ordinal,
            data_type="STRING",
            nullable=True,
            null_count=total,
            null_ratio=1.0 if total > 0 else 0.0,
            distinct_estimate=0,
            sample_values=[],
            pii_category=pii_cat,
            pii_hit_ratio=pii_ratio,
            source_comment=source_comment,
        )
        return series, profile, 0

    # Check native container types first (BINARY, LIST, STRUCT)
    if all(isinstance(v, (bytes, bytearray, memoryview)) for v in probe_vals):
        bin_out: list[bytes | None] = [
            bytes(v) if isinstance(v, (bytes, bytearray, memoryview)) else None for v in values
        ]
        series = pl.Series(col_name, bin_out, dtype=pl.Binary)
        return _finalize_profile(
            series,
            col_name=col_name,
            ordinal=ordinal,
            data_type="BINARY",
            pii_cat=pii_cat,
            pii_ratio=pii_ratio,
            str_samples=str_samples,
            source_comment=source_comment,
            malformed_count=0,
        )

    if all(isinstance(v, list) and not any(isinstance(x, dict) for x in v) for v in probe_vals):
        list_out: list[list[str] | None] = [
            [_stringify_for_sample(x) for x in v] if isinstance(v, list) else None for v in values
        ]
        series = pl.Series(col_name, list_out, dtype=pl.List(pl.String))
        return _finalize_profile(
            series,
            col_name=col_name,
            ordinal=ordinal,
            data_type="LIST",
            pii_cat=pii_cat,
            pii_ratio=pii_ratio,
            str_samples=str_samples,
            source_comment=source_comment,
            malformed_count=0,
        )

    # Identifiers classified as PHONE / ID_CARD / BANK_CARD / SECRET stay STRING
    inferred_type: DataType = "STRING"
    dec_scale = 2

    if pii_cat not in {"PHONE", "ID_CARD", "BANK_CARD", "SECRET"}:
        n_probe = len(probe_vals)
        # 1. BOOLEAN
        bool_hits = sum(1 for v in probe_vals if _parse_bool(v) is not None)
        if bool_hits / n_probe >= threshold:
            inferred_type = "BOOLEAN"
        else:
            # Check if column looks like 8-digit YYYYMMDD date before INT64
            col_date_hint = bool(re.search(r"(?i)(?:date|dt|日期|生日|birth)", col_name))
            date8_hits = (
                sum(
                    1
                    for v in probe_vals
                    if isinstance(v, str)
                    and _DATE_8DIGIT_RE.fullmatch(v.strip()) is not None
                    and _parse_date(v, allow_8digit=True) is not None
                )
                if col_date_hint
                else 0
            )
            if col_date_hint and date8_hits / n_probe >= threshold:
                inferred_type = "DATE"
            else:
                # 2. INT64
                int_hits = sum(1 for v in probe_vals if _parse_int64(v) is not None)
                if int_hits / n_probe >= threshold:
                    inferred_type = "INT64"
                else:
                    # 3. DECIMAL vs 4. FLOAT64
                    dec_parsed = [_parse_decimal_with_scale(v) for v in probe_vals]
                    dec_valid = [d for d in dec_parsed if d is not None]
                    if len(dec_valid) / n_probe >= threshold:
                        scales = {d[2] for d in dec_valid if d[2] > 0}
                        max_scale = max((d[2] for d in dec_valid), default=0)
                        if len(scales) == 1 and 1 <= max_scale <= 6:
                            inferred_type = "DECIMAL"
                            dec_scale = max_scale
                        else:
                            inferred_type = "FLOAT64"
                    else:
                        flt_hits = sum(1 for v in probe_vals if _parse_float64(v) is not None)
                        if flt_hits / n_probe >= threshold:
                            inferred_type = "FLOAT64"
                        else:
                            # 5. DATE
                            has_dmy_gt12 = any(
                                isinstance(v, str)
                                and (m := _DATE_DMY_RE.fullmatch(v.strip())) is not None
                                and int(m.group(1)) > 12
                                for v in probe_vals
                            )
                            date_hits = sum(
                                1
                                for v in probe_vals
                                if _parse_date(v, allow_dmy=has_dmy_gt12) is not None
                            )
                            if date_hits / n_probe >= threshold:
                                inferred_type = "DATE"
                            else:
                                # 6. TIMESTAMP
                                ts_hits = sum(
                                    1 for v in probe_vals if _parse_timestamp(v, tz) is not None
                                )
                                if ts_hits / n_probe >= threshold:
                                    inferred_type = "TIMESTAMP"
                                else:
                                    # 7. JSON
                                    json_hits = sum(
                                        1 for v in probe_vals if _parse_json_str(v) is not None
                                    )
                                    if json_hits / n_probe >= threshold:
                                        inferred_type = "JSON"

    # Convert full column according to inferred_type
    if inferred_type == "BOOLEAN":
        b_out: list[bool | None] = []
        for v in values:
            if v is None or (isinstance(v, str) and v.strip() == ""):
                b_out.append(None)
                continue
            bv = _parse_bool(v)
            if bv is None:
                malformed_count += 1
            b_out.append(bv)
        series = pl.Series(col_name, b_out, dtype=pl.Boolean)

    elif inferred_type == "INT64":
        i_out: list[int | None] = []
        for v in values:
            if v is None or (isinstance(v, str) and v.strip() == ""):
                i_out.append(None)
                continue
            iv = _parse_int64(v)
            if iv is None:
                malformed_count += 1
            i_out.append(iv)
        series = pl.Series(col_name, i_out, dtype=pl.Int64)

    elif inferred_type == "DECIMAL":
        d_out: list[Decimal | None] = []
        max_prec = 18
        for v in values:
            if v is None or (isinstance(v, str) and v.strip() == ""):
                d_out.append(None)
                continue
            dv = _parse_decimal_with_scale(v)
            if dv is None:
                malformed_count += 1
                d_out.append(None)
            else:
                d_out.append(dv[0])
                if dv[1] > max_prec:
                    max_prec = min(dv[1] + dec_scale, 38)
        series = pl.Series(col_name, d_out, dtype=pl.Decimal(precision=max_prec, scale=dec_scale))

    elif inferred_type == "FLOAT64":
        f_out: list[float | None] = []
        for v in values:
            if (
                v is None
                or (isinstance(v, str) and v.strip() == "")
                or (isinstance(v, float) and math.isnan(v))
            ):
                f_out.append(None)
                continue
            fv = _parse_float64(v)
            if fv is None:
                malformed_count += 1
            f_out.append(fv)
        series = pl.Series(col_name, f_out, dtype=pl.Float64)

    elif inferred_type == "DATE":
        has_dmy = any(
            isinstance(v, str)
            and (m := _DATE_DMY_RE.fullmatch(v.strip())) is not None
            and int(m.group(1)) > 12
            for v in probe_vals
        )
        allow_8 = bool(re.search(r"(?i)(?:date|dt|日期|生日|birth)", col_name))
        dt_out: list[date | None] = []
        for v in values:
            if v is None or (isinstance(v, str) and v.strip() == ""):
                dt_out.append(None)
                continue
            parsed_d = _parse_date(v, allow_dmy=has_dmy, allow_8digit=allow_8)
            if parsed_d is None:
                malformed_count += 1
            dt_out.append(parsed_d)
        series = pl.Series(col_name, dt_out, dtype=pl.Date)

    elif inferred_type == "TIMESTAMP":
        ts_out: list[datetime | None] = []
        for v in values:
            if v is None or (isinstance(v, str) and v.strip() == ""):
                ts_out.append(None)
                continue
            parsed_ts = _parse_timestamp(v, tz)
            if parsed_ts is None:
                malformed_count += 1
            ts_out.append(parsed_ts)
        series = pl.Series(col_name, ts_out, dtype=pl.Datetime("us", "UTC"))

    elif inferred_type == "JSON":
        j_out: list[str | None] = []
        for v in values:
            if v is None or (isinstance(v, str) and v.strip() == ""):
                j_out.append(None)
                continue
            jv = _parse_json_str(v)
            if jv is None:
                malformed_count += 1
            j_out.append(jv)
        series = pl.Series(col_name, j_out, dtype=pl.String)

    else:
        s_out: list[str | None] = []
        for v in values:
            if v is None:
                s_out.append(None)
            elif isinstance(v, str):
                s_out.append(v if v != "" else None)
            elif isinstance(v, float) and math.isnan(v):
                s_out.append(None)
            else:
                s_out.append(_stringify_for_sample(v))
        series = pl.Series(col_name, s_out, dtype=pl.String)

    return _finalize_profile(
        series,
        col_name=col_name,
        ordinal=ordinal,
        data_type=inferred_type,
        pii_cat=pii_cat,
        pii_ratio=pii_ratio,
        str_samples=str_samples,
        source_comment=source_comment,
        malformed_count=malformed_count,
    )


def _finalize_profile(
    series: pl.Series,
    *,
    col_name: str,
    ordinal: int,
    data_type: DataType,
    pii_cat: PiiCategory,
    pii_ratio: float,
    str_samples: list[str],
    source_comment: str,
    malformed_count: int,
) -> tuple[pl.Series, ColumnProfile, int]:
    total = series.len()
    null_cnt = int(series.null_count())
    null_ratio = round(null_cnt / total, 6) if total > 0 else 0.0
    try:
        non_null_s = series.drop_nulls()
        distinct_est = non_null_s.n_unique() if non_null_s.len() > 0 else 0
    except Exception:
        distinct_est = len(set(str_samples))

    seen_samples: list[str] = []
    for raw_s in str_samples:
        if not raw_s:
            continue
        masked = mask_pii_value(raw_s, pii_cat) if pii_cat != "NONE" else raw_s
        if masked not in seen_samples:
            seen_samples.append(masked)
        if len(seen_samples) >= 10:
            break

    profile = ColumnProfile(
        name=col_name,
        ordinal=ordinal,
        data_type=data_type,
        nullable=null_cnt > 0,
        null_count=null_cnt,
        null_ratio=null_ratio,
        distinct_estimate=distinct_est,
        sample_values=seen_samples,
        pii_category=pii_cat,
        pii_hit_ratio=pii_ratio,
        source_comment=source_comment,
    )
    return series, profile, malformed_count


def profile_polars_df(
    df: pl.DataFrame,
    *,
    timezone_str: str = "Asia/Shanghai",
    type_overrides: dict[str, DataType] | None = None,
) -> list[ColumnProfile]:
    """Generate ColumnProfile list directly from an already-typed Polars DataFrame."""
    profiles: list[ColumnProfile] = []
    overrides = type_overrides or {}
    for idx, col in enumerate(df.columns):
        s = df[col]
        dt = overrides.get(col) or _polars_dtype_to_datatype(s.dtype)
        non_null = s.drop_nulls()
        raw_samples = [_stringify_for_sample(v) for v in non_null.head(1000).to_list()]
        pii_cat, pii_ratio = detect_pii_category(col, raw_samples)
        total = s.len()
        null_cnt = int(s.null_count())
        null_ratio = round(null_cnt / total, 6) if total > 0 else 0.0
        try:
            distinct_est = non_null.n_unique() if non_null.len() > 0 else 0
        except Exception:
            distinct_est = len(set(raw_samples))

        seen: list[str] = []
        for rs in raw_samples:
            if not rs:
                continue
            mv = mask_pii_value(rs, pii_cat) if pii_cat != "NONE" else rs
            if mv not in seen:
                seen.append(mv)
            if len(seen) >= 10:
                break

        profiles.append(
            ColumnProfile(
                name=col,
                ordinal=idx,
                data_type=dt,
                nullable=null_cnt > 0,
                null_count=null_cnt,
                null_ratio=null_ratio,
                distinct_estimate=distinct_est,
                sample_values=seen,
                pii_category=pii_cat,
                pii_hit_ratio=pii_ratio,
            )
        )
    return profiles


def _polars_dtype_to_datatype(dtype: pl.DataType) -> DataType:
    if dtype == pl.Boolean:
        return "BOOLEAN"
    if dtype.is_integer():
        return "INT64"
    if isinstance(dtype, pl.Decimal):
        return "DECIMAL"
    if dtype.is_float():
        return "FLOAT64"
    if dtype == pl.Date:
        return "DATE"
    if isinstance(dtype, pl.Datetime):
        return "TIMESTAMP"
    if dtype == pl.Binary:
        return "BINARY"
    if isinstance(dtype, pl.List):
        return "LIST"
    if isinstance(dtype, pl.Struct):
        return "STRUCT"
    return "STRING"
