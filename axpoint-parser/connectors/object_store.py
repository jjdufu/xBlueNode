from __future__ import annotations

import fnmatch
import io
from collections.abc import Iterator
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import polars as pl
import pyarrow as pa
import pyarrow.parquet as pq

from connectors.base import BaseConnector, ReadBatch
from parsers.engine import parse_unified
from schemas import ConnectorSpecModel, DiscoveredObject
from security import (
    SidecarError,
    check_outbound_host,
    check_outbound_url,
    resolve_path_under_root,
)
from settings import get_data_root

FILE_UPLOAD_SPEC = ConnectorSpecModel(
    connector_type="FILE_UPLOAD",
    display_name="本地文件上传",
    category="FILE",
    config_schema={"type": "object", "properties": {}},
    secret_fields=[],
    supports_modes=["SNAPSHOT"],
    supports_discover=False,
    image_tier="lite",
)

S3_SPEC = ConnectorSpecModel(
    connector_type="S3",
    display_name="S3 / MinIO / OSS / COS",
    category="FILE",
    config_schema={
        "type": "object",
        "required": ["bucket"],
        "properties": {
            "endpoint_url": {"type": "string"},
            "region": {"type": "string", "default": "us-east-1"},
            "bucket": {"type": "string"},
            "prefix": {"type": "string", "default": ""},
            "path_style": {"type": "boolean", "default": True},
        },
    },
    secret_fields=["access_key_id", "secret_access_key"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)

SFTP_SPEC = ConnectorSpecModel(
    connector_type="SFTP",
    display_name="SFTP 文件服务器",
    category="FILE",
    config_schema={
        "type": "object",
        "required": ["host", "root_path"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 22},
            "username": {"type": "string"},
            "root_path": {"type": "string", "default": "/"},
            "host_key_sha256": {"type": "string"},
        },
    },
    secret_fields=["password", "private_key"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)


def _bytes_to_arrow_via_parser(filename: str, content: bytes, tenant_id: str) -> pa.Table:
    suffix = Path(filename).suffix.lower()
    if suffix == ".parquet":
        return pq.read_table(io.BytesIO(content))
    res = parse_unified(filename, content, tenant_id=tenant_id)
    if not res.tables:
        return pa.table({})
    first_pq = resolve_path_under_root(res.tables[0].staging_path, data_root=get_data_root())
    return pq.read_table(first_pq)


class FileUploadConnector(BaseConnector):
    """Virtual connector card for direct browser file uploads (POST /api/axhub/uploads)."""

    spec = FILE_UPLOAD_SPEC

    def test(self) -> tuple[bool, str]:
        return True, "ready"

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        staging_path = str(ref.get("staging_path") or "").strip()
        if not staging_path:
            return pa.table({})
        pq_file = resolve_path_under_root(staging_path, data_root=get_data_root())
        tbl = pq.read_table(pq_file)
        return tbl.slice(0, min(max(limit, 1), 1000))

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        staging_path = str(ref.get("staging_path") or "").strip()
        if not staging_path:
            return
        pq_file = resolve_path_under_root(staging_path, data_root=get_data_root())
        tbl = pq.read_table(pq_file)
        for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
            yield ReadBatch(batch=rb)


class S3Connector(BaseConnector):
    """S3 / MinIO / OSS / COS object store connector via s3fs."""

    spec = S3_SPEC

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
        ep = str(self.config.get("endpoint_url") or "").strip()
        if ep:
            check_outbound_url(ep, allow_private_network=allow_private_network)
        self._fs: Any = None

    def _get_fs(self) -> Any:
        import s3fs

        if self._fs is None:
            client_kwargs: dict[str, Any] = {}
            if self.config.get("endpoint_url"):
                client_kwargs["endpoint_url"] = str(self.config["endpoint_url"])
            if self.config.get("region"):
                client_kwargs["region_name"] = str(self.config["region"])
            config_kwargs: dict[str, Any] = {}
            if self.config.get("path_style", True):
                config_kwargs["s3"] = {"addressing_style": "path"}
            self._fs = s3fs.S3FileSystem(
                key=self.secrets.get("access_key_id") or self.config.get("access_key_id"),
                secret=self.secrets.get("secret_access_key")
                or self.config.get("secret_access_key"),
                client_kwargs=client_kwargs,
                config_kwargs=config_kwargs,
            )
        return self._fs

    def test(self) -> tuple[bool, str]:
        try:
            bucket = str(self.config.get("bucket") or "")
            prefix = str(self.config.get("prefix") or "").lstrip("/")
            target = f"{bucket}/{prefix}".rstrip("/")
            self._get_fs().ls(target, detail=False)
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        fs = self._get_fs()
        bucket = str(self.config.get("bucket") or "")
        prefix = str(self.config.get("prefix") or "").lstrip("/")
        base = f"{bucket}/{prefix}".rstrip("/")
        entries = fs.find(base, detail=True)
        grouped: dict[str, int] = {}
        for path, info in entries.items():
            if info.get("type") == "directory":
                continue
            ext = Path(str(path)).suffix.lower()
            if ext:
                parent = str(Path(str(path)).parent)
                pattern = f"{parent}/*{ext}"
                grouped[pattern] = grouped.get(pattern, 0) + 1
        return [
            DiscoveredObject(
                ref={"glob": pat.removeprefix(f"{bucket}/")},
                display_name=pat.removeprefix(f"{bucket}/"),
                source_kind="FILE_TABLE",
                columns=[],
                estimated_rows=cnt,
            )
            for pat, cnt in sorted(grouped.items())
        ]

    def _iter_matching_files(
        self, ref: dict[str, Any], mode: str, cursor_from: str | None
    ) -> list[tuple[str, str]]:
        fs = self._get_fs()
        bucket = str(self.config.get("bucket") or "")
        glob_pat = str(ref.get("glob") or ref.get("key") or "*")
        entries = fs.find(bucket, detail=True)
        matched: list[tuple[str, str]] = []
        for full_key, info in entries.items():
            if info.get("type") == "directory":
                continue
            rel = str(full_key).removeprefix(f"{bucket}/")
            if not fnmatch.fnmatch(rel, glob_pat):
                continue
            lm = info.get("LastModified")
            mtime_iso = (
                lm.astimezone(UTC).isoformat().replace("+00:00", "Z")
                if isinstance(lm, datetime)
                else str(lm or "")
            )
            cursor_tok = f"{mtime_iso}|{rel}"
            if mode.upper() == "APPEND" and cursor_from and cursor_tok <= cursor_from:
                continue
            matched.append((str(full_key), cursor_tok))
        matched.sort(key=lambda item: item[1])
        return matched

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        files = self._iter_matching_files(ref, "SNAPSHOT", None)
        if not files:
            return pa.table({})
        fs = self._get_fs()
        full_key, _tok = files[0]
        with fs.open(full_key, "rb") as fh:
            raw = fh.read()
        tbl = _bytes_to_arrow_via_parser(Path(full_key).name, raw, self.tenant_id)
        return tbl.slice(0, min(max(limit, 1), 1000))

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        files = self._iter_matching_files(ref, mode, cursor_from)
        fs = self._get_fs()
        for full_key, cursor_tok in files:
            with fs.open(full_key, "rb") as fh:
                raw = fh.read()
            tbl = _bytes_to_arrow_via_parser(Path(full_key).name, raw, self.tenant_id)
            if tbl.num_rows == 0:
                continue
            pl_df = pl.from_arrow(tbl)
            assert isinstance(pl_df, pl.DataFrame)
            pl_df = pl_df.with_columns(pl.lit(full_key).alias("_source_file"))
            out_tbl = pl_df.to_arrow()
            for rb in out_tbl.to_batches(max_chunksize=max(batch_rows, 1)):
                yield ReadBatch(batch=rb, cursor_to=cursor_tok)


class SftpConnector(BaseConnector):
    """SFTP file server connector via paramiko."""

    spec = SFTP_SPEC

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
        port = int(self.config.get("port") or 22)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "SFTP config missing 'host'")
        check_outbound_host(host, port, allow_private_network=allow_private_network)

    def _open_sftp(self) -> tuple[Any, Any]:
        import hashlib as _hl

        import paramiko

        ssh = paramiko.SSHClient()
        expected_fp = str(self.config.get("host_key_sha256") or "").strip()
        if not expected_fp:
            ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        else:
            ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())

        pkey: Any = None
        if self.secrets.get("private_key"):
            pkey = paramiko.RSAKey.from_private_key(io.StringIO(self.secrets["private_key"]))

        ssh.connect(
            hostname=str(self.config["host"]),
            port=int(self.config.get("port") or 22),
            username=str(self.config.get("username") or "root"),
            password=self.secrets.get("password"),
            pkey=pkey,
            timeout=10.0,
            look_for_keys=False,
            allow_agent=False,
        )
        if expected_fp:
            transport = ssh.get_transport()
            remote_key = transport.get_remote_server_key() if transport else None
            if remote_key is not None:
                actual_sha = _hl.sha256(remote_key.asbytes()).hexdigest()
                if actual_sha.lower() != expected_fp.lower().removeprefix("sha256:"):
                    ssh.close()
                    raise SidecarError(
                        403, "PERMISSION_DENIED", "SFTP host key fingerprint mismatch"
                    )
        return ssh, ssh.open_sftp()

    def test(self) -> tuple[bool, str]:
        try:
            ssh, sftp = self._open_sftp()
            try:
                sftp.listdir(str(self.config.get("root_path") or "/"))
            finally:
                sftp.close()
                ssh.close()
            return True, "connected"
        except SidecarError:
            raise
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        ssh, sftp = self._open_sftp()
        try:
            root_p = str(self.config.get("root_path") or "/")
            names = sftp.listdir(root_p)
            return [
                DiscoveredObject(
                    ref={"glob": n},
                    display_name=n,
                    source_kind="FILE_TABLE",
                    columns=[],
                )
                for n in sorted(names)
            ]
        finally:
            sftp.close()
            ssh.close()

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        ssh, sftp = self._open_sftp()
        try:
            root_p = str(self.config.get("root_path") or "/").rstrip("/")
            pat = str(ref.get("glob") or "*")
            for attr in sftp.listdir_attr(root_p or "/"):
                if fnmatch.fnmatch(attr.filename, pat):
                    with sftp.open(f"{root_p}/{attr.filename}", "rb") as fh:
                        raw = fh.read()
                    tbl = _bytes_to_arrow_via_parser(attr.filename, raw, self.tenant_id)
                    return tbl.slice(0, min(max(limit, 1), 1000))
            return pa.table({})
        finally:
            sftp.close()
            ssh.close()

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        ssh, sftp = self._open_sftp()
        try:
            root_p = str(self.config.get("root_path") or "/").rstrip("/")
            pat = str(ref.get("glob") or "*")
            matched: list[tuple[str, str]] = []
            for attr in sftp.listdir_attr(root_p or "/"):
                if not fnmatch.fnmatch(attr.filename, pat):
                    continue
                mtime_iso = (
                    datetime.fromtimestamp(attr.st_mtime or 0, tz=UTC)
                    .isoformat()
                    .replace("+00:00", "Z")
                )
                tok = f"{mtime_iso}|{attr.filename}"
                if mode.upper() == "APPEND" and cursor_from and tok <= cursor_from:
                    continue
                matched.append((attr.filename, tok))
            matched.sort(key=lambda x: x[1])
            for fname, tok in matched:
                with sftp.open(f"{root_p}/{fname}", "rb") as fh:
                    raw = fh.read()
                tbl = _bytes_to_arrow_via_parser(fname, raw, self.tenant_id)
                for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
                    yield ReadBatch(batch=rb, cursor_to=tok)
        finally:
            sftp.close()
            ssh.close()
