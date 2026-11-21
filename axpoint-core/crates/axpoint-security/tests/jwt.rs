//! Integration tests for `EdDSA` access `JWT` signing, verification, leeway, algorithm confusion
//! rejection, live key rotation, `JWKS`, password hashing, refresh cookies, `OIDC` redirect
//! validation, and `Authenticator` Branch 4 (`11-STEP13` §13.3–§13.5).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axpoint_domain::AppError;
use axpoint_domain::enums::{Permission, Sensitivity, TokenKind};
use axpoint_domain::guard::{GuardPrincipalProfile, UserAccountStatus};
use axpoint_domain::platform::{ApiTokenRecord, PrincipalRecord};
use axpoint_domain::ports::CredentialStore;
use axpoint_security::jwt::{
    AccessClaims, JWT_AUDIENCE, JWT_ISSUER, JwtKeyManager, generate_ed25519_keypair_pem,
};
use axpoint_security::oidc::OidcService;
use axpoint_security::password::{
    hash_password, needs_rehash, validate_password_policy, verify_dummy_password, verify_password,
};
use axpoint_security::session::{
    REFRESH_COOKIE_NAME, REFRESH_MAX_AGE_SECS, build_clear_refresh_cookie,
    build_set_refresh_cookie, extract_refresh_cookie, generate_refresh_token, hash_refresh_token,
};
use axpoint_security::{Authenticator, sha256_hex};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use tokio::sync::Mutex;
use uuid::Uuid;

fn sample_profile(user_id: Uuid) -> GuardPrincipalProfile {
    GuardPrincipalProfile {
        user_id,
        tenant_id: String::from("default"),
        username: String::from("analyst_east"),
        display_name: String::from("East Analyst"),
        email: String::from("east@axpoint.io"),
        dept_id: String::from("east"),
        clearance: Sensitivity::Confidential,
        status: UserAccountStatus::Active,
        tokens_valid_after: chrono::DateTime::UNIX_EPOCH,
        roles: vec![String::from("ANALYST")],
        permissions: BTreeSet::from([Permission::DataRead]),
        attributes: BTreeMap::new(),
    }
}

