from __future__ import annotations

import re
from collections import Counter
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

import orjson
import polars as pl
from drain3 import TemplateMiner
from drain3.template_miner_config import TemplateMinerConfig

from parsers.base import ParseContext, ParseResult
from parsers.blocks import normalize_table_name, render_markdown_table
from parsers.encoding import detect_and_decode
from parsers.staging import write_polars_to_staging

_MULTILINE_HEAD_RE = re.compile(
    r"^(?:"
    r"\d{4}-\d{2}-\d{2}"
    r"|\[\d{4}-"
    r"|(?:<\d{1,3}>)?[A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2}"
    r"|<\d{1,3}>1\s+\d{4}-\d{2}-\d{2}"
    r"|\{"
    r"|(?:[0-9a-fA-F:.]+|\d{1,3}(?:\.\d{1,3}){3})\s+\S+\s+\S+\s+\[\d{2}/[A-Za-z]{3}/\d{4}:"
    r")"
)

_NGINX_RE = re.compile(
    r"^(?P<client_ip>\S+)\s+\S+\s+\S+\s+\[(?P<timestamp>[^\]]+)\]\s+"
    r'"(?P<method>[A-Z]+)\s+(?P<path>\S+)\s+[^"]*"\s+'
    r"(?P<status>\d{3})\s+(?P<bytes>\d+|-)"
    r'(?:\s+"(?P<referer>[^"]*)"\s+"(?P<user_agent>[^"]*)"'
    r"(?:\s+(?P<latency_ms>\d+(?:\.\d+)?))?)?"
)

_SYSLOG_3164_RE = re.compile(
    r"^(?:<(?P<pri>\d{1,3})>)?(?P<timestamp>[A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})\s+"
    r"(?P<host>\S+)\s+(?P<service>[^:\[\s]+)(?:\[\d+\])?:\s*(?P<message>.*)$",
    re.DOTALL,
)

_SYSLOG_5424_RE = re.compile(
    r"^(?:<(?P<pri>\d{1,3})>)?1\s+(?P<timestamp>\S+)\s+(?P<host>\S+)\s+"
    r"(?P<service>\S+)\s+\S+\s+\S+\s+(?:\[.*?\]|-)\s*(?P<message>.*)$",
    re.DOTALL,
)

_LOG4J_RE = re.compile(
    r"^\[?(?P<timestamp>\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}"
    r"(?:[.,]\d{1,9})?(?:Z|[+-]\d{2}:?\d{2})?)\]?\s+"
    r"(?P<level>TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL)\s+"
    r"(?:\[(?P<thread>[^\]]+)\]\s+)?(?P<service>\S+)\s+-\s+(?P<message>.*)$",
    re.DOTALL,
)

_K8S_CRI_RE = re.compile(
    r"^(?P<timestamp>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2}))\s+"
    r"(?P<stream>stdout|stderr)\s+[FP]\s+(?P<message>.*)$",
    re.DOTALL,
)

_MONTHS = {
    "Jan": 1,
    "Feb": 2,
    "Mar": 3,
    "Apr": 4,
    "May": 5,
    "Jun": 6,
    "Jul": 7,
    "Aug": 8,
    "Sep": 9,
    "Oct": 10,
    "Nov": 11,
    "Dec": 12,
}


def _syslog_pri_to_level(pri_str: str | None) -> str:
    if not pri_str or not pri_str.isdigit():
        return "INFO"
    sev = int(pri_str) & 0x07
    if sev <= 2:
        return "FATAL"
    if sev == 3:
        return "ERROR"
    if sev == 4:
        return "WARN"
    if sev <= 6:
        return "INFO"
    return "DEBUG"


def _resolve_tz(tz_name: str) -> ZoneInfo:
    try:
        return ZoneInfo(tz_name)
    except (ZoneInfoNotFoundError, KeyError, ValueError):
        return ZoneInfo("Asia/Shanghai")


