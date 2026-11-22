# 02 · Step 01：AxParser 双轨统一解析（01.1～01.11）

AxParser 把任意上传文件解析为**双轨载荷**：

- **文本轨**：结构化 Markdown（标题层级、段落、表格、备注、OCR 文本），供 Core 写入 `hg_docs / hg_paras / hg_entities` 做五路召回。
- **表格轨**：一个或多个强类型 Parquet 暂存文件 + 列画像（类型、空值率、去重估计、样例、PII 类别），供 Step 02 提交到 DuckLake 并注册为数据集。

Step 01 只交付 Sidecar 能力与测试；Core 侧“文件 → 数据集”入库接口在 Step 02（§02.6）交付。

---

## 01.0 目标、前置与契约

- **目标**：11 个子步骤全部通过；每种格式都有确定性样本、期望快照、`pytest -k` 命令与 `curl` 验收。
- **前置**：Step 00 通过（内部令牌中间件、样本生成器、门禁框架可用）。
- **契约**：`schemas.py` 中的 `UnifiedParseResponse`、`ExtractedTable`、`ColumnProfile`、`ParseWarning`；`POST /parse/unified`、`POST /parse` 写入 `docs/API_CONTRACT.md` 的 Sidecar 章节。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 修改 | `axpoint-parser/schemas.py` | 双轨契约模型 |
| 修改 | `axpoint-parser/main.py` | `/parse/unified`、`/parse` 路由 |
| 修改 | `axpoint-parser/settings.py` | 追加 `axpoint_parse_timeout_s: int = 300`、`axpoint_pdf_max_pages: int = 2000`（A1 §10） |
| 新建 | `axpoint-parser/security.py`（追加） | 上传上限、压缩包检查、路径约束、XML 安全解析器、SSRF 地址校验（Step 02 复用） |
| 新建 | `axpoint-parser/parsers/base.py` | `BaseParser` 协议、`ParseContext`、`ParseResult` |
| 新建 | `axpoint-parser/parsers/registry.py` | 后缀 + MIME + 魔数路由 |
| 新建 | `axpoint-parser/parsers/encoding.py` | 编码探测 |
| 新建 | `axpoint-parser/parsers/blocks.py` | 表格区块识别（Excel / CSV / Word 表格共用） |
| 新建 | `axpoint-parser/parsers/profiler.py` | 列画像、类型推断、PII 识别（含校验位） |
| 新建 | `axpoint-parser/parsers/staging.py` | Parquet 暂存写入 |
| 新建 | `axpoint-parser/parsers/office_convert.py` | LibreOffice headless 转换与公式重算（full 镜像） |
| 新建 | `axpoint-parser/parsers/{csv_parser.py, text_parser.py, excel_parser.py, pptx_parser.py, docx_parser.py, pdf_parser.py, json_parser.py, xml_parser.py, log_parser.py}` | 各格式解析器 |
| 修改 | `axpoint-parser/parsers/engine.py` | 保留 `parse_bytes_to_markdown`，内部改为调用 registry 的文本轨结果 |
| 新建 | `axpoint-parser/tests/{test_security.py, test_csv.py, test_text.py, test_excel.py, test_pptx.py, test_docx.py, test_pdf.py, test_json.py, test_xml.py, test_logs.py, test_profiler.py}` | 每个子步骤一个测试文件 |
| 新建 | `axpoint-parser/tests/snapshots/` | 期望 `UnifiedParseResponse` 快照（syrupy） |
| 修改 | `axpoint-parser/pyproject.toml` | 依赖见 [A6 §3](../../../docs/standards/A6-DEPENDENCY-BASELINE.md) |
| 新建 | `scripts/gate/step01.sh` | 门禁 |

---

## 01.1 Sidecar 安全基线

