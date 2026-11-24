//! Auto-generated per-`(tenant_id, main_version)` dynamic GraphQL schema & batched neighbor `DataLoader`
//! for `POST /api/xblue/graphql` (`06-STEP05-06` §05.6.1).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock, PoisonError};

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::dynamic::{
    Field, FieldFuture, FieldValue, InputObject, InputValue, Object, Scalar, Schema, TypeRef,
};
use async_graphql::{Error as GqlError, Request as GqlRequest, Value as GqlValue, Variables};
use async_graphql_axum::GraphQLResponse;
use axpoint_db::PgStore;
use axpoint_db::studio_repo::NeighborBatchKey;
use axpoint_domain::AppError;
use axpoint_domain::enums::{AuditAction, DataType, DesignMaturity, Permission, PiiCategory};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::StudioRepository as _;
use axpoint_domain::studio::{
    ActionParamSpec, ActionSpec, DomainObjectListQuery, InvokeObjectActionRequest,
    InvokeObjectActionResponse, LinkDirection, NodeTypeSpec, ObjectNeighborSig, PropertyKind,
    PropertySpec, SchemaDoc,
};
use axpoint_domain::topology::BlueNode;
use axpoint_security::cedar_authz::CedarAuthorizer;
use axpoint_security::{AuditWriter, AuthPrincipal};
use axum::extract::State;
use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::str::FromStr;

use crate::state::{ApiJson, AppState, HttpError};
use crate::telemetry::current_trace_id;

const MAX_GRAPHQL_DEPTH: usize = 6;
const MAX_GRAPHQL_COMPLEXITY: usize = 200;
const MAX_CACHED_SCHEMAS: usize = 64;

/// Incoming HTTP JSON body for `POST /api/xblue/graphql`.
#[derive(Debug, Deserialize)]
pub(crate) struct GraphqlHttpPayload {
    /// GraphQL query string.
    pub(crate) query: String,
    /// Optional GraphQL operation name.
    #[serde(default, rename = "operationName")]
    pub(crate) operation_name: Option<String>,
    /// Optional GraphQL variables object.
    #[serde(default)]
    pub(crate) variables: Option<serde_json::Value>,
}

#[derive(Clone)]
struct GqlRequestCtx {
    tenant_id: String,
    principal: AuthPrincipal,
    client_ip: String,
    has_pii_raw: bool,
    policy: std::sync::Arc<axpoint_domain::guard::PolicyContext>,
    tenant_hmac_key: [u8; 32],
    cedar: std::sync::Arc<CedarAuthorizer>,
    audit: AuditWriter,
    store: PgStore,
}

/// Per-request `async_graphql` `DataLoader` eliminating 1-hop neighbor N+1 queries.
struct NeighborBatchLoader {
    tenant_id: String,
    policy: std::sync::Arc<axpoint_domain::guard::PolicyContext>,
    store: PgStore,
}

impl Loader<NeighborBatchKey> for NeighborBatchLoader {
    type Value = Vec<BlueNode>;
    type Error = String;

    async fn load(
        &self,
        keys: &[NeighborBatchKey],
    ) -> Result<HashMap<NeighborBatchKey, Self::Value>, Self::Error> {
        let mut raw_map = self
            .store
            .batch_load_neighbors(&self.tenant_id, keys)
            .await
            .map_err(|err| err.client_message())?;

        if !self.policy.bypass_row_policies {
            let mut candidate_ids = Vec::new();
            for list in raw_map.values() {
                for n in list {
                    candidate_ids.push(n.id);
                }
            }
            if !candidate_ids.is_empty() {
                let visible = self
                    .store
                    .filter_visible_node_ids(
                        &self.tenant_id,
                        &candidate_ids,
                        &self.policy.node_types,
                    )
                    .await
                    .map_err(|err| err.client_message())?;
                for list in raw_map.values_mut() {
                    list.retain(|n| visible.contains(&n.id));
                }
            }
        }
        Ok(raw_map)
    }
}

struct SchemaCacheState {
    order: VecDeque<(String, i32)>,
    entries: HashMap<(String, i32), Schema>,
}

static SCHEMA_CACHE: OnceLock<Mutex<SchemaCacheState>> = OnceLock::new();

fn schema_cache() -> &'static Mutex<SchemaCacheState> {
    SCHEMA_CACHE.get_or_init(|| {
        Mutex::new(SchemaCacheState {
            order: VecDeque::with_capacity(MAX_CACHED_SCHEMAS),
            entries: HashMap::with_capacity(MAX_CACHED_SCHEMAS),
        })
    })
}

fn get_or_build_cached_schema(
    tenant_id: &str,
    version: i32,
    doc: &SchemaDoc,
) -> Result<Schema, AppError> {
    let key = (String::from(tenant_id), version);
    {
        let mut guard = schema_cache()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = guard.entries.get(&key).cloned() {
            if let Some(pos) = guard.order.iter().position(|k| *k == key) {
                let _ = guard.order.remove(pos);
            }
            guard.order.push_back(key);
            return Ok(existing);
        }
    }

    let built = build_dynamic_schema(version, doc)?;

    let mut guard = schema_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    guard
        .entries
        .retain(|(t, v), _| t != tenant_id || *v == version);
    guard.order.retain(|(t, v)| t != tenant_id || *v == version);

    while guard.entries.len() >= MAX_CACHED_SCHEMAS {
        if let Some(oldest) = guard.order.pop_front() {
            let _ = guard.entries.remove(&oldest);
        } else {
            break;
        }
    }
    guard.order.push_back(key.clone());
    guard.entries.insert(key, built.clone());
    Ok(built)
}

