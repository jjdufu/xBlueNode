//! `Cedar` `ABAC` policy validator, per-tenant `PolicySet` cache, department lineage resolver,
//! and authorization engine (`11-STEP13` §13.6.1).

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use axpoint_domain::AppError;
use axpoint_domain::enums::Permission;
use axpoint_domain::guard::{
    CedarPolicyRecord, DepartmentRecord, GuardPrincipalProfile, ResourceRef,
    SimulateCedarPolicyResponse, ValidateCedarPolicyResponse,
};
use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid, Policy,
    PolicyId, PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};

const SCHEMA_CEDARSCHEMA: &str = include_str!("../cedar/schema.cedarschema");
const BASE_CEDAR: &str = include_str!("../cedar/base.cedar");

/// Maximum recursion depth accepted when parsing custom `Cedar` policy expressions.
const MAX_CEDAR_DEPTH: usize = 64;

/// Maximum cached compiled tenant `PolicySet` entries.
const MAX_TENANT_POLICY_CACHE: usize = 256;

/// Request-level environmental attributes passed into [`CedarAuthorizer::evaluate`] and
/// [`CedarAuthorizer::authorize`] (`§13.6.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CedarContext {
    /// Client IP address string.
    pub ip: String,
    /// Current hour of day (`0..=23`) in tenant timezone.
    pub hour: u8,
    /// Whether multi-factor authentication was verified for this session.
    pub mfa: bool,
    /// Whether an active `sec_grants` row grants the requested action on the target resource.
    pub granted: bool,
}

impl Default for CedarContext {
    fn default() -> Self {
        Self {
            ip: String::from("127.0.0.1"),
            hour: 12,
            mfa: false,
            granted: false,
        }
    }
}

/// Computes the cycle-safe department ancestor chain `[dept_id, parent, grandparent, ...]`
/// from `departments` (`§13.6.1`).
#[must_use]
pub fn dept_lineage(dept_id: &str, departments: &[DepartmentRecord]) -> Vec<String> {
    let trimmed = dept_id.trim();
    if trimmed.is_empty() {
        return vec![String::from("default")];
    }

    let parent_lookup: HashMap<&str, &str> = departments
        .iter()
        .filter_map(|d| {
            let parent = d.parent_id.as_deref()?.trim();
            if parent.is_empty() || parent == d.dept_id {
                None
            } else {
                Some((d.dept_id.as_str(), parent))
            }
        })
        .collect();

    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    let mut cursor = trimmed;

    while visited.insert(cursor) {
        chain.push(String::from(cursor));
        let Some(&next) = parent_lookup.get(cursor) else {
            break;
        };
        cursor = next;
    }

    chain
}

struct CachedTenantPolicies {
    entries: HashMap<String, (u64, Arc<PolicySet>)>,
}

/// Schema-validated `Cedar` `ABAC` engine combining embedded baseline rules (`base.cedar`) with
/// per-tenant custom `sec_policies` (`§13.6.1`).
#[derive(Clone)]
pub struct CedarAuthorizer {
    schema: Arc<Schema>,
    validator: Arc<Validator>,
    base_policies: Arc<PolicySet>,
    authorizer: Arc<Authorizer>,
    env_mode: String,
    tenant_cache: Arc<Mutex<CachedTenantPolicies>>,
}

impl fmt::Debug for CedarAuthorizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CedarAuthorizer")
            .field("env_mode", &self.env_mode)
            .finish_non_exhaustive()
    }
}