fn temp_jwt_dir(label: &str) -> Result<PathBuf, AppError> {
    let dir = std::env::temp_dir().join(format!("axpoint-jwt-{label}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("public"))
        .map_err(|err| AppError::Config(format!("mkdir failed: {err}")))?;
    Ok(dir)
}

#[test]
fn password_policy_and_argon2id_hashing_round_trip() -> Result<(), AppError> {
    assert!(validate_password_policy("alice", "short123").is_err());
    assert!(validate_password_policy("alice", "MyPasswordForAlice2026!").is_err());
    validate_password_policy("alice", "Axp#Strong_2026!99")?;

    let phc = hash_password("Axp#Strong_2026!99")?;
    assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(verify_password("Axp#Strong_2026!99", &phc));
    assert!(!verify_password("wrong-password-attempt", &phc));
    assert!(!needs_rehash(&phc));

    let weaker = "$argon2id$v=19$m=4096,t=1,p=1$c29tZXNhbHQxMjM0NTY3OA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    assert!(needs_rehash(weaker));
    verify_dummy_password("probe-password-12345");
    Ok(())
}

#[test]
fn refresh_token_and_cookie_helpers_enforce_attributes() {
    let (raw, hash) = generate_refresh_token();
    assert!(raw.starts_with("axr_"));
    assert_eq!(raw.len(), 36);
    assert_eq!(hash, hash_refresh_token(&raw));
    assert_eq!(hash, sha256_hex(&raw));

    let set_prod = build_set_refresh_cookie(&raw, REFRESH_MAX_AGE_SECS, true);
    assert!(set_prod.contains(&format!("{REFRESH_COOKIE_NAME}={raw}")));
    assert!(set_prod.contains("HttpOnly"));
    assert!(set_prod.contains("SameSite=Strict"));
    assert!(set_prod.contains("Path=/api/auth"));
    assert!(set_prod.contains("Secure"));

    let set_dev = build_set_refresh_cookie(&raw, REFRESH_MAX_AGE_SECS, false);
    assert!(!set_dev.contains("Secure"));

    let cookie_hdr = format!("theme=dark; {REFRESH_COOKIE_NAME}={raw}; lang=zh");
    assert_eq!(extract_refresh_cookie(Some(&cookie_hdr)), Some(raw));

    let cleared = build_clear_refresh_cookie(true);
    assert!(cleared.contains("Max-Age=0"));
}

#[test]
fn oidc_return_to_rejects_open_redirects() -> Result<(), AppError> {
    assert_eq!(OidcService::validate_return_to(None)?, "/");
    assert_eq!(
        OidcService::validate_return_to(Some("/guard?tab=users"))?,
        "/guard?tab=users"
    );
    assert!(OidcService::validate_return_to(Some("https://evil.example")).is_err());
    assert!(OidcService::validate_return_to(Some("//evil.example/path")).is_err());
    assert!(OidcService::validate_return_to(Some("/\\evil.example")).is_err());
    Ok(())
}

#[test]
fn jwt_sign_verify_rotation_and_jwks_publication() -> Result<(), AppError> {
    let dir = temp_jwt_dir("rotation")?;
    let pub_dir = dir.join("public");
    let active_priv = dir.join("private.pem");

    let (priv_k1, pub_k1) = generate_ed25519_keypair_pem()?;
    std::fs::write(&active_priv, &priv_k1).map_err(|e| AppError::Config(e.to_string()))?;
    std::fs::write(pub_dir.join("k1.pem"), &pub_k1).map_err(|e| AppError::Config(e.to_string()))?;

    let mgr = JwtKeyManager::new(Some(active_priv.clone()), Some(pub_dir.clone()));
    let uid = Uuid::new_v4();
    let profile = sample_profile(uid);

    let tok_k1 = mgr.sign_access_token(&profile)?;
    assert_eq!(tok_k1.token_type, "Bearer");
    assert_eq!(tok_k1.expires_in, 900);

    let claims_k1 = mgr.verify_access_token(&tok_k1.access_token)?;
    assert_eq!(claims_k1.sub, uid.to_string());
    assert_eq!(claims_k1.tid, "default");
    assert_eq!(claims_k1.dept, "east");
    assert_eq!(claims_k1.clr, Sensitivity::Confidential);

    // Rotate to k2 while retaining k1 in public_keys_dir
    std::thread::sleep(std::time::Duration::from_millis(15));
    let (priv_k2, pub_k2) = generate_ed25519_keypair_pem()?;
    std::fs::write(pub_dir.join("k2.pem"), &pub_k2).map_err(|e| AppError::Config(e.to_string()))?;
    std::fs::write(&active_priv, &priv_k2).map_err(|e| AppError::Config(e.to_string()))?;

    let tok_k2 = mgr.sign_access_token(&profile)?;
    // Both k1 and k2 tokens verify cleanly during overlap window!
    let _verified_old = mgr.verify_access_token(&tok_k1.access_token)?;
    let _verified_new = mgr.verify_access_token(&tok_k2.access_token)?;

    let jwks = mgr.jwks_json()?;
    let keys_arr = jwks
        .get("keys")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| AppError::Config(String::from("missing JWKS keys array")))?;
    assert_eq!(keys_arr.len(), 2);

    std::fs::remove_dir_all(&dir).map_err(|e| AppError::Config(e.to_string()))?;
    Ok(())
}

#[test]
fn jwt_rejects_expired_leeway_boundary_and_alg_confusion() -> Result<(), AppError> {
    let dir = temp_jwt_dir("security")?;
    let pub_dir = dir.join("public");
    let active_priv = dir.join("private.pem");

    let (priv_pem, pub_pem) = generate_ed25519_keypair_pem()?;
    std::fs::write(&active_priv, &priv_pem).map_err(|e| AppError::Config(e.to_string()))?;
    std::fs::write(pub_dir.join("k1.pem"), &pub_pem)
        .map_err(|e| AppError::Config(e.to_string()))?;

    let mgr = JwtKeyManager::new(Some(active_priv), Some(pub_dir));
    let enc_key = EncodingKey::from_ed_pem(priv_pem.as_bytes())
        .map_err(|e| AppError::Config(e.to_string()))?;
    let now = Utc::now().timestamp();

    // 1. Within 30s leeway (`exp = now - 15`) -> accepted
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some(String::from("k1"));
    let leeway_claims = AccessClaims {
        iss: String::from(JWT_ISSUER),
        aud: String::from(JWT_AUDIENCE),
        sub: Uuid::new_v4().to_string(),
        tid: String::from("default"),
        unm: String::from("east"),
        dept: String::from("east"),
        clr: Sensitivity::Internal,
        roles: vec![String::from("VIEWER")],
        jti: Uuid::new_v4().to_string(),
        iat: now - 915,
        exp: now - 15,
    };
    let leeway_tok =
        encode(&header, &leeway_claims, &enc_key).map_err(|e| AppError::Config(e.to_string()))?;
    assert!(mgr.verify_access_token(&leeway_tok).is_ok());

    // 2. Beyond 30s leeway (`exp = now - 60`) -> TOKEN_EXPIRED
    let mut expired_claims = leeway_claims.clone();
    expired_claims.exp = now - 60;
    let expired_tok =
        encode(&header, &expired_claims, &enc_key).map_err(|e| AppError::Config(e.to_string()))?;
    assert!(matches!(
        mgr.verify_access_token(&expired_tok),
        Err(AppError::Unauthorized("TOKEN_EXPIRED"))
    ));

    // 3. HS256 algorithm confusion -> TOKEN_INVALID
    let hs_tok = encode(
        &Header::new(Algorithm::HS256),
        &leeway_claims,
        &EncodingKey::from_secret(pub_pem.as_bytes()),
    )
    .map_err(|e| AppError::Config(e.to_string()))?;
    assert!(matches!(
        mgr.verify_access_token(&hs_tok),
        Err(AppError::Unauthorized("TOKEN_INVALID"))
    ));

    // 4. `alg: "none"` unsecured token -> TOKEN_INVALID
    let none_hdr = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload_b64 = URL_SAFE_NO_PAD.encode(
        serde_json::to_string(&leeway_claims).map_err(|e| AppError::Config(e.to_string()))?,
    );
    let none_tok = format!("{none_hdr}.{payload_b64}.");
    assert!(matches!(
        mgr.verify_access_token(&none_tok),
        Err(AppError::Unauthorized("TOKEN_INVALID"))
    ));

    std::fs::remove_dir_all(&dir).map_err(|e| AppError::Config(e.to_string()))?;
    Ok(())
}

#[derive(Clone, Default)]
struct CountingStore {
    principals: Arc<Mutex<HashMap<(String, Uuid), PrincipalRecord>>>,
    loads: Arc<AtomicUsize>,
}

impl CredentialStore for CountingStore {
    fn resolve_api_token(
        &self,
        _token_hash: &str,
    ) -> impl Future<Output = Result<Option<ApiTokenRecord>, AppError>> + Send {
        std::future::ready(Ok(None))
    }

    fn touch_api_token(
        &self,
        _tenant_id: &str,
        _token_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> + Send {
        std::future::ready(Ok(()))
    }

    async fn load_principal(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<Option<PrincipalRecord>, AppError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let guard = self.principals.lock().await;
        Ok(guard.get(&(String::from(tenant_id), user_id)).cloned())
    }
}

#[tokio::test]
async fn authenticator_branch4_caches_principal_and_enforces_tokens_valid_after()
-> Result<(), AppError> {
    let dir = temp_jwt_dir("auth-branch4")?;
    let pub_dir = dir.join("public");
    let active_priv = dir.join("private.pem");
    let (priv_pem, pub_pem) = generate_ed25519_keypair_pem()?;
    std::fs::write(&active_priv, &priv_pem).map_err(|e| AppError::Config(e.to_string()))?;
    std::fs::write(pub_dir.join("k1.pem"), &pub_pem)
        .map_err(|e| AppError::Config(e.to_string()))?;

    let jwt = Arc::new(JwtKeyManager::new(Some(active_priv), Some(pub_dir)));
    let store = CountingStore::default();
    let user_id = Uuid::new_v4();
    {
        let mut guard = store.principals.lock().await;
        guard.insert(
            (String::from("default"), user_id),
            PrincipalRecord {
                user_id,
                tenant_id: String::from("default"),
                username: String::from("analyst_east"),
                display_name: String::from("East"),
                dept_id: String::from("east"),
                roles: vec![String::from("ANALYST")],
                permissions: BTreeSet::from([Permission::DataRead]),
                clearance: Sensitivity::Confidential,
                tokens_valid_after: chrono::DateTime::UNIX_EPOCH,
            },
        );
    }

    let auth = Authenticator::new(store.clone(), "production", None)?.with_jwt(Arc::clone(&jwt));
    let signed = jwt.sign_access_token(&sample_profile(user_id))?;
    let bearer = format!("Bearer {}", signed.access_token);

    // First call hits store; second call hits 30s moka cache!
    let p1 = auth.authenticate(Some(&bearer)).await?;
    let p2 = auth.authenticate(Some(&bearer)).await?;
    assert_eq!(p1.token_kind, TokenKind::Access);
    assert_eq!(p2.user_id, user_id);
    assert_eq!(store.loads.load(Ordering::SeqCst), 1);

    // Bump tokens_valid_after into the future and invalidate user cache -> immediate revocation!
    {
        let mut guard = store.principals.lock().await;
        if let Some(rec) = guard.get_mut(&(String::from("default"), user_id)) {
            rec.tokens_valid_after = Utc::now() + Duration::seconds(60);
        }
    }
    auth.invalidate_user("default", user_id);
    let revoked_res = auth.authenticate(Some(&bearer)).await;
    assert!(matches!(
        revoked_res,
        Err(AppError::Unauthorized("TOKEN_INVALID"))
    ));

    std::fs::remove_dir_all(&dir).map_err(|e| AppError::Config(e.to_string()))?;
    Ok(())
}
