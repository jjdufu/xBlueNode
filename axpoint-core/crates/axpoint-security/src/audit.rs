//! Buffered asynchronous security audit writer (`AuditWriter`).

use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::AuditSink;
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

/// Default bounded channel capacity for queued audit records.
const DEFAULT_CHANNEL_CAPACITY: usize = 8192;

/// Maximum buffered audit entries before an immediate batch flush is triggered.
const BATCH_FLUSH_THRESHOLD: usize = 200;

/// Periodic background flush interval when fewer than [`BATCH_FLUSH_THRESHOLD`] records arrive.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

enum AuditCommand {
    Record(AuditRecord),
    Flush(oneshot::Sender<Result<(), AppError>>),
}

/// Non-blocking buffered security audit writer flushing to an [`AuditSink`] every 200 records or 500ms.
#[derive(Debug, Clone)]
pub struct AuditWriter {
    tx: mpsc::Sender<AuditCommand>,
}

impl AuditWriter {
    /// Spawns a background Tokio flush worker backed by `sink` and returns a cloneable [`AuditWriter`].
    #[must_use]
    pub fn new<S>(sink: S) -> Self
    where
        S: AuditSink + 'static,
    {
        Self::with_capacity(sink, DEFAULT_CHANNEL_CAPACITY)
    }

    /// Spawns a background Tokio flush worker with a custom bounded channel capacity.
    #[must_use]
    pub fn with_capacity<S>(sink: S, capacity: usize) -> Self
    where
        S: AuditSink + 'static,
    {
        let bounded = capacity.max(1);
        let (tx, rx) = mpsc::channel(bounded);
        tokio::spawn(run_audit_worker(sink, rx));
        Self { tx }
    }

    /// Enqueues an [`AuditRecord`] without blocking the caller.
    ///
    /// Logs a warning if the audit channel is full or closed.
    pub fn record(&self, entry: AuditRecord) {
        if let Err(err) = self.tx.try_send(AuditCommand::Record(entry)) {
            tracing::warn!(error = %err, "audit writer channel saturated or closed");
        }
    }

    /// Enqueues an [`AuditRecord`] asynchronously, awaiting channel capacity if full.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the background audit worker has shut down.
    pub async fn record_async(&self, entry: AuditRecord) -> Result<(), AppError> {
        self.tx
            .send(AuditCommand::Record(entry))
            .await
            .map_err(|_| AppError::Database(String::from("audit writer worker closed")))
    }

    /// Flushes all currently queued audit records to the underlying [`AuditSink`].
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the worker has shut down or the sink batch write fails.
    pub async fn flush(&self) -> Result<(), AppError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(AuditCommand::Flush(ack_tx))
            .await
            .map_err(|_| AppError::Database(String::from("audit writer worker closed")))?;
        ack_rx
            .await
            .map_err(|_| AppError::Database(String::from("audit writer flush canceled")))?
    }
}

async fn run_audit_worker<S>(sink: S, mut rx: mpsc::Receiver<AuditCommand>)
where
    S: AuditSink,
{
    let mut buffer: Vec<AuditRecord> = Vec::with_capacity(BATCH_FLUSH_THRESHOLD);
    let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            maybe_cmd = rx.recv() => {
                match maybe_cmd {
                    Some(AuditCommand::Record(entry)) => {
                        buffer.push(entry);
                        if buffer.len() >= BATCH_FLUSH_THRESHOLD {
                            let batch = std::mem::replace(
                                &mut buffer,
                                Vec::with_capacity(BATCH_FLUSH_THRESHOLD),
                            );
                            if let Err(err) = sink.write_batch(batch).await {
                                tracing::error!(error = %err, "failed to write audit batch");
                            }
                        }
                    }
                    Some(AuditCommand::Flush(ack_tx)) => {
                        let mut waiters = vec![ack_tx];
                        while let Ok(pending) = rx.try_recv() {
                            match pending {
                                AuditCommand::Record(entry) => buffer.push(entry),
                                AuditCommand::Flush(other_ack) => waiters.push(other_ack),
                            }
                        }
                        let res = if buffer.is_empty() {
                            Ok(())
                        } else {
                            let batch = std::mem::replace(
                                &mut buffer,
                                Vec::with_capacity(BATCH_FLUSH_THRESHOLD),
                            );
                            sink.write_batch(batch).await
                        };
                        if let Err(ref err) = res {
                            tracing::error!(error = %err, "failed to flush audit batch");
                        }
                        for waiter in waiters {
                            let err_copy = match res {
                                Ok(()) => Ok(()),
                                Err(ref e) => Err(AppError::Database(e.to_string())),
                            };
                            if waiter.send(err_copy).is_err() {
                                tracing::debug!("audit flush waiter receiver dropped");
                            }
                        }
                    }
                    None => {
                        if !buffer.is_empty()
                            && let Err(err) = sink.write_batch(buffer).await
                        {
                            tracing::error!(error = %err, "failed to flush final audit batch on shutdown");
                        }
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                if !buffer.is_empty() {
                    let batch = std::mem::replace(
                        &mut buffer,
                        Vec::with_capacity(BATCH_FLUSH_THRESHOLD),
                    );
                    if let Err(err) = sink.write_batch(batch).await {
                        tracing::error!(error = %err, "failed periodic audit batch write");
                    }
                }
            }
        }
    }
}