| 威胁 | 控制 | 实现位置 |
| :--- | :--- | :--- |
| 越权调用 | 除 `/health` 外必须携带 `X-AxPoint-Internal-Token`（Step 00） | `security.py::InternalTokenMiddleware` |
| 任意路径写入 | 请求中没有任何路径字段；输出目录由服务端计算：`{AXPOINT_DATA_ROOT}/tenants/{tenant_id}/staging/{parse_id}/`；`tenant_id` 由 Core 通过请求头 `X-AxPoint-Tenant` 传入并校验 `^[a-z0-9_-]{1,64}$`；`Path.resolve()` 后必须位于 `AXPOINT_DATA_ROOT` 之下 | `security.safe_tenant_dir()` |
| 超大上传 | 流式读取，累计超过 `AXPOINT_MAX_UPLOAD_MB` 立即返回 `413 UPLOAD_TOO_LARGE` | `security.read_limited()` |
| 压缩炸弹 | OOXML / ODF 文件先以 `zipfile` 检查：条目数 ≤ 10 000、解压总大小 ≤ 1 GiB、单条压缩比 ≤ 100；超出返回 `413 ARCHIVE_BOMB` | `security.check_zip()` |
| XXE / 实体膨胀 | 所有 XML（含 OOXML 内部 XML 由各库处理，独立 XML 文件由本模块处理）使用 `lxml.etree.XMLParser(resolve_entities=False, no_network=True, load_dtd=False, huge_tree=False)`；文件中出现 `<!DOCTYPE` 且含 `<!ENTITY` 时返回 `422 XML_UNSAFE` | `security.safe_xml_parser()` |
| SSRF（Step 02 起） | 出站 URL 解析 DNS 后拒绝回环、链路本地 `169.254.0.0/16`、私有网段 `10/8 172.16/12 192.168/16 100.64/10 fc00::/7 ::1`；重定向逐跳复检；连接超时 10 秒、响应上限 50 MB；连接配置显式 `allow_private_network=true` 且操作者具备 `DATA_ADMIN` 才放行私网 | `security.check_outbound_url()` |
| 解析耗时失控 | 单文件解析在线程池中执行，硬超时 `AXPOINT_PARSE_TIMEOUT_S`（默认 300）；PDF 页数上限 `AXPOINT_PDF_MAX_PAGES`（默认 2000） | `main.py` |
| 日志泄密 | `SecretFilter` 屏蔽敏感键 | `settings.py` |

**测试**（`tests/test_security.py`）：

| 用例 | 期望 |
| :--- | :--- |
| `X-AxPoint-Tenant: ../../etc` | 400 `INVALID_ARGUMENT` |
| 上传 `AXPOINT_MAX_UPLOAD_MB + 1` MB 的文件（测试中把上限设为 1） | 413 `UPLOAD_TOO_LARGE` |
| `fixtures/zip_bomb.xlsx`（由生成器构造，单条压缩比 > 100） | 413 `ARCHIVE_BOMB` |
| `fixtures/xxe.xml`（外部实体引用 `/etc/passwd`） | 422 `XML_UNSAFE`，响应中不含 `root:` |
| `fixtures/billion_laughs.xml` | 422 `XML_UNSAFE` |
| `check_outbound_url("http://169.254.169.254/latest/meta-data")` | 抛出 `OutboundBlocked` |
| `check_outbound_url("http://127.0.0.1:8001/health")` | 抛出 `OutboundBlocked` |
| `check_outbound_url("https://example.com/")` 且 DNS 解析为公网地址（测试中 monkeypatch 解析结果） | 通过 |

```bash
cd axpoint-parser && pytest -v tests/test_security.py
```

---

## 01.2 统一契约、路由与暂存

### 01.2.1 `schemas.py`