def _parse_any_log_ts(raw_ts: str, default_year: int, tz: ZoneInfo) -> datetime | None:
    s = raw_ts.strip()
    if not s:
        return None

    # 1. Nginx / Apache: 01/Jan/2026:00:00:00 +0800
    m_clf = re.match(r"^(\d{2})/([A-Za-z]{3})/(\d{4}):(\d{2}):(\d{2}):(\d{2})\s*([+-]\d{4}|Z)?$", s)
    if m_clf:
        day = int(m_clf.group(1))
        mon = _MONTHS.get(m_clf.group(2), 1)
        year = int(m_clf.group(3))
        hh, mm, ss = int(m_clf.group(4)), int(m_clf.group(5)), int(m_clf.group(6))
        tz_s = m_clf.group(7)
        if not tz_s or tz_s == "Z":
            return datetime(year, mon, day, hh, mm, ss, tzinfo=UTC)
        iso_tz = f"{tz_s[:3]}:{tz_s[3:]}"
        return datetime.fromisoformat(
            f"{year:04d}-{mon:02d}-{day:02d}T{hh:02d}:{mm:02d}:{ss:02d}{iso_tz}"
        ).astimezone(UTC)

    # 2. RFC 3164 without year: Jan  1 00:00:00
    m_3164 = re.match(r"^([A-Za-z]{3})\s+(\d{1,2})\s+(\d{2}):(\d{2}):(\d{2})$", s)
    if m_3164:
        mon = _MONTHS.get(m_3164.group(1), 1)
        day = int(m_3164.group(2))
        hh, mm, ss = int(m_3164.group(3)), int(m_3164.group(4)), int(m_3164.group(5))
        return datetime(default_year, mon, day, hh, mm, ss, tzinfo=tz).astimezone(UTC)

    # 3. ISO 8601 / log4j: 2026-01-01 00:00:00,000 or 2025-12-31T16:00:00.000000000Z
    m_iso = re.match(
        r"^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2}):(\d{2})(?:[.,](\d+))?(Z|[+-]\d{2}:?\d{2})?$",
        s,
    )
    if m_iso:
        year, mon, day = int(m_iso.group(1)), int(m_iso.group(2)), int(m_iso.group(3))
        hh, mm, ss = int(m_iso.group(4)), int(m_iso.group(5)), int(m_iso.group(6))
        frac = m_iso.group(7) or ""
        us = int(frac[:6].ljust(6, "0")) if frac else 0
        tz_part = m_iso.group(8)
        if tz_part is None:
            return datetime(year, mon, day, hh, mm, ss, us, tzinfo=tz).astimezone(UTC)
        if tz_part == "Z":
            return datetime(year, mon, day, hh, mm, ss, us, tzinfo=UTC)
        tz_norm = tz_part if ":" in tz_part else f"{tz_part[:3]}:{tz_part[3:]}"
        return datetime.fromisoformat(
            f"{year:04d}-{mon:02d}-{day:02d}T{hh:02d}:{mm:02d}:{ss:02d}.{us:06d}{tz_norm}"
        ).astimezone(UTC)

    return None


def _merge_multiline_entries(lines: list[str]) -> list[str]:
    entries: list[str] = []
    for raw in lines:
        line = raw.rstrip("\r\n")
        if not line.strip() and not entries:
            continue
        if not line.strip():
            continue
        if _MULTILINE_HEAD_RE.match(line) or not entries:
            entries.append(line)
        else:
            entries[-1] = f"{entries[-1]}\n{line}"
    return entries


def _detect_log_format(sample_entries: list[str]) -> str:
    sample = sample_entries[:200]
    if not sample:
        return "log4j_default"

    candidates: list[tuple[str, Any]] = [
        ("nginx_combined", lambda s: _NGINX_RE.match(s) is not None),
        ("syslog_rfc5424", lambda s: _SYSLOG_5424_RE.match(s) is not None),
        ("syslog_rfc3164", lambda s: _SYSLOG_3164_RE.match(s) is not None),
        ("k8s_cri", lambda s: _K8S_CRI_RE.match(s) is not None),
        ("log4j_default", lambda s: _LOG4J_RE.match(s) is not None),
        ("jsonl", lambda s: s.lstrip().startswith("{") and s.rstrip().endswith("}")),
    ]
    best_fmt = "log4j_default"
    best_ratio = 0.0
    for name, matcher in candidates:
        hits = sum(1 for entry in sample if matcher(entry))
        ratio = hits / len(sample)
        if ratio > best_ratio:
            best_ratio = ratio
            best_fmt = name
    return best_fmt if best_ratio >= 0.80 else best_fmt


