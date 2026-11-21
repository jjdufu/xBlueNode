//! Tenant-scoped delivery tracking `ax_work_items` and `ax_work_item_events` (`§14.1`, `§14.7`).

use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::ops::{
    CreateWorkItemRequest, PatchWorkItemRequest, WorkItem, WorkItemDetail, WorkItemEvent,
    WorkItemEventKind, WorkItemKind, WorkItemListQuery, WorkItemPriority, WorkItemSourceKind,
    WorkItemStatus,
};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::{is_sqlstate, validate_page_limit};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn decode_work_item(row: &sqlx::postgres::PgRow) -> Result<WorkItem, AppError> {
    let item_no: i64 = row.get("item_no");
    let kind_raw: String = row.get("item_kind");
    let status_raw: String = row.get("status");
    let priority_raw: String = row.get("priority");
    let source_raw: String = row.get("source_kind");

    Ok(WorkItem {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        item_no,
        code: format!("WI-{item_no}"),
        title: row.get("title"),
        description: row.get("description"),
        item_kind: WorkItemKind::from_str(&kind_raw)?,
        status: WorkItemStatus::from_str(&status_raw)?,
        priority: WorkItemPriority::from_str(&priority_raw)?,
        assignee: row.get("assignee"),
        source_kind: WorkItemSourceKind::from_str(&source_raw)?,
        source_ref: row.get("source_ref"),
        node_ids: row.get("node_ids"),
        due_at: row.get("due_at"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        closed_at: row.get("closed_at"),
    })
}

fn decode_work_item_event(row: &sqlx::postgres::PgRow) -> Result<WorkItemEvent, AppError> {
    let kind_raw: String = row.get("event_kind");
    Ok(WorkItemEvent {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        work_item_id: row.get("work_item_id"),
        event_kind: WorkItemEventKind::from_str(&kind_raw)?,
        detail: row.get("detail"),
        actor: row.get("actor"),
        created_at: row.get("created_at"),
    })
}

async fn insert_work_item_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    work_item_id: Uuid,
    event_kind: WorkItemEventKind,
    actor: Option<Uuid>,
    detail: &serde_json::Value,
) -> Result<WorkItemEvent, AppError> {
    let row = sqlx::query(
        "INSERT INTO ax_work_item_events (tenant_id, work_item_id, event_kind, detail, actor)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, tenant_id, work_item_id, event_kind, detail, actor, created_at",
    )
    .bind(tenant_id)
    .bind(work_item_id)
    .bind(event_kind.as_str())
    .bind(detail)
    .bind(actor)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert ax_work_item_events failed: {err}")))?;

    decode_work_item_event(&row)
}

pub(super) async fn create_work_item_impl(
    store: &PgStore,
    tenant_id: &str,
    created_by: Option<Uuid>,
    req: &CreateWorkItemRequest,
) -> Result<WorkItem, AppError> {
    let title = req.title.trim();
    if title.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "work item title must not be empty",
        ));
    }

    let item_kind = req.item_kind.unwrap_or(WorkItemKind::Task);
    let priority = req.priority.unwrap_or(WorkItemPriority::P2);
    let source_kind = req.source_kind.unwrap_or(WorkItemSourceKind::Manual);

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('wi:' || $1))")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("acquire work item advisory lock failed: {err}"))
        })?;

    let next_no: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(item_no), 0) + 1 FROM ax_work_items WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("compute next work item_no failed: {err}")))?;

    let row = sqlx::query(
        "INSERT INTO ax_work_items (
             tenant_id, item_no, title, description, item_kind, status, priority,
             assignee, source_kind, source_ref, node_ids, due_at, created_by
         )
         VALUES ($1, $2, $3, $4, $5, 'OPEN', $6, $7, $8, $9, $10, $11, $12)
         RETURNING id, tenant_id, item_no, title, description, item_kind, status,
                   priority, assignee, source_kind, source_ref, node_ids, due_at,
                   created_by, created_at, updated_at, closed_at",
    )
    .bind(tenant_id)
    .bind(next_no)
    .bind(title)
    .bind(&req.description)
    .bind(item_kind.as_str())
    .bind(priority.as_str())
    .bind(req.assignee)
    .bind(source_kind.as_str())
    .bind(&req.source_ref)
    .bind(&req.node_ids)
    .bind(req.due_at)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_sqlstate(&err, "23503") {
            AppError::bad("INVALID_ARGUMENT", "assignee user does not exist")
        } else {
            AppError::Database(format!("insert ax_work_items failed: {err}"))
        }
    })?;

    let item = decode_work_item(&row)?;
    let created_detail = serde_json::json!({
        "title": title,
        "priority": priority.as_str(),
        "source_kind": source_kind.as_str(),
        "source_ref": &req.source_ref,
    });
    let _ev = insert_work_item_event_tx(
        &mut tx,
        tenant_id,
        item.id,
        WorkItemEventKind::Created,
        created_by,
        &created_detail,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_work_item tx failed: {err}")))?;

    Ok(item)
}