```python
from __future__ import annotations

from typing import Literal
from pydantic import BaseModel, Field

DataType = Literal["STRING", "INT64", "FLOAT64", "DECIMAL", "BOOLEAN", "DATE", "TIMESTAMP",
                   "BINARY", "JSON", "LIST", "STRUCT"]
PiiCategory = Literal["NONE", "PHONE", "ID_CARD", "EMAIL", "BANK_CARD", "NAME", "ADDRESS", "SECRET"]


class ColumnProfile(BaseModel):
    name: str
    ordinal: int
    data_type: DataType
    nullable: bool
    null_count: int
    null_ratio: float
    distinct_estimate: int
    sample_values: list[str] = Field(max_length=10)   # PII 列的样例已按 A1 MaskRule 脱敏
    pii_category: PiiCategory = "NONE"
    pii_hit_ratio: float = 0.0
    source_comment: str = ""


class ExtractedTable(BaseModel):
    table_name: str                  # ^[a-z][a-z0-9_]{0,62}$，由源名称规范化
    origin: str                      # 例：sheet=订单;block=1 / slide=3;table=1 / page=2;table=1 / record_tag=item
    row_count: int
    staging_path: str                # 相对 AXPOINT_DATA_ROOT 的路径
    columns: list[ColumnProfile]
    header_rows: int = 1


class ParseWarning(BaseModel):
    code: Literal["ENCODING_FALLBACK", "FORMULA_NOT_CACHED", "TS_INFERRED", "TRUNCATED_PAGES",
                  "OCR_LOW_CONFIDENCE", "TABLE_LOW_QUALITY", "COLUMN_RENAMED", "ROW_MALFORMED",
                  "NESTED_DEPTH_LIMIT", "EARLY_OFFICE_CONVERTED"]
    detail: str


class UnifiedParseResponse(BaseModel):
    parse_id: str                    # UUID
    title: str
    source_format: str               # csv / tsv / txt / xlsx / xls / xlsb / ods / pptx / ppt / docx / doc / pdf / image / json / jsonl / xml / log
    engine: str                      # 实际使用的解析引擎组合，例 "calamine+openpyxl"
    sha256: str
    markdown: str
    tables: list[ExtractedTable]
    warnings: list[ParseWarning] = []


class ParseResponse(BaseModel):
    title: str
    markdown: str
    engine: str
```

### 01.2.2 接口

| 方法与路径 | 请求 | 响应 |
| :--- | :--- | :--- |
| `POST /parse/unified` | `multipart/form-data`：`file`（必填）、`log_format`（可选，见 01.11）、`json_explode`（可选，`AUTO`/`NONE`/`ALL`，默认 `AUTO`）、`ocr`（可选，`AUTO`/`FORCE`/`OFF`，默认 `AUTO`）；请求头 `X-AxPoint-Internal-Token`、`X-AxPoint-Tenant` | `UnifiedParseResponse` |
| `POST /parse` | `multipart/form-data`：`file` | `ParseResponse`：只返回文本轨的轻量接口，内部调用同一条解析管线并丢弃表格轨（不写暂存文件） |

### 01.2.3 解析器协议（`parsers/base.py`）

```python
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

from schemas import ExtractedTable, ParseWarning


@dataclass
class ParseContext:
    tenant_id: str
    parse_id: str
    staging_dir: Path
    filename: str
    options: dict[str, str]
    warnings: list[ParseWarning] = field(default_factory=list)


@dataclass
class ParseResult:
    title: str
    engine: str
    markdown: str
    tables: list[ExtractedTable]


class BaseParser(Protocol):
    suffixes: tuple[str, ...]
    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult: ...
```

`registry.py` 的路由顺序：① 魔数（`PK\x03\x04` → OOXML / ODF 再按内部 `[Content_Types].xml` 区分；`%PDF-`；`\xD0\xCF\x11\xE0` → 早期 Office；PNG / JPEG / TIFF 签名）；② 后缀；③ 内容试探（JSON 首个非空字符 `{`/`[`、XML `<?xml`/`<`、日志行首时间戳）。后缀与魔数冲突时以魔数为准并记录 `warnings`。

### 01.2.4 暂存与表名

- `staging.write_table(ctx, name, polars_df) -> ExtractedTable`：写 `staging_dir/<table_name>.parquet`（`zstd` 压缩，行组 128 MB），同时调用 `profiler.profile(df)` 生成列画像。
- 表名规范化：中文用 `pypinyin` 转写为不带声调的拼音并以 `_` 连接（`库存` → `ku_cun`），其余非字母数字字符替换为 `_`，转小写，以字母开头（否则加前缀 `t_`），长度 ≤ 63，重名追加 `_2`、`_3`。原始名称写入 `origin`。
- 列名规范化：保留原始字符（含中文，下游一律以双引号引用标识符）；去除首尾空白；空列名改为 `col_<序号>`；重复列名追加后缀；发生改名时写 `COLUMN_RENAMED` 警告，`ColumnProfile.source_comment` 保存原列名。
- 暂存目录由 Step 02 提交入湖后删除；未提交的暂存目录由 `RETENTION` 任务按 24 小时清理。

