from __future__ import annotations

import argparse
import hashlib
import io
import os
import random
import re
import shutil
import struct
import subprocess
import tempfile
import zipfile
from datetime import UTC, datetime, timedelta, timezone
from pathlib import Path

import openpyxl
import orjson
from docx import Document as DocxDocument
from docx.enum.style import WD_STYLE_TYPE
from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE
from pptx.util import Inches, Pt
from reportlab import rl_config
from reportlab.lib.pagesizes import A4
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
from reportlab.pdfgen import canvas

SEED = 20260101
TZ_CN = timezone(timedelta(hours=8))
BASE_DT = datetime(2026, 1, 1, 0, 0, 0, tzinfo=TZ_CN)
BASE_ISO_UTC = "2026-01-01T00:00:00Z"
ZIP_FIXED_DT = (2026, 1, 1, 0, 0, 0)
# Set file mtime inside 2026 across both UTC and Asia/Shanghai (for RFC 3164 syslog year inference)
FIXED_MTIME_EPOCH = int(datetime(2026, 1, 1, 8, 0, 0, tzinfo=UTC).timestamp())

PARSER_ROOT = Path(__file__).resolve().parents[2]
FIXTURES_DIR = PARSER_ROOT / "tests" / "fixtures"
TEMPLATES_DIR = PARSER_ROOT / "tests" / "data" / "templates"
MODELS_DIR = PARSER_ROOT / "tests" / "data" / "models"
MANIFEST_PATH = FIXTURES_DIR / "fixtures.sha256"

_CORE_CREATED_RE = re.compile(r"(<dcterms:created\b[^>]*>)[^<]*(</dcterms:created>)")
_CORE_MODIFIED_RE = re.compile(r"(<dcterms:modified\b[^>]*>)[^<]*(</dcterms:modified>)")
_ODF_CREATION_RE = re.compile(r"(<meta:creation-date>)[^<]*(</meta:creation-date>)")
_ODF_DATE_RE = re.compile(r"(<dc:date>)[^<]*(</dc:date>)")
_UUID_ATTR_RE = re.compile(
    r'(\b(?:xr:uid|xr14:uid|a16:creationId\b[^=]*|id)=")\{[0-9A-Fa-f-]{36}\}(")'
)

# Deterministic 5x7 bitmap glyphs for OS/FreeType-independent invoice image rendering
_GLYPHS_5X7: dict[str, tuple[str, ...]] = {
    " ": ("00000", "00000", "00000", "00000", "00000", "00000", "00000"),
    "-": ("00000", "00000", "00000", "11111", "00000", "00000", "00000"),
    ".": ("00000", "00000", "00000", "00000", "00000", "01100", "01100"),
    ":": ("00000", "01100", "01100", "00000", "01100", "01100", "00000"),
    "%": ("11001", "11010", "00100", "01000", "10110", "00110", "00000"),
    "/": ("00001", "00010", "00100", "01000", "10000", "00000", "00000"),
    "0": ("01110", "10001", "10011", "10101", "11001", "10001", "01110"),
    "1": ("00100", "01100", "00100", "00100", "00100", "00100", "01110"),
    "2": ("01110", "10001", "00001", "00110", "01000", "10000", "11111"),
    "3": ("11110", "00001", "00001", "01110", "00001", "00001", "11110"),
    "4": ("00010", "00110", "01010", "10010", "11111", "00010", "00010"),
    "5": ("11111", "10000", "11110", "00001", "00001", "10001", "01110"),
    "6": ("00110", "01000", "10000", "11110", "10001", "10001", "01110"),
    "7": ("11111", "00001", "00010", "00100", "01000", "01000", "01000"),
    "8": ("01110", "10001", "10001", "01110", "10001", "10001", "01110"),
    "9": ("01110", "10001", "10001", "01111", "00001", "00010", "01100"),
    "A": ("01110", "10001", "10001", "11111", "10001", "10001", "10001"),
    "B": ("11110", "10001", "10001", "11110", "10001", "10001", "11110"),
    "C": ("01110", "10001", "10000", "10000", "10000", "10001", "01110"),
    "D": ("11100", "10010", "10001", "10001", "10001", "10010", "11100"),
    "E": ("11111", "10000", "10000", "11110", "10000", "10000", "11111"),
    "F": ("11111", "10000", "10000", "11110", "10000", "10000", "10000"),
    "G": ("01110", "10001", "10000", "10111", "10001", "10001", "01110"),
    "H": ("10001", "10001", "10001", "11111", "10001", "10001", "10001"),
    "I": ("01110", "00100", "00100", "00100", "00100", "00100", "01110"),
    "J": ("00111", "00010", "00010", "00010", "10010", "10010", "01100"),
    "K": ("10001", "10010", "10100", "11000", "10100", "10010", "10001"),
    "L": ("10000", "10000", "10000", "10000", "10000", "10000", "11111"),
    "M": ("10001", "11011", "10101", "10101", "10001", "10001", "10001"),
    "N": ("10001", "11001", "10101", "10011", "10001", "10001", "10001"),
    "O": ("01110", "10001", "10001", "10001", "10001", "10001", "01110"),
    "P": ("11110", "10001", "10001", "11110", "10000", "10000", "10000"),
    "Q": ("01110", "10001", "10001", "10001", "10101", "10010", "01101"),
    "R": ("11110", "10001", "10001", "11110", "10100", "10010", "10001"),
    "S": ("01111", "10000", "10000", "01110", "00001", "00001", "11110"),
    "T": ("11111", "00100", "00100", "00100", "00100", "00100", "00100"),
    "U": ("10001", "10001", "10001", "10001", "10001", "10001", "01110"),
    "V": ("10001", "10001", "10001", "10001", "10001", "01010", "00100"),
    "W": ("10001", "10001", "10001", "10101", "10101", "11011", "10001"),
    "X": ("10001", "10001", "01010", "00100", "01010", "10001", "10001"),
    "Y": ("10001", "10001", "01010", "00100", "00100", "00100", "00100"),
    "Z": ("11111", "00001", "00010", "00100", "01000", "10000", "11111"),
}


