//! `AxGuard` authentication, principal verification, and buffered security audit writer.

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::enums::{Permission, TokenKind};
use serde::Serialize;
use uuid::Uuid;

/// Authenticated caller identity extracted from `Authorization: Bearer <token>`.
#[derive(Debug, Clone, Serialize)]
pub struct AuthPrincipal {
    /// User UUID (all-zero UUID for static development token).
    pub user_id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Username.
    pub username: String,
    /// Department identifier.
    pub dept_id: String,
    /// Assigned role names.
    pub roles: Vec<String>,
    /// Effective permission union.
    pub permissions: BTreeSet<Permission>,
    /// Credential type used to authenticate.
    pub token_kind: TokenKind,
}

impl AuthPrincipal {
    /// Checks whether this principal holds the required [`Permission`].
    ///
    /// # Errors
    /// Returns `AppError::Forbidden("PERMISSION_DENIED")` if the permission is missing.
    pub fn require(&self, permission: Permission) -> Result<(), AppError> {
        if self.permissions.contains(&permission) {
            Ok(())
        } else {
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        }
    }
}
