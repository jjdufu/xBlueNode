//! `OpenAPI` 3.1.0 specification and external agent `SkillManifest` generators (`08-STEP09` §09.3.3).

use axpoint_domain::agent::{EndpointSpec, SkillInvokeSpec, SkillManifest};
use axpoint_domain::enums::{EndpointKind, Environment, OutputShape};

/// Formats an [`Environment`] into the lowercase path segment used by `/api/published/{env}/...`.
#[must_use]
pub const fn env_path_segment(environment: Environment) -> &'static str {
    match environment {
        Environment::Test => "test",
        Environment::Prod => "prod",
    }
}

/// Generates an `OpenAPI` `3.1.0` JSON document describing `endpoints`.
///
/// Each enabled endpoint produces `POST /api/published/{env}/{tenant_id}/{slug}`, and `FLOW`
/// endpoints additionally document `GET /api/published/{env}/{tenant_id}/{slug}/runs/{run_id}`.
#[must_use]
pub fn build_openapi_spec(
    endpoints: &[EndpointSpec],
    server_url: Option<&str>,
) -> serde_json::Value {
    let mut paths = serde_json::Map::new();
    for ep in endpoints {
        if !ep.enabled {
            continue;
        }
        let env_seg = env_path_segment(ep.environment);
        let invoke_path = format!("/api/published/{env_seg}/{}/{}", ep.tenant_id, ep.slug);
        paths.insert(invoke_path, build_post_path_item(ep, env_seg));

        if ep.kind == EndpointKind::Flow {
            let poll_path = format!(
                "/api/published/{env_seg}/{}/{}/runs/{{run_id}}",
                ep.tenant_id, ep.slug
            );
            paths.insert(poll_path, build_flow_poll_path_item(ep, env_seg));
        }
    }

    let mut doc = serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "AxPoint Published Capabilities API",
            "version": "1.0.0",
            "description": "Tenant-isolated capabilities published via AxAgent CapabilityPublisher."
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "PersonalAccessToken (axp_...)"
                }
            }
        },
        "security": [{ "bearerAuth": [] }],
        "paths": paths
    });

    if let Some(url) = server_url.map(str::trim).filter(|s| !s.is_empty())
        && let Some(obj) = doc.as_object_mut()
    {
        obj.insert(
            String::from("servers"),
            serde_json::json!([{ "url": url.trim_end_matches('/') }]),
        );
    }
    doc
}

/// Generates an external agent [`SkillManifest`] (`GET /api/axagent/endpoints/{id}/skill.json`).
#[must_use]
pub fn build_skill_manifest(endpoint: &EndpointSpec, base_url: &str) -> SkillManifest {
    let env_seg = env_path_segment(endpoint.environment);
    let prefix = base_url.trim().trim_end_matches('/');
    let url = format!(
        "{prefix}/api/published/{env_seg}/{}/{}",
        endpoint.tenant_id, endpoint.slug
    );
    let description = if endpoint.description.trim().is_empty() {
        endpoint.title.clone()
    } else {
        endpoint.description.clone()
    };

    SkillManifest {
        name: endpoint.slug.replace('-', "_"),
        description,
        input_schema: endpoint.input_schema.clone(),
        invoke: SkillInvokeSpec {
            method: String::from("POST"),
            url,
        },
        auth: String::from("bearer"),
    }
}

fn build_post_path_item(ep: &EndpointSpec, env_seg: &str) -> serde_json::Value {
    let op_id = format!("invoke_{env_seg}_{}", ep.slug.replace('-', "_"));
    let ok_code = if ep.kind == EndpointKind::Flow {
        "202"
    } else {
        "200"
    };
    serde_json::json!({
        "post": {
            "operationId": op_id,
            "summary": ep.title,
            "description": ep.description,
            "tags": [ep.kind.as_str()],
            "security": [{ "bearerAuth": [] }],
            "x-required-scope": ep.required_scope,
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": ep.input_schema
                    }
                }
            },
            "responses": {
                (ok_code): {
                    "description": "Capability invocation succeeded",
                    "content": {
                        "application/json": {
                            "schema": output_shape_schema(ep.output_shape)
                        }
                    }
                },
                "400": { "description": "SCHEMA_INVALID or INVALID_ARGUMENT" },
                "401": { "description": "TOKEN_MISSING or TOKEN_INVALID" },
                "403": { "description": "PERMISSION_DENIED" },
                "404": { "description": "NOT_FOUND" },
                "429": { "description": "RATE_LIMITED" }
            }
        }
    })
}