impl CedarAuthorizer {
    /// Initializes [`CedarAuthorizer`] by compiling and validating the embedded `schema.cedarschema`
    /// and `base.cedar` baseline policy set.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if the embedded schema or baseline policies fail validation.
    pub fn new() -> Result<Self, AppError> {
        let (schema, _warnings) = Schema::from_cedarschema_str(SCHEMA_CEDARSCHEMA)
            .map_err(|err| AppError::Config(format!("invalid embedded Cedar schema: {err}")))?;
        let base_pset =
            PolicySet::from_str_with_depth_limit(BASE_CEDAR, MAX_CEDAR_DEPTH).map_err(|err| {
                AppError::Config(format!("invalid embedded baseline Cedar policy: {err}"))
            })?;
        let validator = Validator::new(schema.clone());
        let res = validator.validate(&base_pset, ValidationMode::Strict);
        if !res.validation_passed() {
            let errs: Vec<String> = res.validation_errors().map(ToString::to_string).collect();
            return Err(AppError::Config(format!(
                "baseline Cedar policies failed strict validation: {}",
                errs.join("; ")
            )));
        }
        let env_mode = std::env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
        Ok(Self {
            schema: Arc::new(schema),
            validator: Arc::new(validator),
            base_policies: Arc::new(base_pset),
            authorizer: Arc::new(Authorizer::new()),
            env_mode,
            tenant_cache: Arc::new(Mutex::new(CachedTenantPolicies {
                entries: HashMap::new(),
            })),
        })
    }

    /// Overrides the `context.env` value injected into `Cedar` requests.
    #[must_use]
    pub fn with_env_mode(mut self, env_mode: impl Into<String>) -> Self {
        self.env_mode = env_mode.into();
        self
    }

    /// Validates candidate `policy_text` syntax and types against `AxPoint` `schema.cedarschema`
    /// under `ValidationMode::Strict` (`POST /api/axguard/policies/validate`, `§13.6.1`).
    #[must_use]
    pub fn validate_policy_text(&self, policy_text: &str) -> ValidateCedarPolicyResponse {
        let trimmed = policy_text.trim();
        if trimmed.is_empty() {
            return ValidateCedarPolicyResponse {
                valid: false,
                errors: vec![String::from("policy_text cannot be empty")],
            };
        }

        let pset = match PolicySet::from_str_with_depth_limit(trimmed, MAX_CEDAR_DEPTH) {
            Ok(p) => p,
            Err(parse_errs) => {
                return ValidateCedarPolicyResponse {
                    valid: false,
                    errors: vec![parse_errs.to_string()],
                };
            }
        };

        if pset.is_empty() {
            return ValidateCedarPolicyResponse {
                valid: false,
                errors: vec![String::from("policy_text contains no policy statements")],
            };
        }

        let result = self.validator.validate(&pset, ValidationMode::Strict);
        if result.validation_passed() {
            ValidateCedarPolicyResponse {
                valid: true,
                errors: Vec::new(),
            }
        } else {
            let errors = result
                .validation_errors()
                .map(ToString::to_string)
                .collect();
            ValidateCedarPolicyResponse {
                valid: false,
                errors,
            }
        }
    }

    /// Ensures `policy_text` passes `AxPoint` schema validation before saving to `sec_policies`.
    ///
    /// # Errors
    /// Returns `AppError::BadRequest` (`SCHEMA_INVALID`) containing joined diagnostics when invalid.
    pub fn ensure_valid_policy_text(&self, policy_text: &str) -> Result<(), AppError> {
        let check = self.validate_policy_text(policy_text);
        if check.valid {
            Ok(())
        } else {
            Err(AppError::bad("SCHEMA_INVALID", check.errors.join("; ")))
        }
    }

    /// Evicts the cached `PolicySet` for `tenant_id` (invoked on `ax_policy_changed` notifications).
    pub fn invalidate_tenant(&self, tenant_id: &str) {
        let mut guard = self
            .tenant_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.entries.remove(tenant_id);
    }