def _write_deterministic(path: Path, data: bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    os.utime(path, (FIXED_MTIME_EPOCH, FIXED_MTIME_EPOCH))
    return path


def _canonicalize_zip(raw_bytes: bytes, compress_level: int = 6) -> bytes:
    """Normalize ZIP/OOXML/ODF archive timestamps, permissions, order, and nested archives."""
    with zipfile.ZipFile(io.BytesIO(raw_bytes), "r") as zf_in:
        entries: list[tuple[str, bytes]] = []
        for info in zf_in.infolist():
            if info.is_dir():
                continue
            payload = zf_in.read(info.filename)
            lower_name = info.filename.lower()
            if lower_name.endswith((".xlsx", ".pptx", ".docx", ".ods", ".zip")):
                payload = _canonicalize_zip(payload, compress_level=compress_level)
            elif info.filename in {"docProps/core.xml", "meta.xml"} or lower_name.endswith(".xml"):
                try:
                    xml_text = payload.decode("utf-8")
                    xml_text = _CORE_CREATED_RE.sub(rf"\g<1>{BASE_ISO_UTC}\g<2>", xml_text)
                    xml_text = _CORE_MODIFIED_RE.sub(rf"\g<1>{BASE_ISO_UTC}\g<2>", xml_text)
                    xml_text = _ODF_CREATION_RE.sub(rf"\g<1>{BASE_ISO_UTC}\g<2>", xml_text)
                    xml_text = _ODF_DATE_RE.sub(rf"\g<1>{BASE_ISO_UTC}\g<2>", xml_text)
                    xml_text = _UUID_ATTR_RE.sub(
                        r"\g<1>{00000000-0000-0000-0000-000000000001}\g<2>", xml_text
                    )
                    payload = xml_text.encode("utf-8")
                except UnicodeDecodeError:
                    pass
            entries.append((info.filename, payload))

    entries.sort(key=lambda item: (0 if item[0] == "mimetype" else 1, item[0]))

    out_buf = io.BytesIO()
    with zipfile.ZipFile(out_buf, "w") as zf_out:
        for name, payload in entries:
            zi = zipfile.ZipInfo(filename=name, date_time=ZIP_FIXED_DT)
            zi.create_system = 3
            zi.external_attr = 0o644 << 16
            zi.flag_bits = 0
            if name == "mimetype":
                zi.compress_type = zipfile.ZIP_STORED
                zf_out.writestr(zi, payload, compress_type=zipfile.ZIP_STORED)
            else:
                zi.compress_type = zipfile.ZIP_DEFLATED
                zf_out.writestr(
                    zi,
                    payload,
                    compress_type=zipfile.ZIP_DEFLATED,
                    compresslevel=compress_level,
                )
    return out_buf.getvalue()


def _make_orders_csv_and_gbk(out_dir: Path) -> list[Path]:
    rng = random.Random(SEED + 1)
    surnames = ("张", "王", "李", "赵", "陈", "刘", "周", "吴", "孙", "马")
    given_names = ("伟", "芳", "敏", "静", "磊", "洋", "勇", "艳", "杰", "涛", "明", "华")
    prefixes = ("138", "139", "150", "158", "186", "188")
    statuses = ("PAID", "PENDING", "SHIPPED", "CANCELED")
    warehouses = ("W-1", "W-2", "W-3", "W-4")

    lines = ["order_id,order_date,customer_name,phone,amount,status,warehouse_code"]
    for idx in range(1, 2001):
        order_id = f"ORD-2026-{idx:04d}"
        order_date = (BASE_DT + timedelta(days=rng.randint(0, 89))).strftime("%Y-%m-%d")
        cname = rng.choice(surnames) + rng.choice(given_names)
        phone = rng.choice(prefixes) + f"{rng.randint(0, 99_999_999):08d}"
        amount = f"{rng.randint(500, 950_000) / 100:.2f}"
        status = rng.choice(statuses)
        wh = rng.choice(warehouses)
        lines.append(f"{order_id},{order_date},{cname},{phone},{amount},{status},{wh}")

    content = "\n".join(lines) + "\n"
    p_utf8 = _write_deterministic(out_dir / "orders.csv", content.encode("utf-8"))
    p_gbk = _write_deterministic(out_dir / "orders_gbk.csv", content.encode("gb18030"))
    return [p_utf8, p_gbk]


def _make_sensors_tsv(out_dir: Path) -> Path:
    rng = random.Random(SEED + 2)
    lines = ["ts\tdevice\ttemperature\thumidity"]
    for idx in range(500):
        ts = (BASE_DT + timedelta(minutes=idx)).isoformat()
        dev = f"DEV-{(idx % 10) + 1:02d}"
        temp = f"{18.0 + rng.random() * 14.0:.3f}"
        hum = f"{40.0 + rng.random() * 35.0:.3f}"
        lines.append(f"{ts}\t{dev}\t{temp}\t{hum}")
    return _write_deterministic(out_dir / "sensors.tsv", ("\n".join(lines) + "\n").encode("utf-8"))


def _make_broken_csv(out_dir: Path) -> Path:
    lines = ["id,device_code,note,reading"]
    broken_rows = {5, 11, 17}
    for idx in range(1, 21):
        if idx in broken_rows:
            # Unterminated/invalid quote token confined to a single line
            lines.append(f'{idx},DEV-{idx:02d},"unclosed_quote_{idx}"corrupt,{idx * 1.5:.1f}')
        else:
            lines.append(f"{idx},DEV-{idx:02d},normal_batch_{idx},{idx * 1.5:.1f}")
    return _write_deterministic(out_dir / "broken.csv", ("\n".join(lines) + "\n").encode("utf-8"))


def _make_plain_txt(out_dir: Path) -> Path:
    topics = (
        "华东区域中心仓在春季备货周期内持续优化波次拣选策略，通过动态库位调整缩短了平均拣货路径并提升了整单出库效率。",
        "智能补货引擎每日凌晨汇总各省市分仓的在途库存与安全库存水位，结合历史销量波动向晶圆与组装工厂下发补货建议。",
        "冷链运输车队全部接入温湿度遥测终端，当车厢温度偏离设定区间超过五分钟时自动触发运营预警并记录批次轨迹。",
        "跨部门供应链协同依赖统一的业务节点本体模型，将工厂产能、仓库库存与客户订单在同一张因果拓扑图中实时串联。",
        "季度盘点流程引入条码复核与差异追踪机制，任何账实不符的库位都会生成复盘工单并在驾驶舱看板上高亮显示。",
    )
    chunks: list[str] = []
    for idx in range(30):
        base = topics[idx % len(topics)]
        para = f"第{idx + 1}段，{base}本段记录编号为第{idx + 1}号运营纪要。"
        chunks.append(para)
    # Use triple newline at chapter boundaries (after paragraph 10 and 20)
    parts: list[str] = []
    for idx, para in enumerate(chunks):
        parts.append(para)
        if idx in {9, 19}:
            parts.append("\n\n\n")
        elif idx < 29:
            parts.append("\n\n")
    return _write_deterministic(out_dir / "plain.txt", ("".join(parts) + "\n").encode("utf-8"))


def _make_fixed_width_txt(out_dir: Path) -> Path:
    rng = random.Random(SEED + 3)
    regions = ("EAST", "SOUTH", "NORTH", "WEST")
    statuses = ("ACTIVE", "STANDBY", "BUSY")
    header = (
        f"{'CODE':<10}   "
        f"{'WAREHOUSE':<14}   "
        f"{'REGION':<8}   "
        f"{'CAPACITY':>10}   "
        f"{'STOCK':>10}   "
        f"{'STATUS':<8}"
    )
    lines = [header]
    for idx in range(1, 201):
        code = f"WH-{idx:04d}"
        wh_name = f"NODE_HUB_{idx:03d}"
        region = rng.choice(regions)
        cap = rng.randint(5000, 50000)
        stock = rng.randint(500, cap)
        status = rng.choice(statuses)
        row = f"{code:<10}   {wh_name:<14}   {region:<8}   {cap:>10d}   {stock:>10d}   {status:<8}"
        lines.append(row)
    return _write_deterministic(
        out_dir / "fixed_width.txt", ("\n".join(lines) + "\n").encode("utf-8")
    )


def _make_key_value_txt(out_dir: Path) -> Path:
    regions = ("华东大区", "华南大区", "华北大区", "西南大区", "华中大区")
    blocks: list[str] = []
    # 10 records x 5 key-value lines = 50 key: value lines
    for idx in range(1, 11):
        rec = "\n".join(
            [
                f"node_key: W-{idx:02d}",
                f"warehouse_name: 区域履约中心{idx:02d}号仓",
                f"region: {regions[(idx - 1) % len(regions)]}",
                f"capacity: {10000 + idx * 1500}",
                f"manager: 负责人{idx:02d}",
            ]
        )
        blocks.append(rec)
    return _write_deterministic(
        out_dir / "key_value.txt", ("\n\n".join(blocks) + "\n").encode("utf-8")
    )


def _make_delimited_txt(out_dir: Path) -> Path:
    rng = random.Random(SEED + 4)
    categories = ("RAW_MATERIAL", "SEMI_FINISHED", "FINISHED_GOODS", "PACKAGING")
    lines = ["sku|product_name|category|unit_price|stock_qty"]
    for idx in range(1, 301):
        sku = f"SKU-{idx:04d}"
        pname = f"工业组件_{idx:04d}"
        cat = rng.choice(categories)
        price = f"{rng.randint(100, 50_000) / 100:.2f}"
        qty = rng.randint(10, 5000)
        lines.append(f"{sku}|{pname}|{cat}|{price}|{qty}")
    return _write_deterministic(
        out_dir / "delimited.txt", ("\n".join(lines) + "\n").encode("utf-8")
    )


def _make_supply_chain_xlsx(out_dir: Path) -> Path:
    rng = random.Random(SEED + 5)
    wb = openpyxl.Workbook()

    # Sheet 1: 工厂 (3 rows)
    ws_factory = wb.active
    assert ws_factory is not None
    ws_factory.title = "工厂"
    ws_factory.append(["工厂编码", "工厂名称", "城市", "日产能"])
    ws_factory.append(["F-1", "华东晶圆一厂", "苏州", 1200])
    ws_factory.append(["F-2", "华南封测二厂", "深圳", 950])
    ws_factory.append(["F-3", "华北模组三厂", "天津", 800])

    # Sheet 2: 订单 (2-level header with horizontal merge + vertical merged cells, 40 rows)
    ws_orders = wb.create_sheet("订单")
    ws_orders.append(["归属", "单据", None, "数量", None, "履约"])
    ws_orders.append(["仓库", "订单号", "日期", "计划", "实际", "状态"])
    ws_orders.merge_cells("B1:C1")
    ws_orders.merge_cells("D1:E1")

    statuses = ("PAID", "SHIPPED", "PENDING")
    for idx in range(40):
        excel_row = idx + 3
        wh_group = f"W-{(idx // 10) + 1}"
        wh_cell = wh_group if idx % 10 == 0 else None
        ord_no = f"SO-2026-{idx + 1:03d}"
        ord_date = (BASE_DT + timedelta(days=idx % 28)).strftime("%Y-%m-%d")
        planned = rng.randint(50, 300)
        actual = planned - rng.randint(0, 15)
        st = statuses[idx % len(statuses)]
        ws_orders.append([wh_cell, ord_no, ord_date, planned, actual, st])
        if idx % 10 == 9:
            start_r = excel_row - 9
            ws_orders.merge_cells(
                start_row=start_r, start_column=1, end_row=excel_row, end_column=1
            )

    # Sheet 3: 库存 (2 blocks in 1 sheet separated by 3 empty rows, each with a 合计 summary row)
    ws_inv = wb.create_sheet("库存")
    ws_inv.append(["仓库编码", "物料编码", "物料名称", "在库数量", "安全库存"])
    total_on_hand = 0
    total_safety = 0
    for idx in range(1, 11):
        on_hand = 300 + idx * 45
        safety = 150 + idx * 10
        total_on_hand += on_hand
        total_safety += safety
        ws_inv.append(
            [f"W-{(idx % 2) + 1}", f"MAT-{idx:03d}", f"核心物料{idx:02d}", on_hand, safety]
        )
    ws_inv.append(["合计", "", "", total_on_hand, total_safety])

    # 3 completely empty rows separating block 1 and block 2
    for _ in range(3):
        ws_inv.append([])

    ws_inv.append(["仓库编码", "批次号", "冻结数量", "质检状态"])
    total_frozen = 0
    for idx in range(1, 11):
        frozen = 10 + idx * 4
        total_frozen += frozen
        qc = "合格" if idx % 3 != 0 else "待检"
        ws_inv.append([f"W-{(idx % 2) + 1}", f"LOT-202601-{idx:02d}", frozen, qc])
    ws_inv.append(["合计", "", total_frozen, ""])

    wb.properties.creator = "AxPoint"
    wb.properties.lastModifiedBy = "AxPoint"
    buf = io.BytesIO()
    wb.save(buf)
    return _write_deterministic(out_dir / "supply_chain.xlsx", _canonicalize_zip(buf.getvalue()))


def _make_formulas_uncached_xlsx(out_dir: Path) -> Path:
    wb = openpyxl.Workbook()
    ws = wb.active
    assert ws is not None
    ws.title = "结算单"
    ws.append(["item_code", "qty", "unit_price", "line_total"])
    for idx in range(1, 16):
        r = idx + 1
        ws.append([f"ITEM-{idx:02d}", idx * 5, 12.5 + idx, f"=B{r}*C{r}"])
    wb.properties.creator = "AxPoint"
    wb.properties.lastModifiedBy = "AxPoint"
    buf = io.BytesIO()
    wb.save(buf)
    return _write_deterministic(
        out_dir / "formulas_uncached.xlsx", _canonicalize_zip(buf.getvalue())
    )


def _make_plan_ods(out_dir: Path) -> Path:
    mimetype = b"application/vnd.oasis.opendocument.spreadsheet"
    manifest_xml = (
        b'<?xml version="1.0" encoding="UTF-8"?>\n'
        b'<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"'
        b' manifest:version="1.2">\n'
        b'  <manifest:file-entry manifest:full-path="/" manifest:version="1.2"'
        b' manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>\n'
        b'  <manifest:file-entry manifest:full-path="content.xml"'
        b' manifest:media-type="text/xml"/>\n'
        b'  <manifest:file-entry manifest:full-path="styles.xml"'
        b' manifest:media-type="text/xml"/>\n'
        b'  <manifest:file-entry manifest:full-path="meta.xml"'
        b' manifest:media-type="text/xml"/>\n'
        b"</manifest:manifest>\n"
    )

    meta_xml = f"""<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"
  xmlns:dc="http://purl.org/dc/elements/1.1/"
  office:version="1.2">
  <office:meta>
    <meta:generator>AxPointFixtureGenerator/1.0</meta:generator>
    <meta:creation-date>{BASE_ISO_UTC}</meta:creation-date>
    <dc:date>{BASE_ISO_UTC}</dc:date>
  </office:meta>
</office:document-meta>
""".encode()

    styles_xml = (
        b'<?xml version="1.0" encoding="UTF-8"?>\n'
        b'<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"'
        b' office:version="1.2">\n'
        b"  <office:styles/>\n"
        b"</office:document-styles>\n"
    )

    def _str_cell(val: str) -> str:
        return (
            f'<table:table-cell office:value-type="string">'
            f"<text:p>{val}</text:p></table:table-cell>"
        )

    def _num_cell(val: int) -> str:
        return (
            f'<table:table-cell office:value-type="float" office:value="{val}">'
            f"<text:p>{val}</text:p></table:table-cell>"
        )

    row_xmls: list[str] = [
        "<table:table-row>"
        + "".join(
            _str_cell(col)
            for col in ("plan_id", "factory_code", "plan_date", "planned_qty", "completed_qty")
        )
        + "</table:table-row>"
    ]
    for idx in range(1, 51):
        plan_id = f"PLN-2026-{idx:03d}"
        factory = f"F-{(idx % 3) + 1}"
        pdate = (BASE_DT + timedelta(days=idx - 1)).strftime("%Y-%m-%d")
        planned = 200 + (idx * 15) % 400
        completed = planned - (idx % 12)
        row_xmls.append(
            "<table:table-row>"
            + _str_cell(plan_id)
            + _str_cell(factory)
            + _str_cell(pdate)
            + _num_cell(planned)
            + _num_cell(completed)
            + "</table:table-row>"
        )

    content_xml = (
        """<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
  office:version="1.2">
  <office:body>
    <office:spreadsheet>
      <table:table table:name="生产计划">
        """
        + "\n        ".join(row_xmls)
        + """
      </table:table>
    </office:spreadsheet>
  </office:body>
</office:document-content>
"""
    ).encode()

    raw_buf = io.BytesIO()
    with zipfile.ZipFile(raw_buf, "w") as zf:
        zf.writestr("mimetype", mimetype, compress_type=zipfile.ZIP_STORED)
        zf.writestr("META-INF/manifest.xml", manifest_xml)
        zf.writestr("meta.xml", meta_xml)
        zf.writestr("styles.xml", styles_xml)
        zf.writestr("content.xml", content_xml)

    return _write_deterministic(out_dir / "plan.ods", _canonicalize_zip(raw_buf.getvalue()))


def _inject_smartart_on_slide5(pptx_bytes: bytes) -> bytes:
    """Attach an OOXML SmartArt diagram data part (ppt/diagrams/data1.xml) to slide 5."""
    pt_nodes = "\n".join(
        f'    <dgm:pt modelId="{i}"><dgm:t><a:bodyPr/><a:lstStyle/>'
        f"<a:p><a:r><a:t>{label}</a:t></a:r></a:p></dgm:t></dgm:pt>"
        for i, label in enumerate(("采购入库", "质检上架", "波次拣选", "出库发运"), start=1)
    )
    diagram_data_xml = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"\n'
        '               xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">\n'
        "  <dgm:ptLst>\n"
        '    <dgm:pt modelId="0" type="doc"/>\n'
        f"{pt_nodes}\n"
        "  </dgm:ptLst>\n"
        "  <dgm:cxnLst/>\n"
        "  <dgm:bg/>\n"
        "  <dgm:whole/>\n"
        "</dgm:dataModel>\n"
    ).encode()

    graphic_frame_xml = (
        '<p:graphicFrame xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" '
        'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
        "<p:nvGraphicFramePr>"
        '<p:cNvPr id="99" name="SmartArt Diagram 1"/>'
        "<p:cNvGraphicFramePr/><p:nvPr/>"
        "</p:nvGraphicFramePr>"
        '<p:xfrm><a:off x="914400" y="1828800"/><a:ext cx="6400800" cy="2743200"/></p:xfrm>'
        "<a:graphic>"
        '<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">'
        '<dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" '
        'r:dm="rIdSmartArtData1" r:lo="" r:qs="" r:cs=""/>'
        "</a:graphicData>"
        "</a:graphic>"
        "</p:graphicFrame>"
    )

    out_buf = io.BytesIO()
    with (
        zipfile.ZipFile(io.BytesIO(pptx_bytes), "r") as zf_in,
        zipfile.ZipFile(out_buf, "w") as zf_out,
    ):
        for info in zf_in.infolist():
            data = zf_in.read(info.filename)
            if info.filename == "[Content_Types].xml":
                text = data.decode("utf-8").replace(
                    "</Types>",
                    '<Override PartName="/ppt/diagrams/data1.xml" '
                    'ContentType="application/vnd.openxmlformats-officedocument.drawingml.diagramData+xml"/>'
                    "</Types>",
                )
                data = text.encode("utf-8")
            elif info.filename == "ppt/slides/slide5.xml":
                text = data.decode("utf-8").replace(
                    "</p:spTree>", f"{graphic_frame_xml}</p:spTree>"
                )
                data = text.encode("utf-8")
            elif info.filename == "ppt/slides/_rels/slide5.xml.rels":
                rel_type = (
                    "http://schemas.openxmlformats.org/officeDocument/2006/"
                    "relationships/diagramData"
                )
                text = data.decode("utf-8").replace(
                    "</Relationships>",
                    f'<Relationship Id="rIdSmartArtData1" Type="{rel_type}" '
                    'Target="../diagrams/data1.xml"/></Relationships>',
                )
                data = text.encode("utf-8")
            zf_out.writestr(info.filename, data)
        zf_out.writestr("ppt/diagrams/data1.xml", diagram_data_xml)
    return out_buf.getvalue()


