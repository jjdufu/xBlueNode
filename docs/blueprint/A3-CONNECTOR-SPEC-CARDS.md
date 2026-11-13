# A3 · 连接器规格卡片

本附录为 `ConnectorType`（A1 §4）的 32 个取值逐一给出规格卡片。实现位于 `axpoint-parser/connectors/`（协议见 [03](./03-STEP02-AXHUB-CONNECTORS-AND-SYNC.md) §02.3），全部由 Step 02 交付。卡片中的“秘密字段”即 `ConnectorSpec.secret_fields`，加密存入 `ds_credentials`，任何接口不回显；“镜像层”决定驱动打包在 `lite` 还是 `full` 镜像（`lite` 中缺失的驱动返回 415 `UNSUPPORTED_FORMAT`）。

通用约定：

- 全部带主机名的连接器在构建时执行出站地址校验（私网地址需 `allow_private_network=true`，仅 `DATA_ADMIN` 可开启）；连接超时 10 秒，读取超时默认 60 秒（`timeout_s` 上限 600）。
- 读取输出 Arrow `RecordBatch`，默认每批 50 000 行；写湖时附加系统列 `_ax_batch_id`、`_ax_synced_at`，消息类附加 `_ax_msg_id`。
- 类型映射未列出的原生类型按 [03](./03-STEP02-AXHUB-CONNECTORS-AND-SYNC.md) §02.3.3 公共映射处理，未知类型为 `STRING`；每张卡片的特有映射都有 `tests/test_typemap.py` 表驱动用例。
- “测试”栏中标注 heavy 的镜像由 `testcontainers` 在测试内启动（`pytest -m heavy`，nightly 执行）；其余在 `deploy/docker-compose.test.yml` 中常驻。

## 总览

| # | ConnectorType | 类别 | 镜像层 | 同步模式 |
| :--- | :--- | :--- | :--- | :--- |
| 1 | `FILE_UPLOAD` | 文件类 | lite | SNAPSHOT |
| 2 | `S3` | 文件类 | lite | SNAPSHOT、APPEND |
| 3 | `SFTP` | 文件类 | lite | SNAPSHOT、APPEND |
| 4 | `POSTGRES` | 关系库 | lite | SNAPSHOT、APPEND、UPSERT |
| 5 | `MYSQL` | 关系库 | lite | SNAPSHOT、APPEND、UPSERT |
| 6 | `MSSQL` | 关系库 | full | SNAPSHOT、APPEND、UPSERT |
| 7 | `ORACLE` | 关系库 | full | SNAPSHOT、APPEND、UPSERT |
| 8 | `SQLITE` | 关系库 | lite | SNAPSHOT、APPEND、UPSERT |
| 9 | `CLICKHOUSE` | 分析型库 | lite | SNAPSHOT、APPEND、UPSERT |
| 10 | `DAMENG` | 国产库 | full | SNAPSHOT、APPEND、UPSERT |
| 11 | `KINGBASE` | 国产库 | lite | SNAPSHOT、APPEND、UPSERT |
| 12 | `OPENGAUSS` | 国产库 | lite | SNAPSHOT、APPEND、UPSERT |
| 13 | `OCEANBASE` | 国产库 | lite | SNAPSHOT、APPEND、UPSERT |
| 14 | `TIDB` | 国产库 | lite | SNAPSHOT、APPEND、UPSERT |
| 15 | `DORIS` | 分析型库 | lite | SNAPSHOT、APPEND、UPSERT |
| 16 | `STARROCKS` | 分析型库 | lite | SNAPSHOT、APPEND、UPSERT |
| 17 | `HIVE` | 分析型库 | full | SNAPSHOT、APPEND |
| 18 | `MONGODB` | NoSQL | lite | SNAPSHOT、APPEND、UPSERT |
| 19 | `REDIS` | NoSQL | lite | SNAPSHOT；`STREAM` 键为 APPEND |
| 20 | `ELASTICSEARCH` | NoSQL | lite | SNAPSHOT、APPEND |
| 21 | `CASSANDRA` | NoSQL | full | SNAPSHOT；有 `time_bucket_column` 时 APPEND |
| 22 | `HBASE` | NoSQL | full | SNAPSHOT、APPEND |
| 23 | `NEO4J` | NoSQL | lite | SNAPSHOT、APPEND |
| 24 | `INFLUXDB` | 时序库 | lite | SNAPSHOT、APPEND |
| 25 | `TDENGINE` | 时序库 | lite | SNAPSHOT、APPEND |
| 26 | `IOTDB` | 时序库 | lite | SNAPSHOT、APPEND |
| 27 | `KAFKA` | 消息流 | lite | APPEND |
| 28 | `MQTT` | 消息流 | lite | APPEND |
| 29 | `PULSAR` | 消息流 | lite | APPEND |
| 30 | `REST_API` | 接口 | lite | SNAPSHOT、APPEND、UPSERT |
| 31 | `GRAPHQL` | 接口 | lite | SNAPSHOT、APPEND |
| 32 | `WEBHOOK` | 接口 | lite | APPEND |

