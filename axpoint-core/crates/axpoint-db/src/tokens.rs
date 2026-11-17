//! Credential and personal access token persistence (`CredentialStore` + `ApiTokenRepository`).

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::agent::{NewApiTokenInput, PersonalAccessTokenSummary};
use axpoint_domain::enums::Permission;
use axpoint_domain::platform::{ApiTokenRecord, PrincipalRecord};
use axpoint_domain::ports::{ApiTokenRepository, CredentialStore};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn parse_permission_list(raw_values: Vec<String>) -> Result<Vec<Permission>, AppError> {
    let mut out = Vec::with_capacity(raw_values.len());
    for raw in raw_values {
        let perm: Permission = raw.parse()?;
        out.push(perm);
    }
    Ok(out)
}

fn parse_permission_set(raw_values: Vec<String>) -> Result<BTreeSet<Permission>, AppError> {
    let mut set = BTreeSet::new();
    for raw in raw_values {
        let perm: Permission = raw.parse()?;
        set.insert(perm);
    }
    Ok(set)
}

fn decode_pat_summary_row(
    row: &sqlx::postgres::PgRow,
) -> Result<PersonalAccessTokenSummary, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|err| AppError::Database(format!("decode sec_api_tokens.id failed: {err}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|err| AppError::Database(format!("decode sec_api_tokens.name failed: {err}")))?;
    let token_prefix: String = row.try_get("token_prefix").map_err(|err| {
        AppError::Database(format!("decode sec_api_tokens.token_prefix failed: {err}"))
    })?;
    let raw_scopes: Vec<String> = row
        .try_get("scopes")
        .map_err(|err| AppError::Database(format!("decode sec_api_tokens.scopes failed: {err}")))?;
    let scopes = parse_permission_list(raw_scopes)?;
    let expires_at: Option<DateTime<Utc>> = row.try_get("expires_at").map_err(|err| {
        AppError::Database(format!("decode sec_api_tokens.expires_at failed: {err}"))
    })?;
    let last_used_at: Option<DateTime<Utc>> = row.try_get("last_used_at").map_err(|err| {
        AppError::Database(format!("decode sec_api_tokens.last_used_at failed: {err}"))
    })?;
    let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at").map_err(|err| {
        AppError::Database(format!("decode sec_api_tokens.revoked_at failed: {err}"))
    })?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(|err| {
        AppError::Database(format!("decode sec_api_tokens.created_at failed: {err}"))
    })?;

    Ok(PersonalAccessTokenSummary {
        id,
        name,
        token_prefix,
        scopes,
        expires_at,
        last_used_at,
        revoked_at,
        created_at,
    })
}

