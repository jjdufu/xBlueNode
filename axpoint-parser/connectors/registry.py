from __future__ import annotations

import importlib.util
from typing import Any

from connectors.analytic import (
    CLICKHOUSE_SPEC,
    DORIS_SPEC,
    HIVE_SPEC,
    STARROCKS_SPEC,
    ClickHouseConnector,
    DorisConnector,
    HiveConnector,
    StarRocksConnector,
)
from connectors.base import BaseConnector
from connectors.domestic import (
    DAMENG_SPEC,
    KINGBASE_SPEC,
    OCEANBASE_SPEC,
    OPENGAUSS_SPEC,
    TIDB_SPEC,
    DamengConnector,
    KingbaseConnector,
    OceanBaseConnector,
    OpenGaussConnector,
    TiDbConnector,
)
from connectors.graphql_api import GRAPHQL_SPEC, GraphqlConnector
from connectors.nosql import (
    CASSANDRA_SPEC,
    ELASTICSEARCH_SPEC,
    HBASE_SPEC,
    MONGODB_SPEC,
    NEO4J_SPEC,
    REDIS_SPEC,
    CassandraConnector,
    ElasticsearchConnector,
    HBaseConnector,
    MongoDbConnector,
    Neo4jConnector,
    RedisConnector,
)
from connectors.object_store import (
    FILE_UPLOAD_SPEC,
    S3_SPEC,
    SFTP_SPEC,
    FileUploadConnector,
    S3Connector,
    SftpConnector,
)
from connectors.rest_api import REST_API_SPEC, RestApiConnector
from connectors.sql_family import (
    MSSQL_SPEC,
    MYSQL_SPEC,
    ORACLE_SPEC,
    POSTGRES_SPEC,
    SQLITE_SPEC,
    MsSqlConnector,
    MySqlConnector,
    OracleConnector,
    PostgresConnector,
    SqliteConnector,
)
from connectors.streams import (
    KAFKA_SPEC,
    MQTT_SPEC,
    PULSAR_SPEC,
    KafkaConnector,
    MqttConnector,
    PulsarConnector,
)
from connectors.timeseries import (
    INFLUXDB_SPEC,
    IOTDB_SPEC,
    TDENGINE_SPEC,
    InfluxDbConnector,
    IoTDbConnector,
    TDengineConnector,
)
from connectors.webhook import WEBHOOK_SPEC, WebhookConnector
from schemas import ConnectorSpecModel, ConnectorType
from security import SidecarError

_OPTIONAL_DRIVER_MODULES: dict[ConnectorType, str] = {
    "MSSQL": "pymssql",
    "ORACLE": "oracledb",
    "DAMENG": "dmPython",
    "HIVE": "pyhive",
    "CASSANDRA": "cassandra",
    "HBASE": "happybase",
}

_REGISTRY_ORDER: list[tuple[ConnectorSpecModel, type[BaseConnector]]] = [
    (FILE_UPLOAD_SPEC, FileUploadConnector),
    (S3_SPEC, S3Connector),
    (SFTP_SPEC, SftpConnector),
    (POSTGRES_SPEC, PostgresConnector),
    (MYSQL_SPEC, MySqlConnector),
    (MSSQL_SPEC, MsSqlConnector),
    (ORACLE_SPEC, OracleConnector),
    (SQLITE_SPEC, SqliteConnector),
    (CLICKHOUSE_SPEC, ClickHouseConnector),
    (DAMENG_SPEC, DamengConnector),
    (KINGBASE_SPEC, KingbaseConnector),
    (OPENGAUSS_SPEC, OpenGaussConnector),
    (OCEANBASE_SPEC, OceanBaseConnector),
    (TIDB_SPEC, TiDbConnector),
    (DORIS_SPEC, DorisConnector),
    (STARROCKS_SPEC, StarRocksConnector),
    (HIVE_SPEC, HiveConnector),
    (MONGODB_SPEC, MongoDbConnector),
    (REDIS_SPEC, RedisConnector),
    (ELASTICSEARCH_SPEC, ElasticsearchConnector),
    (CASSANDRA_SPEC, CassandraConnector),
    (HBASE_SPEC, HBaseConnector),
    (NEO4J_SPEC, Neo4jConnector),
    (INFLUXDB_SPEC, InfluxDbConnector),
    (TDENGINE_SPEC, TDengineConnector),
    (IOTDB_SPEC, IoTDbConnector),
    (KAFKA_SPEC, KafkaConnector),
    (MQTT_SPEC, MqttConnector),
    (PULSAR_SPEC, PulsarConnector),
    (REST_API_SPEC, RestApiConnector),
    (GRAPHQL_SPEC, GraphqlConnector),
    (WEBHOOK_SPEC, WebhookConnector),
]

_CLASS_BY_TYPE: dict[str, type[BaseConnector]] = {
    spec.connector_type: cls for spec, cls in _REGISTRY_ORDER
}


def is_driver_available(connector_type: ConnectorType) -> bool:
    """Return True if the required Python driver module is installed in the current image."""
    mod_name = _OPTIONAL_DRIVER_MODULES.get(connector_type)
    if mod_name is None:
        return True
    return importlib.util.find_spec(mod_name) is not None


def list_connector_specs() -> list[ConnectorSpecModel]:
    """Return all 32 ConnectorSpecModel cards with runtime driver availability."""
    out: list[ConnectorSpecModel] = []
    for spec, _cls in _REGISTRY_ORDER:
        avail = is_driver_available(spec.connector_type)
        out.append(spec.model_copy(update={"available": avail}))
    return out


def create_connector(
    connector_type: ConnectorType,
    config: dict[str, Any],
    secrets: dict[str, str],
    *,
    allow_private_network: bool = False,
    tenant_id: str = "default",
) -> BaseConnector:
    """Instantiate a concrete BaseConnector after verifying driver availability."""
    cls = _CLASS_BY_TYPE.get(connector_type)
    if cls is None:
        raise SidecarError(400, "INVALID_ARGUMENT", f"unknown connector_type: {connector_type!r}")
    if not is_driver_available(connector_type):
        req_mod = _OPTIONAL_DRIVER_MODULES.get(connector_type, connector_type)
        raise SidecarError(
            status_code=422,
            error_code="CONNECTOR_DRIVER_UNAVAILABLE",
            message=(
                f"connector {connector_type} requires optional driver {req_mod!r} "
                "which is only included in the full parser image"
            ),
        )
    return cls(
        config,
        secrets,
        allow_private_network=allow_private_network,
        tenant_id=tenant_id,
    )
