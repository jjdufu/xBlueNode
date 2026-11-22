# AxPoint Parser（Python Sidecar）编码记忆与强制约束

本规则在操作 `axpoint-parser/` 下任意代码与测试时无条件生效，与根目录 [../AGENTS.md](../AGENTS.md) 叠加执行。

---

## 1. 模块边界与契约规范

1. **严格分包与注册表解耦**：
   - `parsers/`：多格式文件双轨解析（文本轨 Markdown + 表格轨结构化记录），坏行隔离、压缩炸弹与 XXE 防护；
   - `connectors/`：关系库、国产库、分析型数仓、NoSQL、时序库、消息流、对象存储与 API 只读抽取及增量位点；
   - `lake/`：DuckLake 目录挂载、Parquet 写入、Schema 演进与相邻小文件合并/快照过期维护；
   - `exporters/` & `charts/`：Excel / CSV / PDF / DOCX / PPTX / Markdown 出品渲染与服务端 ECharts PNG/SVG 渲染；
   - `pii/`：列画像与文本敏感实体识别。
   - 严禁把具体格式解析或连接器实现堆砌进 `main.py`；跨边界请求/响应模型统一在 `schemas.py` 用 Pydantic 2.x 定义，枚举字面量与 [A1](../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md) §4 完全一致。
2. **内部网络鉴权**：除 `/health` 外，全部 Sidecar 端点必须校验 `X-AxPoint-Internal-Token` 常量时间比对（`security.py`），严禁裸露无鉴权内部接口。

---

## 2. 类型安全、SQL 防注入与密钥保护红线（`scripts/python-guard.py` 强制核验）

1. **严格静态类型**：
   - 每个非空 `.py` 文件顶部必须声明 `from __future__ import annotations`。
   - 全部函数参数、返回值与类属性必须具备完整 Python `3.11+` 类型注解并通过 `mypy --strict .`。
   - 严禁出现无具体错误码的裸 `# type: ignore`；确因第三方无类型 C 扩展需要豁免时，必须写明具体错误码（如 `# type: ignore[import-untyped]`）并限制在单行。
2. **连接器与 DuckDB SQL 安全**：
   - 连接器一律以只读会话访问源系统，严禁向外部业务源库执行写操作或 DDL。
   - 所有外部传入的表名、列名、Schema 名在进入 DDL 或查询标识符位置前，必须经过严格白名单正则校验与安全引号包裹；所有过滤条件值、游标位点一律使用参数化绑定，严禁使用 f-string / `%` / `.format()` 将外部值直接拼入 SQL 字符串字面量。
3. **安全解析与反序列化**：
   - XML 解析必须使用 `defusedxml` 阻断外部实体（XXE）与实体膨胀攻击；压缩归档（`.xlsx` / `.docx` / `.pptx`）必须执行条目大小与压缩比上限检查。
   - 生产代码严禁调用 `eval()`、`exec()`、`pickle.load/loads()` 或未指定 `SafeLoader` 的 `yaml.load()`。
4. **凭证零落盘与零日志回显**：
   - 数据源连接密码、Token、私钥仅由 Core 解密后随单次内部请求传入内存，使用完毕即释放；严禁写入本地临时文件、异常消息或任何级别的日志输出。
5. **可复现测试样本**：
   - 测试夹具统一由 `tests/fixtures/make_fixtures.py` 固定随机种子与固定时间戳确定性生成，校验值对齐 `fixtures.sha256`，禁止提交来历不明的二进制样本。

---

## 3. 本目录提交前必跑命令

```bash
python3 ../scripts/python-guard.py
ruff check .
mypy --strict .
pytest -v -m "not heavy" tests
```