    /// Evaluates `(profile, action, resource, ctx)` against baseline + enabled `custom_policies`
    /// and returns full decision & policy-ID diagnostics (`POST /api/axguard/policies/simulate`).
    #[must_use]
    pub fn evaluate(
        &self,
        profile: &GuardPrincipalProfile,
        action: Permission,
        resource: &ResourceRef,
        ctx: &CedarContext,
        departments: &[DepartmentRecord],
        custom_policies: &[CedarPolicyRecord],
    ) -> SimulateCedarPolicyResponse {
        let pset = match self.get_or_compile_policy_set(&profile.tenant_id, custom_policies) {
            Ok(ps) => ps,
            Err(err) => {
                return SimulateCedarPolicyResponse {
                    decision: String::from("DENY"),
                    allowed: false,
                    determining_policies: Vec::new(),
                    diagnostics: vec![err.to_string()],
                };
            }
        };

        let (request, entities) =
            match self.build_request_and_entities(profile, action, resource, ctx, departments) {
                Ok(pair) => pair,
                Err(err) => {
                    return SimulateCedarPolicyResponse {
                        decision: String::from("DENY"),
                        allowed: false,
                        determining_policies: Vec::new(),
                        diagnostics: vec![err.to_string()],
                    };
                }
            };

        let resp = self.authorizer.is_authorized(&request, &pset, &entities);
        let allowed = resp.decision() == Decision::Allow;
        let mut determining: Vec<String> = resp
            .diagnostics()
            .reason()
            .map(ToString::to_string)
            .collect();
        determining.sort();
        let diagnostics: Vec<String> = resp
            .diagnostics()
            .errors()
            .map(ToString::to_string)
            .collect();

        SimulateCedarPolicyResponse {
            decision: if allowed {
                String::from("ALLOW")
            } else {
                String::from("DENY")
            },
            allowed,
            determining_policies: determining,
            diagnostics,
        }
    }

    /// Enforces `Cedar` `ABAC` authorization for `(profile, action, resource, ctx)`.
    ///
    /// # Errors
    /// Returns `AppError::Forbidden("POLICY_DENIED")` when the `Cedar` decision is `Deny`.
    pub fn authorize(
        &self,
        profile: &GuardPrincipalProfile,
        action: Permission,
        resource: &ResourceRef,
        ctx: &CedarContext,
        departments: &[DepartmentRecord],
        custom_policies: &[CedarPolicyRecord],
    ) -> Result<(), AppError> {
        let sim = self.evaluate(profile, action, resource, ctx, departments, custom_policies);
        if sim.allowed {
            Ok(())
        } else {
            Err(AppError::Forbidden("POLICY_DENIED"))
        }
    }

    fn get_or_compile_policy_set(
        &self,
        tenant_id: &str,
        custom_policies: &[CedarPolicyRecord],
    ) -> Result<Arc<PolicySet>, AppError> {
        let fingerprint = fingerprint_policies(custom_policies);
        {
            let guard = self
                .tenant_cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some((cached_fp, cached_pset)) = guard.entries.get(tenant_id)
                && *cached_fp == fingerprint
            {
                return Ok(Arc::clone(cached_pset));
            }
        }

        let mut combined = (*self.base_policies).clone();
        for rec in custom_policies {
            if !rec.enabled {
                continue;
            }
            add_custom_record_policies(&mut combined, rec)?;
        }

        let compiled = Arc::new(combined);
        let mut guard = self
            .tenant_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if guard.entries.len() >= MAX_TENANT_POLICY_CACHE {
            guard.entries.clear();
        }
        guard.entries.insert(
            String::from(tenant_id),
            (fingerprint, Arc::clone(&compiled)),
        );
        Ok(compiled)
    }