---

## 文件类

### 1. `FILE_UPLOAD`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | uploads（Core）+ `parsers/*` |
| 驱动 | Step 01 解析器 |
| 镜像层 | `lite` |
| 配置字段 | 上传表单：`to_knowledge`、`to_datasets`、`dataset_prefix`、`log_format`、`json_explode`、`ocr` |
| 秘密字段 | — |
| 发现 | 不适用：每张抽取表即一个数据集 |
| 同步模式 | SNAPSHOT |
| 增量机制 | 同一数据集再次上传覆盖为新快照；同 SHA-256 去重 |
| 特有类型映射 | 按 Step 01 列画像推断 |
| 测试 | `supply_chain.xlsx` 门禁上传（step02） |

### 2. `S3`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/object_store.py` |
| 驱动 | `s3fs` |
| 镜像层 | `lite` |
| 配置字段 | `endpoint_url, bucket, prefix, region, path_style, include_glob, group_pattern` |
| 秘密字段 | `access_key_id, secret_access_key` |
| 发现 | 列出前缀下匹配文件，按 `group_pattern` 分组 |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | `(mtime, path)` 游标 |
| 特有类型映射 | 按文件格式走 Step 01；`parquet` 保留原类型 |
| 测试 | compose `minio`：3 个 `orders_*.csv` 合并、第 4 个文件增量 |

### 3. `SFTP`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/object_store.py` |
| 驱动 | `paramiko` |
| 镜像层 | `lite` |
| 配置字段 | `host, port, base_dir, include_glob, group_pattern, host_key_fingerprint` |
| 秘密字段 | `password` 或 `private_key` |
| 发现 | 同 S3 |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | `(mtime, path)` 游标 |
| 特有类型映射 | 同 S3 |
| 测试 | compose `sftp`（`atmoz/sftp:alpine`） |

---

## 关系库

### 4. `POSTGRES`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/sql_family.py` |
| 驱动 | SQLAlchemy + `connectorx`（`psycopg`） |
| 镜像层 | `lite` |
| 配置字段 | `host, port, database, schema, sslmode, allow_private_network` |
| 秘密字段 | `password`（用户名在 config `username`） |
| 发现 | `inspect()`：表、视图、列、主键、注释；`pg_class.reltuples` 估算行数 |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 游标列 `>` / `>=`（UPSERT） |
| 特有类型映射 | `jsonb→JSON`、`uuid→STRING`、`numeric(p,s)→DECIMAL`、数组→`LIST` |
| 测试 | 主库 `demo_src` 模式（门禁种子） |

### 5. `MYSQL`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/sql_family.py` |
| 驱动 | SQLAlchemy + `connectorx`（`pymysql`） |
| 镜像层 | `lite` |
| 配置字段 | `host, port, database, charset(utf8mb4), ssl` |
| 秘密字段 | `password` |
| 发现 | `inspect()`；`information_schema.TABLES.TABLE_ROWS` |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | `tinyint(1)→BOOLEAN`、`datetime` 视为会话时区后转 UTC、`json→JSON` |
| 测试 | compose `mysql:8.4` |