/// Axum handler for `POST /api/xblue/graphql`.
pub(crate) async fn graphql_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(payload): ApiJson<GraphqlHttpPayload>,
) -> Result<GraphQLResponse, HttpError> {
    principal.require(Permission::DataRead)?;

    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy = std::sync::Arc::new(
        axpoint_server::policy::build_policy_context_for_store(
            &state.store,
            &state.cedar,
            &principal,
            false,
            &client_ip,
        )
        .await?,
    );
    let tenant_hmac_key = state.mask_keys.tenant_key(&principal.tenant_id);

    let published = axpoint_domain::ports::StudioRepository::get_published_schema(
        &state.store,
        &principal.tenant_id,
        "main",
    )
    .await?;

    let schema =
        get_or_build_cached_schema(&principal.tenant_id, published.version, &published.doc)?;

    let mut gql_req = GqlRequest::new(payload.query);
    if let Some(op_name) = payload.operation_name {
        gql_req = gql_req.operation_name(op_name);
    }
    if let Some(vars_json) = payload.variables {
        gql_req = gql_req.variables(Variables::from_json(vars_json));
    }

    let req_ctx = GqlRequestCtx {
        tenant_id: principal.tenant_id.clone(),
        has_pii_raw: principal.has(Permission::PiiRawRead),
        principal: principal.clone(),
        client_ip,
        policy: std::sync::Arc::clone(&policy),
        tenant_hmac_key,
        cedar: std::sync::Arc::clone(&state.cedar),
        audit: state.audit.clone(),
        store: state.store.clone(),
    };
    let loader = DataLoader::new(
        NeighborBatchLoader {
            tenant_id: principal.tenant_id,
            policy,
            store: state.store,
        },
        tokio::spawn,
    );

    let gql_req = gql_req.data(req_ctx).data(loader);
    let resp = schema.execute(gql_req).await;
    Ok(GraphQLResponse::from(resp))
}

/// Axum handler for `GET /api/xblue/graphql/sdl`.
pub(crate) async fn graphql_sdl_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<axum::Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataRead)?;

    let published = axpoint_domain::ports::StudioRepository::get_published_schema(
        &state.store,
        &principal.tenant_id,
        "main",
    )
    .await?;

    let schema =
        get_or_build_cached_schema(&principal.tenant_id, published.version, &published.doc)?;

    Ok(axum::Json(serde_json::json!({
        "version": published.version,
        "sdl": schema.sdl(),
    })))
}

#[derive(Clone)]
struct PageEnvelope {
    items: Vec<BlueNode>,
    next_cursor: Option<String>,
}

fn build_dynamic_schema(version: i32, doc: &SchemaDoc) -> Result<Schema, AppError> {
    let json_scalar = Scalar::new("JSON");
    let geo_obj = build_geo_object();

    let mut query_root = Object::new("Query").field(Field::new(
        "_schemaVersion",
        TypeRef::named_nn(TypeRef::INT),
        move |_| FieldFuture::new(async move { Ok(Some(GqlValue::from(version))) }),
    ));

    let known_node_names: BTreeSet<&str> =
        doc.node_types.iter().map(|nt| nt.name.as_str()).collect();
    let mut objects_to_register: Vec<Object> = vec![geo_obj];

    for nt in &doc.node_types {
        let obj = build_node_type_object(nt, doc, &known_node_names);
        let page_type_name = format!("{}Page", nt.name);
        let page_obj = build_page_object(&page_type_name, &nt.name);
        objects_to_register.push(obj);
        objects_to_register.push(page_obj);

        let single_field_name = lower_camel_case(&nt.name);
        let list_field_name = format!("{single_field_name}List");
        query_root = query_root
            .field(build_root_single_field(&single_field_name, &nt.name))
            .field(build_root_list_field(
                &list_field_name,
                &page_type_name,
                &nt.name,
            ));
    }

    let (mutation_root_opt, input_objects) = build_mutation_root_and_inputs(doc);
    let mutation_type_name = mutation_root_opt.as_ref().map(|_| "Mutation");

    let mut builder = Schema::build("Query", mutation_type_name, None)
        .register(json_scalar)
        .register(query_root);
    for obj in objects_to_register {
        builder = builder.register(obj);
    }
    if let Some(mut_root) = mutation_root_opt {
        builder = builder
            .register(build_action_payload_object())
            .register(mut_root);
        for in_obj in input_objects {
            builder = builder.register(in_obj);
        }
    }

    builder
        .limit_depth(MAX_GRAPHQL_DEPTH)
        .limit_complexity(MAX_GRAPHQL_COMPLEXITY)
        .finish()
        .map_err(|err| AppError::Database(format!("build dynamic GraphQL schema failed: {err}")))
}

