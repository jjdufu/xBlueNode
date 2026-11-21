from __future__ import annotations

import hashlib
import time
from collections.abc import Iterator
from datetime import UTC, datetime
from typing import Any
from urllib.parse import urlparse

import orjson
import pyarrow as pa

from connectors.base import BaseConnector, ReadBatch, records_to_arrow_table
from parsers.json_parser import flatten_json_records
from schemas import ConnectorSpecModel, DiscoveredObject
from security import SidecarError, check_outbound_host

KAFKA_SPEC = ConnectorSpecModel(
    connector_type="KAFKA",
    display_name="Apache Kafka",
    category="STREAM",
    config_schema={
        "type": "object",
        "required": ["bootstrap_servers"],
        "properties": {
            "bootstrap_servers": {"type": "string"},
            "security_protocol": {"type": "string", "default": "PLAINTEXT"},
            "sasl_mechanism": {"type": "string"},
            "group_id": {"type": "string", "default": "axhub-consumer"},
            "max_messages_per_run": {"type": "integer", "default": 50000},
            "max_wait_s": {"type": "integer", "default": 10},
        },
    },
    secret_fields=["sasl_username", "sasl_password", "ssl_key_pem"],
    supports_modes=["APPEND"],
    supports_discover=True,
    image_tier="lite",
)

MQTT_SPEC = ConnectorSpecModel(
    connector_type="MQTT",
    display_name="MQTT Broker",
    category="STREAM",
    config_schema={
        "type": "object",
        "required": ["host"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 1883},
            "tls": {"type": "boolean", "default": False},
            "qos": {"type": "integer", "default": 1},
            "sample_seconds": {"type": "number", "default": 5.0},
            "max_messages": {"type": "integer", "default": 10000},
        },
    },
    secret_fields=["username", "password"],
    supports_modes=["APPEND"],
    supports_discover=True,
    image_tier="lite",
)

PULSAR_SPEC = ConnectorSpecModel(
    connector_type="PULSAR",
    display_name="Apache Pulsar",
    category="STREAM",
    config_schema={
        "type": "object",
        "required": ["service_url"],
        "properties": {
            "service_url": {"type": "string"},
            "subscription": {"type": "string", "default": "axhub-sub"},
            "max_messages_per_run": {"type": "integer", "default": 50000},
            "max_wait_s": {"type": "integer", "default": 10},
        },
    },
    secret_fields=["token"],
    supports_modes=["APPEND"],
    supports_discover=True,
    image_tier="lite",
)


def parse_stream_payload(raw_bytes: bytes, msg_id: str, ts_utc: datetime) -> dict[str, Any]:
    """Decode stream message payload as JSON dict if valid, otherwise store raw string."""
    base: dict[str, Any] = {
        "_ax_msg_id": msg_id,
        "_ax_received_at": ts_utc,
    }
    try:
        parsed = orjson.loads(raw_bytes)
        if isinstance(parsed, dict):
            flat = flatten_json_records([parsed], json_explode="NONE")
            if flat.main_rows:
                base.update(dict(zip(flat.main_columns, flat.main_rows[0], strict=False)))
            return base
    except orjson.JSONDecodeError:
        pass
    base["raw"] = raw_bytes.decode("utf-8", errors="replace")
    return base