def _make_briefing_pptx(out_dir: Path) -> Path:
    prs = Presentation()
    prs.core_properties.author = "AxPoint"
    prs.core_properties.last_modified_by = "AxPoint"
    prs.core_properties.created = datetime(2026, 1, 1, 0, 0, 0)
    prs.core_properties.modified = datetime(2026, 1, 1, 0, 0, 0)

    title_layout = prs.slide_layouts[0]
    title_only_layout = prs.slide_layouts[5]

    # Slide 1: Overview + speaker notes
    s1 = prs.slides.add_slide(title_layout)
    s1.shapes.title.text = "2026 第一季度供应链运营简报"
    s1.placeholders[1].text = "xBlueNode / AxPoint 数字化履约中心"
    s1.notes_slide.notes_text_frame.text = (
        "备注：本季度华东仓履约率提升至 98.5%，库存周转天数下降 2.3 天。"
    )

    # Slide 2: Table + speaker notes
    s2 = prs.slides.add_slide(title_only_layout)
    s2.shapes.title.text = "各区域中心仓核心指标"
    tbl_shape = s2.shapes.add_table(5, 4, Inches(0.8), Inches(1.8), Inches(8.2), Inches(3.2))
    tbl = tbl_shape.table
    rows = [
        ("仓库编码", "区域", "在库库存", "准时履约率"),
        ("W-1", "华东", "12500", "98.5%"),
        ("W-2", "华南", "9800", "97.2%"),
        ("W-3", "华北", "8400", "96.8%"),
        ("W-4", "西南", "6100", "95.9%"),
    ]
    for r_idx, row_vals in enumerate(rows):
        for c_idx, val in enumerate(row_vals):
            tbl.cell(r_idx, c_idx).text = val
    s2.notes_slide.notes_text_frame.text = "备注：W-1 华东仓吞吐量居首，建议优先扩容自动化分拣线。"

    # Slide 3: Bar chart
    s3 = prs.slides.add_slide(title_only_layout)
    s3.shapes.title.text = "计划与实际出库对比"
    chart_data = CategoryChartData()  # type: ignore[no-untyped-call]
    chart_data.categories = ["W-1", "W-2", "W-3", "W-4"]
    chart_data.add_series("计划出库", (1200, 950, 820, 600))  # type: ignore[no-untyped-call]
    chart_data.add_series("实际出库", (1185, 930, 805, 590))  # type: ignore[no-untyped-call]
    s3.shapes.add_chart(
        XL_CHART_TYPE.COLUMN_CLUSTERED,
        Inches(1.0),
        Inches(1.8),
        Inches(8.0),
        Inches(4.5),
        chart_data,
    )

    # Slide 4: Group shape
    s4 = prs.slides.add_slide(title_only_layout)
    s4.shapes.title.text = "仓配协同架构节点"
    grp = s4.shapes.add_group_shape()
    box_a = grp.shapes.add_textbox(Inches(1.0), Inches(2.0), Inches(3.5), Inches(1.2))
    box_a.text_frame.text = "组合节点 A：华东区域仓"
    box_b = grp.shapes.add_textbox(Inches(5.0), Inches(2.0), Inches(3.5), Inches(1.2))
    box_b.text_frame.text = "组合节点 B：智能分拣线"

    # Slide 5: SmartArt diagram
    s5 = prs.slides.add_slide(title_only_layout)
    s5.shapes.title.text = "标准作业闭环流程"

    buf = io.BytesIO()
    prs.save(buf)
    with_smartart = _inject_smartart_on_slide5(buf.getvalue())
    return _write_deterministic(out_dir / "briefing.pptx", _canonicalize_zip(with_smartart))