### 01.2.5 列画像与类型推断（`parsers/profiler.py`）

1. 类型推断对每列取全部非空值（超过 100 000 行时取均匀抽样 100 000 行），按优先级 `BOOLEAN → INT64 → DECIMAL → FLOAT64 → DATE → TIMESTAMP → JSON → STRING` 判定：命中率 ≥ 98% 即采用，不符合的值置空并计入 `ROW_MALFORMED` 警告计数。
2. `DECIMAL`：小数位固定且 ≤ 6 位、总位数 ≤ 38 的数值列（金额、单价）；`FLOAT64`：其余带小数数值。
3. 日期时间格式集合：`YYYY-MM-DD`、`YYYY/MM/DD`、`YYYY年MM月DD日`、`YYYYMMDD`（8 位且月日合法）、ISO 8601、`DD/MM/YYYY`（只在 `MM` 部分 > 12 出现时判定）、Excel 序列日期（来自 Excel 的数值且列名含“日期/时间/date/time”）。时间统一转 UTC，无时区信息按租户时区（`sec_tenants.timezone`，由 Core 通过 `X-AxPoint-Timezone` 传入）解释。
4. 去重估计：`polars` 的 `approx_n_unique()`。
5. PII 识别（每列取至多 1 000 个样本，命中率 ≥ 30% 才标记整列）：

| 类别 | 判定 |
| :--- | :--- |
| `PHONE` | `^1[3-9]\d{9}$`（去除空格、`-`、`+86` 前缀后） |
| `ID_CARD` | 18 位，前 17 位数字，校验位符合 GB 11643-1999（加权因子 `7 9 10 5 8 4 2 1 6 3 7 9 10 5 8 4 2`，模 11 映射 `1 0 X 9 8 7 6 5 4 3 2`），出生日期合法 |
| `BANK_CARD` | 13～19 位数字，Luhn 校验通过，且 BIN 前缀为 `62`、`4`、`5`、`3` 之一 |
| `EMAIL` | RFC 5322 简化正则 `^[^@\s]+@[^@\s]+\.[A-Za-z]{2,}$` |
| `SECRET` | 列名匹配 `(?i)password|passwd|secret|token|api[_-]?key|private[_-]?key`，或值匹配 `^(sk|ak|ghp|xox[bp])-[A-Za-z0-9_-]{16,}$` |
| `NAME` / `ADDRESS` | full 镜像中由 Presidio + 中文识别器判定，lite 镜像只按列名（`姓名/name/联系人`、`地址/address`）判定 |

6. PII 列的 `sample_values` 以对应 `MaskRule` 脱敏后输出（规则与 Step 13 `masking.rs` 相同，测试用例共用 `tests/data/mask_cases.json`）。

**测试**（`tests/test_profiler.py`）：类型推断表驱动用例 40 条；`310101199001011234` 判为 `ID_CARD`（校验位正确的测试号码由生成器计算）；`6222021234567890120`（Luhn 不通过）不判为 `BANK_CARD`；10% 手机号混入的备注列不标 PII。

---

## 01.3 CSV / TSV

| 项 | 规则 |
| :--- | :--- |
| 编码 | `charset-normalizer` 探测；失败依次尝试 `utf-8-sig → gb18030 → big5 → latin-1`，使用了回退编码时写 `ENCODING_FALLBACK` |
| 分隔符 | `.tsv` 固定 `\t`；`.csv` 在前 64 KB 上用 DuckDB `sniff_csv` 判定（候选 `, ; \t | ^`） |
| 表头 | 交给 `blocks.detect_header`（见 01.5.2 规则 1、2） |
| 容错 | 引号不闭合、列数不一致的行：DuckDB `read_csv(..., ignore_errors=true, store_rejects=true)`，拒绝行数写入 `ROW_MALFORMED` 警告 |
| 大文件 | 超过 256 MB 时用 DuckDB 直接 `COPY (SELECT * FROM read_csv(...)) TO '<staging>.parquet'`，不经过 Python 内存 |
| 文本轨 | 文件名、列清单、行数、前 20 行 Markdown 表格 |
| 表格轨 | 1 张表 |

