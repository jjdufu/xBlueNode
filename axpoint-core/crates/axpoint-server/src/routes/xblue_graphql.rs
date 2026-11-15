//! Auto-generated per-`(tenant_id, main_version)` dynamic GraphQL schema & batched neighbor `DataLoader`
//! for `POST /api/xblue/graphql` (`06-STEP05-06` §05.6.1).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock, PoisonError};

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::dynamic::{
    Field, FieldFuture, FieldValue, InputValue, Object, Scalar, Schema, TypeRef,
};
use async_graphql::{Error as GqlError, Request as GqlRequest, Value as GqlValue, Variables};
use async_graphql_axum::GraphQLResponse;
use axpoint_analytics::mask_pii_str;
use axpoint_db::PgStore;
use axpoint_db::studio_repo::NeighborBatchKey;
use axpoint_domain::AppError;
use axpoint_domain::enums::{DataType, Permission, PiiCategory};
use axpoint_domain::studio::{
    DomainObjectListQuery, LinkDirection, NodeTypeSpec, ObjectNeighborSig, PropertyKind,
    PropertySpec, SchemaDoc,
};
use axpoint_domain::topology::BlueNode;
use axpoint_security::AuthPrincipal;
use axum::extract::State;
use serde::Deserialize;

use crate::state::{ApiJson, AppState, HttpError};

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
    has_pii_raw: bool,
    store: PgStore,
}

/// Per-request `async_graphql` `DataLoader` eliminating 1-hop neighbor N+1 queries.
struct NeighborBatchLoader {
    tenant_id: String,
    store: PgStore,
}

impl Loader<NeighborBatchKey> for NeighborBatchLoader {
    type Value = Vec<BlueNode>;
    type Error = String;

    async fn load(
        &self,
        keys: &[NeighborBatchKey],
    ) -> Result<HashMap<NeighborBatchKey, Self::Value>, Self::Error> {
        self.store
            .batch_load_neighbors(&self.tenant_id, keys)
            .await
            .map_err(|err| err.client_message())
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
    principal: AuthPrincipal,
    ApiJson(payload): ApiJson<GraphqlHttpPayload>,
) -> Result<GraphQLResponse, HttpError> {
    principal.require(Permission::DataRead)?;

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
        store: state.store.clone(),
    };
    let loader = DataLoader::new(
        NeighborBatchLoader {
            tenant_id: principal.tenant_id,
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

    let mut builder = Schema::build("Query", None, None)
        .register(json_scalar)
        .register(query_root);
    for obj in objects_to_register {
        builder = builder.register(obj);
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
            Ok(node_opt.map(FieldValue::owned_any))
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

            let (items, next_cursor) = req_ctx
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
            Ok(resolve_property_gql_value(node, &p, req_ctx.has_pii_raw))
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
    has_pii_raw: bool,
) -> Option<GqlValue> {
    if prop.kind == PropertyKind::Metric {
        let val = node.metrics.get(&prop.name).copied()?;
        return GqlValue::from_json(serde_json::json!(val)).ok();
    }
    let raw_val = node.properties.get(&prop.name)?;
    if raw_val.is_null() {
        return None;
    }
    if prop.pii_category != PiiCategory::None && !has_pii_raw {
        let text = match raw_val {
            serde_json::Value::String(s) => mask_pii_str(s, prop.pii_category),
            other => mask_pii_str(&other.to_string(), prop.pii_category),
        };
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

/// Applies `AxGuard` PII masking to a runtime node's static attributes based on `NodeTypeSpec`.
pub(crate) fn mask_node_attributes(
    node: &BlueNode,
    nt: Option<&NodeTypeSpec>,
    has_pii_raw: bool,
) -> serde_json::Map<String, serde_json::Value> {
    let mut attrs = node.properties.clone();
    if has_pii_raw {
        return attrs;
    }
    let Some(spec) = nt else {
        return attrs;
    };
    for prop in &spec.properties {
        if prop.pii_category == PiiCategory::None {
            continue;
        }
        if let Some(val) = attrs.get_mut(&prop.name)
            && !val.is_null()
        {
            let masked = match &*val {
                serde_json::Value::String(s) => mask_pii_str(s, prop.pii_category),
                other => mask_pii_str(&other.to_string(), prop.pii_category),
            };
            *val = serde_json::Value::String(masked);
        }
    }
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
                    }],
                    agg_rules: Vec::new(),
                    derived_rules: Vec::new(),
                    sync_to_knowledge: false,
                    pos_x: 0.0,
                    pos_y: 0.0,
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