def _make_contract_docx(out_dir: Path) -> Path:
    doc = DocxDocument()
    doc.core_properties.author = "AxPoint"
    doc.core_properties.last_modified_by = "AxPoint"
    doc.core_properties.created = datetime(2026, 1, 1, 0, 0, 0)
    doc.core_properties.modified = datetime(2026, 1, 1, 0, 0, 0)

    doc.add_heading("供应链仓储与干线履约服务合同", level=1)
    doc.add_paragraph(
        "本合同由委托方与承运仓储方于 2026 年 1 月 1 日共同签署，约定多仓协同履约条款。"
    )

    doc.add_heading("第一章 服务范围与仓库节点", level=2)
    doc.add_paragraph("承运仓储方负责 W-1 华东仓、W-2 华南仓的入库质检、恒温存储与波次出库。")

    doc.add_heading("1.1 计费标准与 SLA 阶梯附表", level=3)
    doc.add_paragraph("以下表格规定区域计费单价及嵌套的 SLA 奖惩阶梯标准：")

    # Table 1 (>= 3 rows): horizontal header merge + vertical data merge + nested table
    t1 = doc.add_table(rows=5, cols=4)
    t1.cell(0, 0).text = "区域"
    t1.cell(0, 1).text = "单价明细"
    t1.cell(0, 1).merge(t1.cell(0, 2))
    t1.cell(0, 3).text = "备注与附表"

    t1.cell(1, 0).text = "大区"
    t1.cell(1, 1).text = "仓储费"
    t1.cell(1, 2).text = "操作费"
    t1.cell(1, 3).text = "结算说明"

    t1.cell(2, 0).text = "华东"
    t1.cell(2, 1).text = "35.00"
    t1.cell(2, 2).text = "4.50"
    t1.cell(2, 3).text = "月结三十天"

    t1.cell(3, 0).text = "华东"
    t1.cell(2, 0).merge(t1.cell(3, 0))
    t1.cell(3, 1).text = "32.00"
    t1.cell(3, 2).text = "4.20"
    t1.cell(3, 3).text = "季度返利适用"

    t1.cell(4, 0).text = "华南"
    t1.cell(4, 1).text = "33.50"
    t1.cell(4, 2).text = "4.30"
    nested_host = t1.cell(4, 3)
    nested_host.text = "详见 SLA 附表"
    nested = nested_host.add_table(rows=4, cols=3)
    nested_rows = [
        ("等级", "准时率下限", "系数"),
        ("S", "99.0%", "1.05"),
        ("A", "97.5%", "1.00"),
        ("B", "95.0%", "0.95"),
    ]
    for r_idx, r_vals in enumerate(nested_rows):
        for c_idx, val in enumerate(r_vals):
            nested.cell(r_idx, c_idx).text = val

    doc.add_paragraph("双方签章信息如下（两行小表，不作为业务数据表入湖）：")

    # Table 2 (2-row small layout table -> < 3 rows, excluded from table track)
    t_small = doc.add_table(rows=2, cols=2)
    t_small.cell(0, 0).text = "委托方签章"
    t_small.cell(0, 1).text = "承运仓储方签章"
    t_small.cell(1, 0).text = "华东智造供应链有限公司"
    t_small.cell(1, 1).text = "蓝点数字物流科技有限公司"

    buf = io.BytesIO()
    doc.save(buf)
    return _write_deterministic(out_dir / "contract.docx", _canonicalize_zip(buf.getvalue()))


