//! Tenant-scoped transactions that activate PostgreSQL row level security.

use axpoint_domain::AppError;
use sqlx::{PgPool, Postgres, Transaction};

/// Begins a PostgreSQL transaction and sets `app.tenant_id` (`SET LOCAL` semantics)
/// so PostgreSQL Row-Level Security policies isolate tenant rows for the duration of the transaction.
///
/// # Errors
/// Returns [`AppError::Database`] if beginning the transaction or setting `app.tenant_id` fails.
pub async fn begin_tenant_tx<'a>(
    pool: &'a PgPool,
    tenant_id: &str,
) -> Result<Transaction<'a, Postgres>, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| AppError::Database(format!("begin tenant tx failed: {err}")))?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("set app.tenant_id failed: {err}")))?;
    Ok(tx)
}
