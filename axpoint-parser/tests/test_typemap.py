from __future__ import annotations

import pytest

from connectors.typemap import map_native_type
from schemas import DataType


@pytest.mark.parametrize(
    ("native_type", "dialect", "expected"),
    [
        ("bigint", "postgres", "INT64"),
        ("numeric(18, 2)", "postgres", "DECIMAL"),
        ("numeric(50, 2)", "postgres", "STRING"),
        ("jsonb", "postgres", "JSON"),
        ("uuid", "postgres", "STRING"),
        ("_int4", "postgres", "LIST"),
        ("integer[]", "postgres", "LIST"),
        ("bytea", "postgres", "BINARY"),
        ("timestamptz", "postgres", "TIMESTAMP"),
        ("tinyint(1)", "mysql", "BOOLEAN"),
        ("tinyint(4)", "mysql", "INT64"),
        ("largeint", "doris", "STRING"),
        ("largeint", "starrocks", "STRING"),
        ("uniqueidentifier", "mssql", "STRING"),
        ("datetimeoffset", "mssql", "TIMESTAMP"),
        ("money", "mssql", "DECIMAL"),
        ("NUMBER(10, 0)", "oracle", "INT64"),
        ("NUMBER(22, 0)", "oracle", "DECIMAL"),
        ("NUMBER(18, 4)", "oracle", "DECIMAL"),
        ("DATE", "oracle", "TIMESTAMP"),
        ("DATE", "dameng", "TIMESTAMP"),
        ("RAW(16)", "oracle", "BINARY"),
        ("INTEGER", "sqlite", "INT64"),
        ("VARCHAR(100)", "sqlite", "STRING"),
        ("REAL", "sqlite", "FLOAT64"),
        ("BLOB", "sqlite", "BINARY"),
        ("Nullable(LowCardinality(String))", "clickhouse", "STRING"),
        ("Nullable(DateTime64(3, 'UTC'))", "clickhouse", "TIMESTAMP"),
        ("Array(Int32)", "clickhouse", "LIST"),
        ("Tuple(String, Int64)", "clickhouse", "STRUCT"),
        ("Map(String, Int64)", "clickhouse", "JSON"),
        ("Int128", "clickhouse", "STRING"),
        ("struct<a:int,b:string>", "hive", "STRUCT"),
        ("array<string>", "hive", "LIST"),
        ("map<string,int>", "hive", "JSON"),
        ("NCHAR(64)", "tdengine", "STRING"),
    ],
)
def test_map_native_type_across_dialects(
    native_type: str, dialect: str, expected: DataType
) -> None:
    assert map_native_type(native_type, dialect=dialect) == expected