def _make_annual_report_pdf(out_dir: Path) -> Path:
    prev_invariant = rl_config.invariant
    rl_config.invariant = 1
    try:
        pdfmetrics.registerFont(UnicodeCIDFont("STSong-Light"))
        buf = io.BytesIO()
        c = canvas.Canvas(buf, pagesize=A4, invariant=1)
        c.setTitle("2025-2026 年度供应链运营报告")
        c.setAuthor("AxPoint")
        c.setSubject("Annual Supply Chain Report")
        c.setCreator("AxPoint Fixture Generator")

        chapters = [
            (
                "第一章 年度供应链运营综述",
                [
                    (
                        "报告期内，全网四个核心履约中心累计处理订单二十四万笔，"
                        "整体准时履约率达到 97.6%。"
                    ),
                    (
                        "通过超图知识库与 OLAP 双擎融合分析，库存呆滞金额同比下降 18.4%，"
                        "应急调拨响应缩短至两小时内。"
                    ),
                ],
            ),
            (
                "第二章 仓储网络与履约效率",
                [
                    (
                        "W-1 华东仓与 W-2 华南仓完成自动化分拣改造，"
                        "单仓日均峰值吞吐能力突破一万五千件。"
                    ),
                    "跨仓调拨策略依托写时复制因果沙盘进行预演，有效规避了节假日波峰期间的爆仓风险。",
                ],
            ),
            (
                "第三章 风险管控与智能补货展望",
                [
                    "下一年度将继续推进供应商产能实时接入，建立多级安全库存动态水位线与断供预警闭环。",
                    "全部核心指标均纳入统一脱敏与审计治理框架，实现端到端可追溯运营决策。",
                ],
            ),
        ]

        width, height = A4
        for title, paras in chapters:
            c.setFont("STSong-Light", 18)
            c.drawString(60, height - 80, title)
            c.setFont("STSong-Light", 11)
            y = height - 130
            for para in paras:
                c.drawString(60, y, para[:42])
                y -= 22
                if len(para) > 42:
                    c.drawString(60, y, para[42:])
                    y -= 28
            c.showPage()

        # Page 4: Table page (4 columns x 9 rows including header)
        c.setFont("STSong-Light", 16)
        c.drawString(60, height - 80, "附表：核心仓库年度运营汇总表")

        table_rows = [
            ("仓库编码", "所属区域", "年度吞吐量", "准时履约率"),
            ("W-1", "华东", "152000", "98.5%"),
            ("W-2", "华南", "118400", "97.2%"),
            ("W-3", "华北", "96300", "96.8%"),
            ("W-4", "西南", "74500", "95.9%"),
            ("W-5", "华中", "68200", "96.4%"),
            ("W-6", "西北", "41900", "95.1%"),
        ]
        col_x = [60, 170, 290, 420, 535]
        top_y = height - 120
        row_h = 28

        c.setLineWidth(0.8)
        for r_idx, row in enumerate(table_rows):
            y_top = top_y - r_idx * row_h
            y_bot = y_top - row_h
            c.line(col_x[0], y_top, col_x[-1], y_top)
            c.setFont("STSong-Light", 11)
            for c_idx, val in enumerate(row):
                c.drawString(col_x[c_idx] + 8, y_bot + 9, val)
        bottom_y = top_y - len(table_rows) * row_h
        c.line(col_x[0], bottom_y, col_x[-1], bottom_y)
        for x in col_x:
            c.line(x, top_y, x, bottom_y)

        c.showPage()
        c.save()
        return _write_deterministic(out_dir / "annual_report.pdf", buf.getvalue())
    finally:
        rl_config.invariant = prev_invariant


def _draw_bitmap_text(
    draw: ImageDraw.ImageDraw, x: int, y: int, text: str, scale: int = 6, color: int = 16
) -> None:
    cursor_x = x
    for ch in text.upper():
        rows = _GLYPHS_5X7.get(ch, _GLYPHS_5X7[" "])
        for r_idx, row_bits in enumerate(rows):
            for c_idx, bit in enumerate(row_bits):
                if bit == "1":
                    px0 = cursor_x + c_idx * scale
                    py0 = y + r_idx * scale
                    draw.rectangle([px0, py0, px0 + scale - 1, py0 + scale - 1], fill=color)
        cursor_x += 6 * scale


def _make_scanned_invoice_png(out_dir: Path) -> Path:
    # 1654 x 2339 grayscale image (A4 @ 200 DPI), deterministic pixel rendering
    img = Image.new("L", (1654, 2339), color=248)
    draw = ImageDraw.Draw(img)

    # Outer border & header divider
    draw.rectangle([80, 80, 1574, 2259], outline=40, width=4)
    _draw_bitmap_text(draw, 260, 150, "AXPOINT TAX INVOICE", scale=9, color=15)
    draw.line([120, 255, 1534, 255], fill=40, width=3)

    meta_lines = [
        "INVOICE NO: INV-2026-0001",
        "ISSUE DATE: 2026-01-01",
        "BUYER: EAST WAREHOUSE W-1",
        "SELLER: AXPOINT SUPPLY CO.",
    ]
    for idx, text in enumerate(meta_lines):
        _draw_bitmap_text(draw, 140, 310 + idx * 78, text, scale=6, color=20)

    # Items table
    table_top = 680
    row_h = 90
    col_x = [120, 520, 780, 1140, 1534]
    table_rows = [
        ("ITEM", "QTY", "PRICE", "AMOUNT"),
        ("SKU-001", "100", "58.00", "5800.00"),
        ("SKU-002", "80", "46.00", "3680.00"),
        ("SKU-003", "50", "62.00", "3100.00"),
    ]
    for r_idx, row in enumerate(table_rows):
        y0 = table_top + r_idx * row_h
        draw.line([col_x[0], y0, col_x[-1], y0], fill=50, width=2)
        for c_idx, cell in enumerate(row):
            _draw_bitmap_text(draw, col_x[c_idx] + 24, y0 + 24, cell, scale=5, color=20)
    bottom_y = table_top + len(table_rows) * row_h
    draw.line([col_x[0], bottom_y, col_x[-1], bottom_y], fill=50, width=2)
    for x in col_x:
        draw.line([x, table_top, x, bottom_y], fill=50, width=2)

    totals = [
        "SUBTOTAL: 12580.00",
        "TAX 13%: 1635.40",
        "TOTAL: 14215.40",
    ]
    for idx, text in enumerate(totals):
        _draw_bitmap_text(draw, 820, 1140 + idx * 80, text, scale=6, color=15)

    buf = io.BytesIO()
    img.save(buf, format="PNG", compress_level=9, optimize=False)
    return _write_deterministic(out_dir / "scanned_invoice.png", buf.getvalue())