fn build_geo_object() -> Object {
    Object::new("GeoPoint")
        .field(Field::new(
            "lng",
            TypeRef::named_nn(TypeRef::FLOAT),
            |ctx| {
                FieldFuture::new(async move {
                    let pt = ctx
                        .parent_value
                        .try_downcast_ref::<axpoint_domain::graph::GeoPoint>()?;
                    Ok(Some(
                        GqlValue::from_json(serde_json::json!(pt.lng)).unwrap_or(GqlValue::Null),
                    ))
                })
            },
        ))
        .field(Field::new(
            "lat",
            TypeRef::named_nn(TypeRef::FLOAT),
            |ctx| {
                FieldFuture::new(async move {
                    let pt = ctx
                        .parent_value
                        .try_downcast_ref::<axpoint_domain::graph::GeoPoint>()?;
                    Ok(Some(
                        GqlValue::from_json(serde_json::json!(pt.lat)).unwrap_or(GqlValue::Null),
                    ))
                })
            },
        ))
}

fn build_page_object(page_type_name: &str, item_type_name: &str) -> Object {
    Object::new(page_type_name)
        .field(Field::new(
            "items",
            TypeRef::named_nn_list_nn(item_type_name),
            |ctx| {
                FieldFuture::new(async move {
                    let page = ctx.parent_value.try_downcast_ref::<PageEnvelope>()?;
                    let items: Vec<FieldValue<'static>> = page
                        .items
                        .iter()
                        .cloned()
                        .map(FieldValue::owned_any)
                        .collect();
                    Ok(Some(FieldValue::list(items)))
                })
            },
        ))
        .field(Field::new(
            "nextCursor",
            TypeRef::named(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let page = ctx.parent_value.try_downcast_ref::<PageEnvelope>()?;
                    Ok(page.next_cursor.clone().map(GqlValue::String))
                })
            },
        ))
}

fn build_root_single_field(field_name: &str, type_name: &str) -> Field {
    let owned_type = String::from(type_name);
    Field::new(field_name, TypeRef::named(type_name), move |ctx| {
        let tname = owned_type.clone();
        FieldFuture::new(async move {
            let req_ctx = ctx.data::<GqlRequestCtx>()?;
            let node_key = ctx.args.try_get("nodeKey")?.string()?.to_owned();
            let node_opt = req_ctx
                .store
                .get_domain_object_by_key(&req_ctx.tenant_id, &tname, &node_key)
                .await
                .map_err(|e| GqlError::new(e.client_message()))?;
            let Some(node) = node_opt else {
                return Ok(None);
            };
            if !req_ctx.policy.bypass_row_policies {
                let visible = req_ctx
                    .store
                    .filter_visible_node_ids(
                        &req_ctx.tenant_id,
                        &[node.id],
                        &req_ctx.policy.node_types,
                    )
                    .await
                    .map_err(|e| GqlError::new(e.client_message()))?;
                if !visible.contains(&node.id) {
                    return Ok(None);
                }
            }
            Ok(Some(FieldValue::owned_any(node)))
        })
    })
    .argument(InputValue::new(
        "nodeKey",
        TypeRef::named_nn(TypeRef::STRING),
    ))
}

fn build_root_list_field(field_name: &str, page_type_name: &str, type_name: &str) -> Field {
    let owned_type = String::from(type_name);
    Field::new(field_name, TypeRef::named_nn(page_type_name), move |ctx| {
        let tname = owned_type.clone();
        FieldFuture::new(async move {
            let req_ctx = ctx.data::<GqlRequestCtx>()?;
            let q = ctx
                .args
                .get("q")
                .and_then(|v| v.string().ok())
                .map(str::to_owned);
            let cursor = ctx
                .args
                .get("cursor")
                .and_then(|v| v.string().ok())
                .map(str::to_owned);
            let limit = ctx
                .args
                .get("limit")
                .and_then(|v| v.i64().ok())
                .and_then(|n| u32::try_from(n.clamp(1, 200)).ok())
                .or(Some(50));
            let filter = match ctx.args.get("filter") {
                Some(val) => {
                    let jv = val
                        .deserialize::<serde_json::Value>()
                        .map_err(|e| GqlError::new(format!("invalid filter JSON: {e:?}")))?;
                    if jv.is_null() {
                        None
                    } else {
                        Some(jv.to_string())
                    }
                }
                None => None,
            };

            let (mut items, next_cursor) = req_ctx
                .store
                .query_domain_objects_list(
                    &req_ctx.tenant_id,
                    &tname,
                    &DomainObjectListQuery {
                        q,
                        filter,
                        limit,
                        cursor,
                    },
                )
                .await
                .map_err(|e| GqlError::new(e.client_message()))?;

            if !req_ctx.policy.bypass_row_policies && !items.is_empty() {
                let ids: Vec<uuid::Uuid> = items.iter().map(|n| n.id).collect();
                let visible = req_ctx
                    .store
                    .filter_visible_node_ids(&req_ctx.tenant_id, &ids, &req_ctx.policy.node_types)
                    .await
                    .map_err(|e| GqlError::new(e.client_message()))?;
                items.retain(|n| visible.contains(&n.id));
            }

            Ok(Some(FieldValue::owned_any(PageEnvelope {
                items,
                next_cursor,
            })))
        })
    })
    .argument(InputValue::new("filter", TypeRef::named("JSON")))
    .argument(InputValue::new("q", TypeRef::named(TypeRef::STRING)))
    .argument(
        InputValue::new("limit", TypeRef::named(TypeRef::INT)).default_value(GqlValue::from(50)),
    )
    .argument(InputValue::new("cursor", TypeRef::named(TypeRef::STRING)))
}