    fn build_request_and_entities(
        &self,
        profile: &GuardPrincipalProfile,
        action: Permission,
        resource: &ResourceRef,
        ctx: &CedarContext,
        departments: &[DepartmentRecord],
    ) -> Result<(Request, Entities), AppError> {
        let user_type = entity_type("AxPoint::User")?;
        let dept_type = entity_type("AxPoint::Dept")?;
        let res_type = entity_type("AxPoint::Resource")?;
        let action_type = entity_type("AxPoint::Action")?;

        let user_dept = non_empty_or_default(&profile.dept_id);
        let owner_dept = non_empty_or_default(&resource.owner_dept);
        let lineage = dept_lineage(&user_dept, departments);

        let dept_entities = build_dept_entities(&dept_type, departments, &user_dept, &owner_dept);

        let user_uid =
            EntityUid::from_type_name_and_id(user_type, EntityId::new(profile.user_id.to_string()));
        let user_parent_dept =
            EntityUid::from_type_name_and_id(dept_type.clone(), EntityId::new(&user_dept));
        let user_entity = build_user_entity(user_uid.clone(), user_parent_dept, profile, &lineage)?;

        let res_key = format!("{}:{}", resource.kind.as_str(), resource.id);
        let res_uid = EntityUid::from_type_name_and_id(res_type, EntityId::new(res_key));
        let owner_dept_uid = EntityUid::from_type_name_and_id(dept_type, EntityId::new(owner_dept));
        let res_entity = build_resource_entity(
            res_uid.clone(),
            owner_dept_uid,
            resource,
            ctx.granted,
            profile.user_id,
        )?;

        let mut all_entities = dept_entities;
        all_entities.push(user_entity);
        all_entities.push(res_entity);

        let entities = Entities::from_entities(all_entities, Some(&self.schema))
            .map_err(|err| AppError::Config(format!("Cedar entity graph error: {err}")))?;

        let action_uid =
            EntityUid::from_type_name_and_id(action_type, EntityId::new(action.as_str()));
        let cedar_ctx = Context::from_pairs([
            (
                String::from("action"),
                RestrictedExpression::new_string(String::from(action.as_str())),
            ),
            (
                String::from("ip"),
                RestrictedExpression::new_string(ctx.ip.clone()),
            ),
            (
                String::from("hour"),
                RestrictedExpression::new_long(i64::from(ctx.hour.min(23))),
            ),
            (
                String::from("env"),
                RestrictedExpression::new_string(self.env_mode.clone()),
            ),
            (String::from("mfa"), RestrictedExpression::new_bool(ctx.mfa)),
        ])
        .map_err(|err| AppError::Config(format!("Cedar context construction failed: {err}")))?;

        let request = Request::new(user_uid, action_uid, res_uid, cedar_ctx, Some(&self.schema))
            .map_err(|err| AppError::Config(format!("Cedar request validation failed: {err}")))?;

        Ok((request, entities))
    }
}

fn entity_type(name: &str) -> Result<EntityTypeName, AppError> {
    EntityTypeName::from_str(name)
        .map_err(|err| AppError::Config(format!("invalid Cedar entity type '{name}': {err}")))
}

fn non_empty_or_default(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        String::from("default")
    } else {
        String::from(trimmed)
    }
}

fn reaches_in_dag(start: &str, target: &str, parent_map: &BTreeMap<String, String>) -> bool {
    let mut cur = start;
    for _ in 0..=parent_map.len() {
        if cur == target {
            return true;
        }
        let Some(next) = parent_map.get(cur) else {
            return false;
        };
        cur = next.as_str();
    }
    true
}

fn build_dept_entities(
    dept_type: &EntityTypeName,
    departments: &[DepartmentRecord],
    user_dept: &str,
    owner_dept: &str,
) -> Vec<Entity> {
    let mut all_dept_ids: BTreeSet<String> = departments
        .iter()
        .map(|d| non_empty_or_default(&d.dept_id))
        .collect();
    let mut raw_parents: BTreeMap<String, String> = BTreeMap::new();
    for d in departments {
        let child = non_empty_or_default(&d.dept_id);
        if let Some(ref p) = d.parent_id {
            let parent = non_empty_or_default(p);
            all_dept_ids.insert(parent.clone());
            if child != parent {
                raw_parents.insert(child, parent);
            }
        }
    }
    all_dept_ids.insert(String::from(user_dept));
    all_dept_ids.insert(String::from(owner_dept));

    let mut dag_parents: BTreeMap<String, String> = BTreeMap::new();
    let user_chain = dept_lineage(user_dept, departments);
    for pair in user_chain.windows(2) {
        if let [child, parent] = pair
            && !reaches_in_dag(parent, child, &dag_parents)
        {
            dag_parents.insert(child.clone(), parent.clone());
        }
    }
    for (child, parent) in &raw_parents {
        if !dag_parents.contains_key(child) && !reaches_in_dag(parent, child, &dag_parents) {
            dag_parents.insert(child.clone(), parent.clone());
        }
    }

    let mut entities = Vec::with_capacity(all_dept_ids.len());
    for did in all_dept_ids {
        let mut parents = HashSet::new();
        if let Some(direct_parent) = dag_parents.get(&did) {
            parents.insert(EntityUid::from_type_name_and_id(
                dept_type.clone(),
                EntityId::new(direct_parent),
            ));
        }
        let uid = EntityUid::from_type_name_and_id(dept_type.clone(), EntityId::new(&did));
        entities.push(Entity::new_no_attrs(uid, parents));
    }
    entities
}