class KafkaConnector(BaseConnector):
    """Kafka micro-batch pull connector via confluent-kafka."""

    spec = KAFKA_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        super().__init__(
            config, secrets, allow_private_network=allow_private_network, tenant_id=tenant_id
        )
        servers = str(self.config.get("bootstrap_servers") or "").strip()
        if not servers:
            raise SidecarError(400, "INVALID_ARGUMENT", "Kafka config missing 'bootstrap_servers'")
        for broker in servers.split(","):
            b = broker.strip()
            if not b:
                continue
            host, _, port_s = b.partition(":")
            check_outbound_host(
                host,
                int(port_s) if port_s.isdigit() else 9092,
                allow_private_network=allow_private_network,
            )

    def _kafka_conf(self) -> dict[str, Any]:
        conf: dict[str, Any] = {
            "bootstrap.servers": str(self.config["bootstrap_servers"]),
            "group.id": str(self.config.get("group_id") or f"axhub-{self.tenant_id}"),
            "enable.auto.commit": False,
            "auto.offset.reset": "earliest",
            "security.protocol": str(self.config.get("security_protocol") or "PLAINTEXT"),
        }
        if self.config.get("sasl_mechanism"):
            conf["sasl.mechanism"] = str(self.config["sasl_mechanism"])
        if self.secrets.get("sasl_username"):
            conf["sasl.username"] = str(self.secrets["sasl_username"])
        if self.secrets.get("sasl_password"):
            conf["sasl.password"] = str(self.secrets["sasl_password"])
        return conf

    def test(self) -> tuple[bool, str]:
        from confluent_kafka.admin import AdminClient

        try:
            adm = AdminClient({"bootstrap.servers": str(self.config["bootstrap_servers"])})
            adm.list_topics(timeout=5.0)
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        from confluent_kafka.admin import AdminClient

        adm = AdminClient({"bootstrap.servers": str(self.config["bootstrap_servers"])})
        meta = adm.list_topics(timeout=5.0)
        return [
            DiscoveredObject(
                ref={"topic": str(t_name)},
                display_name=str(t_name),
                source_kind="STREAM",
                columns=[],
            )
            for t_name in sorted(meta.topics)
            if not str(t_name).startswith("__")
        ]

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        batches = list(self.read(ref, "APPEND", None, None, batch_rows=min(max(limit, 1), 100)))
        if not batches:
            return pa.table({})
        return pa.Table.from_batches([b.batch for b in batches])

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        from confluent_kafka import OFFSET_BEGINNING, Consumer, TopicPartition

        topic = str(ref.get("topic") or "").strip()
        if not topic:
            raise SidecarError(400, "INVALID_ARGUMENT", "Kafka ref missing 'topic'")

        max_msgs = min(int(self.config.get("max_messages_per_run") or 50_000), batch_rows)
        max_wait_s = float(self.config.get("max_wait_s") or 5.0)
        offsets_in = dict(stream_offsets or {})

        consumer = Consumer(self._kafka_conf())
        try:
            meta = consumer.list_topics(topic=topic, timeout=5.0)
            t_meta = meta.topics.get(topic)
            part_ids = sorted(t_meta.partitions.keys()) if t_meta else [0]
            tps: list[Any] = []
            for pid in part_ids:
                p_key = str(pid)
                start_off = int(offsets_in[p_key]) if p_key in offsets_in else OFFSET_BEGINNING
                tps.append(TopicPartition(topic, pid, start_off))
            consumer.assign(tps)

            records: list[dict[str, Any]] = []
            new_offsets = dict(offsets_in)
            deadline = time.monotonic() + max_wait_s

            while len(records) < max_msgs and time.monotonic() < deadline:
                msg = consumer.poll(timeout=min(0.5, max(0.05, deadline - time.monotonic())))
                if msg is None:
                    if records:
                        break
                    continue
                if msg.error():
                    continue
                part = int(msg.partition() or 0)
                off = int(msg.offset() or 0)
                msg_id = f"{topic}:{part}:{off}"
                _ts_type, ts_ms = msg.timestamp()
                ts_dt = (
                    datetime.fromtimestamp(ts_ms / 1000.0, tz=UTC)
                    if ts_ms and ts_ms > 0
                    else datetime.now(UTC)
                )
                rec = parse_stream_payload(msg.value() or b"", msg_id, ts_dt)
                records.append(rec)
                new_offsets[str(part)] = str(off + 1)

            if not records:
                return
            tbl = records_to_arrow_table(records)
            for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
                yield ReadBatch(batch=rb, offsets=new_offsets)
        finally:
            consumer.close()


class MqttConnector(BaseConnector):
    """MQTT subscription sampling pull connector via paho-mqtt."""

    spec = MQTT_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        super().__init__(
            config, secrets, allow_private_network=allow_private_network, tenant_id=tenant_id
        )
        host = str(self.config.get("host") or "").strip()
        port = int(self.config.get("port") or 1883)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "MQTT config missing 'host'")
        check_outbound_host(host, port, allow_private_network=allow_private_network)

    def test(self) -> tuple[bool, str]:
        import paho.mqtt.client as mqtt

        try:
            cli = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2)  # type: ignore[attr-defined]
            if self.secrets.get("username") or self.config.get("username"):
                cli.username_pw_set(
                    str(self.secrets.get("username") or self.config.get("username") or ""),
                    self.secrets.get("password"),
                )
            cli.connect(str(self.config["host"]), int(self.config.get("port") or 1883), keepalive=5)
            cli.disconnect()
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        batches = list(self.read(ref, "APPEND", None, None, batch_rows=min(max(limit, 1), 100)))
        if not batches:
            return pa.table({})
        return pa.Table.from_batches([b.batch for b in batches])

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        import paho.mqtt.client as mqtt

        topic_filter = str(ref.get("topic_filter") or ref.get("topic") or "#")
        sample_s = min(float(self.config.get("sample_seconds") or 2.0), 30.0)
        max_msgs = min(int(self.config.get("max_messages") or 10_000), batch_rows)
        qos = int(self.config.get("qos") or 1)

        collected: list[dict[str, Any]] = []

        def _on_message(_client: Any, _userdata: Any, message: Any) -> None:
            if len(collected) >= max_msgs:
                return
            now_dt = datetime.now(UTC)
            payload_bytes = bytes(message.payload or b"")
            digest = hashlib.sha256(
                f"{message.topic}:{int(now_dt.timestamp() * 1000)}:".encode() + payload_bytes
            ).hexdigest()[:24]
            rec = parse_stream_payload(payload_bytes, f"mqtt:{digest}", now_dt)
            rec["topic"] = str(message.topic)
            collected.append(rec)

        cli = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2)  # type: ignore[attr-defined]
        cli.on_message = _on_message
        if self.secrets.get("username") or self.config.get("username"):
            cli.username_pw_set(
                str(self.secrets.get("username") or self.config.get("username") or ""),
                self.secrets.get("password"),
            )
        cli.connect(str(self.config["host"]), int(self.config.get("port") or 1883), keepalive=10)
        cli.subscribe(topic_filter, qos=qos)
        cli.loop_start()
        try:
            deadline = time.monotonic() + sample_s
            while time.monotonic() < deadline and len(collected) < max_msgs:
                time.sleep(0.05)
        finally:
            cli.loop_stop()
            cli.disconnect()

        if not collected:
            return
        tbl = records_to_arrow_table(collected)
        for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
            yield ReadBatch(batch=rb)