fn build_flow_poll_path_item(ep: &EndpointSpec, env_seg: &str) -> serde_json::Value {
    let op_id = format!("get_run_{env_seg}_{}", ep.slug.replace('-', "_"));
    serde_json::json!({
        "get": {
            "operationId": op_id,
            "summary": format!("Poll flow run status for {}", ep.title),
            "tags": ["FLOW"],
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                {
                    "name": "run_id",
                    "in": "path",
                    "required": true,
                    "schema": { "type": "string", "format": "uuid" }
                }
            ],
            "responses": {
                "200": {
                    "description": "Flow run record",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "required": ["id", "status"],
                                "properties": {
                                    "id": { "type": "string", "format": "uuid" },
                                    "status": { "type": "string" },
                                    "outputs": {}
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn output_shape_schema(shape: OutputShape) -> serde_json::Value {
    match shape {
        OutputShape::Rows => serde_json::json!({
            "type": "object",
            "required": ["columns", "rows"],
            "properties": {
                "columns": { "type": "array", "items": { "type": "object" } },
                "rows": { "type": "array", "items": { "type": "object" } }
            }
        }),
        OutputShape::Object => serde_json::json!({
            "type": "object",
            "required": ["items"],
            "properties": {
                "items": { "type": "array", "items": { "type": "object" } },
                "next_cursor": { "type": ["string", "null"] }
            }
        }),
        OutputShape::Text => serde_json::json!({
            "type": "object",
            "required": ["conversation_id", "status", "answer"],
            "properties": {
                "conversation_id": { "type": "string", "format": "uuid" },
                "status": { "type": "string" },
                "answer": { "type": "string" },
                "structured": {}
            }
        }),
        OutputShape::Run => serde_json::json!({
            "type": "object",
            "required": ["run_id", "status"],
            "properties": {
                "run_id": { "type": "string", "format": "uuid" },
                "status": { "type": "string" },
                "existing": { "type": "boolean" }
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use axpoint_domain::agent::EndpointSpec;
    use axpoint_domain::enums::{EndpointKind, Environment, OutputShape, Permission};
    use chrono::Utc;
    use uuid::Uuid;

    use super::{build_openapi_spec, build_skill_manifest};

    fn sample_endpoint(slug: &str, kind: EndpointKind, shape: OutputShape) -> EndpointSpec {
        EndpointSpec {
            id: Uuid::new_v4(),
            tenant_id: String::from("default"),
            environment: Environment::Prod,
            slug: String::from(slug),
            title: format!("Title {slug}"),
            description: format!("Desc {slug}"),
            kind,
            target_id: String::from("paid_amount"),
            target_version: Some(1),
            target_snapshot: None,
            input_schema: serde_json::json!({ "type": "object" }),
            output_shape: shape,
            required_scope: Permission::DataRead,
            rate_limit_rpm: 60,
            cache_ttl_s: 30,
            expose_mcp: true,
            show_as_app: true,
            enabled: true,
            published_by: None,
            published_at: Utc::now(),
        }
    }

    #[test]
    fn openapi_and_skill_manifest_include_published_paths() {
        let ep1 = sample_endpoint(
            "paid-amount-by-status",
            EndpointKind::Metric,
            OutputShape::Rows,
        );
        let ep2 = sample_endpoint(
            "warehouse-objects",
            EndpointKind::ObjectType,
            OutputShape::Object,
        );
        let spec = build_openapi_spec(&[ep1.clone(), ep2], Some("https://axpoint.example.com/"));
        assert_eq!(spec.get("openapi").and_then(|v| v.as_str()), Some("3.1.0"));

        let paths = spec.get("paths").and_then(|v| v.as_object());
        assert!(
            paths.is_some_and(
                |p| p.contains_key("/api/published/prod/default/paid-amount-by-status")
            )
        );
        assert!(
            paths.is_some_and(|p| p.contains_key("/api/published/prod/default/warehouse-objects"))
        );

        let skill = build_skill_manifest(&ep1, "https://axpoint.example.com/");
        assert_eq!(skill.name, "paid_amount_by_status");
        assert_eq!(
            skill.invoke.url,
            "https://axpoint.example.com/api/published/prod/default/paid-amount-by-status"
        );
        assert_eq!(skill.auth, "bearer");
    }
}