fn build_user_entity(
    user_uid: EntityUid,
    parent_dept: EntityUid,
    profile: &GuardPrincipalProfile,
    lineage: &[String],
) -> Result<Entity, AppError> {
    let perm_exprs = profile
        .permissions
        .iter()
        .map(|p| RestrictedExpression::new_string(String::from(p.as_str())));

    let mut grant_keys = Vec::new();
    grant_keys.push(RestrictedExpression::new_string(format!(
        "user:{}",
        profile.user_id
    )));
    for dept in lineage {
        grant_keys.push(RestrictedExpression::new_string(format!("dept:{dept}")));
    }
    for role in &profile.roles {
        grant_keys.push(RestrictedExpression::new_string(format!("role:{role}")));
    }

    let attrs = HashMap::from([
        (
            String::from("permissions"),
            RestrictedExpression::new_set(perm_exprs),
        ),
        (
            String::from("clearance"),
            RestrictedExpression::new_long(profile.clearance.level()),
        ),
        (
            String::from("grant_keys"),
            RestrictedExpression::new_set(grant_keys),
        ),
    ]);
    Entity::new(user_uid, attrs, HashSet::from([parent_dept]))
        .map_err(|err| AppError::Config(format!("failed constructing User entity: {err}")))
}

fn build_resource_entity(
    res_uid: EntityUid,
    owner_dept_uid: EntityUid,
    resource: &ResourceRef,
    granted: bool,
    user_id: uuid::Uuid,
) -> Result<Entity, AppError> {
    let granted_set = if granted {
        RestrictedExpression::new_set([RestrictedExpression::new_string(format!("user:{user_id}"))])
    } else {
        RestrictedExpression::new_set([])
    };
    let attrs = HashMap::from([
        (
            String::from("kind"),
            RestrictedExpression::new_string(String::from(resource.kind.as_str())),
        ),
        (
            String::from("owner_dept"),
            RestrictedExpression::new_entity_uid(owner_dept_uid),
        ),
        (
            String::from("sensitivity"),
            RestrictedExpression::new_long(resource.sensitivity.level()),
        ),
        (String::from("granted"), granted_set),
    ]);
    Entity::new(res_uid, attrs, HashSet::new())
        .map_err(|err| AppError::Config(format!("failed constructing Resource entity: {err}")))
}

fn add_custom_record_policies(
    target: &mut PolicySet,
    rec: &CedarPolicyRecord,
) -> Result<(), AppError> {
    let single_id = PolicyId::new(format!("custom:{}", rec.name));
    if let Ok(single) = Policy::parse(Some(single_id), &rec.policy_text) {
        target
            .add(single)
            .map_err(|err| AppError::Config(format!("duplicate Cedar policy ID: {err}")))?;
        return Ok(());
    }

    let parsed_set = PolicySet::from_str_with_depth_limit(&rec.policy_text, MAX_CEDAR_DEPTH)
        .map_err(|err| AppError::bad("SCHEMA_INVALID", err.to_string()))?;
    for (idx, p) in parsed_set.policies().enumerate() {
        let pid = PolicyId::new(format!("custom:{}#{idx}", rec.name));
        let renamed = Policy::parse(Some(pid), p.to_string())
            .map_err(|err| AppError::bad("SCHEMA_INVALID", err.to_string()))?;
        target
            .add(renamed)
            .map_err(|err| AppError::Config(format!("duplicate Cedar policy ID: {err}")))?;
    }
    Ok(())
}

fn fingerprint_policies(policies: &[CedarPolicyRecord]) -> u64 {
    let mut hasher = DefaultHasher::new();
    policies.len().hash(&mut hasher);
    for rec in policies {
        rec.id.hash(&mut hasher);
        rec.name.hash(&mut hasher);
        rec.version.hash(&mut hasher);
        rec.enabled.hash(&mut hasher);
        rec.policy_text.hash(&mut hasher);
    }
    hasher.finish()
}