def _make_api_orders_json(out_dir: Path) -> Path:
    rng = random.Random(SEED + 6)
    tiers = ("VIP", "ENTERPRISE", "STANDARD")
    warehouses = ("W-1", "W-2", "W-3")
    items: list[dict[str, object]] = []

    for idx in range(1, 201):
        line_count = (idx % 3) + 1
        order_lines: list[dict[str, object]] = []
        for l_idx in range(line_count):
            qty = rng.randint(2, 50)
            price = f"{rng.randint(1500, 80000) / 100:.2f}"
            order_lines.append(
                {
                    "line_no": l_idx + 1,
                    "sku": f"SKU-{(idx + l_idx) % 40 + 1:03d}",
                    "qty": qty,
                    "unit_price": price,
                }
            )
        items.append(
            {
                "order_id": f"API-ORD-{idx:04d}",
                "order_date": (BASE_DT + timedelta(days=idx % 60)).strftime("%Y-%m-%d"),
                "warehouse_code": warehouses[idx % len(warehouses)],
                "status": "PAID" if idx % 4 != 0 else "PENDING",
                "customer": {
                    "id": f"CUST-{(idx % 50) + 1:03d}",
                    "name": f"客户企业_{(idx % 50) + 1:03d}",
                    "tier": tiers[idx % len(tiers)],
                },
                "lines": order_lines,
            }
        )

    doc = {
        "status": "ok",
        "generated_at": BASE_DT.isoformat(),
        "data": {"items": items},
    }
    raw = orjson.dumps(doc, option=orjson.OPT_INDENT_2 | orjson.OPT_SORT_KEYS) + b"\n"
    return _write_deterministic(out_dir / "api_orders.json", raw)


def _make_events_jsonl(out_dir: Path) -> Path:
    rng = random.Random(SEED + 7)
    event_types = ("INVENTORY_SNAPSHOT", "ORDER_CREATED", "SHIPMENT_DEPARTED", "TEMP_READING")
    out_lines: list[bytes] = []
    for idx in range(5000):
        ts = (BASE_DT + timedelta(seconds=idx * 15)).isoformat()
        rec: dict[str, object] = {
            "event_id": f"EVT-{idx + 1:05d}",
            "ts": ts,
            "event_type": event_types[idx % len(event_types)],
            "node_key": f"W-{(idx % 4) + 1}",
            "value": round(100.0 + rng.random() * 900.0, 2),
        }
        # Simulate schema drift across batches
        if idx >= 1500:
            rec["region"] = ("EAST", "SOUTH", "NORTH", "WEST")[idx % 4]
        if idx >= 3200:
            rec["schema_version"] = 2
            rec["latency_ms"] = rng.randint(3, 120)
        out_lines.append(orjson.dumps(rec, option=orjson.OPT_SORT_KEYS))
    return _write_deterministic(out_dir / "events.jsonl", b"\n".join(out_lines) + b"\n")


def _make_catalog_xml(out_dir: Path) -> Path:
    categories = ("fastener", "sensor", "controller", "packaging")
    currencies = ("CNY", "USD")
    item_blocks: list[str] = []
    for idx in range(1, 101):
        cat = categories[idx % len(categories)]
        curr = currencies[0] if idx % 5 != 0 else currencies[1]
        amt = f"{25.0 + idx * 3.75:.2f}"
        stock = 100 + (idx * 17) % 900
        item_blocks.append(
            f'  <item id="ITEM-{idx:04d}" category="{cat}" warehouse="W-{(idx % 3) + 1}">\n'
            f"    <name>工业备件_{idx:04d}</name>\n"
            f"    <stock>{stock}</stock>\n"
            f'    <price currency="{curr}"><amount>{amt}</amount></price>\n'
            f"  </item>"
        )
    xml = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<catalog xmlns="urn:axpoint:catalog:v1" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" '
        'xsi:schemaLocation="urn:axpoint:catalog:v1 catalog.xsd">\n'
        + "\n".join(item_blocks)
        + "\n</catalog>\n"
    )
    return _write_deterministic(out_dir / "catalog.xml", xml.encode("utf-8"))


def _make_xxe_xml(out_dir: Path) -> Path:
    xml = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        "<!DOCTYPE payload [\n"
        '  <!ENTITY xxe SYSTEM "file:///etc/passwd">\n'
        "]>\n"
        "<payload>\n"
        "  <record>&xxe;</record>\n"
        "</payload>\n"
    )
    return _write_deterministic(out_dir / "xxe.xml", xml.encode("utf-8"))


def _make_billion_laughs_xml(out_dir: Path) -> Path:
    entities = ['  <!ENTITY lol0 "lol">']
    for level in range(1, 10):
        prev = f"&lol{level - 1};" * 10
        entities.append(f'  <!ENTITY lol{level} "{prev}">')
    xml = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        "<!DOCTYPE lolz [\n" + "\n".join(entities) + "\n]>\n<lolz>&lol9;</lolz>\n"
    )
    return _write_deterministic(out_dir / "billion_laughs.xml", xml.encode("utf-8"))


def _make_zip_bomb_xlsx(out_dir: Path) -> Path:
    """Create an OOXML archive whose worksheet entry exceeds compression ratio > 100."""
    content_types = (
        b'<?xml version="1.0" encoding="UTF-8"?>'
        b'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        b'<Default Extension="xml" ContentType="application/xml"/>'
        b'<Override PartName="/xl/worksheets/sheet1.xml" '
        b'ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        b"</Types>"
    )
    # ~2.4 MB of highly repetitive XML compresses to ~4 KB (ratio > 500 > 100)
    repeated_rows = b"<row><c><v>0</v></c></row>" * 90_000
    huge_sheet = (
        b'<?xml version="1.0" encoding="UTF-8"?><worksheet><sheetData>'
        + repeated_rows
        + b"</sheetData></worksheet>"
    )
    raw_buf = io.BytesIO()
    with zipfile.ZipFile(raw_buf, "w") as zf:
        zf.writestr("[Content_Types].xml", content_types)
        zf.writestr("xl/worksheets/sheet1.xml", huge_sheet)
    return _write_deterministic(
        out_dir / "zip_bomb.xlsx", _canonicalize_zip(raw_buf.getvalue(), compress_level=9)
    )


def _make_nginx_access_log(out_dir: Path) -> Path:
    rng = random.Random(SEED + 8)
    paths = ("/api/health", "/api/xblue/nodes", "/api/analytics/sql/run", "/api/axops/jobs")
    methods = ("GET", "POST", "GET", "GET")
    statuses = (200, 200, 200, 201, 400, 401, 500)
    lines: list[str] = []
    for idx in range(1000):
        dt = BASE_DT + timedelta(seconds=idx * 3)
        ts_str = dt.strftime("%d/%b/%Y:%H:%M:%S %z")
        ip = f"10.20.{(idx // 250) + 1}.{(idx % 250) + 1}"
        m_idx = idx % len(paths)
        status = statuses[rng.randint(0, len(statuses) - 1)]
        size = rng.randint(128, 16384)
        lines.append(
            f'{ip} - - [{ts_str}] "{methods[m_idx]} {paths[m_idx]} HTTP/1.1" '
            f'{status} {size} "-" "AxPointClient/1.0"'
        )
    return _write_deterministic(
        out_dir / "nginx_access.log", ("\n".join(lines) + "\n").encode("utf-8")
    )


