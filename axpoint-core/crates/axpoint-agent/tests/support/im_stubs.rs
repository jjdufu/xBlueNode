//! In-memory Step 21 `ImGatewayRepository`, `PrincipalHydrationPort`, and `ConnectionRepository`
//! implementations on [`super::MockStep09Store`].

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::agent::ConversationRecord;
use axpoint_domain::axhub::{
    Connection, ConnectionStatus, NewConnectionInput, SealedCredential, UpdateConnectionInput,
};
use axpoint_domain::enums::{
    ConversationChannel, ConversationStatus, ImConnectorStatus, ImIdentityPolicy, ImPlatform,
};
use axpoint_domain::im::{
    ConsumedImBindChallenge, HydratedImPrincipal, ImBindChallengePreview, ImConnectorRecord,
    ImUserBindingRecord, NewImBindChallengeInput, NewImConnectorInput, UpdateImConnectorInput,
    apply_im_identity_guardrails,
};
use axpoint_domain::ports::{ConnectionRepository, ImGatewayRepository, PrincipalHydrationPort};
use chrono::{TimeDelta, Utc};
use uuid::Uuid;

use super::MockStep09Store;

impl ConnectionRepository for MockStep09Store {
    fn list_connections(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<(Connection, Option<SealedCredential>)>, AppError>> + Send
    {
        std::future::ready(Ok(Vec::new()))
    }

    fn get_connection(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<Option<(Connection, Option<SealedCredential>)>, AppError>> + Send
    {
        std::future::ready(Ok(None))
    }

    fn create_connection(
        &self,
        _tenant_id: &str,
        _input: NewConnectionInput,
    ) -> impl Future<Output = Result<Connection, AppError>> + Send {
        std::future::ready(Err(AppError::NotFound("NOT_IMPLEMENTED")))
    }

    fn update_connection(
        &self,
        _tenant_id: &str,
        _id: Uuid,
        _input: UpdateConnectionInput,
    ) -> impl Future<Output = Result<Option<Connection>, AppError>> + Send {
        std::future::ready(Ok(None))
    }

    fn delete_connection(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send {
        std::future::ready(Ok(false))
    }

    fn update_test_status(
        &self,
        _tenant_id: &str,
        _id: Uuid,
        _status: ConnectionStatus,
        _message: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send {
        std::future::ready(Ok(()))
    }

    async fn get_credential(
        &self,
        tenant_id: &str,
        credential_id: Uuid,
    ) -> Result<Option<SealedCredential>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .credentials
            .get(&(String::from(tenant_id), credential_id))
            .cloned())
    }

    async fn list_credentials(&self, tenant_id: &str) -> Result<Vec<SealedCredential>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .credentials
            .iter()
            .filter(|((t, _), _)| t == tenant_id)
            .map(|(_, c)| c.clone())
            .collect())
    }

    async fn update_rotated_credential(
        &self,
        tenant_id: &str,
        credential: SealedCredential,
    ) -> Result<(), AppError> {
        let mut guard = self.inner.lock().await;
        guard
            .credentials
            .insert((String::from(tenant_id), credential.id), credential);
        Ok(())
    }
}