### 6. `MSSQL`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/sql_family.py` |
| 驱动 | SQLAlchemy + `pymssql` |
| 镜像层 | `full` |
| 配置字段 | `host, port, database, schema, encrypt` |
| 秘密字段 | `password` |
| 发现 | `inspect()`；`sys.partitions` 行数 |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | `uniqueidentifier→STRING`、`datetimeoffset→TIMESTAMP`、`bit→BOOLEAN`、`money→DECIMAL` |
| 测试 | `mcr.microsoft.com/mssql/server:2022-latest`（heavy） |

### 7. `ORACLE`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/sql_family.py` |
| 驱动 | SQLAlchemy + `oracledb`（thin 模式） |
| 镜像层 | `full` |
| 配置字段 | `host, port, service_name, schema` |
| 秘密字段 | `password` |
| 发现 | `ALL_TABLES / ALL_TAB_COLUMNS / ALL_CONSTRAINTS` |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | `NUMBER` 无精度→`DECIMAL`（精度 38）、`NUMBER(p,0) p≤18→INT64`、`DATE→TIMESTAMP`、`CLOB→STRING` |
| 测试 | `gvenzl/oracle-free:23-slim`（heavy） |

### 8. `SQLITE`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/sql_family.py` |
| 驱动 | 标准库 `sqlite3` |
| 镜像层 | `lite` |
| 配置字段 | `path`（必须位于 `${AXPOINT_DATA_ROOT}/tenants/<tenant>/files/`） |
| 秘密字段 | — |
| 发现 | `sqlite_master` + `PRAGMA table_info` |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | 按声明类型亲和性映射 |
| 测试 | 单测内生成临时库 |

---

## 分析型库

### 9. `CLICKHOUSE`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/analytic.py` |
| 驱动 | `clickhouse-connect`（`query_arrow_stream`） |
| 镜像层 | `lite` |
| 配置字段 | `host, port, database, secure` |
| 秘密字段 | `password` |
| 发现 | `system.tables / system.columns` |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上（UPSERT 依赖声明主键） |
| 特有类型映射 | `Nullable(T)` 解包、`LowCardinality(T)→T`、`Tuple→STRUCT`、`Array→LIST`、`DateTime64→TIMESTAMP` |
| 测试 | compose `clickhouse/clickhouse-server:25` |

---

## 国产库

### 10. `DAMENG`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/domestic.py` |
| 驱动 | `dmPython` |
| 镜像层 | `full` |
| 配置字段 | `host, port(5236), schema` |
| 秘密字段 | `password` |
| 发现 | `ALL_TABLES / ALL_TAB_COLUMNS / ALL_CONSTRAINTS` |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 服务端游标 `fetchmany` |
| 特有类型映射 | 同 Oracle 族；`TEXT→STRING` |
| 测试 | 达梦官方开发版镜像（heavy；镜像需自行导入私有仓库，标签写入 `tests/heavy_images.txt`） |

### 11. `KINGBASE`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/domestic.py` |
| 驱动 | `psycopg`（PostgreSQL 协议） |
| 镜像层 | `lite` |
| 配置字段 | `host, port(54321), database, schema` |
| 秘密字段 | `password` |
| 发现 | 同 POSTGRES（系统视图兼容） |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | 同 POSTGRES |
| 测试 | 人大金仓官方镜像（heavy，同达梦的镜像处理方式） |

### 12. `OPENGAUSS`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/domestic.py` |
| 驱动 | `psycopg`（服务端需允许 MD5 或 SCRAM） |
| 镜像层 | `lite` |
| 配置字段 | `host, port(5432), database, schema` |
| 秘密字段 | `password` |
| 发现 | 同 POSTGRES |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | 同 POSTGRES |
| 测试 | `opengauss/opengauss`（heavy） |

