from __future__ import annotations

import io
import os
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient
from PIL import Image

from charts import (
    ChartRendererPool,
    get_chart_pool,
    render_chart_bytes,
    shutdown_chart_pool,
)
from main import app
from schemas import RenderChartRequest
from security import SidecarError

TEST_TOKEN = "step12-chart-internal-token-0123456789abcdef"
PNG_MAGIC = bytes.fromhex("89504e470d0a1a0a")


@pytest.fixture(autouse=True)
def _configure_env(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    monkeypatch.setenv("AXPOINT_CHART_WORKERS", "2")


def _sample_bar_option() -> dict[str, Any]:
    return {
        "title": {"text": "供应概览"},
        "xAxis": {"type": "category", "data": ["甲", "乙", "华东仓"]},
        "yAxis": {"type": "value"},
        "series": [{"name": "订单量", "type": "bar", "data": [120, 260, 180]}],
    }


def _sample_line_option() -> dict[str, Any]:
    return {
        "title": {"text": "仓库库存趋势"},
        "xAxis": {"type": "category", "data": ["周一", "周二", "周三", "周四"]},
        "yAxis": {"type": "value"},
        "series": [{"name": "库存", "type": "line", "data": [500, 460, 520, 610]}],
    }


def _sample_pie_option() -> dict[str, Any]:
    return {
        "title": {"text": "订单状态占比"},
        "series": [
            {
                "type": "pie",
                "radius": "60%",
                "data": [
                    {"name": "已支付", "value": 820},
                    {"name": "待履约", "value": 180},
                ],
            }
        ],
    }


def test_render_cjk_bar_line_pie_svg_and_hidpi_png() -> None:
    for opt in (_sample_bar_option(), _sample_line_option(), _sample_pie_option()):
        svg_bytes, svg_mime = render_chart_bytes(
            RenderChartRequest(option=opt, width=600, height=400, format="svg")
        )
        assert svg_mime == "image/svg+xml"
        svg_text = svg_bytes.decode("utf-8")
        assert "<svg" in svg_text
        assert "Noto Sans CJK" in svg_text

        png_bytes, png_mime = render_chart_bytes(
            RenderChartRequest(option=opt, width=600, height=400, format="png", theme="dark")
        )
        assert png_mime == "image/png"
        assert png_bytes[:8] == PNG_MAGIC

        with Image.open(io.BytesIO(png_bytes)) as im:
            assert im.format == "PNG"
            assert im.size == (1200, 800)


def test_render_bar_svg_preserves_chinese_labels() -> None:
    svg_bytes, _ = render_chart_bytes(
        RenderChartRequest(option=_sample_bar_option(), width=640, height=360, format="svg")
    )
    svg_text = svg_bytes.decode("utf-8")
    assert "供应概览" in svg_text
    assert "甲" in svg_text
    assert "乙" in svg_text


def test_rejects_executable_or_unsafe_chart_options() -> None:
    bad_options: list[dict[str, Any]] = [
        {
            "xAxis": {"type": "category", "data": ["甲"]},
            "yAxis": {},
            "tooltip": {"formatter": "function (params) { return params.value; }"},
            "series": [{"type": "bar", "data": [1]}],
        },
        {
            "xAxis": {"type": "category", "data": ["甲"]},
            "yAxis": {},
            "tooltip": {"formatter": {"not": "a_string"}},
            "series": [{"type": "bar", "data": [1]}],
        },
        {
            "title": {"text": "(x) => { return x; }"},
            "series": [],
        },
        {
            "title": {"text": "<script>alert(1)</script>"},
            "series": [],
        },
    ]
    for bad_opt in bad_options:
        with pytest.raises(SidecarError) as exc_info:
            render_chart_bytes(RenderChartRequest(option=bad_opt, width=400, height=300))
        assert exc_info.value.status_code == 400
        assert exc_info.value.error_code == "INVALID_ARGUMENT"


def test_concurrent_worker_pool_renders() -> None:
    def _job(idx: int) -> bytes:
        opt = {
            "title": {"text": f"并发图表-{idx}"},
            "xAxis": {"type": "category", "data": ["甲", "乙"]},
            "yAxis": {"type": "value"},
            "series": [{"type": "bar", "data": [idx + 1, idx + 10]}],
        }
        data, mime = render_chart_bytes(
            RenderChartRequest(option=opt, width=480, height=320, format="png")
        )
        assert mime == "image/png"
        return data

    with ThreadPoolExecutor(max_workers=6) as ex:
        outputs = list(ex.map(_job, range(8)))

    assert len(outputs) == 8
    for out in outputs:
        assert out[:8] == PNG_MAGIC


def test_hung_worker_timeout_kills_process_and_recovers() -> None:
    # 1. Deterministic hung worker script that hangs on request 1 and replies if respawned
    hung_then_alive_code = (
        "import json, os, sys, time\n"
        "marker = sys.argv[1]\n"
        "for line in sys.stdin:\n"
        "    req = json.loads(line)\n"
        "    if not os.path.exists(marker):\n"
        "        open(marker, 'w').write(str(os.getpid()))\n"
        "        time.sleep(30)\n"
        "    out = {'id': req['id'], 'ok': True, 'data_b64': 'iVBORw0KGgo=', 'mime': 'image/png'}\n"
        "    sys.stdout.write(json.dumps(out) + '\\n')\n"
        "    sys.stdout.flush()\n"
    )
    marker_file = Path(os.environ["AXPOINT_DATA_ROOT"]) / "hung_once.marker"
    pool = ChartRendererPool(
        num_workers=1,
        worker_cmd=[sys.executable, "-u", "-c", hung_then_alive_code, str(marker_file)],
    )
    try:
        initial_pids = pool.active_pids()
        assert len(initial_pids) == 1

        req = RenderChartRequest(option=_sample_bar_option(), width=400, height=300, format="png")
        with pytest.raises(SidecarError) as exc_info:
            pool.render(req, timeout_s=0.15)
        assert exc_info.value.status_code == 504
        assert exc_info.value.error_code == "UPSTREAM_TIMEOUT"

        # Worker must have been killed and replaced with a fresh PID automatically
        recovered_pids = pool.active_pids()
        assert len(recovered_pids) == 1
        assert recovered_pids[0] != initial_pids[0]

        # Very next render succeeds immediately on the replacement worker
        raw_bytes, mime = pool.render(req, timeout_s=5.0)
        assert mime == "image/png"
        assert raw_bytes == b"\x89PNG\r\n\x1a\n"
    finally:
        pool.shutdown()

    # 2. Also verify real Node.js singleton pool timeout + automatic recovery
    shutdown_chart_pool()
    real_pool = get_chart_pool()
    heavy_req = RenderChartRequest(
        option=_sample_bar_option(), width=3200, height=2400, format="png"
    )
    _ = real_pool.active_pids()
    with pytest.raises(SidecarError) as real_exc:
        render_chart_bytes(heavy_req, timeout_s=0.0005)
    assert real_exc.value.status_code == 504
    assert real_exc.value.error_code == "UPSTREAM_TIMEOUT"

    # Next call on real Node pool recovers cleanly
    recovered_png, recovered_mime = render_chart_bytes(
        RenderChartRequest(option=_sample_bar_option(), width=600, height=400, format="png"),
        timeout_s=15.0,
    )
    assert recovered_mime == "image/png"
    assert recovered_png[:8] == PNG_MAGIC


def test_render_chart_http_endpoint() -> None:
    headers = {"X-AxPoint-Internal-Token": TEST_TOKEN}
    with TestClient(app) as client:
        unauth = client.post(
            "/render/chart",
            json={"option": _sample_bar_option(), "width": 600, "height": 400, "format": "png"},
        )
        assert unauth.status_code == 401
        assert unauth.json()["error_code"] == "TOKEN_INVALID"

        res_png = client.post(
            "/render/chart",
            headers=headers,
            json={
                "option": {
                    "xAxis": {"type": "category", "data": ["甲", "乙"]},
                    "yAxis": {},
                    "series": [{"type": "bar", "data": [1, 2]}],
                },
                "width": 600,
                "height": 400,
                "format": "png",
            },
        )
        assert res_png.status_code == 200
        assert res_png.headers["content-type"].startswith("image/png")
        assert res_png.content[:8] == PNG_MAGIC

        res_svg = client.post(
            "/render/chart",
            headers=headers,
            json={
                "option": _sample_bar_option(),
                "width": 600,
                "height": 400,
                "format": "svg",
            },
        )
        assert res_svg.status_code == 200
        assert res_svg.headers["content-type"].startswith("image/svg+xml")
        assert "供应概览" in res_svg.text

        bad_dim = client.post(
            "/render/chart",
            headers=headers,
            json={"option": _sample_bar_option(), "width": 16, "height": 400, "format": "png"},
        )
        assert bad_dim.status_code == 400
        assert bad_dim.json()["error_code"] == "INVALID_ARGUMENT"
