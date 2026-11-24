from __future__ import annotations

import asyncio
import time
from collections import deque
from collections.abc import AsyncIterator, Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any

import pyarrow as pa

from connectors.base import BaseConnector, ReadBatch, records_to_arrow_table

DEFAULT_MAX_BATCH_ROWS = 2_000
DEFAULT_FLUSH_WINDOW_MS = 1_000


@dataclass
class StreamMicroBatch:
    """Flushed micro-batch from an adaptive continuous stream window."""

    records: list[dict[str, Any]]
    offsets: dict[str, str]
    flushed_reason: str


@dataclass
class AdaptiveStreamBuffer:
    """Bounded in-memory ring/micro-batch buffer flushing at 2,000 rows or 1,000ms."""

    max_batch_rows: int = DEFAULT_MAX_BATCH_ROWS
    flush_window_ms: int = DEFAULT_FLUSH_WINDOW_MS
    initial_offsets: dict[str, str] = field(default_factory=dict)
    _ring: deque[dict[str, Any]] = field(init=False, repr=False)
    _offsets: dict[str, str] = field(init=False, repr=False)
    _window_started_at: float = field(init=False, repr=False)

    def __post_init__(self) -> None:
        cap = max(self.max_batch_rows, 1)
        self.max_batch_rows = cap
        self.flush_window_ms = max(self.flush_window_ms, 50)
        self._ring = deque(maxlen=cap * 4)
        self._offsets = dict(self.initial_offsets)
        self._window_started_at = time.monotonic()

    @property
    def buffered_count(self) -> int:
        """Number of records currently buffered in memory."""
        return len(self._ring)

    @property
    def offsets(self) -> dict[str, str]:
        """Latest tracked stream partition offsets."""
        return dict(self._offsets)

    def push(
        self,
        record: dict[str, Any],
        offset_updates: dict[str, str] | None = None,
        *,
        now_monotonic: float | None = None,
    ) -> bool:
        """Append a record and return True if the buffer reached flush capacity or window."""
        now = time.monotonic() if now_monotonic is None else now_monotonic
        if not self._ring:
            self._window_started_at = now
        self._ring.append(record)
        if offset_updates:
            self._offsets.update(offset_updates)
        return self.should_flush(now_monotonic=now)

    def should_flush(self, *, now_monotonic: float | None = None) -> bool:
        """Return True when buffered rows reach max_batch_rows or flush_window_ms elapsed."""
        if not self._ring:
            return False
        if len(self._ring) >= self.max_batch_rows:
            return True
        now = time.monotonic() if now_monotonic is None else now_monotonic
        elapsed_ms = (now - self._window_started_at) * 1000.0
        return elapsed_ms >= float(self.flush_window_ms)

    def drain(self, *, now_monotonic: float | None = None) -> StreamMicroBatch | None:
        """Pop up to max_batch_rows buffered records into a StreamMicroBatch."""
        if not self._ring:
            return None
        now = time.monotonic() if now_monotonic is None else now_monotonic
        reason = "row_threshold" if len(self._ring) >= self.max_batch_rows else "window_elapsed"
        take_n = min(len(self._ring), self.max_batch_rows)
        items = [self._ring.popleft() for _ in range(take_n)]
        self._window_started_at = now
        return StreamMicroBatch(
            records=items,
            offsets=dict(self._offsets),
            flushed_reason=reason,
        )


def drain_stream_microbatch(
    connector: BaseConnector,
    ref: dict[str, Any],
    stream_offsets: dict[str, str],
    *,
    max_batch_rows: int = DEFAULT_MAX_BATCH_ROWS,
    flush_window_ms: int = DEFAULT_FLUSH_WINDOW_MS,
) -> list[ReadBatch]:
    """Pull a sub-second micro-batch (<= 2,000 rows or <= 1,000ms) from a stream connector."""
    capped_rows = min(max(max_batch_rows, 1), DEFAULT_MAX_BATCH_ROWS)
    window_s = max(min(flush_window_ms / 1000.0, 2.0), 0.1)

    orig_config = dict(connector.config)
    connector.config["max_messages_per_run"] = capped_rows
    connector.config["max_messages"] = capped_rows
    connector.config["max_wait_s"] = window_s
    connector.config["sample_seconds"] = min(
        float(orig_config.get("sample_seconds") or window_s), window_s
    )

    buf = AdaptiveStreamBuffer(
        max_batch_rows=capped_rows,
        flush_window_ms=flush_window_ms,
        initial_offsets=stream_offsets,
    )
    flushed_batches: list[ReadBatch] = []

    try:
        for rb in connector.read(
            ref,
            "APPEND",
            None,
            None,
            batch_rows=capped_rows,
            stream_offsets=stream_offsets,
        ):
            rows = rb.batch.to_pylist()
            for row in rows:
                ready = buf.push(dict(row), rb.offsets)
                if ready:
                    flushed = buf.drain()
                    if flushed and flushed.records:
                        tbl = records_to_arrow_table(flushed.records)
                        for arrow_rb in tbl.to_batches(max_chunksize=capped_rows):
                            flushed_batches.append(
                                ReadBatch(batch=arrow_rb, offsets=flushed.offsets)
                            )
            total_flushed = sum(b.batch.num_rows for b in flushed_batches)
            if flushed_batches and total_flushed >= capped_rows:
                break

        tail = buf.drain()
        if tail and tail.records:
            tbl = records_to_arrow_table(tail.records)
            for arrow_rb in tbl.to_batches(max_chunksize=capped_rows):
                flushed_batches.append(ReadBatch(batch=arrow_rb, offsets=tail.offsets))
    finally:
        connector.config = orig_config

    return flushed_batches


async def run_stream_microbatch_loop(
    source: AsyncIterator[tuple[dict[str, Any], dict[str, str]]],
    on_flush: Callable[[pa.Table, dict[str, str], str], Awaitable[None]],
    *,
    max_batch_rows: int = DEFAULT_MAX_BATCH_ROWS,
    flush_window_ms: int = DEFAULT_FLUSH_WINDOW_MS,
    initial_offsets: dict[str, str] | None = None,
) -> dict[str, str]:
    """Consume an async event stream and flush Arrow tables on 2,000-row or 1,000ms triggers."""
    buf = AdaptiveStreamBuffer(
        max_batch_rows=max_batch_rows,
        flush_window_ms=flush_window_ms,
        initial_offsets=dict(initial_offsets or {}),
    )
    timeout_s = max(flush_window_ms / 1000.0, 0.05)
    ait = source.__aiter__()
    exhausted = False

    while not exhausted:
        try:
            rec, off = await asyncio.wait_for(ait.__anext__(), timeout=timeout_s)
            if buf.push(rec, off):
                mb = buf.drain()
                if mb and mb.records:
                    await on_flush(
                        records_to_arrow_table(mb.records),
                        mb.offsets,
                        mb.flushed_reason,
                    )
        except TimeoutError:
            if buf.should_flush():
                mb = buf.drain()
                if mb and mb.records:
                    await on_flush(
                        records_to_arrow_table(mb.records),
                        mb.offsets,
                        mb.flushed_reason,
                    )
        except StopAsyncIteration:
            exhausted = True

    tail = buf.drain()
    if tail and tail.records:
        await on_flush(
            records_to_arrow_table(tail.records),
            tail.offsets,
            tail.flushed_reason,
        )
    return buf.offsets