impl ImGatewayRepository for MockStep09Store {
    async fn list_im_connectors(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ImConnectorRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .im_connectors
            .iter()
            .filter(|c| c.tenant_id == tenant_id)
            .cloned()
            .collect())
    }

    async fn get_im_connector(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
    ) -> Result<Option<ImConnectorRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .im_connectors
            .iter()
            .find(|c| c.tenant_id == tenant_id && c.id == connector_id)
            .cloned())
    }

    async fn create_im_connector(
        &self,
        tenant_id: &str,
        input: NewImConnectorInput,
    ) -> Result<ImConnectorRecord, AppError> {
        let mut guard = self.inner.lock().await;
        if guard
            .im_connectors
            .iter()
            .any(|c| c.tenant_id == tenant_id && c.name == input.name)
        {
            return Err(AppError::Conflict("NAME_TAKEN"));
        }
        guard.credentials.insert(
            (String::from(tenant_id), input.credential_id),
            input.credential,
        );
        let now = Utc::now();
        let rec = ImConnectorRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            name: input.name,
            platform: input.platform,
            agent_id: input.agent_id,
            credential_id: input.credential_id,
            has_secret: true,
            config: input.config,
            identity_policy: input.identity_policy,
            delegated_user_id: input.delegated_user_id,
            enabled: input.enabled,
            leader_holder: None,
            lease_until: None,
            status: ImConnectorStatus::Disconnected,
            last_error: String::new(),
            last_heartbeat_at: None,
            created_by: input.created_by,
            created_at: now,
            updated_at: now,
        };
        guard.im_connectors.push(rec.clone());
        Ok(rec)
    }

    async fn update_im_connector(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
        input: UpdateImConnectorInput,
    ) -> Result<Option<ImConnectorRecord>, AppError> {
        let mut guard = self.inner.lock().await;
        let mut new_cred_opt = None;
        if let Some((cid, sealed)) = input.rotated_credential {
            guard
                .credentials
                .insert((String::from(tenant_id), cid), sealed);
            new_cred_opt = Some(cid);
        }
        let Some(rec) = guard
            .im_connectors
            .iter_mut()
            .find(|c| c.tenant_id == tenant_id && c.id == connector_id)
        else {
            return Ok(None);
        };
        let old_cid = rec.credential_id;
        rec.name = input.name;
        rec.agent_id = input.agent_id;
        if let Some(cid) = new_cred_opt {
            rec.credential_id = cid;
        }
        rec.config = input.config;
        rec.identity_policy = input.identity_policy;
        rec.delegated_user_id = input.delegated_user_id;
        rec.enabled = input.enabled;
        if !input.enabled {
            rec.leader_holder = None;
            rec.lease_until = None;
            rec.status = ImConnectorStatus::Disconnected;
            rec.last_error.clear();
        }
        rec.updated_at = Utc::now();
        let out = rec.clone();
        if let Some(cid) = new_cred_opt
            && cid != old_cid
        {
            guard
                .credentials
                .remove(&(String::from(tenant_id), old_cid));
        }
        Ok(Some(out))
    }

    async fn delete_im_connector(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
    ) -> Result<bool, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(pos) = guard
            .im_connectors
            .iter()
            .position(|c| c.tenant_id == tenant_id && c.id == connector_id)
        else {
            return Ok(false);
        };
        let removed = guard.im_connectors.remove(pos);
        guard
            .credentials
            .remove(&(String::from(tenant_id), removed.credential_id));
        Ok(true)
    }

    async fn claim_im_connectors(
        &self,
        holder_id: &str,
        limit: i32,
        lease_seconds: i32,
    ) -> Result<Vec<ImConnectorRecord>, AppError> {
        let mut guard = self.inner.lock().await;
        let now = Utc::now();
        let cap = usize::try_from(limit.max(0)).unwrap_or(0);
        let mut out = Vec::new();
        for conn in &mut guard.im_connectors {
            if out.len() >= cap {
                break;
            }
            let claimable = conn.enabled
                && (conn.leader_holder.as_deref() == Some(holder_id)
                    || conn.lease_until.is_none_or(|exp| exp < now));
            if claimable {
                conn.leader_holder = Some(String::from(holder_id));
                conn.lease_until = Some(now + TimeDelta::seconds(i64::from(lease_seconds)));
                conn.last_heartbeat_at = Some(now);
                conn.updated_at = now;
                out.push(conn.clone());
            }
        }
        Ok(out)
    }

    async fn release_im_connectors(&self, holder_id: &str) -> Result<i32, AppError> {
        let mut guard = self.inner.lock().await;
        let mut count: i32 = 0;
        for conn in &mut guard.im_connectors {
            if conn.leader_holder.as_deref() == Some(holder_id) {
                conn.leader_holder = None;
                conn.lease_until = None;
                conn.status = ImConnectorStatus::Disconnected;
                count = count.saturating_add(1);
            }
        }
        Ok(count)
    }

    async fn update_im_connector_runtime_state(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
        holder_id: &str,
        status: ImConnectorStatus,
        last_error: &str,
    ) -> Result<bool, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(conn) = guard
            .im_connectors
            .iter_mut()
            .find(|c| c.tenant_id == tenant_id && c.id == connector_id)
        else {
            return Ok(false);
        };
        if conn.leader_holder.as_deref() != Some(holder_id) {
            return Ok(false);
        }
        conn.status = status;
        conn.last_error = String::from(last_error);
        conn.last_heartbeat_at = Some(Utc::now());
        Ok(true)
    }

    async fn record_im_event_dedup(
        &self,
        _tenant_id: &str,
        connector_id: Uuid,
        event_id: &str,
    ) -> Result<bool, AppError> {
        let mut guard = self.inner.lock().await;
        Ok(guard
            .im_dedup
            .insert((connector_id, String::from(event_id))))
    }

    async fn get_im_user_binding(
        &self,
        tenant_id: &str,
        platform: ImPlatform,
        external_user_id: &str,
    ) -> Result<Option<ImUserBindingRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .im_bindings
            .iter()
            .find(|b| {
                b.tenant_id == tenant_id
                    && b.platform == platform
                    && b.external_user_id == external_user_id
            })
            .cloned())
    }

    async fn list_im_user_bindings(
        &self,
        tenant_id: &str,
        only_user_id: Option<Uuid>,
    ) -> Result<Vec<ImUserBindingRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .im_bindings
            .iter()
            .filter(|b| b.tenant_id == tenant_id && only_user_id.is_none_or(|u| b.user_id == u))
            .cloned()
            .collect())
    }

    async fn delete_im_user_binding(
        &self,
        tenant_id: &str,
        binding_id: Uuid,
        only_user_id: Option<Uuid>,
    ) -> Result<Option<ImUserBindingRecord>, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(pos) = guard.im_bindings.iter().position(|b| {
            b.tenant_id == tenant_id
                && b.id == binding_id
                && only_user_id.is_none_or(|u| b.user_id == u)
        }) else {
            return Ok(None);
        };
        Ok(Some(guard.im_bindings.remove(pos)))
    }

    async fn create_im_bind_challenge(
        &self,
        tenant_id: &str,
        input: NewImBindChallengeInput,
    ) -> Result<(), AppError> {
        let mut guard = self.inner.lock().await;
        guard.im_challenges.insert(
            input.token_hash.clone(),
            (String::from(tenant_id), input, false),
        );
        Ok(())
    }

    async fn preview_im_bind_challenge(
        &self,
        tenant_id: &str,
        token_hash: &str,
    ) -> Result<Option<ImBindChallengePreview>, AppError> {
        let guard = self.inner.lock().await;
        let Some((t, ch, consumed)) = guard.im_challenges.get(token_hash) else {
            return Ok(None);
        };
        if t != tenant_id || *consumed || ch.expires_at <= Utc::now() {
            return Ok(None);
        }
        let cname = guard
            .im_connectors
            .iter()
            .find(|c| c.id == ch.connector_id)
            .map_or_else(|| String::from("bot"), |c| c.name.clone());
        Ok(Some(ImBindChallengePreview {
            connector_id: ch.connector_id,
            connector_name: cname,
            platform: ch.platform,
            external_user_id: ch.external_user_id.clone(),
            external_display_name: ch.external_display_name.clone(),
            expires_at: ch.expires_at,
        }))
    }

    async fn consume_im_bind_challenge(
        &self,
        tenant_id: &str,
        token_hash: &str,
        user_id: Uuid,
    ) -> Result<Option<ConsumedImBindChallenge>, AppError> {
        let mut guard = self.inner.lock().await;
        let Some((t, ch, consumed)) = guard.im_challenges.get_mut(token_hash) else {
            return Ok(None);
        };
        if t != tenant_id || *consumed || ch.expires_at <= Utc::now() {
            return Ok(None);
        }
        *consumed = true;
        let out = ConsumedImBindChallenge {
            connector_id: ch.connector_id,
            platform: ch.platform,
            external_user_id: ch.external_user_id.clone(),
            external_display_name: ch.external_display_name.clone(),
        };
        guard.im_bindings.retain(|b| {
            !(b.tenant_id == tenant_id
                && b.platform == out.platform
                && b.external_user_id == out.external_user_id)
        });
        guard.im_bindings.push(ImUserBindingRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            platform: out.platform,
            external_user_id: out.external_user_id.clone(),
            external_display_name: out.external_display_name.clone(),
            user_id,
            username: None,
            bound_at: Utc::now(),
        });
        Ok(Some(out))
    }

    async fn find_active_im_conversation(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
        external_chat_id: &str,
        user_id: Option<Uuid>,
    ) -> Result<Option<ConversationRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .conversations
            .iter()
            .rev()
            .find(|c| {
                c.tenant_id == tenant_id
                    && c.channel == ConversationChannel::Im
                    && c.im_connector_id == Some(connector_id)
                    && c.external_chat_id.as_deref() == Some(external_chat_id)
                    && c.status == ConversationStatus::Active
                    && (user_id.is_none() || c.user_id == user_id)
            })
            .cloned())
    }
}

impl PrincipalHydrationPort for MockStep09Store {
    async fn hydrate_im_principal(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        policy: ImIdentityPolicy,
    ) -> Result<Option<HydratedImPrincipal>, AppError> {
        let guard = self.inner.lock().await;
        if guard.disabled_users.contains(&user_id) {
            return Ok(None);
        }
        let Some(mut principal) = guard
            .principals
            .get(&(String::from(tenant_id), user_id))
            .cloned()
        else {
            return Ok(None);
        };
        apply_im_identity_guardrails(&mut principal, policy);
        Ok(Some(HydratedImPrincipal {
            principal,
            attributes: BTreeMap::new(),
        }))
    }
}
