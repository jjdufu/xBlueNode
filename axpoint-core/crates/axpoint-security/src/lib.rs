//! `AxGuard` authentication, `EdDSA` `JWT` & `OIDC` sessions, `Cedar` `ABAC`, row/column policies,
//! dynamic `PII` masking, outbound `LLM` egress filtering, and tamper-evident audit chains.
#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]

pub mod audit;
pub mod audit_chain;
pub mod authenticator;
pub mod cedar_authz;
pub mod jwt;
pub mod keyring;
pub mod masking;
pub mod net_guard;
pub mod oidc;
pub mod password;
pub mod pii_local;
pub mod principal;
pub mod row_policy;
pub mod session;

pub use audit::AuditWriter;
pub use audit_chain::{
    AuditHashFields, compute_audit_row_hash, truncate_to_micros, verify_audit_chain,
};
pub use authenticator::{Authenticator, sha256_hex};
pub use cedar_authz::{CedarAuthorizer, CedarContext, dept_lineage};
pub use jwt::{AccessClaims, JwtKeyManager, generate_ed25519_keypair_pem};
pub use keyring::{CREDENTIAL_KEYS_ENV_VAR, CredentialKeyring, Keyring};
pub use masking::{
    MASK_KEYS_ENV_VAR, MaskKeyRing, apply_mask_rule, derive_tenant_hmac_key, mask_json_properties,
    mask_text_spans,
};
pub use net_guard::NetGuard;
pub use oidc::{OidcConfig, OidcRoleMapConfig, OidcService};
pub use password::{
    hash_password, needs_rehash, validate_password_policy, verify_dummy_password, verify_password,
};
pub use pii_local::{
    EgressSession, analyze_pii_local, preview_llm_egress, redact_for_egress, restore_from_egress,
};
pub use principal::{AuthPrincipal, TokenKind};
pub use row_policy::{
    CompiledPredicate, DatasetPolicyInput, NodePropertyMeta, NodeTypePolicyInput,
    compile_predicate, resolve_dataset_policy, resolve_node_type_policy, validate_predicate_syntax,
};
pub use session::{
    REFRESH_COOKIE_NAME, REFRESH_MAX_AGE_SECS, REFRESH_TTL_DAYS, build_clear_refresh_cookie,
    build_set_refresh_cookie, extract_refresh_cookie, generate_refresh_token, hash_refresh_token,
};

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    use axpoint_domain::AppError;
    use axpoint_domain::enums::{AuditAction, Permission, Sensitivity, TokenKind};
    use axpoint_domain::platform::{ApiTokenRecord, AuditRecord, PrincipalRecord};
    use axpoint_domain::ports::{AuditSink, CredentialStore};
    use chrono::Utc;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    use super::{AuditWriter, AuthPrincipal, Authenticator, sha256_hex};

    #[derive(Clone, Default)]
    struct MockCredentialStore {
        tokens: Arc<Mutex<HashMap<String, ApiTokenRecord>>>,
        principals: Arc<Mutex<HashMap<(String, Uuid), PrincipalRecord>>>,
        touched: Arc<Mutex<Vec<(String, Uuid)>>>,
    }

    impl CredentialStore for MockCredentialStore {
        async fn resolve_api_token(
            &self,
            token_hash: &str,
        ) -> Result<Option<ApiTokenRecord>, AppError> {
            let guard = self.tokens.lock().await;
            Ok(guard.get(token_hash).cloned())
        }

        async fn touch_api_token(&self, tenant_id: &str, token_id: Uuid) -> Result<(), AppError> {
            let mut guard = self.touched.lock().await;
            guard.push((String::from(tenant_id), token_id));
            Ok(())
        }

        async fn load_principal(
            &self,
            tenant_id: &str,
            user_id: Uuid,
        ) -> Result<Option<PrincipalRecord>, AppError> {
            let guard = self.principals.lock().await;
            Ok(guard.get(&(String::from(tenant_id), user_id)).cloned())
        }
    }

    #[derive(Clone, Default)]
    struct MockAuditSink {
        batches: Arc<Mutex<Vec<Vec<AuditRecord>>>>,
    }

    impl AuditSink for MockAuditSink {
        async fn write_batch(&self, records: Vec<AuditRecord>) -> Result<(), AppError> {
            let mut guard = self.batches.lock().await;
            guard.push(records);
            Ok(())
        }
    }

    #[tokio::test]
    async fn dev_token_rejected_in_production() -> Result<(), AppError> {
        let store = MockCredentialStore::default();
        let dev_token = String::from("dev_static_token_with_at_least_32_bytes_len");
        let res = Authenticator::new(store.clone(), "production", Some(dev_token.clone()));
        assert!(matches!(res, Err(AppError::Config(_))));

        let prod_auth = Authenticator::new(store.clone(), "production", None)?;
        let header = format!("Bearer {dev_token}");
        let auth_res = prod_auth.authenticate(Some(&header)).await;
        assert!(matches!(
            auth_res,
            Err(AppError::Unauthorized("TOKEN_INVALID"))
        ));

        let ok_empty = Authenticator::new(store, "production", Some(String::new()));
        assert!(ok_empty.is_ok());
        Ok(())
    }

    #[test]
    fn short_dev_token_rejected_in_non_production() {
        let store = MockCredentialStore::default();
        let res = Authenticator::new(store, "development", Some(String::from("too_short")));
        assert!(matches!(res, Err(AppError::Config(_))));
    }

    #[tokio::test]
    async fn dev_static_token_authenticates_and_enforces_permissions() -> Result<(), AppError> {
        let store = MockCredentialStore::default();
        let secret = String::from("0123456789abcdef0123456789abcdef_dev_tok");
        let auth = Authenticator::new(store, "test", Some(secret.clone()))?;

        let missing = auth.authenticate(None).await;
        assert!(matches!(
            missing,
            Err(AppError::Unauthorized("TOKEN_MISSING"))
        ));

        let non_bearer = auth.authenticate(Some("Basic abc")).await;
        assert!(matches!(
            non_bearer,
            Err(AppError::Unauthorized("TOKEN_MISSING"))
        ));

        let empty_bearer = auth.authenticate(Some("Bearer    ")).await;
        assert!(matches!(
            empty_bearer,
            Err(AppError::Unauthorized("TOKEN_MISSING"))
        ));

        let wrong = auth
            .authenticate(Some("Bearer wrong_token_value_that_does_not_match"))
            .await;
        assert!(matches!(
            wrong,
            Err(AppError::Unauthorized("TOKEN_INVALID"))
        ));

        let header = format!("Bearer {secret}");
        let principal: AuthPrincipal = auth.authenticate(Some(&header)).await?;
        assert_eq!(principal.user_id, Uuid::nil());
        assert_eq!(principal.tenant_id, "default");
        assert_eq!(principal.username, "dev");
        assert_eq!(principal.dept_id, "default");
        assert_eq!(principal.roles, vec![String::from("ADMIN")]);
        assert_eq!(principal.token_kind, TokenKind::DevStatic);
        principal.require(Permission::GuardAdmin)?;
        principal.require(Permission::DataRead)?;
        Ok(())
    }

    #[tokio::test]
    async fn api_token_resolves_principal_and_intersects_scopes() -> Result<(), AppError> {
        let store = MockCredentialStore::default();
        let user_id = Uuid::new_v4();
        let token_id = Uuid::new_v4();
        let raw_token = "axp_12345678_abcdefghijklmnopqrstuvwxyz012345";
        let hash = sha256_hex(raw_token);

        {
            let mut tokens = store.tokens.lock().await;
            tokens.insert(
                hash,
                ApiTokenRecord {
                    token_id,
                    tenant_id: String::from("t_acme"),
                    user_id,
                    scopes: BTreeSet::from([Permission::DataRead, Permission::GuardAdmin]),
                    token_profile: axpoint_domain::enums::TokenProfile::General,
                    web_base_url: None,
                },
            );
        }

        {
            let mut principals = store.principals.lock().await;
            principals.insert(
                (String::from("t_acme"), user_id),
                PrincipalRecord {
                    user_id,
                    tenant_id: String::from("t_acme"),
                    username: String::from("alice"),
                    display_name: String::from("Alice"),
                    dept_id: String::from("supply_chain"),
                    roles: vec![String::from("ANALYST")],
                    permissions: BTreeSet::from([Permission::DataRead, Permission::StudioEdit]),
                    clearance: Sensitivity::Internal,
                    tokens_valid_after: chrono::DateTime::UNIX_EPOCH,
                },
            );
        }

        let auth = Authenticator::new(store.clone(), "production", None)?;
        let header = format!("Bearer {raw_token}");
        let principal = auth.authenticate(Some(&header)).await?;

        assert_eq!(principal.user_id, user_id);
        assert_eq!(principal.tenant_id, "t_acme");
        assert_eq!(principal.username, "alice");
        assert_eq!(principal.token_kind, TokenKind::ApiToken);

        assert_eq!(
            principal.permissions,
            BTreeSet::from([Permission::DataRead])
        );
        principal.require(Permission::DataRead)?;
        assert!(matches!(
            principal.require(Permission::StudioEdit),
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));
        assert!(matches!(
            principal.require(Permission::GuardAdmin),
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));

        tokio::task::yield_now().await;
        let touched = store.touched.lock().await;
        assert_eq!(touched.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn audit_writer_flushes_on_threshold_and_explicit_flush() -> Result<(), AppError> {
        let sink = MockAuditSink::default();
        let writer = AuditWriter::new(sink.clone());

        for idx in 0..205_i32 {
            writer.record(AuditRecord {
                tenant_id: String::from("default"),
                user_id: None,
                username: String::from("dev"),
                action: AuditAction::QuerySql,
                resource_ref: format!("q:{idx}"),
                status_code: 200,
                client_ip: String::from("127.0.0.1"),
                trace_id: String::from("trace-1"),
                detail_json: serde_json::json!({"seq": idx}),
                created_at: Utc::now(),
            });
        }

        writer.flush().await?;

        let total: usize = {
            let guard = sink.batches.lock().await;
            assert!(guard.len() >= 2);
            guard.iter().map(Vec::len).sum()
        };
        assert_eq!(total, 205);
        Ok(())
    }
}