样本：`fixtures/orders.csv`（UTF-8，2 000 行，含金额、日期、手机号列）、`fixtures/orders_gbk.csv`（GB18030 编码同内容）、`fixtures/sensors.tsv`、`fixtures/broken.csv`（3 行引号不闭合）。

```bash
pytest -v tests/test_csv.py
```

通过标准：`orders.csv` 与 `orders_gbk.csv` 的列画像完全一致；`amount` 为 `DECIMAL`、`order_date` 为 `DATE`、`phone` 为 `PHONE`；`broken.csv` 返回 `ROW_MALFORMED` 且 `row_count = 总行数 - 3`。

---

## 01.4 TXT

按顺序判定结构，命中即停止：

1. **分隔符文本**：前 50 行用 `sniff_csv` 判定出一致的分隔符且列数 ≥ 2 → 按 CSV 处理。
2. **固定宽度文本**：前 50 行中 ≥ 90% 的行长度一致，且存在 ≥ 2 个在所有行上都为空格的列位置 → 以这些列位置切分。
3. **键值文本**：≥ 80% 的非空行匹配 `^\s*([^:=：]{1,64})\s*[:=：]\s*(.*)$` → 以空行分隔记录，键作为列。
4. **纯文本**：只走文本轨，按空行切段；连续 2 个以上空行视为章节分隔。

样本：`fixtures/plain.txt`、`fixtures/fixed_width.txt`、`fixtures/key_value.txt`、`fixtures/delimited.txt`。

```bash
pytest -v tests/test_text.py
```

通过标准：四个样本分别得到 0 / 1 / 1 / 1 张表；`fixed_width.txt` 切出 5 列；`key_value.txt` 得到 3 条记录 4 列。

---

## 01.5 Excel（xlsx / xlsm / xls / xlsb / ods）

### 01.5.1 引擎

- 读值：`python-calamine`（全部五种格式）。
- 读合并区域与公式：`openpyxl`（只对 `xlsx/xlsm`，`read_only=False, data_only=False` 读取合并区域；`data_only=True` 读取缓存值）。
- 公式缓存为空：写 `FORMULA_NOT_CACHED`；full 镜像中通过 `office_convert.recalculate(path)`（`soffice --headless --convert-to xlsx --outdir <tmp>`）重算后再读一次，仍为空则保持空值。

### 01.5.2 区块识别规则（`parsers/blocks.py`，Excel、CSV、Word 表格共用）

1. **表头行**：前 20 行中第一个“非空单元格占比 ≥ 60% 且字符串单元格占比 ≥ 80%”的行。
2. **多级表头**：表头行之后紧邻的一行也满足规则 1，且其上一行存在横向合并单元格时，合并为两级表头，列名为 `上级_下级`；最多三级。
3. **合并单元格填充**：表头区域横向填充，数据区域纵向填充。
4. **多区块**：同一 Sheet 中被 ≥ 2 个全空行或 ≥ 2 个全空列分隔的矩形区域，各自识别为独立表，命名 `<sheet>_<n>`（再经表名规范化）。
5. **汇总行**：区块末尾首列为“合计 / 总计 / 小计 / Total”的行从表格轨剔除，在文本轨保留。
6. **隐藏 Sheet**：解析但 `origin` 中标注 `hidden=true`。

### 01.5.3 输出

- 文本轨：每个 Sheet 一个二级标题，每个区块输出前 50 行 Markdown 表格。
- 表格轨：每个区块一张表。

样本：`fixtures/supply_chain.xlsx`（3 个 Sheet：`工厂`、`订单`（含两级表头与纵向合并单元格）、`库存`（一个 Sheet 内 2 个区块 + 合计行））、`fixtures/formulas_uncached.xlsx`（由 openpyxl 写入公式且无缓存值）、`fixtures/early_format.xls`、`fixtures/report.xlsb`、`fixtures/plan.ods`。