impl ApiTokenRepository for PgStore {
    async fn list_user_api_tokens(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<Vec<PersonalAccessTokenSummary>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, name, token_prefix, scopes, expires_at, last_used_at, revoked_at, created_at
               FROM sec_api_tokens
              WHERE user_id = $1
              ORDER BY created_at DESC, id DESC",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list sec_api_tokens failed: {err}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(decode_pat_summary_row(row)?);
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_user_api_tokens tx failed: {err}"))
        })?;
        Ok(out)
    }

    async fn create_api_token(
        &self,
        tenant_id: &str,
        input: NewApiTokenInput,
    ) -> Result<PersonalAccessTokenSummary, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let scope_strings: Vec<String> = input
            .scopes
            .iter()
            .map(|perm| String::from(perm.as_str()))
            .collect();

        let row = sqlx::query(
            "INSERT INTO sec_api_tokens
                (tenant_id, user_id, name, token_prefix, token_hash, scopes, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id, name, token_prefix, scopes, expires_at, last_used_at, revoked_at, created_at",
        )
        .bind(tenant_id)
        .bind(input.user_id)
        .bind(&input.name)
        .bind(&input.token_prefix)
        .bind(&input.token_hash)
        .bind(&scope_strings)
        .bind(input.expires_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert sec_api_tokens failed: {err}")))?;

        let summary = decode_pat_summary_row(&row)?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_api_token tx failed: {err}"))
        })?;
        Ok(summary)
    }

    async fn revoke_api_token(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        token_id: Uuid,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let res = sqlx::query(
            "UPDATE sec_api_tokens
                SET revoked_at = COALESCE(revoked_at, NOW())
              WHERE id = $1 AND user_id = $2",
        )
        .bind(token_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("revoke sec_api_tokens failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit revoke_api_token tx failed: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }
}

impl CredentialStore for PgStore {
    async fn resolve_api_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<ApiTokenRecord>, AppError> {
        let row_opt = sqlx::query(
            "SELECT token_id, tenant_id, user_id, scopes FROM ax_resolve_api_token($1)",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::Database(format!("ax_resolve_api_token failed: {err}")))?;

        let Some(row) = row_opt else {
            return Ok(None);
        };

        let token_id: Uuid = row
            .try_get("token_id")
            .map_err(|err| AppError::Database(format!("decode token_id failed: {err}")))?;
        let tenant_id: String = row
            .try_get("tenant_id")
            .map_err(|err| AppError::Database(format!("decode tenant_id failed: {err}")))?;
        let user_id: Uuid = row
            .try_get("user_id")
            .map_err(|err| AppError::Database(format!("decode user_id failed: {err}")))?;
        let raw_scopes: Vec<String> = row
            .try_get("scopes")
            .map_err(|err| AppError::Database(format!("decode scopes failed: {err}")))?;

        let scopes = parse_permission_set(raw_scopes)?;
        Ok(Some(ApiTokenRecord {
            token_id,
            tenant_id,
            user_id,
            scopes,
        }))
    }

    async fn load_principal(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<Option<PrincipalRecord>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;

        let user_opt = sqlx::query(
            "SELECT id, tenant_id, username, display_name, dept_id
               FROM sec_users
              WHERE id = $1 AND status = 'ACTIVE'",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load sec_users failed: {err}")))?;

        let Some(user_row) = user_opt else {
            tx.commit()
                .await
                .map_err(|err| AppError::Database(format!("commit principal lookup tx: {err}")))?;
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
        .map_err(|err| AppError::Database(format!("load sec_roles failed: {err}")))?;

        let mut roles = Vec::with_capacity(role_rows.len());
        let mut permissions = BTreeSet::new();
        for row in role_rows {
            let role_name: String = row
                .try_get("name")
                .map_err(|err| AppError::Database(format!("decode role name failed: {err}")))?;
            let raw_perms: Vec<String> = row.try_get("permissions").map_err(|err| {
                AppError::Database(format!("decode role permissions failed: {err}"))
            })?;
            roles.push(role_name);
            permissions.extend(parse_permission_set(raw_perms)?);
        }

        let username: String = user_row
            .try_get("username")
            .map_err(|err| AppError::Database(format!("decode username failed: {err}")))?;
        let display_name: String = user_row
            .try_get("display_name")
            .map_err(|err| AppError::Database(format!("decode display_name failed: {err}")))?;
        let dept_id: String = user_row
            .try_get("dept_id")
            .map_err(|err| AppError::Database(format!("decode dept_id failed: {err}")))?;
        let resolved_tenant: String = user_row
            .try_get("tenant_id")
            .map_err(|err| AppError::Database(format!("decode tenant_id failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit principal lookup tx: {err}")))?;

        Ok(Some(PrincipalRecord {
            user_id,
            tenant_id: resolved_tenant,
            username,
            display_name,
            dept_id,
            roles,
            permissions,
        }))
    }

    async fn touch_api_token(&self, tenant_id: &str, token_id: Uuid) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        sqlx::query("UPDATE sec_api_tokens SET last_used_at = NOW() WHERE id = $1")
            .bind(token_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("touch sec_api_tokens failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit touch_api_token tx: {err}")))?;
        Ok(())
    }
}