def _build_drain3_miner() -> TemplateMiner:
    cfg = TemplateMinerConfig()
    cfg.drain_sim_th = 0.4
    cfg.drain_depth = 4
    cfg.profiling_enabled = False
    return TemplateMiner(config=cfg)


class LogParser:
    """Structured log parser with multiline stack merging and Drain3 clustering (Step 01 §01.11)."""

    suffixes: tuple[str, ...] = (".log",)

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        stem = Path(ctx.filename).stem or "logs"
        text, encoding, used_fallback = detect_and_decode(data)
        if used_fallback:
            ctx.add_warning(
                "ENCODING_FALLBACK",
                f"decoded {ctx.filename} using fallback encoding {encoding}",
            )

        raw_lines = text.splitlines()
        entries = _merge_multiline_entries(raw_lines)

        requested_fmt = ctx.options.get("log_format", "auto").strip()
        custom_re: re.Pattern[str] | None = None
        if requested_fmt.lower().startswith("regex:"):
            pattern_str = requested_fmt[6:]
            custom_re = re.compile(pattern_str, re.DOTALL)
            detected_fmt = "regex"
        elif requested_fmt.lower() == "auto" or not requested_fmt:
            detected_fmt = _detect_log_format(entries)
        else:
            detected_fmt = requested_fmt.lower()

        tz = _resolve_tz(ctx.timezone)
        mtime_year_raw = ctx.options.get("mtime_year", "")
        default_year = int(mtime_year_raw) if mtime_year_raw.isdigit() else datetime.now(UTC).year

        is_http_access = detected_fmt in {"nginx_combined", "apache_combined"}
        miner = _build_drain3_miner()

        ts_col: list[datetime] = []
        level_col: list[str] = []
        service_col: list[str | None] = []
        host_col: list[str | None] = []
        trace_col: list[str | None] = []
        tpl_id_col: list[int] = []
        msg_col: list[str] = []
        raw_col: list[str] = []

        # Extra HTTP columns for nginx/apache
        ip_col: list[str | None] = []
        method_col: list[str | None] = []
        path_col: list[str | None] = []
        status_col: list[int | None] = []
        bytes_col: list[int | None] = []
        ua_col: list[str | None] = []
        lat_col: list[float | None] = []

        last_ts = datetime(default_year, 1, 1, 0, 0, 0, tzinfo=UTC)
        ts_inferred_count = 0

        for entry in entries:
            parsed_ts: datetime | None = None
            lvl = "INFO"
            svc: str | None = None
            hst: str | None = None
            trc: str | None = None
            msg = entry

            if custom_re is not None:
                mc = custom_re.match(entry)
                if mc:
                    gd = mc.groupdict()
                    parsed_ts = _parse_any_log_ts(str(gd.get("timestamp") or ""), default_year, tz)
                    lvl = str(gd.get("level") or "INFO").upper()
                    svc = gd.get("service")
                    hst = gd.get("host")
                    trc = gd.get("trace_id")
                    msg = str(gd.get("message") or entry)

            elif is_http_access:
                mn = _NGINX_RE.match(entry)
                if mn:
                    gd = mn.groupdict()
                    parsed_ts = _parse_any_log_ts(str(gd.get("timestamp") or ""), default_year, tz)
                    st_code = int(gd["status"]) if gd.get("status") else 200
                    lvl = "ERROR" if st_code >= 500 else ("WARN" if st_code >= 400 else "INFO")
                    svc = "nginx" if detected_fmt == "nginx_combined" else "apache"
                    msg = f"{gd.get('method', '')} {gd.get('path', '')} -> {st_code}"
                    ip_col.append(gd.get("client_ip"))
                    method_col.append(gd.get("method"))
                    path_col.append(gd.get("path"))
                    status_col.append(st_code)
                    b_raw = gd.get("bytes")
                    bytes_col.append(int(b_raw) if b_raw and b_raw.isdigit() else 0)
                    ua_col.append(gd.get("user_agent"))
                    l_raw = gd.get("latency_ms")
                    lat_col.append(float(l_raw) if l_raw else None)
                else:
                    ip_col.append(None)
                    method_col.append(None)
                    path_col.append(None)
                    status_col.append(None)
                    bytes_col.append(None)
                    ua_col.append(None)
                    lat_col.append(None)

            elif detected_fmt == "jsonl":
                try:
                    jobj = orjson.loads(entry)
                except orjson.JSONDecodeError:
                    jobj = {}
                if isinstance(jobj, dict):
                    raw_t = str(jobj.get("ts") or jobj.get("time") or jobj.get("timestamp") or "")
                    parsed_ts = _parse_any_log_ts(raw_t, default_year, tz)
                    lvl = str(jobj.get("level") or "INFO").upper()
                    svc = str(jobj["service"]) if jobj.get("service") else None
                    hst = str(jobj["host"]) if jobj.get("host") else None
                    trc = str(jobj["trace_id"]) if jobj.get("trace_id") else None
                    msg = str(jobj.get("message") or jobj.get("log") or entry).rstrip("\r\n")

            elif detected_fmt == "k8s_cri":
                mk = _K8S_CRI_RE.match(entry)
                if mk:
                    gd = mk.groupdict()
                    parsed_ts = _parse_any_log_ts(str(gd.get("timestamp") or ""), default_year, tz)
                    lvl = "ERROR" if gd.get("stream") == "stderr" else "INFO"
                    msg = str(gd.get("message") or "")

            elif detected_fmt == "syslog_rfc5424":
                m5 = _SYSLOG_5424_RE.match(entry)
                if m5:
                    gd = m5.groupdict()
                    parsed_ts = _parse_any_log_ts(str(gd.get("timestamp") or ""), default_year, tz)
                    lvl = _syslog_pri_to_level(gd.get("pri"))
                    hst = gd.get("host")
                    svc = gd.get("service")
                    msg = str(gd.get("message") or "")

            elif detected_fmt == "syslog_rfc3164":
                m3 = _SYSLOG_3164_RE.match(entry)
                if m3:
                    gd = m3.groupdict()
                    parsed_ts = _parse_any_log_ts(str(gd.get("timestamp") or ""), default_year, tz)
                    lvl = _syslog_pri_to_level(gd.get("pri"))
                    hst = gd.get("host")
                    svc = gd.get("service")
                    msg = str(gd.get("message") or "")

            else:
                ml = _LOG4J_RE.match(entry)
                if ml:
                    gd = ml.groupdict()
                    parsed_ts = _parse_any_log_ts(str(gd.get("timestamp") or ""), default_year, tz)
                    lvl = str(gd.get("level") or "INFO").strip().upper()
                    if lvl == "WARNING":
                        lvl = "WARN"
                    svc = gd.get("service")
                    msg = str(gd.get("message") or "")

            if parsed_ts is None:
                parsed_ts = last_ts
                ts_inferred_count += 1
            else:
                last_ts = parsed_ts

            first_msg_line = msg.splitlines()[0] if msg else ""
            mine_res = miner.add_log_message(first_msg_line)
            cluster_id = int(mine_res["cluster_id"])

            ts_col.append(parsed_ts)
            level_col.append(lvl)
            service_col.append(svc)
            host_col.append(hst)
            trace_col.append(trc)
            tpl_id_col.append(cluster_id)
            msg_col.append(msg)
            raw_col.append(entry)

        if ts_inferred_count > 0:
            ctx.add_warning(
                "TS_INFERRED",
                f"inferred {ts_inferred_count} missing log timestamps from preceding entries",
            )

        series_list: list[pl.Series] = [
            pl.Series("ts", ts_col, dtype=pl.Datetime("us", "UTC")),
            pl.Series("level", level_col, dtype=pl.String),
            pl.Series("service", service_col, dtype=pl.String),
            pl.Series("host", host_col, dtype=pl.String),
            pl.Series("trace_id", trace_col, dtype=pl.String),
            pl.Series("template_id", tpl_id_col, dtype=pl.Int64),
            pl.Series("message", msg_col, dtype=pl.String),
            pl.Series("raw", raw_col, dtype=pl.String),
        ]
        if is_http_access:
            series_list.extend(
                [
                    pl.Series("client_ip", ip_col, dtype=pl.String),
                    pl.Series("method", method_col, dtype=pl.String),
                    pl.Series("path", path_col, dtype=pl.String),
                    pl.Series("status", status_col, dtype=pl.Int64),
                    pl.Series("bytes", bytes_col, dtype=pl.Int64),
                    pl.Series("user_agent", ua_col, dtype=pl.String),
                    pl.Series("latency_ms", lat_col, dtype=pl.Float64),
                ]
            )

        main_df = pl.DataFrame(series_list)
        base_norm = normalize_table_name(stem)
        main_tbl = write_polars_to_staging(
            ctx,
            base_norm,
            main_df,
            origin=f"log_format={detected_fmt}",
        )

        # Build Drain3 template summary table <table>__templates(template_id, template, count)
        clusters = sorted(
            miner.drain.clusters,
            key=lambda c: (-int(c.size), int(c.cluster_id)),
        )
        tpl_df = pl.DataFrame(
            [
                pl.Series("template_id", [int(c.cluster_id) for c in clusters], dtype=pl.Int64),
                pl.Series("template", [str(c.get_template()) for c in clusters], dtype=pl.String),
                pl.Series("count", [int(c.size) for c in clusters], dtype=pl.Int64),
            ]
        )
        tpl_tbl = write_polars_to_staging(
            ctx,
            f"{base_norm}__templates",
            tpl_df,
            origin=f"log_format={detected_fmt};templates=drain3",
        )

        # Build Markdown summary track
        lvl_counts = Counter(level_col)
        min_ts_s = min(ts_col).isoformat() if ts_col else "N/A"
        max_ts_s = max(ts_col).isoformat() if ts_col else "N/A"
        lvl_summary = ", ".join(f"{k}: {v}" for k, v in sorted(lvl_counts.items()))

        tpl_md_rows: list[list[Any]] = [["template_id", "count", "template"]]
        for c in clusters[:20]:
            tpl_md_rows.append([int(c.cluster_id), int(c.size), str(c.get_template())])

        err_blocks: list[str] = []
        for t_v, l_v, m_v in zip(ts_col, level_col, msg_col, strict=False):
            if l_v in {"ERROR", "FATAL"}:
                err_blocks.append(
                    f"### [{l_v}] {t_v.isoformat() if t_v else ''}\n\n```text\n{m_v}\n```"
                )
                if len(err_blocks) >= 50:
                    break

        md_parts = [
            f"# {stem}",
            f"- **识别格式**：`{detected_fmt}`\n"
            f"- **记录总数**：{len(entries)}\n"
            f"- **时间范围**：`{min_ts_s}` ～ `{max_ts_s}`\n"
            f"- **级别分布**：{lvl_summary}",
            "## Top 日志模板\n\n" + render_markdown_table(tpl_md_rows, max_rows=20),
        ]
        if err_blocks:
            md_parts.append("## ERROR / FATAL 详情\n\n" + "\n\n".join(err_blocks))

        return ParseResult(
            title=stem,
            source_format="log",
            engine="drain3-log",
            markdown="\n\n".join(md_parts),
            tables=[main_tbl, tpl_tbl],
        )
