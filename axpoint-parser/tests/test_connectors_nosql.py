from __future__ import annotations

import pytest

from connectors.nosql import Neo4jConnector
from connectors.registry import create_connector, list_connector_specs
from schemas import ConnectorType
from security import OutboundBlocked, SidecarError


def test_nosql_connector_specs_registered() -> None:
    specs = {s.connector_type: s for s in list_connector_specs()}
    for ctype in ("MONGODB", "REDIS", "ELASTICSEARCH", "CASSANDRA", "HBASE", "NEO4J"):
        assert ctype in specs
        assert specs[ctype].category == "NOSQL"


@pytest.mark.parametrize(
    "mutating_cypher",
    [
        "CREATE (n:Warehouse {code: 'W-1'}) RETURN n",
        "MATCH (n) DETACH DELETE n",
        "MERGE (n:Order {id: 'O-1'}) SET n.flag = true RETURN n",
        "DROP INDEX idx_nodes",
    ],
)
def test_neo4j_rejects_mutating_cypher_statements(mutating_cypher: str) -> None:
    with pytest.raises(SidecarError) as exc_info:
        Neo4jConnector.validate_cypher_readonly(mutating_cypher)
    assert exc_info.value.status_code == 400
    assert exc_info.value.error_code == "SQL_REJECTED"


def test_neo4j_allows_readonly_match_and_validates_label() -> None:
    assert (
        Neo4jConnector.validate_cypher_readonly("MATCH (w:Warehouse) RETURN w.code AS code")
        == "MATCH (w:Warehouse) RETURN w.code AS code"
    )
    conn = Neo4jConnector(
        config={"uri": "bolt://93.184.216.34:7687"},
        secrets={"username": "neo4j", "password": "secret"},
    )
    try:
        with pytest.raises(SidecarError) as exc_info:
            conn.preview({"label": "Warehouse`; DELETE n //"}, limit=5)
        assert exc_info.value.status_code == 400
    finally:
        conn.close()


@pytest.mark.parametrize(
    ("ctype", "cfg"),
    [
        ("MONGODB", {"host": "127.0.0.1", "port": 27017, "database": "app"}),
        ("REDIS", {"host": "127.0.0.1", "port": 6379}),
        ("ELASTICSEARCH", {"hosts": ["http://127.0.0.1:9200"]}),
        ("NEO4J", {"uri": "bolt://127.0.0.1:7687"}),
    ],
)
def test_nosql_blocks_private_hosts_unless_explicitly_allowed(
    ctype: ConnectorType, cfg: dict[str, object]
) -> None:
    with pytest.raises(OutboundBlocked):
        create_connector(ctype, dict(cfg), {}, allow_private_network=False)