### 13. `OCEANBASE`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/domestic.py` |
| 驱动 | `pymysql`（MySQL 租户模式） |
| 镜像层 | `lite` |
| 配置字段 | `host, port(2881), database, tenant_user`（`user@tenant#cluster` 形式） |
| 秘密字段 | `password` |
| 发现 | 同 MYSQL |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | 同 MYSQL |
| 测试 | 协议级：compose `mysql`；真实库：`oceanbase/oceanbase-ce`（heavy） |

### 14. `TIDB`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/domestic.py` |
| 驱动 | `pymysql` |
| 镜像层 | `lite` |
| 配置字段 | `host, port(4000), database` |
| 秘密字段 | `password` |
| 发现 | 同 MYSQL |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | 同 MYSQL |
| 测试 | 协议级：compose `mysql`；真实库：`pingcap/tidb`（heavy） |

---

## 分析型库

### 15. `DORIS`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/analytic.py` |
| 驱动 | `pymysql`；可选 Arrow Flight SQL（`config.flight_port`） |
| 镜像层 | `lite` |
| 配置字段 | `host, query_port(9030), database, flight_port?` |
| 秘密字段 | `password` |
| 发现 | `information_schema` |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | 同 MYSQL；`LARGEINT→STRING` |
| 测试 | 协议级：compose `mysql`；真实库：`apache/doris` all-in-one（heavy） |

### 16. `STARROCKS`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/analytic.py` |
| 驱动 | `pymysql`；可选 Arrow Flight SQL |
| 镜像层 | `lite` |
| 配置字段 | `host, query_port(9030), database, flight_port?` |
| 秘密字段 | `password` |
| 发现 | `information_schema` |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | 同上 |
| 特有类型映射 | 同 DORIS |
| 测试 | 协议级：compose `mysql`；真实库：`starrocks/allin1-ubuntu`（heavy） |

### 17. `HIVE`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/analytic.py` |
| 驱动 | `pyhive`（HiveServer2） |
| 镜像层 | `full` |
| 配置字段 | `host, port(10000), database, auth(NONE/LDAP/KERBEROS), kerberos_service_name?` |
| 秘密字段 | `password`（LDAP）或 `keytab_b64`（KERBEROS） |
| 发现 | `SHOW TABLES` + `DESCRIBE FORMATTED` |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | 分区列或游标列 `>` |
| 特有类型映射 | `struct→STRUCT`、`array→LIST`、`map→JSON` |
| 测试 | `apache/hive:4.0.1`（heavy） |

---

## NoSQL

### 18. `MONGODB`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/nosql.py` |
| 驱动 | `pymongo` |
| 镜像层 | `lite` |
| 配置字段 | `uri_hosts, database, replica_set?, tls, auth_source` |
| 秘密字段 | `username, password` |
| 发现 | `list_collection_names()` + `$sample` 1 000 条推断 |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | `_id` 或时间字段 `$gt` |
| 特有类型映射 | 文档→扁平列（深度 6），数组按 `json_explode`；`ObjectId→STRING`、`Decimal128→DECIMAL` |
| 测试 | compose `mongo:8` |

### 19. `REDIS`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/nosql.py` |
| 驱动 | `redis`（redis-py） |
| 镜像层 | `lite` |
| 配置字段 | `host, port, db, tls, key_patterns` |
| 秘密字段 | `password`（可选 `username`） |
| 发现 | `SCAN` 前缀统计，按键类型分组 |
| 同步模式 | SNAPSHOT；`STREAM` 键为 APPEND |
| 增量机制 | `STREAM` 条目 ID |
| 特有类型映射 | `HASH` 字段为列；`ZSET→key,member,score` |
| 测试 | compose `redis:8` |

### 20. `ELASTICSEARCH`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/nosql.py` |
| 驱动 | `elasticsearch`（9.x 客户端） |
| 镜像层 | `lite` |
| 配置字段 | `hosts, index_patterns, verify_certs` |
| 秘密字段 | `api_key` 或 `username, password` |
| 发现 | `GET _mapping` |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | 时间字段 `range` + `search_after` + PIT |
| 特有类型映射 | `object` 扁平化、`nested` 拆子表、`keyword/text→STRING`、`date→TIMESTAMP` |
| 测试 | compose ES 9.x |