class PulsarConnector(BaseConnector):
    """Apache Pulsar micro-batch pull connector via pulsar-client."""

    spec = PULSAR_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        super().__init__(
            config, secrets, allow_private_network=allow_private_network, tenant_id=tenant_id
        )
        svc_url = str(self.config.get("service_url") or "").strip()
        if not svc_url:
            raise SidecarError(400, "INVALID_ARGUMENT", "Pulsar config missing 'service_url'")
        parsed = urlparse(svc_url)
        if not parsed.hostname:
            raise SidecarError(400, "INVALID_ARGUMENT", "Pulsar service_url missing hostname")
        check_outbound_host(
            parsed.hostname, parsed.port or 6650, allow_private_network=allow_private_network
        )

    def _create_client(self) -> Any:
        import pulsar

        kwargs: dict[str, Any] = {"operation_timeout_seconds": 5}
        if self.secrets.get("token"):
            kwargs["authentication"] = pulsar.AuthenticationToken(self.secrets["token"])
        return pulsar.Client(str(self.config["service_url"]), **kwargs)

    def test(self) -> tuple[bool, str]:
        try:
            cli = self._create_client()
            try:
                cli.get_topic_partitions("persistent://public/default/__axpoint_probe__")
            finally:
                cli.close()
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        batches = list(self.read(ref, "APPEND", None, None, batch_rows=min(max(limit, 1), 100)))
        if not batches:
            return pa.table({})
        return pa.Table.from_batches([b.batch for b in batches])

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        import pulsar

        topic = str(ref.get("topic") or "").strip()
        if not topic:
            raise SidecarError(400, "INVALID_ARGUMENT", "Pulsar ref missing 'topic'")

        cli = self._create_client()
        try:
            sub = str(self.config.get("subscription") or f"axhub-{self.tenant_id}")
            consumer = cli.subscribe(
                topic,
                subscription_name=sub,
                initial_position=pulsar.InitialPosition.Earliest,
            )
            records: list[dict[str, Any]] = []
            max_msgs = min(int(self.config.get("max_messages_per_run") or 50_000), batch_rows)
            max_wait_s = max(float(self.config.get("max_wait_s") or 5.0), 0.1)
            deadline = time.monotonic() + max_wait_s
            last_id_str = ""
            while len(records) < max_msgs and time.monotonic() < deadline:
                rem_ms = max(50, int((deadline - time.monotonic()) * 1000))
                try:
                    msg = consumer.receive(timeout_millis=min(rem_ms, 500))
                except Exception:
                    if records:
                        break
                    continue
                mid = str(msg.message_id())
                pub_ts = msg.publish_timestamp()
                ts_dt = (
                    datetime.fromtimestamp(pub_ts / 1000.0, tz=UTC)
                    if pub_ts and pub_ts > 0
                    else datetime.now(UTC)
                )
                records.append(parse_stream_payload(msg.data(), f"pulsar:{mid}", ts_dt))
                consumer.acknowledge(msg)
                last_id_str = mid
            consumer.close()
            if not records:
                return
            new_off = dict(stream_offsets or {})
            if last_id_str:
                new_off["0"] = last_id_str
            tbl = records_to_arrow_table(records)
            for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
                yield ReadBatch(batch=rb, offsets=new_off)
        finally:
            cli.close()