```bash
pytest -v tests/test_excel.py
```

通过标准：`supply_chain.xlsx` 输出 4 张表（`gong_chang`、`ding_dan`、`ku_cun_1`、`ku_cun_2`）；`订单` 表列名含 `数量_计划`、`数量_实际`；`ku_cun_2` 不含合计行；`formulas_uncached.xlsx` 返回 `FORMULA_NOT_CACHED`（lite）或数值正确（full）；`.xls/.xlsb/.ods` 三个样本的行数与列名与快照一致。

---

## 01.6 PPT（pptx / ppt）

- `.ppt`：full 镜像用 `office_convert.to_ooxml(path, "pptx")` 转换后走 pptx 解析器并写 `EARLY_OFFICE_CONVERTED`；lite 镜像返回 `415 UNSUPPORTED_EARLY_OFFICE_FORMAT`。
- 文本轨（full 镜像优先 Docling；lite 镜像用 python-pptx）：每页 `## 第 N 页：<标题>`，正文按形状阅读顺序（`top` 再 `left`）输出；**组合形状与 SmartArt 递归展开**（`GroupShape.shapes`，SmartArt 读取 `dgm` 数据部件中的文本节点）；表格转 Markdown；演讲者备注输出为 `> 备注：...`；图片在 `ocr=FORCE` 或（`ocr=AUTO` 且该页文本 < 20 字）时 OCR。
- 表格轨：每个表格一张表（`slide=N;table=M`）；每个原生图表把 `chart.plots[*].categories` 与 `series[*].values` 展开为一张表（`slide=N;chart=M`，列：`category` + 各系列名）。

样本：`fixtures/briefing.pptx`（5 页：含备注、1 个表格、1 个柱状图、1 个组合形状、1 个 SmartArt）、`fixtures/early_format.ppt`。

```bash
pytest -v tests/test_pptx.py
```

通过标准：Markdown 含 5 个页标题、备注文本、组合形状内文本与 SmartArt 节点文本；表格轨含 1 张表格表与 1 张图表数据表，图表表的系列值与生成器写入值一致。

---

## 01.7 Word（docx / doc）

- `.doc`：同 01.6 的转换策略。
- 文本轨：full 镜像 Docling（保留标题层级、列表、脚注、表格）；lite 镜像 python-docx 按段落样式 `Heading N` 映射标题层级。
- 表格轨：行数 ≥ 3 的表格各成一张表；**横向合并**（`gridSpan`）在表头区域横向填充，**纵向合并**（`vMerge`）纵向填充；**嵌套表格**展开为独立表（`origin` 中记录父表位置），父单元格在文本轨中以 `[嵌套表 N]` 占位。
- 表头识别复用 `blocks.detect_header`。

样本：`fixtures/contract.docx`（3 级标题、1 个合并单元格表格、1 个嵌套表格、1 个 2 行小表）、`fixtures/early_format.doc`。

```bash
pytest -v tests/test_docx.py
```

通过标准：Markdown 中 `#`、`##`、`###` 各至少 1 处；表格轨 2 张表（小表不入表格轨）；合并单元格填充值与快照一致。

---

## 01.8 PDF 与图片（pdf / png / jpg / jpeg / tiff）

- 引擎顺序：full 镜像 Docling（版面分析 + 表格结构 + EasyOCR，模型目录 `AXPOINT_MODEL_PATH`）；lite 镜像或 Docling 失败时，文本型 PDF 用 `pypdf` 抽取文本，图片返回 `415 UNSUPPORTED_FORMAT`。
- 页数超过 `AXPOINT_PDF_MAX_PAGES` 时只解析前 N 页并写 `TRUNCATED_PAGES`。
- OCR 置信度均值 < 0.6 的页写 `OCR_LOW_CONFIDENCE`。
- 表格轨：Docling 识别出的表格中，行数 ≥ 3 且“空单元格占比 ≤ 40%”的表格入表格轨；不满足的只进文本轨并写 `TABLE_LOW_QUALITY`。

样本：`fixtures/annual_report.pdf`（reportlab 生成：文本页 3 页 + 表格页 1 页）、`fixtures/scanned_invoice.png`（生成器把文本渲染为图片）。