### 21. `CASSANDRA`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/nosql.py` |
| 驱动 | `cassandra-driver` |
| 镜像层 | `full` |
| 配置字段 | `contact_points, port, keyspace, local_dc, time_bucket_column?` |
| 秘密字段 | `username, password` |
| 发现 | `system_schema.tables/columns` |
| 同步模式 | SNAPSHOT；有 `time_bucket_column` 时 APPEND |
| 增量机制 | 时间分桶列 |
| 特有类型映射 | `map/set/list→JSON/LIST`、`timeuuid→STRING` |
| 测试 | `cassandra:5`（heavy） |

### 22. `HBASE`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/nosql.py` |
| 驱动 | `happybase`（Thrift） |
| 镜像层 | `full` |
| 配置字段 | `host, thrift_port(9090), table_prefix, column_mapping` |
| 秘密字段 | —（Thrift 网关鉴权在网络层） |
| 发现 | `getTableNames` + 列族 |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | 行键范围 |
| 特有类型映射 | 按 `column_mapping` 指定类型解码字节 |
| 测试 | `harisekhon/hbase`（heavy） |

### 23. `NEO4J`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/nosql.py` |
| 驱动 | `neo4j`（官方驱动） |
| 镜像层 | `lite` |
| 配置字段 | `uri, database` |
| 秘密字段 | `username, password` |
| 发现 | `db.labels()`、`db.relationshipTypes()`、`apoc.meta.schema`（无 APOC 时抽样） |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | 节点属性游标 `>` |
| 特有类型映射 | 节点属性为列；关系数据集含 `src_key, dst_key` |
| 测试 | `neo4j:5`（heavy） |

---

## 时序库

### 24. `INFLUXDB`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/timeseries.py` |
| 驱动 | `influxdb-client`（2.x）/ `influxdb3-python`（3.x） |
| 镜像层 | `lite` |
| 配置字段 | `url, org?, bucket_or_database, version(2|3), pivot` |
| 秘密字段 | `token` |
| 发现 | 2.x：`buckets()`、`schema.measurements()`；3.x：SQL `information_schema` |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | `_time` 区间 |
| 特有类型映射 | 长表 `ts, measurement, tags(JSON), field, value` 或宽表 |
| 测试 | compose `influxdb:3-core`；heavy 覆盖 `influxdb:2.7` |

### 25. `TDENGINE`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/timeseries.py` |
| 驱动 | `taospy`（REST 连接） |
| 镜像层 | `lite` |
| 配置字段 | `url, database` |
| 秘密字段 | `username, password` |
| 发现 | `SHOW STABLES`、`DESCRIBE` |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | `ts >` 游标 |
| 特有类型映射 | `NCHAR/BINARY→STRING`、`TIMESTAMP→TIMESTAMP`、`tbname` 成列 |
| 测试 | `tdengine/tdengine:3`（heavy） |

### 26. `IOTDB`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/timeseries.py` |
| 驱动 | `apache-iotdb`（Session） |
| 镜像层 | `lite` |
| 配置字段 | `host, port(6667), path_prefix` |
| 秘密字段 | `username, password` |
| 发现 | `SHOW TIMESERIES` |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | `time >` 游标，`ALIGN BY DEVICE` |
| 特有类型映射 | `device` 列 + 测点列；`TEXT→STRING` |
| 测试 | `apache/iotdb:2`（heavy） |

---

## 消息流

### 27. `KAFKA`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/streams.py` |
| 驱动 | `confluent-kafka` |
| 镜像层 | `lite` |
| 配置字段 | `bootstrap_servers, topics, security_protocol, sasl_mechanism, value_format, schema_registry_url, start, max_messages, max_wait_s` |
| 秘密字段 | `sasl_username, sasl_password, ssl_key_pem` |
| 发现 | `list_topics()`；抽样 100 条推断 |
| 同步模式 | APPEND |
| 增量机制 | 消费组位点 + `ds_stream_offsets` |
| 特有类型映射 | 按 `value_format` 解码 |
| 测试 | compose `redpanda` |