fn build_node_type_object(
    nt: &NodeTypeSpec,
    doc: &SchemaDoc,
    known_node_names: &BTreeSet<&str>,
) -> Object {
    let mut used_fields: BTreeSet<String> = BTreeSet::from([
        String::from("id"),
        String::from("nodeKey"),
        String::from("title"),
        String::from("geo"),
        String::from("updatedAt"),
    ]);

    let mut obj = Object::new(&nt.name)
        .description(&nt.description)
        .field(Field::new("id", TypeRef::named_nn(TypeRef::ID), |ctx| {
            FieldFuture::new(async move {
                let node = ctx.parent_value.try_downcast_ref::<BlueNode>()?;
                Ok(Some(GqlValue::String(node.id.to_string())))
            })
        }))
        .field(Field::new(
            "nodeKey",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let node = ctx.parent_value.try_downcast_ref::<BlueNode>()?;
                    Ok(Some(GqlValue::String(node.node_key.clone())))
                })
            },
        ))
        .field(Field::new(
            "title",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let node = ctx.parent_value.try_downcast_ref::<BlueNode>()?;
                    Ok(Some(GqlValue::String(node.title.clone())))
                })
            },
        ))
        .field(Field::new("geo", TypeRef::named("GeoPoint"), |ctx| {
            FieldFuture::new(async move {
                let node = ctx.parent_value.try_downcast_ref::<BlueNode>()?;
                Ok(node.geo.map(FieldValue::owned_any))
            })
        }))
        .field(Field::new(
            "updatedAt",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let node = ctx.parent_value.try_downcast_ref::<BlueNode>()?;
                    Ok(Some(GqlValue::String(node.updated_at.to_rfc3339())))
                })
            },
        ));

    for prop in &nt.properties {
        let fname = snake_to_camel(&prop.name);
        if fname.is_empty() || !used_fields.insert(fname.clone()) {
            continue;
        }
        obj = obj.field(build_property_field(&fname, prop));
    }

    for sig in compute_neighbor_sigs(nt, doc) {
        if !known_node_names.contains(sig.target_type.as_str())
            || !used_fields.insert(sig.field_name.clone())
        {
            continue;
        }
        obj = obj.field(build_neighbor_field(&sig));
    }

    obj
}

fn build_property_field(field_name: &str, prop: &PropertySpec) -> Field {
    let prop_clone = prop.clone();
    let type_ref = graphql_type_for_property(prop);
    Field::new(field_name, type_ref, move |ctx| {
        let p = prop_clone.clone();
        FieldFuture::new(async move {
            let req_ctx = ctx.data::<GqlRequestCtx>()?;
            let node = ctx.parent_value.try_downcast_ref::<BlueNode>()?;
            Ok(resolve_property_gql_value(node, &p, req_ctx))
        })
    })
}

fn graphql_type_for_property(prop: &PropertySpec) -> TypeRef {
    if prop.kind == PropertyKind::Metric {
        return TypeRef::named(TypeRef::FLOAT);
    }
    match prop.data_type {
        DataType::Int64 => TypeRef::named(TypeRef::INT),
        DataType::Float64 | DataType::Decimal => TypeRef::named(TypeRef::FLOAT),
        DataType::Boolean => TypeRef::named(TypeRef::BOOLEAN),
        DataType::String | DataType::Date | DataType::Timestamp | DataType::Binary => {
            TypeRef::named(TypeRef::STRING)
        }
        DataType::Json | DataType::List | DataType::Struct => TypeRef::named("JSON"),
    }
}

fn resolve_property_gql_value(
    node: &BlueNode,
    prop: &PropertySpec,
    req_ctx: &GqlRequestCtx,
) -> Option<GqlValue> {
    if prop.kind == PropertyKind::Metric {
        let val = node.metrics.get(&prop.name).copied()?;
        return GqlValue::from_json(serde_json::json!(val)).ok();
    }
    let raw_val = node.properties.get(&prop.name)?;
    if raw_val.is_null() {
        return None;
    }

    let mask_rule = if let Some(spec) = req_ctx.policy.node_types.get(&node.type_name) {
        spec.property_masks
            .get(&prop.name)
            .copied()
            .or_else(|| {
                spec.property_masks
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(&prop.name))
                    .map(|(_, &r)| r)
            })
            .unwrap_or(axpoint_domain::enums::MaskRule::None)
    } else if prop.pii_category != PiiCategory::None && !req_ctx.has_pii_raw {
        axpoint_domain::enums::MaskRule::from_pii_category(prop.pii_category)
    } else {
        axpoint_domain::enums::MaskRule::None
    };

    if mask_rule != axpoint_domain::enums::MaskRule::None {
        let raw_text = match raw_val {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let text = axpoint_security::masking::apply_mask_rule(
            mask_rule,
            &raw_text,
            &req_ctx.tenant_hmac_key,
        );
        return match prop.data_type {
            DataType::String
            | DataType::Date
            | DataType::Timestamp
            | DataType::Binary
            | DataType::Json => Some(GqlValue::String(text)),
            _ => None,
        };
    }

    coerce_json_to_gql_scalar(raw_val, prop.data_type)
}