```bash
pytest -v tests/test_pdf.py          # lite 环境自动跳过 OCR 用例并断言 415
```

通过标准：`annual_report.pdf` 文本轨含 3 个章节标题，表格轨 1 张表 4 列；full 环境下 `scanned_invoice.png` 的 OCR 文本包含发票号字符串。

---

## 01.9 JSON / JSONL

- 读取：≤ 64 MB 用 `orjson`；更大文件用 `ijson` 流式读取顶层数组元素，JSONL 逐行读取。
- 记录定位：顶层为数组 → 每个元素一条记录；顶层为对象 → 找到第一个“元素为对象且长度 ≥ 2”的数组路径作为记录集合（广度优先），路径写入 `origin`（例 `record_path=$.data.items`）；没有则把顶层对象作为 1 条记录。
- 扁平化：嵌套对象以 `.` 连接键名展开，最大深度 6，超出部分以 JSON 字符串保留并写 `NESTED_DEPTH_LIMIT`。
- 数组字段（`json_explode`）：
  - `AUTO`（默认）：标量数组保留为 `LIST` 列；对象数组拆成子表 `<table>__<field>`，子表带 `_parent_row` 外键列；
  - `NONE`：数组一律以 `JSON` 列保留；
  - `ALL`：对象数组按笛卡尔展开到主表（只建议在数组很短时使用）。
- Schema 存档：`genson` 生成 JSON Schema，写入 `staging_dir/<table>.schema.json`，Step 02 用于 API 与 Webhook 的 Schema 漂移比对。

样本：`fixtures/api_orders.json`（`{"data":{"items":[...]}}`，每条含 `customer` 嵌套对象与 `lines` 对象数组）、`fixtures/events.jsonl`（5 000 行）。

```bash
pytest -v tests/test_json.py
```

通过标准：`api_orders.json` 得到主表（含 `customer.name` 列）与子表 `api_orders__lines`（含 `_parent_row`）；`events.jsonl` 行数 5 000；`json_explode=NONE` 时 `lines` 为 `JSON` 列。

---

## 01.10 XML

- 解析器：`security.safe_xml_parser()`；> 64 MB 用 `lxml.etree.iterparse` 流式处理并在处理完每条记录后 `clear()`。
- 命名空间：标签名去掉命名空间 URI，保留前缀映射写入文本轨开头；同名不同命名空间的标签以 `prefix_local` 区分。
- 记录标签识别：统计每个元素路径的出现次数，选择“出现次数 ≥ 2、子元素或属性 ≥ 2、深度最浅”的路径作为记录（可用表单字段 `record_tag` 显式指定）。属性列名为 `@attr`，文本内容列名为 `#text`，子元素按 01.9 规则扁平化。
- 文本轨：标签树转 Markdown 嵌套列表（只展开前 200 个节点）。

样本：`fixtures/catalog.xml`（带默认命名空间与 `xsi` 前缀，100 个 `item` 记录，每个含属性与嵌套 `price`）、`fixtures/xxe.xml`、`fixtures/billion_laughs.xml`。

```bash
pytest -v tests/test_xml.py
```

通过标准：`catalog.xml` 得到 100 行、含 `@id`、`price.amount`、`price.@currency` 列；两个攻击样本返回 422。

---

## 01.11 日志

