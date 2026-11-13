//! Personal access token and principal repository (`CredentialStore`) for [`PgStore`].

use std::collections::BTreeSet;
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::enums::Permission;
use axpoint_domain::platform::{ApiTokenRecord, PrincipalRecord};
use axpoint_domain::ports::CredentialStore;
use sqlx::Row;
use uuid::Uuid;

use crate::PgStore;

impl CredentialStore for PgStore {
    async fn resolve_api_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<ApiTokenRecord>, AppError> {
        let maybe_row = sqlx::query(
            "SELECT token_id, tenant_id, user_id, scopes
               FROM ax_resolve_api_token($1)",
        )
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("ax_resolve_api_token failed: {err}")))?;

        let Some(row) = maybe_row else {
            return Ok(None);
        };

        let raw_scopes: Vec<String> = row.get("scopes");
        let mut scopes = BTreeSet::new();
        for scope_str in raw_scopes {
            scopes.insert(Permission::from_str(&scope_str)?);
        }

        Ok(Some(ApiTokenRecord {
            token_id: row.get("token_id"),
            tenant_id: row.get("tenant_id"),
            user_id: row.get("user_id"),
            scopes,
        }))
    }

    async fn touch_api_token(&self, tenant_id: &str, token_id: Uuid) -> Result<(), AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        sqlx::query(
            "UPDATE sec_api_tokens
                SET last_used_at = NOW()
              WHERE id = $1",
        )
        .bind(token_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("touch sec_api_token failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit touch sec_api_token tx failed: {err}"))
        })?;
        Ok(())
    }

    async fn load_principal(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<Option<PrincipalRecord>, AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;

        let maybe_user = sqlx::query(
            "SELECT u.id, u.tenant_id, u.username, u.display_name, u.dept_id
               FROM sec_users u
               JOIN sec_tenants t ON t.tenant_id = u.tenant_id
              WHERE u.id = $1
                AND u.status = 'ACTIVE'
                AND t.status = 'ACTIVE'",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query sec_users failed: {err}")))?;

        let Some(user_row) = maybe_user else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit empty load_principal tx failed: {err}"))
            })?;
            return Ok(None);
        };

        let role_rows = sqlx::query(
            "SELECT r.name, r.permissions
               FROM sec_user_roles ur
               JOIN sec_roles r ON r.id = ur.role_id
              WHERE ur.user_id = $1
              ORDER BY r.name",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query sec_user_roles failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit load_principal tx failed: {err}")))?;

        let mut roles = Vec::with_capacity(role_rows.len());
        let mut permissions = BTreeSet::new();
        for role_row in role_rows {
            let role_name: String = role_row.get("name");
            let role_perms: Vec<String> = role_row.get("permissions");
            roles.push(role_name);
            for perm_str in role_perms {
                permissions.insert(Permission::from_str(&perm_str)?);
            }
        }

        Ok(Some(PrincipalRecord {
            user_id: user_row.get("id"),
            tenant_id: user_row.get("tenant_id"),
            username: user_row.get("username"),
            display_name: user_row.get("display_name"),
            dept_id: user_row.get("dept_id"),
            roles,
            permissions,
        }))
    }
}