fn coerce_json_to_gql_scalar(val: &serde_json::Value, dt: DataType) -> Option<GqlValue> {
    match dt {
        DataType::Int64 => val
            .as_i64()
            .or_else(|| val.as_str().and_then(|s| s.parse::<i64>().ok()))
            .map(GqlValue::from),
        DataType::Float64 | DataType::Decimal => val
            .as_f64()
            .or_else(|| val.as_str().and_then(|s| s.parse::<f64>().ok()))
            .and_then(|f| GqlValue::from_json(serde_json::json!(f)).ok()),
        DataType::Boolean => val
            .as_bool()
            .or_else(|| val.as_str().and_then(|s| s.parse::<bool>().ok()))
            .map(GqlValue::Boolean),
        DataType::String | DataType::Date | DataType::Timestamp | DataType::Binary => match val {
            serde_json::Value::String(s) => Some(GqlValue::String(s.clone())),
            other => Some(GqlValue::String(other.to_string())),
        },
        DataType::Json | DataType::List | DataType::Struct => GqlValue::from_json(val.clone()).ok(),
    }
}

fn build_neighbor_field(sig: &ObjectNeighborSig) -> Field {
    let link_type = sig.link_type.clone();
    let direction = sig.direction;
    let target_type = sig.target_type.clone();

    Field::new(
        &sig.field_name,
        TypeRef::named_nn_list_nn(&sig.target_type),
        move |ctx| {
            let lt = link_type.clone();
            let tt = target_type.clone();
            FieldFuture::new(async move {
                let loader = ctx.data::<DataLoader<NeighborBatchLoader>>()?;
                let parent = ctx.parent_value.try_downcast_ref::<BlueNode>()?;
                let limit = ctx
                    .args
                    .get("limit")
                    .and_then(|v| v.i64().ok())
                    .and_then(|n| u32::try_from(n.clamp(1, 200)).ok())
                    .unwrap_or(50);

                let nodes = loader
                    .load_one(NeighborBatchKey {
                        parent_id: parent.id,
                        link_type: lt,
                        direction,
                        target_type: tt,
                        limit,
                    })
                    .await
                    .map_err(GqlError::new)?
                    .unwrap_or_default();

                let items: Vec<FieldValue<'static>> =
                    nodes.into_iter().map(FieldValue::owned_any).collect();
                Ok(Some(FieldValue::list(items)))
            })
        },
    )
    .argument(
        InputValue::new("limit", TypeRef::named(TypeRef::INT)).default_value(GqlValue::from(50)),
    )
}

/// Computes outgoing and incoming 1-hop GraphQL/REST neighbor signatures for `nt` (`06-STEP05-06` §05.6).
pub(crate) fn compute_neighbor_sigs(nt: &NodeTypeSpec, doc: &SchemaDoc) -> Vec<ObjectNeighborSig> {
    let mut out = Vec::new();
    let mut seen_names = BTreeSet::new();

    for edge in &doc.edge_types {
        if edge.src_type_name == nt.name {
            let fname = format!(
                "{}{}",
                upper_snake_to_camel(&edge.name),
                pluralize_pascal(&edge.dst_type_name)
            );
            if seen_names.insert(fname.clone()) {
                out.push(ObjectNeighborSig {
                    field_name: fname,
                    link_type: edge.name.clone(),
                    direction: LinkDirection::Out,
                    target_type: edge.dst_type_name.clone(),
                    cardinality: edge.cardinality,
                });
            }
        }
        if edge.dst_type_name == nt.name {
            let fname = format!(
                "in_{}{}",
                upper_snake_to_camel(&edge.name),
                pluralize_pascal(&edge.src_type_name)
            );
            if seen_names.insert(fname.clone()) {
                out.push(ObjectNeighborSig {
                    field_name: fname,
                    link_type: edge.name.clone(),
                    direction: LinkDirection::In,
                    target_type: edge.src_type_name.clone(),
                    cardinality: edge.cardinality,
                });
            }
        }
    }
    out
}

/// Applies `AxGuard` `PolicyContext` property masking (including `sec_column_policies` overrides
/// and `HMAC_HASH`) to a runtime node's static attributes.
pub(crate) fn mask_node_attributes_with_policy(
    node: &BlueNode,
    nt: Option<&NodeTypeSpec>,
    policy: Option<&axpoint_domain::guard::PolicyContext>,
    has_pii_raw: bool,
    tenant_hmac_key: &[u8; 32],
) -> serde_json::Map<String, serde_json::Value> {
    let mut attrs = node.properties.clone();
    let masks =
        axpoint_server::policy::effective_property_masks(&node.type_name, nt, policy, has_pii_raw);
    axpoint_security::masking::mask_json_properties(&mut attrs, &masks, tenant_hmac_key);
    attrs
}