- 内置格式（`log_format` 取值）：`nginx_combined`、`apache_combined`、`syslog_rfc3164`、`syslog_rfc5424`、`jsonl`、`log4j_default`（`%d %-5p [%t] %c - %m%n`）、`k8s_cri`、`auto`（默认：对前 200 行逐个格式试匹配，命中率最高且 ≥ 80% 的格式胜出）、`regex:<命名分组正则>`（必须含 `timestamp`、`message` 分组，可含 `level`、`service`、`trace_id`）。
- 多行合并：行首匹配 `^(\d{4}-\d{2}-\d{2}|\[\d{4}-|\w{3} +\d{1,2} \d{2}:\d{2}:\d{2}|\{)` 才是新记录，其余行（`\tat `、`Caused by:`、`... N more`、缩进行）并入上一条的 `message`。
- 时间：解析为带时区时间并转 UTC；缺少年份（RFC 3164）取文件修改时间的年份；完全无法解析时取上一条记录时间并写 `TS_INFERRED`。
- 输出表结构：`ts TIMESTAMP, level STRING, service STRING, host STRING, trace_id STRING, template_id INT64, message STRING, raw STRING`；`nginx/apache` 额外输出 `client_ip, method, path, status, bytes, user_agent, latency_ms`。
- 模板聚类：`drain3==0.9.11`（相似度 0.4、深度 4）封装在 `log_parser.py` 内部，对外只暴露 `template_id` 与模板表 `<table>__templates(template_id, template, count)`。
- 文本轨：格式识别结果、时间范围、级别分布、Top 20 模板、全部 `ERROR`/`FATAL` 记录的前 50 条（含合并后的堆栈）。
- 持续采集不在 Sidecar 内实现：Step 02 §02.7 规定由 Vector / Fluent Bit 推送到 Webhook 或 Kafka。

样本：`fixtures/nginx_access.log`（1 000 行）、`fixtures/app_java.log`（log4j 格式，含 3 段 Java 堆栈）、`fixtures/syslog.log`（RFC 3164）、`fixtures/k8s.log`。

```bash
pytest -v tests/test_logs.py
```

通过标准：`app_java.log` 的堆栈行合并后记录数等于生成器写入的事件数；`nginx_access.log` 的 `status` 列为 `INT64`；`syslog.log` 的年份取自文件修改时间；模板表行数 > 0。

---

## 01.12 验收

`scripts/gate/step01.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
cd axpoint-parser
pytest -q tests/test_security.py tests/test_profiler.py tests/test_csv.py tests/test_text.py tests/test_excel.py \
          tests/test_pptx.py tests/test_docx.py tests/test_pdf.py tests/test_json.py tests/test_xml.py tests/test_logs.py
cd ..

parse() { curl -s -H "X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN" -H "X-AxPoint-Tenant: default" \
  -F "file=@axpoint-parser/tests/fixtures/$1" "$PARSER_BASE_URL/parse/unified"; }

expect_json "xlsx-blocks" "$(parse supply_chain.xlsx)" '(.tables | length) == 4 and (.tables | map(.table_name) | index("ku_cun_2") != null)'
expect_json "xlsx-types"  "$(parse supply_chain.xlsx)" '[.tables[] | select(.origin | startswith("sheet=订单")) | .columns[] | select(.name=="数量_实际") | .data_type][0] == "INT64"'
expect_json "csv-pii"     "$(parse orders.csv)" '[.tables[0].columns[] | select(.name=="phone") | .pii_category][0] == "PHONE"'
expect_json "pptx-notes"  "$(parse briefing.pptx)" '(.markdown | contains("备注")) and ([.tables[] | select(.origin | contains("chart="))] | length) == 1'
expect_json "docx-tables" "$(parse contract.docx)" '(.tables | length) == 2'
expect_json "json-child"  "$(parse api_orders.json)" '[.tables[].table_name] | index("api_orders__lines") != null'
expect_json "xml-records" "$(parse catalog.xml)" '.tables[0].row_count == 100'
expect_json "log-ts"      "$(parse app_java.log)" '[.tables[0].columns[] | select(.name=="ts") | .data_type][0] == "TIMESTAMP"'
expect_eq   "xxe-blocked" "$(curl -s -o /dev/null -w '%{http_code}' -H "X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN" \
  -H "X-AxPoint-Tenant: default" -F file=@axpoint-parser/tests/fixtures/xxe.xml "$PARSER_BASE_URL/parse/unified")" "422"
expect_eq   "staging-under-root" "$(parse orders.csv | jq -r '.tables[0].staging_path' | awk '/^tenants\/default\/staging\//{c++} END{print c+0}')" "1"

gate_pass step01
```

**通过标准**：最后一行输出 `GATE step01 PASS`；`tests/snapshots/` 中每个样本的快照与实际输出一致（`pytest --snapshot-update` 只允许在样本生成器变更的同一 PR 中执行）。