### 28. `MQTT`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/streams.py` |
| 驱动 | `paho-mqtt` 2.x |
| 镜像层 | `lite` |
| 配置字段 | `host, port, tls, topic_filters, qos, value_format, max_messages, max_wait_s` |
| 秘密字段 | `username, password` |
| 发现 | 订阅 `max_wait_s` 内抽样推断 |
| 同步模式 | APPEND |
| 增量机制 | 持久会话 + 最后接收时间 |
| 特有类型映射 | 同上 |
| 测试 | compose `eclipse-mosquitto:2` |

### 29. `PULSAR`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/streams.py` |
| 驱动 | `pulsar-client` |
| 镜像层 | `lite` |
| 配置字段 | `service_url, topics, subscription_initial_position, value_format, max_messages, max_wait_s` |
| 秘密字段 | `token` |
| 发现 | `admin` 接口列主题 + 抽样 |
| 同步模式 | APPEND |
| 增量机制 | `Shared` 订阅确认 |
| 特有类型映射 | 同上 |
| 测试 | compose `apachepulsar/pulsar:3.3` |

---

## 接口

### 30. `REST_API`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/rest_api.py` |
| 驱动 | `httpx` + `tenacity` + `jsonpath-ng` |
| 镜像层 | `lite` |
| 配置字段 | `base_url, auth, default_headers, rate_limit_rps, timeout_s, resources[]`（§02.7.1） |
| 秘密字段 | 依 `auth.type`：`username,password` / `token` / `api_key` / `client_id,client_secret` |
| 发现 | 请求每个资源第一页推断列 |
| 同步模式 | SNAPSHOT、APPEND、UPSERT |
| 增量机制 | `incremental.param` + `cursor_field` |
| 特有类型映射 | JSON 扁平化（01.9） |
| 测试 | `respx` 模拟五种分页与 OAuth2 |

### 31. `GRAPHQL`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `connectors/graphql_api.py` |
| 驱动 | `httpx` + `graphql-core`（AST 校验） |
| 镜像层 | `lite` |
| 配置字段 | `endpoint, auth, resources[{name, query, variables, record_path, pagination}]` |
| 秘密字段 | 同 REST_API |
| 发现 | 内省查询列出 Query 根字段 |
| 同步模式 | SNAPSHOT、APPEND |
| 增量机制 | Relay `after` 游标 + 变量中的时间条件 |
| 特有类型映射 | 同 REST_API |
| 测试 | `respx` 模拟；`mutation` 被拒 |

### 32. `WEBHOOK`

| 项 | 内容 |
| :--- | :--- |
| 实现文件 | `routes/axhub.rs`（接收）+ `connectors/webhook.py`（刷入） |
| 驱动 | Core 接收 + Sidecar 刷入 |
| 镜像层 | `lite` |
| 配置字段 | `slug, auth_scheme(HMAC_SHA256/BEARER), record_path, max_body_bytes` |
| 秘密字段 | `secret`（系统生成，只返回一次） |
| 发现 | 不适用：创建即生成数据集 |
| 同步模式 | APPEND |
| 增量机制 | `ds_webhook_buffer.flushed_at` |
| 特有类型映射 | JSON 扁平化 |
| 测试 | 门禁：签名正确 202、错误 401、重复投递不重复入库 |

---

## 验证

```bash
# 32 个类型均已注册，且 lite / full 可用性与卡片一致
curl -s -H "X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN" "$PARSER_BASE_URL/connectors/types" | jq 'length'   # 32
cd axpoint-parser && uv run pytest tests/test_typemap.py tests/test_connectors_sql.py tests/test_connectors_nosql.py tests/test_connectors_ts.py tests/test_connectors_api.py tests/test_streams.py -m 'not heavy' -q
cd axpoint-parser && uv run pytest -m heavy -q   # nightly
```
