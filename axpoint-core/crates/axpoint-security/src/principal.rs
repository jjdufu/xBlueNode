//! Authenticated caller identity (`AuthPrincipal`) and permission enforcement.

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::enums::Permission;
pub use axpoint_domain::enums::{TokenKind, TokenProfile};
use axpoint_domain::platform::MeResponse;
use serde::Serialize;
use uuid::Uuid;

/// Authenticated caller identity extracted from `Authorization: Bearer <token>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// Personal access token profile (`GENERAL` or `BUILDER`).
    pub token_profile: TokenProfile,
    /// Optional explicit frontend base URL configured on a personal access token (`web_base_url`).
    pub web_base_url: Option<String>,
}

impl AuthPrincipal {
    /// Constructs the built-in non-production static development principal (`default` / `dev` / `ADMIN`).
    #[must_use]
    pub fn dev_static() -> Self {
        Self {
            user_id: Uuid::nil(),
            tenant_id: String::from("default"),
            username: String::from("dev"),
            dept_id: String::from("default"),
            roles: vec![String::from("ADMIN")],
            permissions: Permission::all_set(),
            token_kind: TokenKind::DevStatic,
            token_profile: TokenProfile::General,
            web_base_url: None,
        }
    }

    /// Returns `true` if this principal holds `permission`.
    #[must_use]
    pub fn has(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    /// Checks whether this principal holds the required [`Permission`].
    ///
    /// # Errors
    /// Returns `AppError::Forbidden("PERMISSION_DENIED")` if the permission is missing.
    pub fn require(&self, permission: Permission) -> Result<(), AppError> {
        if self.has(permission) {
            Ok(())
        } else {
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        }
    }

    /// Rejects production asset promotion or direct schema overwrites when authenticated with a `BUILDER` token.
    ///
    /// # Errors
    /// Returns [`AppError::Forbidden`] (`BUILDER_TOKEN_PROD_FORBIDDEN`) when `self.token_profile == TokenProfile::Builder`.
    pub fn enforce_no_prod_mutation(&self) -> Result<(), AppError> {
        if self.token_profile == TokenProfile::Builder {
            Err(AppError::Forbidden("BUILDER_TOKEN_PROD_FORBIDDEN"))
        } else {
            Ok(())
        }
    }

    /// Converts this principal into the `GET /api/auth/me` wire response DTO.
    #[must_use]
    pub fn to_me_response(&self) -> MeResponse {
        MeResponse {
            user_id: self.user_id,
            tenant_id: self.tenant_id.clone(),
            username: self.username.clone(),
            dept_id: self.dept_id.clone(),
            roles: self.roles.clone(),
            permissions: self.permissions.clone(),
            token_kind: self.token_kind,
            token_profile: self.token_profile,
        }
    }
}