def _make_app_java_log(out_dir: Path) -> Path:
    error_indices = {15, 55, 95}
    lines: list[str] = []
    for idx in range(120):
        dt = BASE_DT + timedelta(seconds=idx * 5)
        ts_str = dt.strftime("%Y-%m-%d %H:%M:%S") + f",{(idx * 37) % 1000:03d}"
        thread = f"worker-{(idx % 4) + 1}"
        if idx in error_indices:
            lines.append(
                f"{ts_str} ERROR [{thread}] com.axpoint.sync.BatchConnector - "
                f"Failed to flush staging batch seq={idx}"
            )
            lines.append("java.io.IOException: Upstream socket reset during batch commit")
            lines.append("\tat com.axpoint.sync.BatchConnector.flush(BatchConnector.java:142)")
            lines.append("\tat com.axpoint.sync.SyncWorker.runStep(SyncWorker.java:88)")
            lines.append("Caused by: java.net.SocketException: Connection reset")
            lines.append("\t... 4 more")
        elif idx % 10 == 0:
            lines.append(
                f"{ts_str} WARN  [{thread}] com.axpoint.sync.RateLimiter - "
                f"Backpressure watermark high queue_depth={40 + idx}"
            )
        else:
            lines.append(
                f"{ts_str} INFO  [{thread}] com.axpoint.sync.BatchConnector - "
                f"Committed batch seq={idx} rows={200 + (idx % 50)}"
            )
    return _write_deterministic(out_dir / "app_java.log", ("\n".join(lines) + "\n").encode("utf-8"))


def _make_syslog(out_dir: Path) -> Path:
    lines: list[str] = []
    for idx in range(300):
        dt = BASE_DT + timedelta(seconds=idx * 10)
        # RFC 3164: "Jan  1 00:00:00" (space-padded day, no year)
        mon = dt.strftime("%b")
        day = f"{dt.day:2d}"
        hms = dt.strftime("%H:%M:%S")
        host = f"edge-gw-{(idx % 3) + 1:02d}"
        pid = 1000 + (idx % 8)
        lines.append(
            f"<34>{mon} {day} {hms} {host} axhub-agent[{pid}]: "
            f"Telemetry frame seq={idx + 1} warehouse=W-{(idx % 4) + 1} status=ok"
        )
    return _write_deterministic(out_dir / "syslog.log", ("\n".join(lines) + "\n").encode("utf-8"))


def _make_k8s_log(out_dir: Path) -> Path:
    out_lines: list[bytes] = []
    for idx in range(300):
        dt_utc = (BASE_DT + timedelta(seconds=idx * 4)).astimezone(UTC)
        ts_z = dt_utc.strftime("%Y-%m-%dT%H:%M:%S.000000000Z")
        level = "ERROR" if idx % 47 == 0 else ("WARN" if idx % 13 == 0 else "INFO")
        msg = f"processed job_seq={idx + 1} tenant=default latency_ms={(idx * 7) % 95 + 5}"
        entry = {
            "level": level,
            "log": f"{ts_z} {level} [axpoint-core] {msg}\n",
            "message": msg,
            "service": "axpoint-core",
            "stream": "stderr" if level == "ERROR" else "stdout",
            "time": ts_z,
            "trace_id": f"{idx + 1:032x}",
        }
        out_lines.append(orjson.dumps(entry, option=orjson.OPT_SORT_KEYS))
    return _write_deterministic(out_dir / "k8s.log", b"\n".join(out_lines) + b"\n")


def make_templates(templates_dir: Path = TEMPLATES_DIR) -> list[Path]:
    """Generate Step 12 DOCX and PPTX export templates deterministically."""
    # 1. report_template.docx
    doc = DocxDocument()
    doc.core_properties.author = "AxPoint"
    doc.core_properties.last_modified_by = "AxPoint"
    doc.core_properties.created = datetime(2026, 1, 1, 0, 0, 0)
    doc.core_properties.modified = datetime(2026, 1, 1, 0, 0, 0)

    ax_table_style = doc.styles.add_style("AxTable", WD_STYLE_TYPE.TABLE)
    ax_table_style.base_style = doc.styles["Table Grid"]

    section = doc.sections[0]
    header_para = section.header.paragraphs[0]
    header_para.text = "{{title}}"

    doc.add_heading("{{title}}", level=1)
    doc.add_paragraph("{{subtitle}}")
    doc.add_paragraph("生成时间：{{generated_at}}")
    doc.add_paragraph("{{sections}}")

    docx_buf = io.BytesIO()
    doc.save(docx_buf)
    p_docx = _write_deterministic(
        templates_dir / "report_template.docx", _canonicalize_zip(docx_buf.getvalue())
    )

    # 2. deck_template.pptx
    prs = Presentation()
    prs.core_properties.author = "AxPoint"
    prs.core_properties.last_modified_by = "AxPoint"
    prs.core_properties.created = datetime(2026, 1, 1, 0, 0, 0)
    prs.core_properties.modified = datetime(2026, 1, 1, 0, 0, 0)

    prs.slide_layouts[0].name = "标题"
    prs.slide_layouts[1].name = "标题和内容"

    cover = prs.slides.add_slide(prs.slide_layouts[0])
    cover.shapes.title.text = "{{title}}"
    cover.placeholders[1].text = "{{subtitle}}"
    for shape in cover.shapes:
        if shape.has_text_frame:
            for paragraph in shape.text_frame.paragraphs:
                for run in paragraph.runs:
                    run.font.size = Pt(24)

    pptx_buf = io.BytesIO()
    prs.save(pptx_buf)
    p_pptx = _write_deterministic(
        templates_dir / "deck_template.pptx", _canonicalize_zip(pptx_buf.getvalue())
    )
    return [p_docx, p_pptx]


def make_models_3d(models_dir: Path = MODELS_DIR) -> list[Path]:
    """Generate minimal valid glTF 2.0 binary (.glb) model containing mesh node 'Rack_A'."""
    # Unit cube [-0.5, 0.5]^3 with 24 vertices (6 faces x 4 vertices) + normals + 36 indices
    faces: list[tuple[tuple[float, float, float], list[tuple[float, float, float]]]] = [
        ((0.0, 0.0, 1.0), [(-0.5, -0.5, 0.5), (0.5, -0.5, 0.5), (0.5, 0.5, 0.5), (-0.5, 0.5, 0.5)]),
        (
            (0.0, 0.0, -1.0),
            [(0.5, -0.5, -0.5), (-0.5, -0.5, -0.5), (-0.5, 0.5, -0.5), (0.5, 0.5, -0.5)],
        ),
        ((1.0, 0.0, 0.0), [(0.5, -0.5, 0.5), (0.5, -0.5, -0.5), (0.5, 0.5, -0.5), (0.5, 0.5, 0.5)]),
        (
            (-1.0, 0.0, 0.0),
            [(-0.5, -0.5, -0.5), (-0.5, -0.5, 0.5), (-0.5, 0.5, 0.5), (-0.5, 0.5, -0.5)],
        ),
        ((0.0, 1.0, 0.0), [(-0.5, 0.5, 0.5), (0.5, 0.5, 0.5), (0.5, 0.5, -0.5), (-0.5, 0.5, -0.5)]),
        (
            (0.0, -1.0, 0.0),
            [(-0.5, -0.5, -0.5), (0.5, -0.5, -0.5), (0.5, -0.5, 0.5), (-0.5, -0.5, 0.5)],
        ),
    ]

    pos_bytes = bytearray()
    norm_bytes = bytearray()
    idx_bytes = bytearray()
    for face_idx, (normal, verts) in enumerate(faces):
        base = face_idx * 4
        for vx, vy, vz in verts:
            pos_bytes.extend(struct.pack("<3f", vx, vy, vz))
            norm_bytes.extend(struct.pack("<3f", normal[0], normal[1], normal[2]))
        idx_bytes.extend(struct.pack("<6H", base, base + 1, base + 2, base, base + 2, base + 3))

    pos_len = len(pos_bytes)  # 288 bytes
    norm_len = len(norm_bytes)  # 288 bytes
    idx_len = len(idx_bytes)  # 72 bytes
    bin_raw = bytes(pos_bytes) + bytes(norm_bytes) + bytes(idx_bytes)
    while len(bin_raw) % 4 != 0:
        bin_raw += b"\x00"

    gltf_json = {
        "asset": {"version": "2.0", "generator": "AxPoint Fixture Generator"},
        "scene": 0,
        "scenes": [{"name": "WarehouseScene", "nodes": [0]}],
        "nodes": [{"name": "Rack_A", "mesh": 0}],
        "meshes": [
            {
                "name": "Rack_A",
                "primitives": [
                    {
                        "attributes": {"POSITION": 0, "NORMAL": 1},
                        "indices": 2,
                        "material": 0,
                    }
                ],
            }
        ],
        "materials": [
            {
                "name": "RackMaterial",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.145, 0.388, 0.922, 1.0],
                    "metallicFactor": 0.2,
                    "roughnessFactor": 0.7,
                },
            }
        ],
        "buffers": [{"byteLength": len(bin_raw)}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": pos_len, "target": 34962},
            {"buffer": 0, "byteOffset": pos_len, "byteLength": norm_len, "target": 34962},
            {
                "buffer": 0,
                "byteOffset": pos_len + norm_len,
                "byteLength": idx_len,
                "target": 34963,
            },
        ],
        "accessors": [
            {
                "bufferView": 0,
                "componentType": 5126,
                "count": 24,
                "type": "VEC3",
                "min": [-0.5, -0.5, -0.5],
                "max": [0.5, 0.5, 0.5],
            },
            {
                "bufferView": 1,
                "componentType": 5126,
                "count": 24,
                "type": "VEC3",
                "min": [-1.0, -1.0, -1.0],
                "max": [1.0, 1.0, 1.0],
            },
            {
                "bufferView": 2,
                "componentType": 5123,
                "count": 36,
                "type": "SCALAR",
                "min": [0],
                "max": [23],
            },
        ],
    }

    json_chunk = orjson.dumps(gltf_json, option=orjson.OPT_SORT_KEYS)
    while len(json_chunk) % 4 != 0:
        json_chunk += b" "

    total_length = 12 + 8 + len(json_chunk) + 8 + len(bin_raw)
    glb = bytearray()
    glb.extend(struct.pack("<III", 0x46546C67, 2, total_length))
    glb.extend(struct.pack("<II", len(json_chunk), 0x4E4F534A))
    glb.extend(json_chunk)
    glb.extend(struct.pack("<II", len(bin_raw), 0x004E4942))
    glb.extend(bin_raw)

    p_glb = _write_deterministic(models_dir / "warehouse_twin.glb", bytes(glb))
    return [p_glb]


