//! Delivery tracking work-item validation, lifecycle updates, and timeline comments (`§14.7`).

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::ops::{
    CreateWorkItemCommentRequest, CreateWorkItemRequest, PatchWorkItemRequest, WorkItem,
    WorkItemDetail, WorkItemEvent, WorkItemEventKind, WorkItemListQuery,
};
use axpoint_domain::ports::OpsRepository;
use uuid::Uuid;

use super::OpsServices;

/// Maximum character length permitted for `WorkItem.title`.
pub const MAX_WORK_ITEM_TITLE_CHARS: usize = 256;

/// Maximum character length permitted for `WorkItem.description`.
pub const MAX_WORK_ITEM_DESC_CHARS: usize = 20_000;

/// Maximum character length permitted for a `COMMENT` timeline event.
pub const MAX_WORK_ITEM_COMMENT_CHARS: usize = 10_000;

/// Validates and normalizes [`WorkItemListQuery`] pagination limits (`1..=200`, default `50`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`LIMIT_OUT_OF_RANGE`) if `limit` is outside `1..=200`.
pub fn validate_work_item_list_query(query: &mut WorkItemListQuery) -> Result<(), AppError> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=200, got {limit}"),
        ));
    }
    query.limit = Some(limit);
    Ok(())
}

/// Validates and trims a [`CreateWorkItemRequest`].
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `title` is empty or exceeds bounds.
pub fn validate_create_work_item(req: &mut CreateWorkItemRequest) -> Result<(), AppError> {
    let trimmed_title = req.title.trim().to_owned();
    let title_chars = trimmed_title.chars().count();
    if !(1..=MAX_WORK_ITEM_TITLE_CHARS).contains(&title_chars) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("work item title must be 1..={MAX_WORK_ITEM_TITLE_CHARS} characters"),
        ));
    }
    if req.description.chars().count() > MAX_WORK_ITEM_DESC_CHARS {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("work item description exceeds {MAX_WORK_ITEM_DESC_CHARS} characters"),
        ));
    }
    req.title = trimmed_title;
    req.source_ref = req.source_ref.trim().to_owned();
    Ok(())
}

/// Validates and trims a [`PatchWorkItemRequest`].
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `title` is empty, `description`
/// exceeds bounds, or both `assignee` and `clear_assignee` are specified simultaneously.
pub fn validate_patch_work_item(req: &mut PatchWorkItemRequest) -> Result<(), AppError> {
    if req.clear_assignee && req.assignee.is_some() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "cannot specify both `assignee` and `clear_assignee = true`",
        ));
    }
    if let Some(ref raw_title) = req.title {
        let trimmed = raw_title.trim().to_owned();
        let count = trimmed.chars().count();
        if !(1..=MAX_WORK_ITEM_TITLE_CHARS).contains(&count) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("work item title must be 1..={MAX_WORK_ITEM_TITLE_CHARS} characters"),
            ));
        }
        req.title = Some(trimmed);
    }
    if let Some(ref desc) = req.description
        && desc.chars().count() > MAX_WORK_ITEM_DESC_CHARS
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("work item description exceeds {MAX_WORK_ITEM_DESC_CHARS} characters"),
        ));
    }
    Ok(())
}

/// Validates and trims a [`CreateWorkItemCommentRequest`].
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `comment` is empty or exceeds `10_000` characters.
pub fn validate_work_item_comment(req: &mut CreateWorkItemCommentRequest) -> Result<(), AppError> {
    let trimmed = req.comment.trim().to_owned();
    let count = trimmed.chars().count();
    if !(1..=MAX_WORK_ITEM_COMMENT_CHARS).contains(&count) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("comment must be 1..={MAX_WORK_ITEM_COMMENT_CHARS} characters"),
        ));
    }
    req.comment = trimmed;
    Ok(())
}

