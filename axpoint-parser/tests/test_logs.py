from __future__ import annotations

from pathlib import Path

from parsers.engine import parse_unified

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_nginx_java_syslog_and_k8s_logs_with_drain3_templates(tmp_path: Path) -> None:
    nginx = parse_unified(
        "nginx_access.log", (FIXTURES_DIR / "nginx_access.log").read_bytes(), data_root=tmp_path
    )
    assert len(nginx.tables) == 2
    n_main = nginx.tables[0]
    assert n_main.row_count == 1000
    n_cols = {c.name: c.data_type for c in n_main.columns}
    assert n_cols["ts"] == "TIMESTAMP"
    assert n_cols["status"] == "INT64"
    assert nginx.tables[1].table_name == "nginx_access__templates"
    assert nginx.tables[1].row_count >= 1

    java = parse_unified(
        "app_java.log", (FIXTURES_DIR / "app_java.log").read_bytes(), data_root=tmp_path
    )
    assert java.tables[0].row_count == 120
    assert java.tables[1].table_name == "app_java__templates"
    assert "java.io.IOException" in java.markdown
    assert "SocketException" in java.markdown

    sys_res = parse_unified(
        "syslog.log", (FIXTURES_DIR / "syslog.log").read_bytes(), data_root=tmp_path
    )
    assert sys_res.tables[0].row_count == 300

    k8s = parse_unified("k8s.log", (FIXTURES_DIR / "k8s.log").read_bytes(), data_root=tmp_path)
    assert k8s.tables[0].row_count == 300


def test_custom_regex_log_format(tmp_path: Path) -> None:
    sample = (
        b"2026-03-01T10:00:00Z|WARN|billing|invoice #101 delayed\n"
        b"2026-03-01T10:01:00Z|ERROR|billing|invoice #102 failed\n"
    )
    pat = r"regex:^(?P<timestamp>[^|]+)\|(?P<level>[^|]+)\|(?P<service>[^|]+)\|(?P<message>.*)$"
    res = parse_unified("billing.log", sample, options={"log_format": pat}, data_root=tmp_path)
    assert res.tables[0].row_count == 2
    assert res.tables[0].origin == "log_format=regex"