/// Converts `PascalCase` (`Warehouse`, `SalesOrder`) to `lowerCamelCase` (`warehouse`, `salesOrder`).
pub(crate) fn lower_camel_case(name: &str) -> String {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::with_capacity(name.len());
    for c in first.to_lowercase() {
        out.push(c);
    }
    out.extend(chars);
    out
}

/// Converts `snake_case` (`fill_rate`, `inventory`) to `lowerCamelCase` (`fillRate`, `inventory`).
pub(crate) fn snake_to_camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper_next = false;
    for ch in name.chars() {
        if ch == '_' {
            upper_next = true;
        } else if upper_next {
            for uc in ch.to_uppercase() {
                out.push(uc);
            }
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn upper_snake_to_camel(name: &str) -> String {
    let lowered = name.to_ascii_lowercase();
    snake_to_camel(&lowered)
}

fn pluralize_pascal(type_name: &str) -> String {
    if type_name.ends_with('s')
        || type_name.ends_with('x')
        || type_name.ends_with("ch")
        || type_name.ends_with("sh")
    {
        format!("{type_name}es")
    } else if let Some(stem) = type_name.strip_suffix('y') {
        let ends_vowel_y = stem
            .chars()
            .last()
            .is_some_and(|c| matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u'));
        if ends_vowel_y {
            format!("{type_name}s")
        } else {
            format!("{stem}ies")
        }
    } else {
        format!("{type_name}s")
    }
}

fn pascal_from_ident(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut upper_next = true;
    for ch in raw.chars() {
        if ch == '_' {
            upper_next = true;
        } else if upper_next {
            for uc in ch.to_uppercase() {
                out.push(uc);
            }
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn payload_usize_field(name: &str, f: fn(&InvokeObjectActionResponse) -> usize) -> Field {
    Field::new(name, TypeRef::named_nn(TypeRef::INT), move |ctx| {
        FieldFuture::new(async move {
            let r = ctx
                .parent_value
                .try_downcast_ref::<InvokeObjectActionResponse>()?;
            let v = i32::try_from(f(r)).unwrap_or(i32::MAX);
            Ok(Some(GqlValue::from(v)))
        })
    })
}

fn build_action_payload_object() -> Object {
    Object::new("ObjectActionPayload")
        .field(Field::new(
            "status",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let r = ctx
                        .parent_value
                        .try_downcast_ref::<InvokeObjectActionResponse>()?;
                    Ok(Some(GqlValue::String(r.status.clone())))
                })
            },
        ))
        .field(payload_usize_field("mutatedNodes", |r| r.mutated_nodes))
        .field(payload_usize_field("mutatedEdges", |r| r.mutated_links))
        .field(payload_usize_field("mutatedLinks", |r| r.mutated_links))
        .field(payload_usize_field("recomputedMetrics", |r| {
            r.recomputed_metrics
        }))
        .field(Field::new(
            "actionLogId",
            TypeRef::named(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let r = ctx
                        .parent_value
                        .try_downcast_ref::<InvokeObjectActionResponse>()?;
                    Ok(r.action_log_id.map(GqlValue::from))
                })
            },
        ))
        .field(Field::new(
            "approvalId",
            TypeRef::named(TypeRef::ID),
            |ctx| {
                FieldFuture::new(async move {
                    let r = ctx
                        .parent_value
                        .try_downcast_ref::<InvokeObjectActionResponse>()?;
                    Ok(r.approval_id.map(|id| GqlValue::String(id.to_string())))
                })
            },
        ))
        .field(Field::new(
            "idempotencyReplay",
            TypeRef::named_nn(TypeRef::BOOLEAN),
            |ctx| {
                FieldFuture::new(async move {
                    let r = ctx
                        .parent_value
                        .try_downcast_ref::<InvokeObjectActionResponse>()?;
                    Ok(Some(GqlValue::Boolean(r.idempotency_replay)))
                })
            },
        ))
}

fn build_mutation_root_and_inputs(doc: &SchemaDoc) -> (Option<Object>, Vec<InputObject>) {
    let mut mut_root = Object::new("Mutation");
    let mut input_objects = Vec::new();
    let mut used_fields = BTreeSet::new();

    for nt in &doc.node_types {
        let type_camel = lower_camel_case(&nt.name);
        for action in &nt.actions {
            if action.maturity != DesignMaturity::Confirmed {
                continue;
            }
            let act_camel = snake_to_camel(&action.name);
            let field_name = format!("{type_camel}_{act_camel}");
            if field_name.is_empty() || !used_fields.insert(field_name.clone()) {
                continue;
            }
            let input_type_opt = if action.parameters.is_empty() {
                None
            } else {
                let in_name = format!("{}{}Input", nt.name, pascal_from_ident(&action.name));
                input_objects.push(build_action_input_object(&in_name, &action.parameters));
                Some(in_name)
            };
            mut_root = mut_root.field(build_action_mutation_field(
                &field_name,
                &nt.name,
                action,
                input_type_opt.as_deref(),
            ));
        }
    }

    if used_fields.is_empty() {
        (None, Vec::new())
    } else {
        (Some(mut_root), input_objects)
    }
}

fn build_action_input_object(input_name: &str, params: &[ActionParamSpec]) -> InputObject {
    let mut in_obj = InputObject::new(input_name);
    let mut seen = BTreeSet::new();
    for p in params {
        let gql_name = snake_to_camel(&p.name);
        if gql_name.is_empty() || !seen.insert(gql_name.clone()) {
            continue;
        }
        let base_scalar = match p.data_type {
            DataType::Int64 => TypeRef::INT,
            DataType::Float64 | DataType::Decimal => TypeRef::FLOAT,
            DataType::Boolean => TypeRef::BOOLEAN,
            DataType::String | DataType::Date | DataType::Timestamp | DataType::Binary => {
                TypeRef::STRING
            }
            DataType::Json | DataType::List | DataType::Struct => "JSON",
        };
        let tref = if p.required && p.default_value.is_none() {
            TypeRef::named_nn(base_scalar)
        } else {
            TypeRef::named(base_scalar)
        };
        in_obj = in_obj.field(InputValue::new(gql_name, tref));
    }
    in_obj
}

fn build_action_mutation_field(
    field_name: &str,
    type_name: &str,
    action: &ActionSpec,
    input_type_name: Option<&str>,
) -> Field {
    let owned_type = String::from(type_name);
    let owned_action = action.clone();
    let mut field = Field::new(
        field_name,
        TypeRef::named_nn("ObjectActionPayload"),
        move |ctx| {
            let tname = owned_type.clone();
            let act = owned_action.clone();
            FieldFuture::new(async move {
                let req_ctx = ctx.data::<GqlRequestCtx>()?;
                let key = ctx.args.try_get("key")?.string()?.to_owned();
                let idempotency_key = ctx.args.try_get("idempotencyKey")?.string()?.to_owned();
                let expected_version = ctx.args.get("expectedVersion").and_then(|v| v.i64().ok());
                let dry_run = ctx
                    .args
                    .get("dryRun")
                    .and_then(|v| v.boolean().ok())
                    .unwrap_or(false);
                let inputs = extract_gql_action_inputs(ctx.args.get("input"), &act.parameters)?;
                let resp = execute_gql_object_action(
                    req_ctx,
                    &tname,
                    &key,
                    &act,
                    InvokeObjectActionRequest {
                        inputs,
                        idempotency_key,
                        expected_version,
                        dry_run,
                    },
                )
                .await
                .map_err(|e| GqlError::new(e.client_message()))?;
                Ok(Some(FieldValue::owned_any(resp)))
            })
        },
    )
    .description(&action.description)
    .argument(InputValue::new("key", TypeRef::named_nn(TypeRef::STRING)))
    .argument(InputValue::new(
        "idempotencyKey",
        TypeRef::named_nn(TypeRef::STRING),
    ))
    .argument(InputValue::new(
        "expectedVersion",
        TypeRef::named(TypeRef::INT),
    ))
    .argument(InputValue::new("dryRun", TypeRef::named(TypeRef::BOOLEAN)));

    if let Some(in_name) = input_type_name {
        field = field.argument(InputValue::new("input", TypeRef::named_nn(in_name)));
    } else {
        field = field.argument(InputValue::new("input", TypeRef::named("JSON")));
    }
    field
}

fn extract_gql_action_inputs(
    input_arg: Option<async_graphql::dynamic::ValueAccessor<'_>>,
    params: &[ActionParamSpec],
) -> Result<serde_json::Value, GqlError> {
    let Some(val_acc) = input_arg else {
        return Ok(serde_json::json!({}));
    };
    let raw_json = val_acc
        .deserialize::<serde_json::Value>()
        .map_err(|e| GqlError::new(format!("invalid action input JSON: {e:?}")))?;
    let Some(raw_map) = raw_json.as_object() else {
        return Ok(serde_json::json!({}));
    };

    if params.is_empty() {
        return Ok(raw_json);
    }

    let mut normalized = serde_json::Map::with_capacity(raw_map.len());
    for (k, v) in raw_map {
        let mapped = params
            .iter()
            .find(|p| p.name == *k || snake_to_camel(&p.name) == *k)
            .map_or_else(|| k.clone(), |p| p.name.clone());
        normalized.insert(mapped, v.clone());
    }
    Ok(serde_json::Value::Object(normalized))
}

async fn execute_gql_object_action(
    req_ctx: &GqlRequestCtx,
    type_name: &str,
    node_key: &str,
    action: &ActionSpec,
    req: InvokeObjectActionRequest,
) -> Result<InvokeObjectActionResponse, AppError> {
    req_ctx.principal.enforce_no_prod_mutation()?;
    req_ctx.principal.require(Permission::FlowRun)?;
    if action.required_permission != "ACTION_EXECUTE"
        && let Ok(extra_perm) = Permission::from_str(action.required_permission.trim())
    {
        req_ctx.principal.require(extra_perm)?;
    }

    let _ = axpoint_server::policy::authorize_resource_for_store(
        &req_ctx.store,
        &req_ctx.cedar,
        &req_ctx.principal,
        Permission::FlowRun,
        axpoint_domain::guard::GrantResourceKind::NodeType,
        type_name,
        &req_ctx.client_ip,
    )
    .await?;

    if !req_ctx.policy.bypass_row_policies
        && let Some(target_node) = req_ctx
            .store
            .get_domain_object_by_key(&req_ctx.tenant_id, type_name, node_key)
            .await?
    {
        let visible = req_ctx
            .store
            .filter_visible_node_ids(
                &req_ctx.tenant_id,
                &[target_node.id],
                &req_ctx.policy.node_types,
            )
            .await?;
        if !visible.contains(&target_node.id) {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
    }

    let actor = (!req_ctx.principal.user_id.is_nil()).then_some(req_ctx.principal.user_id);
    let digest = hex::encode(Sha256::digest(req.inputs.to_string().as_bytes()));
    let idem_key = req.idempotency_key.clone();
    let resp = req_ctx
        .store
        .invoke_object_action(
            &req_ctx.tenant_id,
            actor,
            type_name,
            node_key,
            &action.name,
            req,
        )
        .await?;

    req_ctx.audit.record(AuditRecord {
        tenant_id: req_ctx.tenant_id.clone(),
        user_id: actor,
        username: req_ctx.principal.username.clone(),
        action: AuditAction::ObjectActionInvoke,
        resource_ref: format!("{type_name}:{node_key}:{}", action.name),
        status_code: if resp.status == "WAITING_APPROVAL" {
            202
        } else {
            200
        },
        client_ip: req_ctx.client_ip.clone(),
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "channel": "GRAPHQL",
            "status": &resp.status,
            "idempotency_key": idem_key,
            "idempotency_replay": resp.idempotency_replay,
            "payload_sha256": digest,
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = req_ctx.audit.flush().await {
        tracing::warn!(error = %err, "graphql object action audit flush failed");
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use axpoint_domain::enums::Cardinality;
    use axpoint_domain::studio::EdgeTypeSpec;

    use super::*;

    fn sample_doc() -> SchemaDoc {
        SchemaDoc {
            node_types: vec![
                NodeTypeSpec {
                    id: None,
                    name: String::from("Warehouse"),
                    display_name: String::from("仓库"),
                    description: String::new(),
                    icon: String::from("box"),
                    color: String::from("#2563eb"),
                    title_template: String::from("{node_key}"),
                    properties: vec![PropertySpec {
                        name: String::from("inventory"),
                        display_name: String::from("库存"),
                        description: String::new(),
                        unit: None,
                        data_type: DataType::Float64,
                        kind: PropertyKind::Metric,
                        required: false,
                        pii_category: PiiCategory::None,
                        allowed_values: None,
                        maturity: DesignMaturity::Confirmed,
                    }],
                    agg_rules: Vec::new(),
                    derived_rules: Vec::new(),
                    sync_to_knowledge: false,
                    pos_x: 0.0,
                    pos_y: 0.0,
                    maturity: DesignMaturity::Confirmed,
                    semantic_layer_tier: None,
                    domain_group: None,
                    actions: Vec::new(),
                },
                NodeTypeSpec {
                    id: None,
                    name: String::from("Order"),
                    display_name: String::from("订单"),
                    description: String::new(),
                    icon: String::from("box"),
                    color: String::from("#16a34a"),
                    title_template: String::from("{node_key}"),
                    properties: Vec::new(),
                    agg_rules: Vec::new(),
                    derived_rules: Vec::new(),
                    sync_to_knowledge: false,
                    pos_x: 100.0,
                    pos_y: 0.0,
                    maturity: DesignMaturity::Confirmed,
                    semantic_layer_tier: None,
                    domain_group: None,
                    actions: Vec::new(),
                },
            ],
            edge_types: vec![EdgeTypeSpec {
                id: None,
                name: String::from("FULFILLS"),
                src_type_name: String::from("Warehouse"),
                dst_type_name: String::from("Order"),
                impact_weight: 0.8,
                cardinality: Cardinality::OneToMany,
                description: String::new(),
                properties: Vec::new(),
                propagation: serde_json::json!({}),
                maturity: DesignMaturity::Confirmed,
                derived_rules: Vec::new(),
                agg_rules: Vec::new(),
            }],
            mappings: Vec::new(),
        }
    }

    #[tokio::test]
    async fn dynamic_graphql_schema_enforces_depth_limit_and_generates_neighbor_fields()
    -> Result<(), AppError> {
        let doc = sample_doc();
        let wh = doc.node_types.first().ok_or(AppError::NotFound("WH"))?;
        let sigs = compute_neighbor_sigs(wh, &doc);
        assert_eq!(
            sigs.first().map(|s| s.field_name.as_str()),
            Some("fulfillsOrders")
        );

        let schema = build_dynamic_schema(1, &doc)?;
        let ok_ver = schema.execute(GqlRequest::new("{ _schemaVersion }")).await;
        assert_eq!(ok_ver.errors.len(), 0);

        let too_deep = "{ warehouse(nodeKey: \"W-1\") { fulfillsOrders { in_fulfillsWarehouses { fulfillsOrders { in_fulfillsWarehouses { fulfillsOrders { in_fulfillsWarehouses { nodeKey } } } } } } } }";
        let deep_resp = schema.execute(GqlRequest::new(too_deep)).await;
        assert!(
            !deep_resp.errors.is_empty(),
            "expected query exceeding depth 6 to be rejected"
        );
        Ok(())
    }
}