impl<S, P> OpsServices<S, P>
where
    S: OpsRepository,
{
    /// Lists tenant work items (`GET /api/axops/work-items`).
    ///
    /// # Errors
    /// Returns `LIMIT_OUT_OF_RANGE`, `INVALID_ARGUMENT`, or database errors.
    pub async fn list_work_items(
        &self,
        tenant_id: &str,
        mut query: WorkItemListQuery,
    ) -> Result<PaginatedList<WorkItem>, AppError> {
        validate_work_item_list_query(&mut query)?;
        self.store.list_work_items(tenant_id, query).await
    }

    /// Loads a work item and its chronological timeline (`GET /api/axops/work-items/{id}`).
    ///
    /// # Errors
    /// Returns `WORK_ITEM_NOT_FOUND` or database errors.
    pub async fn get_work_item(
        &self,
        tenant_id: &str,
        item_id: Uuid,
    ) -> Result<WorkItemDetail, AppError> {
        self.store
            .get_work_item_detail(tenant_id, item_id)
            .await?
            .ok_or(AppError::NotFound("WORK_ITEM_NOT_FOUND"))
    }

    /// Creates a new work item (`POST /api/axops/work-items`, `§14.7`).
    ///
    /// # Errors
    /// Returns `INVALID_ARGUMENT` or database errors.
    pub async fn create_work_item(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        mut req: CreateWorkItemRequest,
    ) -> Result<WorkItem, AppError> {
        validate_create_work_item(&mut req)?;
        self.store
            .create_work_item(tenant_id, created_by, req)
            .await
    }

    /// Updates mutable fields and status of a work item (`PATCH /api/axops/work-items/{id}`, `§14.7`).
    ///
    /// # Errors
    /// Returns `INVALID_ARGUMENT`, `WORK_ITEM_NOT_FOUND`, or database errors.
    pub async fn patch_work_item(
        &self,
        tenant_id: &str,
        item_id: Uuid,
        actor: Option<Uuid>,
        mut req: PatchWorkItemRequest,
    ) -> Result<WorkItem, AppError> {
        validate_patch_work_item(&mut req)?;
        self.store
            .patch_work_item(tenant_id, item_id, actor, req)
            .await?
            .ok_or(AppError::NotFound("WORK_ITEM_NOT_FOUND"))
    }

    /// Appends a user `COMMENT` event to a work item (`POST /api/axops/work-items/{id}/comments`).
    ///
    /// # Errors
    /// Returns `INVALID_ARGUMENT`, `WORK_ITEM_NOT_FOUND`, or database errors.
    pub async fn add_work_item_comment(
        &self,
        tenant_id: &str,
        item_id: Uuid,
        actor: Option<Uuid>,
        mut req: CreateWorkItemCommentRequest,
    ) -> Result<WorkItemEvent, AppError> {
        validate_work_item_comment(&mut req)?;
        self.store
            .add_work_item_event(
                tenant_id,
                item_id,
                WorkItemEventKind::Comment,
                actor,
                serde_json::json!({ "comment": req.comment }),
            )
            .await?
            .ok_or(AppError::NotFound("WORK_ITEM_NOT_FOUND"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_work_item_validation_trims_and_rejects_empty_title() -> Result<(), AppError> {
        let mut empty_req = CreateWorkItemRequest {
            title: String::from("   "),
            description: String::new(),
            item_kind: None,
            priority: None,
            assignee: None,
            source_kind: None,
            source_ref: String::new(),
            node_ids: Vec::new(),
            due_at: None,
        };
        assert!(matches!(
            validate_create_work_item(&mut empty_req),
            Err(AppError::BadRequest {
                code: "INVALID_ARGUMENT",
                ..
            })
        ));

        let mut valid_req = CreateWorkItemRequest {
            title: String::from("  绑定供应链告警通知渠道  "),
            description: String::from("请配置企业微信或邮件通知"),
            item_kind: None,
            priority: None,
            assignee: None,
            source_kind: None,
            source_ref: String::from("  supply_chain "),
            node_ids: Vec::new(),
            due_at: None,
        };
        validate_create_work_item(&mut valid_req)?;
        assert_eq!(valid_req.title, "绑定供应链告警通知渠道");
        assert_eq!(valid_req.source_ref, "supply_chain");
        Ok(())
    }

    #[test]
    fn patch_and_comment_validation_enforce_constraints() -> Result<(), AppError> {
        let mut conflicting = PatchWorkItemRequest {
            assignee: Some(Uuid::new_v4()),
            clear_assignee: true,
            ..PatchWorkItemRequest::default()
        };
        assert!(matches!(
            validate_patch_work_item(&mut conflicting),
            Err(AppError::BadRequest {
                code: "INVALID_ARGUMENT",
                ..
            })
        ));

        let mut empty_title_patch = PatchWorkItemRequest {
            title: Some(String::from("  ")),
            ..PatchWorkItemRequest::default()
        };
        assert!(validate_patch_work_item(&mut empty_title_patch).is_err());

        let mut empty_comment = CreateWorkItemCommentRequest {
            comment: String::from("\n\t "),
        };
        assert!(validate_work_item_comment(&mut empty_comment).is_err());

        let mut ok_comment = CreateWorkItemCommentRequest {
            comment: String::from("  已处理完成  "),
        };
        validate_work_item_comment(&mut ok_comment)?;
        assert_eq!(ok_comment.comment, "已处理完成");

        let mut bad_query = WorkItemListQuery {
            limit: Some(0),
            ..WorkItemListQuery::default()
        };
        assert!(matches!(
            validate_work_item_list_query(&mut bad_query),
            Err(AppError::BadRequest {
                code: "LIMIT_OUT_OF_RANGE",
                ..
            })
        ));
        Ok(())
    }
}