def _convert_legacy_office_if_available(out_dir: Path) -> None:
    """Convert legacy .xls/.ppt/.doc/.xlsb via LibreOffice if soffice is installed.

    Per A5 §1, LibreOffice output varies across soffice versions and is excluded from
    fixtures.sha256 byte-for-byte verification.
    """
    soffice = shutil.which("soffice")
    if not soffice:
        return

    conversions: list[tuple[Path, str, str]] = [
        (out_dir / "supply_chain.xlsx", "xls", "early_format.xls"),
        (out_dir / "briefing.pptx", "ppt", "early_format.ppt"),
        (out_dir / "contract.docx", "doc", "early_format.doc"),
    ]

    with tempfile.TemporaryDirectory() as tmp_str:
        tmp_dir = Path(tmp_str)
        # Also prepare a 100-row workbook for report.xlsb if supported by local soffice
        wb_xlsb = openpyxl.Workbook()
        ws = wb_xlsb.active
        assert ws is not None
        ws.title = "Report"
        ws.append(["row_id", "warehouse", "metric_val"])
        for idx in range(1, 101):
            ws.append([idx, f"W-{(idx % 4) + 1}", idx * 12.5])
        src_xlsb = tmp_dir / "report_src.xlsx"
        wb_xlsb.save(src_xlsb)
        conversions.append((src_xlsb, "xlsb", "report.xlsb"))

        profile_uri = (tmp_dir / "lo_profile").as_uri()
        for src_path, target_ext, final_name in conversions:
            conv_out = tmp_dir / f"out_{final_name}"
            conv_out.mkdir(parents=True, exist_ok=True)
            proc = subprocess.run(
                [
                    soffice,
                    f"-env:UserInstallation={profile_uri}",
                    "--headless",
                    "--convert-to",
                    target_ext,
                    "--outdir",
                    str(conv_out),
                    str(src_path),
                ],
                capture_output=True,
                timeout=60,
                check=False,
            )
            produced = conv_out / f"{src_path.stem}.{target_ext}"
            if proc.returncode == 0 and produced.is_file():
                shutil.copyfile(produced, out_dir / final_name)


def generate_all(root_dir: Path = PARSER_ROOT, convert_legacy: bool = True) -> list[Path]:
    """Generate all deterministic fixtures, export templates, and 3D models under root_dir."""
    fixtures_dir = root_dir / "tests" / "fixtures"
    templates_dir = root_dir / "tests" / "data" / "templates"
    models_dir = root_dir / "tests" / "data" / "models"

    generated: list[Path] = []
    generated.extend(_make_orders_csv_and_gbk(fixtures_dir))
    generated.append(_make_sensors_tsv(fixtures_dir))
    generated.append(_make_broken_csv(fixtures_dir))
    generated.append(_make_plain_txt(fixtures_dir))
    generated.append(_make_fixed_width_txt(fixtures_dir))
    generated.append(_make_key_value_txt(fixtures_dir))
    generated.append(_make_delimited_txt(fixtures_dir))
    generated.append(_make_supply_chain_xlsx(fixtures_dir))
    generated.append(_make_formulas_uncached_xlsx(fixtures_dir))
    generated.append(_make_plan_ods(fixtures_dir))
    generated.append(_make_briefing_pptx(fixtures_dir))
    generated.append(_make_contract_docx(fixtures_dir))
    generated.append(_make_annual_report_pdf(fixtures_dir))
    generated.append(_make_scanned_invoice_png(fixtures_dir))
    generated.append(_make_api_orders_json(fixtures_dir))
    generated.append(_make_events_jsonl(fixtures_dir))
    generated.append(_make_catalog_xml(fixtures_dir))
    generated.append(_make_xxe_xml(fixtures_dir))
    generated.append(_make_billion_laughs_xml(fixtures_dir))
    generated.append(_make_zip_bomb_xlsx(fixtures_dir))
    generated.append(_make_nginx_access_log(fixtures_dir))
    generated.append(_make_app_java_log(fixtures_dir))
    generated.append(_make_syslog(fixtures_dir))
    generated.append(_make_k8s_log(fixtures_dir))
    generated.extend(make_templates(templates_dir))
    generated.extend(make_models_3d(models_dir))

    if convert_legacy:
        _convert_legacy_office_if_available(fixtures_dir)

    generated.sort(key=lambda p: p.relative_to(root_dir).as_posix())
    return generated


def compute_sha256_map(paths: list[Path], root_dir: Path = PARSER_ROOT) -> dict[str, str]:
    """Compute relative-POSIX-path -> hex SHA-256 digest map."""
    digests: dict[str, str] = {}
    for path in sorted(paths, key=lambda p: p.relative_to(root_dir).as_posix()):
        rel = path.relative_to(root_dir).as_posix()
        digests[rel] = hashlib.sha256(path.read_bytes()).hexdigest()
    return digests


def write_manifest(manifest_path: Path = MANIFEST_PATH, root_dir: Path = PARSER_ROOT) -> None:
    """Generate fixtures and write GNU sha256sum-compatible manifest relative to axpoint-parser."""
    paths = generate_all(root_dir=root_dir)
    digests = compute_sha256_map(paths, root_dir=root_dir)
    lines = [f"{sha}  {rel}" for rel, sha in sorted(digests.items())]
    manifest_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate deterministic AxPoint test fixtures.")
    parser.add_argument(
        "--write-manifest",
        action="store_true",
        help="Update tests/fixtures/fixtures.sha256 after generating files.",
    )
    args = parser.parse_args()
    if args.write_manifest:
        write_manifest()
    else:
        generate_all()


if __name__ == "__main__":
    main()
