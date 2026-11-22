//! Live `PostgreSQL` + Row-Level Security (`RLS`) integration tests for `axpoint-db::im_repo` (`07-STEP21`).

use std::collections::BTreeSet;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::agent::{AgentSpec, CreateAgentRequest, NewConversationInput};
use axpoint_domain::axhub::SealedCredential;
use axpoint_domain::enums::{
    ConversationChannel, ConversationStatus, ImConnectorStatus, ImIdentityPolicy, ImPlatform,
    Permission, Sensitivity,
};
use axpoint_domain::im::{
    ImConnectorConfig, NewImBindChallengeInput, NewImConnectorInput, UpdateImConnectorInput,
    hash_im_bind_token,
};
use axpoint_domain::ports::{
    AgentRepository as _, ConnectionRepository as _, ImGatewayRepository as _,
    PrincipalHydrationPort as _,
};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DEFAULT_APP_URL: &str = "postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode";
const DEFAULT_OWNER_URL: &str = "postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode";

async fn test_pools() -> Result<Option<(PgPool, PgPool)>, Box<dyn std::error::Error>> {
    let req = std::env::var("AXPOINT_REQUIRE_DB_TESTS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let app_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_APP_URL));
    let own_url = std::env::var("MIGRATION_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_OWNER_URL));

    let app = match PgPoolOptions::new()
        .max_connections(6)
        .connect(&app_url)
        .await
    {
        Ok(p) => p,
        Err(e) if req => return Err(Box::new(e)),
        Err(_) => return Ok(None),
    };
    let own = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&own_url)
        .await
    {
        Ok(p) => p,
        Err(e) if req => return Err(Box::new(e)),
        Err(_) => return Ok(None),
    };
    Ok(Some((app, own)))
}

fn unique_tenant(prefix: &str) -> String {
    let short: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(10)
        .collect();
    format!("{prefix}_{short}")
}

fn sample_sealed(cred_id: Uuid, tag: u8) -> SealedCredential {
    SealedCredential {
        id: cred_id,
        key_version: 1,
        nonce: vec![tag; 12],
        ciphertext: vec![tag, 0xAA, 0xBB, 0xCC],
    }
}

async fn seed_tenant_and_user(
    owner_pool: &PgPool,
    tenant_id: &str,
    user_id: Uuid,
    username: &str,
) -> TestResult {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name) VALUES ($1, $1) ON CONFLICT DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;
    sqlx::query(
        "INSERT INTO sec_departments (tenant_id, dept_id, name)
         VALUES ($1, 'west', 'West Region')
         ON CONFLICT DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;
    sqlx::query(
        "INSERT INTO sec_users (id, tenant_id, username, display_name, dept_id, clearance, status)
         VALUES ($1, $2, $3, $3, 'west', 'SECRET', 'ACTIVE')
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(username)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn cleanup_im_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM ax_messages WHERE tenant_id = $1",
        "DELETE FROM ax_conversations WHERE tenant_id = $1",
        "DELETE FROM ax_im_event_dedup WHERE tenant_id = $1",
        "DELETE FROM ax_im_bind_challenges WHERE tenant_id = $1",
        "DELETE FROM ax_im_user_bindings WHERE tenant_id = $1",
        "DELETE FROM ax_im_connectors WHERE tenant_id = $1",
        "DELETE FROM ax_agent_versions WHERE tenant_id = $1",
        "DELETE FROM ax_agents WHERE tenant_id = $1",
        "DELETE FROM ds_credentials WHERE tenant_id = $1",
        "DELETE FROM sec_user_attributes WHERE tenant_id = $1",
        "DELETE FROM sec_user_roles WHERE tenant_id = $1",
        "DELETE FROM sec_roles WHERE tenant_id = $1",
        "DELETE FROM sec_users WHERE tenant_id = $1",
        "DELETE FROM sec_departments WHERE tenant_id = $1 AND dept_id <> 'default'",
        "DELETE FROM sec_tenants WHERE tenant_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(tenant_id)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

async fn create_test_agent(store: &PgStore, tenant_id: &str, name: &str) -> Result<Uuid, AppError> {
    let rec = store
        .create_agent(
            tenant_id,
            None,
            CreateAgentRequest {
                name: String::from(name),
                description: String::from("Step 21 IM test agent"),
                spec: AgentSpec::default(),
            },
        )
        .await?;
    Ok(rec.id)
}

#[tokio::test]
async fn im_connector_crud_credential_rotation_and_rls_isolation() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let tenant_a = unique_tenant("t_im_crud_a");
    let tenant_b = unique_tenant("t_im_crud_b");
    let user_a = Uuid::new_v4();
    seed_tenant_and_user(&owner_pool, &tenant_a, user_a, "alice_im").await?;
    seed_tenant_and_user(&owner_pool, &tenant_b, Uuid::new_v4(), "bob_im").await?;

    let agent_id = create_test_agent(&store, &tenant_a, "im_crud_bot").await?;
    let cred_1 = Uuid::new_v4();
    let created = store
        .create_im_connector(
            &tenant_a,
            NewImConnectorInput {
                name: String::from("feishu_main"),
                platform: ImPlatform::Feishu,
                agent_id,
                credential_id: cred_1,
                credential: sample_sealed(cred_1, 1),
                config: ImConnectorConfig::default(),
                identity_policy: ImIdentityPolicy::RequireUserBind,
                delegated_user_id: None,
                enabled: false,
                created_by: Some(user_a),
            },
        )
        .await?;
    assert_eq!(created.name, "feishu_main");
    assert_eq!(created.status, ImConnectorStatus::Disconnected);

    let dup = store
        .create_im_connector(
            &tenant_a,
            NewImConnectorInput {
                name: String::from("feishu_main"),
                platform: ImPlatform::Feishu,
                agent_id,
                credential_id: Uuid::new_v4(),
                credential: sample_sealed(Uuid::new_v4(), 2),
                config: ImConnectorConfig::default(),
                identity_policy: ImIdentityPolicy::RequireUserBind,
                delegated_user_id: None,
                enabled: false,
                created_by: None,
            },
        )
        .await;
    assert!(matches!(dup, Err(AppError::Conflict("NAME_TAKEN"))));

    // Cross-tenant RLS must hide tenant_a's connector completely.
    assert!(store.list_im_connectors(&tenant_b).await?.is_empty());
    assert!(
        store
            .get_im_connector(&tenant_b, created.id)
            .await?
            .is_none()
    );

    let cred_2 = Uuid::new_v4();
    let updated = store
        .update_im_connector(
            &tenant_a,
            created.id,
            UpdateImConnectorInput {
                name: String::from("feishu_rotated"),
                agent_id,
                rotated_credential: Some((cred_2, sample_sealed(cred_2, 9))),
                config: ImConnectorConfig::default(),
                identity_policy: ImIdentityPolicy::CreatorDelegated,
                delegated_user_id: Some(user_a),
                enabled: false,
            },
        )
        .await?
        .ok_or("missing updated connector")?;
    assert_eq!(updated.credential_id, cred_2);
    assert!(store.get_credential(&tenant_a, cred_1).await?.is_none());
    assert!(store.get_credential(&tenant_a, cred_2).await?.is_some());

    assert!(store.delete_im_connector(&tenant_a, created.id).await?);
    assert!(store.get_credential(&tenant_a, cred_2).await?.is_none());

    cleanup_im_tenant(&owner_pool, &tenant_a).await?;
    cleanup_im_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

#[tokio::test]
async fn im_single_leader_lease_fencing_takeover_and_dedup() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let tenant_id = unique_tenant("t_im_lease");
    let user_id = Uuid::new_v4();
    seed_tenant_and_user(&owner_pool, &tenant_id, user_id, "lease_user").await?;
    let agent_id = create_test_agent(&store, &tenant_id, "im_lease_bot").await?;

    // Pause any pre-existing enabled connectors during cross-tenant claim assertions, or scope holder names.
    let cred_id = Uuid::new_v4();
    let conn = store
        .create_im_connector(
            &tenant_id,
            NewImConnectorInput {
                name: String::from("dingtalk_lease"),
                platform: ImPlatform::Dingtalk,
                agent_id,
                credential_id: cred_id,
                credential: sample_sealed(cred_id, 3),
                config: ImConnectorConfig::default(),
                identity_policy: ImIdentityPolicy::RequireUserBind,
                delegated_user_id: None,
                enabled: true,
                created_by: Some(user_id),
            },
        )
        .await?;

    let holder_1 = format!("replica-a-{}", Uuid::new_v4());
    let holder_2 = format!("replica-b-{}", Uuid::new_v4());
    let claimed_1 = store.claim_im_connectors(&holder_1, 128, 30).await?;
    assert!(claimed_1.iter().any(|c| c.id == conn.id));

    let claimed_2 = store.claim_im_connectors(&holder_2, 128, 30).await?;
    assert!(!claimed_2.iter().any(|c| c.id == conn.id));

    assert!(
        store
            .update_im_connector_runtime_state(
                &tenant_id,
                conn.id,
                &holder_1,
                ImConnectorStatus::Connected,
                "",
            )
            .await?
    );
    assert!(
        !store
            .update_im_connector_runtime_state(
                &tenant_id,
                conn.id,
                &holder_2,
                ImConnectorStatus::Connected,
                "",
            )
            .await?
    );

    // Expire lease to simulate crashed holder_1 and verify holder_2 takeover.
    sqlx::query(
        "UPDATE ax_im_connectors SET lease_until = NOW() - INTERVAL '2 seconds' WHERE id = $1",
    )
    .bind(conn.id)
    .execute(&owner_pool)
    .await?;

    let takeover = store.claim_im_connectors(&holder_2, 128, 30).await?;
    assert!(takeover.iter().any(|c| c.id == conn.id));
    assert!(
        !store
            .update_im_connector_runtime_state(
                &tenant_id,
                conn.id,
                &holder_1,
                ImConnectorStatus::Connected,
                "",
            )
            .await?
    );

    assert!(
        store
            .record_im_event_dedup(&tenant_id, conn.id, "evt_1")
            .await?
    );
    assert!(
        !store
            .record_im_event_dedup(&tenant_id, conn.id, "evt_1")
            .await?
    );

    let released = store.release_im_connectors(&holder_2).await?;
    assert!(released >= 1);

    let _ = store.release_im_connectors(&holder_1).await?;
    cleanup_im_tenant(&owner_pool, &tenant_id).await?;
    Ok(())
}

#[tokio::test]
async fn im_bind_challenge_atomic_consume_replay_and_conversation_lookup() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let tenant_id = unique_tenant("t_im_bind");
    let user_id = Uuid::new_v4();
    seed_tenant_and_user(&owner_pool, &tenant_id, user_id, "west_analyst").await?;
    let agent_id = create_test_agent(&store, &tenant_id, "im_bind_bot").await?;
    let cred_id = Uuid::new_v4();
    let conn = store
        .create_im_connector(
            &tenant_id,
            NewImConnectorInput {
                name: String::from("feishu_bind"),
                platform: ImPlatform::Feishu,
                agent_id,
                credential_id: cred_id,
                credential: sample_sealed(cred_id, 5),
                config: ImConnectorConfig::default(),
                identity_policy: ImIdentityPolicy::RequireUserBind,
                delegated_user_id: None,
                enabled: false,
                created_by: Some(user_id),
            },
        )
        .await?;

    let raw_token = "axim_TestChallengeToken0123456789AbCdEf";
    let token_hash = hash_im_bind_token(raw_token)?;
    store
        .create_im_bind_challenge(
            &tenant_id,
            NewImBindChallengeInput {
                token_hash: token_hash.clone(),
                connector_id: conn.id,
                platform: ImPlatform::Feishu,
                external_user_id: String::from("ou_west_01"),
                external_display_name: String::from("王西区"),
                expires_at: Utc::now() + Duration::minutes(10),
            },
        )
        .await?;

    let preview = store
        .preview_im_bind_challenge(&tenant_id, &token_hash)
        .await?
        .ok_or("expected challenge preview")?;
    assert_eq!(preview.connector_name, "feishu_bind");
    assert_eq!(preview.external_user_id, "ou_west_01");

    let consumed = store
        .consume_im_bind_challenge(&tenant_id, &token_hash, user_id)
        .await?
        .ok_or("expected challenge consumption")?;
    assert_eq!(consumed.platform, ImPlatform::Feishu);

    // Replay must return None.
    assert!(
        store
            .consume_im_bind_challenge(&tenant_id, &token_hash, user_id)
            .await?
            .is_none()
    );

    let binding = store
        .get_im_user_binding(&tenant_id, ImPlatform::Feishu, "ou_west_01")
        .await?
        .ok_or("expected persisted binding")?;
    assert_eq!(binding.user_id, user_id);
    assert_eq!(binding.username.as_deref(), Some("west_analyst"));

    let im_conversation = store
        .create_conversation(
            &tenant_id,
            NewConversationInput {
                agent_id,
                agent_version: 0,
                user_id: Some(user_id),
                channel: ConversationChannel::Im,
                embed_id: None,
                im_connector_id: Some(conn.id),
                external_chat_id: Some(String::from("oc_dm_99")),
                title: String::from("IM thread"),
            },
        )
        .await?;
    let found_conv = store
        .find_active_im_conversation(&tenant_id, conn.id, "oc_dm_99", Some(user_id))
        .await?
        .ok_or("expected active IM conversation")?;
    assert_eq!(found_conv.id, im_conversation.id);
    assert_eq!(found_conv.status, ConversationStatus::Active);

    assert!(
        store
            .delete_im_user_binding(&tenant_id, binding.id, Some(Uuid::new_v4()))
            .await?
            .is_none()
    );
    assert!(
        store
            .delete_im_user_binding(&tenant_id, binding.id, Some(user_id))
            .await?
            .is_some()
    );

    cleanup_im_tenant(&owner_pool, &tenant_id).await?;
    Ok(())
}

#[tokio::test]
async fn im_principal_hydration_strips_pii_raw_read_and_rejects_locked_or_disabled() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let tenant_id = unique_tenant("t_im_hydrate");
    let user_id = Uuid::new_v4();
    let role_id = Uuid::new_v4();
    seed_tenant_and_user(&owner_pool, &tenant_id, user_id, "privileged_user").await?;

    let all_perms: Vec<String> = Permission::ALL
        .iter()
        .map(|p| String::from(p.as_str()))
        .collect();
    sqlx::query(
        "INSERT INTO sec_roles (id, tenant_id, name, permissions, is_builtin)
         VALUES ($1, $2, 'FULL_IM_TEST', $3, FALSE)",
    )
    .bind(role_id)
    .bind(&tenant_id)
    .bind(&all_perms)
    .execute(&owner_pool)
    .await?;
    sqlx::query("INSERT INTO sec_user_roles (tenant_id, user_id, role_id) VALUES ($1, $2, $3)")
        .bind(&tenant_id)
        .bind(user_id)
        .bind(role_id)
        .execute(&owner_pool)
        .await?;
    sqlx::query(
        "INSERT INTO sec_user_attributes (tenant_id, user_id, attr_key, attr_values)
         VALUES ($1, $2, 'region', ARRAY['west'])",
    )
    .bind(&tenant_id)
    .bind(user_id)
    .execute(&owner_pool)
    .await?;

    let bound = store
        .hydrate_im_principal(&tenant_id, user_id, ImIdentityPolicy::RequireUserBind)
        .await?
        .ok_or("expected hydrated principal")?;
    assert!(
        !bound
            .principal
            .permissions
            .contains(&Permission::PiiRawRead)
    );
    assert!(bound.principal.permissions.contains(&Permission::FlowRun));
    assert_eq!(bound.principal.dept_id, "west");
    assert_eq!(
        bound.attributes.get("region"),
        Some(&vec![String::from("west")])
    );

    let delegated = store
        .hydrate_im_principal(&tenant_id, user_id, ImIdentityPolicy::CreatorDelegated)
        .await?
        .ok_or("expected delegated principal")?;
    assert_eq!(
        delegated.principal.permissions,
        BTreeSet::from([Permission::DataRead, Permission::AgentRun])
    );
    assert_eq!(delegated.principal.clearance, Sensitivity::Internal);

    // Lock user -> must return None.
    sqlx::query("UPDATE sec_users SET locked_until = NOW() + INTERVAL '10 minutes' WHERE id = $1")
        .bind(user_id)
        .execute(&owner_pool)
        .await?;
    assert!(
        store
            .hydrate_im_principal(&tenant_id, user_id, ImIdentityPolicy::RequireUserBind)
            .await?
            .is_none()
    );

    // Disable user -> must return None.
    sqlx::query("UPDATE sec_users SET locked_until = NULL, status = 'DISABLED' WHERE id = $1")
        .bind(user_id)
        .execute(&owner_pool)
        .await?;
    assert!(
        store
            .hydrate_im_principal(&tenant_id, user_id, ImIdentityPolicy::RequireUserBind)
            .await?
            .is_none()
    );

    cleanup_im_tenant(&owner_pool, &tenant_id).await?;
    Ok(())
}