pub(super) async fn list_work_items_impl(
    store: &PgStore,
    tenant_id: &str,
    query: &WorkItemListQuery,
) -> Result<PaginatedList<WorkItem>, AppError> {
    let (limit_usize, fetch_limit) = validate_page_limit(query.limit)?;
    let cursor_no = match query
        .cursor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => Some(raw.parse::<i64>().map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid work item cursor: {err}"),
            )
        })?),
        None => None,
    };

    let status_filter = query.status.map(WorkItemStatus::as_str);
    let priority_filter = query.priority.map(WorkItemPriority::as_str);
    let source_kind_filter = query.source_kind.map(WorkItemSourceKind::as_str);
    let source_ref_filter = query
        .source_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT id, tenant_id, item_no, title, description, item_kind, status,
                priority, assignee, source_kind, source_ref, node_ids, due_at,
                created_by, created_at, updated_at, closed_at
           FROM ax_work_items
          WHERE tenant_id = $1
            AND ($2::text IS NULL OR status = $2)
            AND ($3::text IS NULL OR priority = $3)
            AND ($4::uuid IS NULL OR assignee = $4)
            AND ($5::text IS NULL OR source_kind = $5)
            AND ($6::text IS NULL OR source_ref = $6)
            AND ($7::bigint IS NULL OR item_no < $7)
          ORDER BY item_no DESC
          LIMIT $8",
    )
    .bind(tenant_id)
    .bind(status_filter)
    .bind(priority_filter)
    .bind(query.assignee)
    .bind(source_kind_filter)
    .bind(source_ref_filter)
    .bind(cursor_no)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list ax_work_items failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_work_items tx failed: {err}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(decode_work_item(row)?);
    }

    let next_cursor = if items.len() > limit_usize {
        items.truncate(limit_usize);
        items.last().map(|last| last.item_no.to_string())
    } else {
        None
    };

    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn get_work_item_detail_impl(
    store: &PgStore,
    tenant_id: &str,
    item_id: Uuid,
) -> Result<Option<WorkItemDetail>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let maybe_row = sqlx::query(
        "SELECT id, tenant_id, item_no, title, description, item_kind, status,
                priority, assignee, source_kind, source_ref, node_ids, due_at,
                created_by, created_at, updated_at, closed_at
           FROM ax_work_items
          WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(item_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("get ax_work_items failed: {err}")))?;

    let Some(item_row) = maybe_row else {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!(
                "commit empty get_work_item_detail tx failed: {err}"
            ))
        })?;
        return Ok(None);
    };

    let item = decode_work_item(&item_row)?;
    let ev_rows = sqlx::query(
        "SELECT id, tenant_id, work_item_id, event_kind, detail, actor, created_at
           FROM ax_work_item_events
          WHERE tenant_id = $1 AND work_item_id = $2
          ORDER BY id ASC",
    )
    .bind(tenant_id)
    .bind(item_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list ax_work_item_events failed: {err}")))?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit get_work_item_detail tx failed: {err}"))
    })?;

    let mut events = Vec::with_capacity(ev_rows.len());
    for row in &ev_rows {
        events.push(decode_work_item_event(row)?);
    }

    Ok(Some(WorkItemDetail { item, events }))
}

pub(super) async fn patch_work_item_impl(
    store: &PgStore,
    tenant_id: &str,
    item_id: Uuid,
    actor: Option<Uuid>,
    req: &PatchWorkItemRequest,
) -> Result<Option<WorkItem>, AppError> {
    if let Some(raw_title) = &req.title
        && raw_title.trim().is_empty()
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "work item title must not be empty",
        ));
    }

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let maybe_locked = sqlx::query(
        "SELECT id, tenant_id, item_no, title, description, item_kind, status,
                priority, assignee, source_kind, source_ref, node_ids, due_at,
                created_by, created_at, updated_at, closed_at
           FROM ax_work_items
          WHERE tenant_id = $1 AND id = $2
            FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(item_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("lock ax_work_items FOR UPDATE failed: {err}")))?;

    let Some(locked_row) = maybe_locked else {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit missing patch_work_item tx failed: {err}"))
        })?;
        return Ok(None);
    };

    let old_item = decode_work_item(&locked_row)?;
    let updated_item = apply_work_item_patch_tx(&mut tx, tenant_id, &old_item, actor, req).await?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit patch_work_item tx failed: {err}")))?;

    Ok(Some(updated_item))
}

async fn apply_work_item_patch_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    old_item: &WorkItem,
    actor: Option<Uuid>,
    req: &PatchWorkItemRequest,
) -> Result<WorkItem, AppError> {
    let new_title = req
        .title
        .as_deref()
        .map_or_else(|| old_item.title.clone(), |t| String::from(t.trim()));
    let new_description = req
        .description
        .clone()
        .unwrap_or_else(|| old_item.description.clone());
    let new_kind = req.item_kind.unwrap_or(old_item.item_kind);
    let new_status = req.status.unwrap_or(old_item.status);
    let new_priority = req.priority.unwrap_or(old_item.priority);
    let new_assignee = if req.clear_assignee {
        None
    } else if req.assignee.is_some() {
        req.assignee
    } else {
        old_item.assignee
    };
    let new_node_ids = req
        .node_ids
        .clone()
        .unwrap_or_else(|| old_item.node_ids.clone());
    let new_due_at = req.due_at.or(old_item.due_at);

    let updated_row = sqlx::query(
        "UPDATE ax_work_items
            SET title = $3,
                description = $4,
                item_kind = $5,
                status = $6,
                priority = $7,
                assignee = $8,
                node_ids = $9,
                due_at = $10,
                updated_at = NOW(),
                closed_at = CASE WHEN $11 THEN COALESCE(closed_at, NOW()) ELSE NULL END
          WHERE tenant_id = $1 AND id = $2
         RETURNING id, tenant_id, item_no, title, description, item_kind, status,
                   priority, assignee, source_kind, source_ref, node_ids, due_at,
                   created_by, created_at, updated_at, closed_at",
    )
    .bind(tenant_id)
    .bind(old_item.id)
    .bind(&new_title)
    .bind(&new_description)
    .bind(new_kind.as_str())
    .bind(new_status.as_str())
    .bind(new_priority.as_str())
    .bind(new_assignee)
    .bind(&new_node_ids)
    .bind(new_due_at)
    .bind(new_status.is_terminal())
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| {
        if is_sqlstate(&err, "23503") {
            AppError::bad("INVALID_ARGUMENT", "assignee user does not exist")
        } else {
            AppError::Database(format!("update ax_work_items failed: {err}"))
        }
    })?;

    record_patch_events_tx(tx, tenant_id, old_item, new_status, new_assignee, actor).await?;
    decode_work_item(&updated_row)
}

async fn record_patch_events_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    old_item: &WorkItem,
    new_status: WorkItemStatus,
    new_assignee: Option<Uuid>,
    actor: Option<Uuid>,
) -> Result<(), AppError> {
    if new_status != old_item.status {
        let status_detail = serde_json::json!({
            "from": old_item.status.as_str(),
            "to": new_status.as_str(),
        });
        let _ev = insert_work_item_event_tx(
            tx,
            tenant_id,
            old_item.id,
            WorkItemEventKind::Status,
            actor,
            &status_detail,
        )
        .await?;
    }

    if new_assignee != old_item.assignee {
        let assign_detail = serde_json::json!({
            "from": old_item.assignee,
            "to": new_assignee,
        });
        let _ev = insert_work_item_event_tx(
            tx,
            tenant_id,
            old_item.id,
            WorkItemEventKind::Assign,
            actor,
            &assign_detail,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn add_work_item_event_impl(
    store: &PgStore,
    tenant_id: &str,
    item_id: Uuid,
    event_kind: WorkItemEventKind,
    actor: Option<Uuid>,
    detail: &serde_json::Value,
) -> Result<Option<WorkItemEvent>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let touched: Option<Uuid> = sqlx::query_scalar(
        "UPDATE ax_work_items
            SET updated_at = NOW()
          WHERE tenant_id = $1 AND id = $2
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(item_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("touch ax_work_items.updated_at failed: {err}")))?;

    if touched.is_none() {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!(
                "commit missing add_work_item_event tx failed: {err}"
            ))
        })?;
        return Ok(None);
    }

    let ev =
        insert_work_item_event_tx(&mut tx, tenant_id, item_id, event_kind, actor, detail).await?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit add_work_item_event tx failed: {err}"))
    })?;

    Ok(Some(ev))
}

pub(super) async fn link_resolved_alert_work_items_impl(
    store: &PgStore,
    tenant_id: &str,
    alert_id: Uuid,
) -> Result<u64, AppError> {
    let alert_ref = alert_id.to_string();
    let link_detail = serde_json::json!({
        "event": "ALERT_RESOLVED",
        "alert_id": alert_id,
    });

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "UPDATE ax_work_items
            SET updated_at = NOW()
          WHERE tenant_id = $1
            AND source_kind = 'ALERT'
            AND source_ref = $2
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(&alert_ref)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| {
        AppError::Database(format!("find/touch alert-linked work items failed: {err}"))
    })?;

    for row in &rows {
        let wi_id: Uuid = row.get("id");
        let _ev = insert_work_item_event_tx(
            &mut tx,
            tenant_id,
            wi_id,
            WorkItemEventKind::Link,
            None,
            &link_detail,
        )
        .await?;
    }

    tx.commit().await.map_err(|err| {
        AppError::Database(format!(
            "commit link_resolved_alert_work_items tx failed: {err}"
        ))
    })?;

    Ok(u64::try_from(rows.len()).unwrap_or(u64::MAX))
}
