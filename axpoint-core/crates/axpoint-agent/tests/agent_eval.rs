//! Offline 20-case `AxAgent` evaluation benchmark (`agent_bench.jsonl` + `CassetteModelAdapter`)
//! and Step 09 integration suite covering Capability Publishing, Streamable HTTP MCP Hub & Client,
//! concurrent tool execution, `max_steps` wrap-up, structured output repair, human approval pause/resume,
//! multi-agent supervisor delegation, and external embed token lifecycle (`08-STEP09` §09.3–§09.11).

mod support;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axpoint_agent::{
    ALL_BUILTIN_TOOL_NAMES, AgentApprovalDecidedSubscriber, AgentCassetteEntry, AgentModelTurn,
    CassetteModelAdapter, EmbedRequestMeta, PublishedCallContext,
};
use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentChatRequest, AgentEvent, AgentSpec, AgentTaskInput, AgentTaskOutcome,
    CreateAgentEmbedRequest, CreateAgentRequest, CreateEndpointRequest, CreateMcpClientRequest,
    KnowledgeScope, ToolCallSpec, ToolRef, UpdateAgentRequest,
};
use axpoint_domain::analytics::UpdateMetricRequest;
use axpoint_domain::enums::{
    AlertStatus, AuditAction, ConversationStatus, EndpointKind, Environment, MessageRole,
    OutboxTopic, Permission, Purpose, TokenKind,
};
use axpoint_domain::flow::{ApprovalDecision, ApprovalSourceKind, DecideApprovalParams};
use axpoint_domain::platform::{OutboxEvent, OutboxSubscriber};
use axpoint_domain::ports::{AgentTaskPort, AnalyticsRepository, FlowRepository};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use uuid::Uuid;

use support::{
    ALERT_A1_ID, FLOW_F1_ID, HarnessStack, MockSimPort, WAREHOUSE_W1_ID, make_principal,
};

#[derive(Debug, Deserialize)]
struct BenchCase {
    id: String,
    domain: String,
    agent: String,
    prompt: String,
    expected_tools: Vec<String>,
    expected_facts: Vec<String>,
    expect_structured: bool,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn default_agent_spec(tools: Vec<ToolRef>) -> AgentSpec {
    AgentSpec {
        system_prompt: String::from("你是企业供应链数字孪生与数据分析智能助手。"),
        provider_purpose: Purpose::Agent,
        provider_id: None,
        temperature: 0.1,
        max_steps: 6,
        memory_turns: 8,
        tools,
        approval_required_tools: Vec::new(),
        knowledge_scope: KnowledgeScope {
            doc_title_prefix: None,
            dataset_names: vec![String::from("demo_orders")],
        },
        output_schema: None,
    }
}

fn cassette_rule(
    match_all: Vec<&str>,
    last_role: Option<&str>,
    turn: AgentModelTurn,
) -> AgentCassetteEntry {
    AgentCassetteEntry {
        request_sha256: None,
        match_all: match_all.into_iter().map(String::from).collect(),
        match_substring: None,
        last_role: last_role.map(String::from),
        turn: Some(turn),
        response: None,
    }
}

fn pub_ctx(rate_key: &str, trace_id: &str) -> PublishedCallContext {
    PublishedCallContext {
        rate_limit_key: String::from(rate_key),
        idempotency_key: None,
        client_ip: String::from("127.0.0.1"),
        trace_id: String::from(trace_id),
    }
}

fn metric_ep_req(slug: &str, rpm: i32, ttl_s: i32) -> CreateEndpointRequest {
    CreateEndpointRequest {
        slug: String::from(slug),
        environment: Environment::Test,
        kind: EndpointKind::Metric,
        target_id: String::from("paid_amount"),
        title: String::from("已支付金额"),
        description: String::from("按状态统计已支付订单金额"),
        input_schema: None,
        output_shape: None,
        required_scope: Permission::DataRead,
        rate_limit_rpm: rpm,
        cache_ttl_s: ttl_s,
        expose_mcp: true,
        show_as_app: true,
        enabled: true,
    }
}

async fn create_and_publish_agent(
    stack: &HarnessStack,
    name: &str,
    spec: AgentSpec,
) -> Result<Uuid, AppError> {
    let admin = make_principal(TokenKind::Access, &[]);
    let rec = stack
        .runtime
        .create_agent(
            &admin,
            CreateAgentRequest {
                name: String::from(name),
                description: String::from(name),
                spec,
            },
        )
        .await?;
    let _ = stack.runtime.publish_agent(&admin, rec.id).await?;
    Ok(rec.id)
}

async fn seed_eval_agents_and_endpoints(
    stack: &HarnessStack,
) -> Result<HashMap<String, Uuid>, AppError> {
    let admin = make_principal(TokenKind::Access, &[]);
    let test_ep = stack
        .publisher
        .publish_endpoint(&admin, metric_ep_req("paid-amount-by-status", 120, 30))
        .await?;
    let prod_ep = stack.publisher.promote_endpoint(&admin, test_ep.id).await?;

    let mut workbench_tools: Vec<ToolRef> = ALL_BUILTIN_TOOL_NAMES
        .iter()
        .filter(|&&n| n != "run_simulation")
        .map(|&name| ToolRef::Builtin {
            name: String::from(name),
        })
        .collect();
    workbench_tools.push(ToolRef::Builtin {
        name: String::from("query_objects"),
    });
    workbench_tools.push(ToolRef::Endpoint {
        endpoint_id: prod_ep.id,
    });
    let workbench_id =
        create_and_publish_agent(stack, "ops_workbench", default_agent_spec(workbench_tools))
            .await?;

    let mut struct_spec = default_agent_spec(vec![ToolRef::Builtin {
        name: String::from("get_node"),
    }]);
    struct_spec.output_schema = Some(serde_json::json!({
        "type": "object",
        "required": ["node_key", "inventory_qty", "risk_level"],
        "properties": {
            "node_key": { "type": "string" },
            "inventory_qty": { "type": "number" },
            "risk_level": { "type": "string", "enum": ["LOW", "MEDIUM", "HIGH"] }
        },
        "additionalProperties": false
    }));
    let auditor_id = create_and_publish_agent(stack, "structured_auditor", struct_spec).await?;

    let analyst_id = create_and_publish_agent(
        stack,
        "data_analyst",
        default_agent_spec(vec![
            ToolRef::Builtin {
                name: String::from("query_metric"),
            },
            ToolRef::Builtin {
                name: String::from("ask_data"),
            },
            ToolRef::Builtin {
                name: String::from("run_sql"),
            },
        ]),
    )
    .await?;
    let kexpert_id = create_and_publish_agent(
        stack,
        "knowledge_expert",
        default_agent_spec(vec![ToolRef::Builtin {
            name: String::from("search_knowledge"),
        }]),
    )
    .await?;
    let supervisor_id = create_and_publish_agent(
        stack,
        "ops_supervisor",
        default_agent_spec(vec![
            ToolRef::Agent {
                agent_id: analyst_id,
                as_tool_name: String::from("data_analyst"),
            },
            ToolRef::Agent {
                agent_id: kexpert_id,
                as_tool_name: String::from("knowledge_expert"),
            },
        ]),
    )
    .await?;

    let mut ids = HashMap::new();
    ids.insert(String::from("ops_workbench"), workbench_id);
    ids.insert(String::from("structured_auditor"), auditor_id);
    ids.insert(String::from("ops_supervisor"), supervisor_id);
    Ok(ids)
}

#[tokio::test]
async fn offline_20_case_agent_benchmark_exceeds_90_percent_tool_accuracy_and_zero_hallucinations()
-> Result<(), AppError> {
    let dir = fixtures_dir();
    let cassette = CassetteModelAdapter::from_jsonl_path(&dir.join("agent_eval_cassette.jsonl"))?;
    let bench_raw = std::fs::read_to_string(dir.join("agent_bench.jsonl"))
        .map_err(|e| AppError::Config(format!("read agent_bench.jsonl: {e}")))?;
    let cases: Vec<BenchCase> = bench_raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| AppError::Config(e.to_string())))
        .collect::<Result<_, _>>()?;
    assert_eq!(cases.len(), 20);

    let stack = HarnessStack::build(Arc::new(cassette), None).await?;
    let agent_ids = seed_eval_agents_and_endpoints(&stack).await?;
    let caller = make_principal(TokenKind::Access, &[]);
    let a1_uuid = Uuid::parse_str(ALERT_A1_ID).map_err(|e| AppError::Config(e.to_string()))?;

    let mut matched_tool_cases: usize = 0;
    let mut grounded_fact_cases: usize = 0;
    for case in &cases {
        for al in &mut stack.store.inner.lock().await.alerts {
            if al.id == a1_uuid {
                al.status = AlertStatus::Open;
            }
        }
        let agent_id = agent_ids
            .get(&case.agent)
            .copied()
            .ok_or_else(|| AppError::Config(format!("missing agent {}", case.agent)))?;
        let resp = stack
            .runtime
            .chat(
                &caller,
                agent_id,
                AgentChatRequest {
                    message: case.prompt.clone(),
                    conversation_id: None,
                    draft: false,
                },
            )
            .await?;
        assert_eq!(resp.status, ConversationStatus::Active, "case {}", case.id);
        if case.expect_structured {
            assert!(resp.structured.is_some(), "case {} structured", case.id);
        }
        let invoked_tools: Vec<String> = resp
            .messages
            .iter()
            .filter(|m| {
                m.role == MessageRole::Assistant
                    && m.agent_name.as_deref() == Some(case.agent.as_str())
            })
            .filter_map(|m| m.tool_calls.as_ref())
            .flat_map(|calls| calls.iter().map(|c| c.name.clone()))
            .collect();
        if invoked_tools == case.expected_tools {
            matched_tool_cases = matched_tool_cases.saturating_add(1);
        }
        let tool_corpus: String = resp
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if case
            .expected_facts
            .iter()
            .all(|fact| resp.answer.contains(fact) && tool_corpus.contains(fact))
        {
            grounded_fact_cases = grounded_fact_cases.saturating_add(1);
        }
        if case.domain == "multi_agent" {
            let speakers: HashSet<&str> = resp
                .messages
                .iter()
                .filter_map(|m| m.agent_name.as_deref())
                .collect();
            assert!(speakers.contains("data_analyst") && speakers.contains("knowledge_expert"));
        }
    }
    tracing::info!(total = cases.len(), matched_tool_cases, grounded_fact_cases);
    assert_eq!(matched_tool_cases, 20);
    assert_eq!(grounded_fact_cases, 20);
    Ok(())
}

#[tokio::test]
async fn published_endpoints_pin_prod_snapshot_while_test_follows_live() -> Result<(), AppError> {
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;
    let admin = make_principal(TokenKind::Access, &[]);
    let pat = make_principal(TokenKind::ApiToken, &[Permission::DataRead]);

    let test_ep = stack
        .publisher
        .publish_endpoint(&admin, metric_ep_req("paid-amount-by-status", 60, 0))
        .await?;
    let prod_ep = stack.publisher.promote_endpoint(&admin, test_ep.id).await?;
    assert_eq!(prod_ep.target_version, Some(2));
    assert!(prod_ep.target_snapshot.is_some());

    let _ = stack
        .store
        .update_metric(
            "default",
            "paid_amount",
            1,
            UpdateMetricRequest {
                display_name: None,
                description: None,
                expression: None,
                filter_sql: Some(String::from("status = 'PENDING'")),
                time_column: None,
                default_grain: None,
                dimensions: None,
                unit: None,
                format: None,
                synonyms: None,
                certified: None,
            },
        )
        .await?;

    let prod_out = stack
        .publisher
        .invoke_published(
            &pat,
            "default",
            Environment::Prod,
            "paid-amount-by-status",
            serde_json::json!({ "dimensions": ["status"], "grain": "MONTH" }),
            pub_ctx("pat-1", "tr-p1"),
        )
        .await?;
    assert!(prod_out.into_json_value().to_string().contains("128500"));

    let test_out = stack
        .publisher
        .invoke_published(
            &pat,
            "default",
            Environment::Test,
            "paid-amount-by-status",
            serde_json::json!({ "dimensions": ["status"], "grain": "MONTH" }),
            pub_ctx("pat-1", "tr-t1"),
        )
        .await?;
    assert!(test_out.into_json_value().to_string().contains("19300"));
    Ok(())
}

#[tokio::test]
async fn published_endpoints_enforce_pat_schema_rate_limit_openapi_and_manifest()
-> Result<(), AppError> {
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;
    let admin = make_principal(TokenKind::Access, &[]);
    let pat = make_principal(TokenKind::ApiToken, &[Permission::DataRead]);

    let test_ep = stack
        .publisher
        .publish_endpoint(&admin, metric_ep_req("paid-amount-by-status", 2, 0))
        .await?;
    let prod_ep = stack.publisher.promote_endpoint(&admin, test_ep.id).await?;

    let mut obj_req = metric_ep_req("warehouse-objects", 60, 30);
    obj_req.kind = EndpointKind::ObjectType;
    obj_req.target_id = String::from("Warehouse");
    let obj_test = stack.publisher.publish_endpoint(&admin, obj_req).await?;
    let _ = stack
        .publisher
        .promote_endpoint(&admin, obj_test.id)
        .await?;

    let no_pat = stack
        .publisher
        .invoke_published(
            &admin,
            "default",
            Environment::Prod,
            "paid-amount-by-status",
            serde_json::json!({}),
            pub_ctx("pat-2", "tr-1"),
        )
        .await;
    assert!(matches!(no_pat, Err(AppError::Unauthorized(_))));

    let bad_schema = stack
        .publisher
        .invoke_published(
            &pat,
            "default",
            Environment::Prod,
            "paid-amount-by-status",
            serde_json::json!({ "grain": "WEEKLY" }),
            pub_ctx("pat-2", "tr-2"),
        )
        .await;
    assert_eq!(
        bad_schema.err().map(|e| e.error_code()),
        Some("SCHEMA_INVALID")
    );

    for i in 0..2_u8 {
        let _ = stack
            .publisher
            .invoke_published(
                &pat,
                "default",
                Environment::Prod,
                "paid-amount-by-status",
                serde_json::json!({ "grain": "MONTH" }),
                pub_ctx("pat-rate", &format!("tr-ok-{i}")),
            )
            .await?;
    }
    let limited = stack
        .publisher
        .invoke_published(
            &pat,
            "default",
            Environment::Prod,
            "paid-amount-by-status",
            serde_json::json!({ "grain": "MONTH" }),
            pub_ctx("pat-rate", "tr-3"),
        )
        .await;
    assert!(matches!(limited, Err(AppError::RateLimited)));

    let openapi = stack
        .publisher
        .export_openapi(&admin, Environment::Prod, None)
        .await?;
    let paths_str = openapi.get("paths").map_or_default(ToString::to_string);
    assert!(paths_str.contains("paid-amount-by-status") && paths_str.contains("warehouse-objects"));

    let manifest = stack
        .publisher
        .export_skill_manifest(&admin, prod_ep.id, "https://ax.example.com")
        .await?;
    assert_eq!(manifest.name, "paid_amount_by_status");
    Ok(())
}

#[tokio::test]
async fn mcp_server_hub_tools_resources_masking_and_session_lifecycle() -> Result<(), AppError> {
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;
    let _ = seed_eval_agents_and_endpoints(&stack).await?;
    let pat_no_pii = make_principal(TokenKind::ApiToken, &[Permission::DataRead]);

    let init = stack
        .mcp_hub
        .handle_post(
            &pat_no_pii,
            None,
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            "127.0.0.1",
            "tr-m1",
        )
        .await?;
    let sid = init
        .session_id
        .ok_or_else(|| AppError::Config(String::from("missing Mcp-Session-Id")))?;

    let list_body = stack
        .mcp_hub
        .handle_post(
            &pat_no_pii,
            Some(&sid),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            "127.0.0.1",
            "tr-m2",
        )
        .await?
        .body
        .unwrap_or_default()
        .to_string();
    assert!(list_body.contains("paid_amount_by_status") && list_body.contains("query_objects"));

    let call_str = stack
        .mcp_hub
        .handle_post(
            &pat_no_pii,
            Some(&sid),
            serde_json::json!({
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"get_node","arguments":{"node_id":WAREHOUSE_W1_ID}}
            }),
            "127.0.0.1",
            "tr-m3",
        )
        .await?
        .body
        .unwrap_or_default()
        .to_string();
    assert!(!call_str.contains("13812345678") && call_str.contains("***"));

    let res_str = stack
        .mcp_hub
        .handle_post(
            &pat_no_pii,
            Some(&sid),
            serde_json::json!({"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"axpoint://objects/Warehouse"}}),
            "127.0.0.1",
            "tr-m4",
        )
        .await?
        .body
        .unwrap_or_default()
        .to_string();
    assert!(res_str.contains("Warehouse"));

    let _sse = stack
        .mcp_hub
        .open_sse_stream(&pat_no_pii, Some(&sid))
        .await?;
    stack.mcp_hub.close_session(&pat_no_pii, Some(&sid)).await?;
    assert!(matches!(
        stack.mcp_hub.open_sse_stream(&pat_no_pii, Some(&sid)).await,
        Err(AppError::NotFound(_))
    ));
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        stack
            .audit_sink
            .records
            .lock()
            .await
            .iter()
            .any(|r| r.action == AuditAction::McpCall)
    );
    Ok(())
}

fn decode_mcp_tool_text(rpc_body: &serde_json::Value) -> Result<serde_json::Value, AppError> {
    let txt = rpc_body
        .pointer("/result/content/0/text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("{}");
    serde_json::from_str(txt).map_err(|e| AppError::Config(format!("decode MCP tool json: {e}")))
}

fn sample_sandbox_schema_doc_json() -> serde_json::Value {
    serde_json::json!({
        "node_types": [{
            "name": "SandboxHub",
            "display_name": "沙盒枢纽",
            "properties": [{
                "name": "region",
                "display_name": "区域",
                "data_type": "STRING",
                "kind": "ATTRIBUTE",
                "required": true
            }]
        }],
        "edge_types": [],
        "mappings": []
    })
}

#[tokio::test]
async fn mcp_axbuilder_permission_filtering_and_preflight() -> Result<(), AppError> {
    use axpoint_domain::enums::TokenProfile;

    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;

    let mut studio_only = make_principal(
        TokenKind::ApiToken,
        &[Permission::DataRead, Permission::StudioEdit],
    );
    studio_only.token_profile = TokenProfile::Builder;
    studio_only.web_base_url = Some(String::from("https://studio.xblue.example"));

    let init = stack
        .mcp_hub
        .handle_post(
            &studio_only,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            }),
            "127.0.0.1",
            "tr-b1",
        )
        .await?;
    let sid = init
        .session_id
        .ok_or_else(|| AppError::Config(String::from("missing Mcp-Session-Id")))?;

    let list_json = stack
        .mcp_hub
        .handle_post(
            &studio_only,
            Some(&sid),
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            "127.0.0.1",
            "tr-b2",
        )
        .await?
        .body
        .unwrap_or_default()
        .to_string();
    assert!(list_json.contains("axbuilder_preflight"));
    assert!(list_json.contains("axbuilder_push_schema_draft"));
    assert!(list_json.contains("axbuilder_clone_template"));
    assert!(list_json.contains("axbuilder_reset_dev_subgraph"));
    assert!(!list_json.contains("axbuilder_run_dev_subgraph"));
    assert!(!list_json.contains("axbuilder_upsert_flow_draft"));
    assert!(!list_json.contains("axbuilder_upsert_dashboard_draft"));

    let preflight_rpc = stack
        .mcp_hub
        .handle_post(
            &studio_only,
            Some(&sid),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "axbuilder_preflight", "arguments": { "subgraph_slug": "main" } }
            }),
            "127.0.0.1",
            "tr-b3",
        )
        .await?
        .body
        .unwrap_or_default();
    let preflight_val = decode_mcp_tool_text(&preflight_rpc)?;
    assert_eq!(preflight_val.get("healthy"), Some(&serde_json::json!(true)));
    assert_eq!(
        preflight_val.get("token_profile"),
        Some(&serde_json::json!("BUILDER"))
    );
    assert_eq!(
        preflight_val.pointer("/deep_links/studio_url"),
        Some(&serde_json::json!(
            "https://studio.xblue.example/studio?subgraph=main"
        ))
    );
    Ok(())
}

#[tokio::test]
async fn mcp_axbuilder_push_draft_and_audit() -> Result<(), AppError> {
    use axpoint_domain::enums::TokenProfile;
    use axpoint_domain::ports::StudioRepository as _;

    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;

    let mut studio_only = make_principal(
        TokenKind::ApiToken,
        &[Permission::DataRead, Permission::StudioEdit],
    );
    studio_only.token_profile = TokenProfile::Builder;
    studio_only.web_base_url = Some(String::from("https://studio.xblue.example"));

    let sid = stack
        .mcp_hub
        .handle_post(
            &studio_only,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            }),
            "127.0.0.1",
            "tr-b10",
        )
        .await?
        .session_id
        .ok_or_else(|| AppError::Config(String::from("missing Mcp-Session-Id")))?;

    let _dev_sbx = stack
        .store
        .create_subgraph(
            "default",
            Some(studio_only.user_id),
            axpoint_domain::studio::CreateSubgraphRequest {
                slug: String::from("dev_sbx"),
                display_name: String::from("测试沙盒"),
                description: String::new(),
                owner_dept: String::new(),
                is_dev_sandbox: true,
                source_template_id: None,
                seed_from_main: false,
                seed_node_types: None,
            },
        )
        .await?;

    let push_rpc = stack
        .mcp_hub
        .handle_post(
            &studio_only,
            Some(&sid),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "axbuilder_push_schema_draft",
                    "arguments": {
                        "subgraph_slug": "dev_sbx",
                        "title": "AI 沙盒草案",
                        "schema_doc": sample_sandbox_schema_doc_json()
                    }
                }
            }),
            "127.0.0.1",
            "tr-b11",
        )
        .await?
        .body
        .unwrap_or_default();
    let push_val = decode_mcp_tool_text(&push_rpc)?;
    assert_eq!(push_val.get("status"), Some(&serde_json::json!("DRAFT")));
    assert_eq!(push_val.get("error_count"), Some(&serde_json::json!(0)));
    let draft_id = push_val
        .get("draft_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let studio_url = push_val
        .pointer("/deep_links/studio_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert!(studio_url.starts_with(&format!(
        "https://studio.xblue.example/studio?subgraph=dev_sbx&draft={draft_id}"
    )));
    assert!(
        stack
            .audit_sink
            .records
            .lock()
            .await
            .iter()
            .any(|r| r.action == AuditAction::BuilderMcpMutate)
    );
    Ok(())
}

#[tokio::test]
async fn mcp_axbuilder_resources_read_templates_and_subgraphs() -> Result<(), AppError> {
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;
    let pat = make_principal(TokenKind::ApiToken, &[Permission::DataRead]);

    let sid = stack
        .mcp_hub
        .handle_post(
            &pat,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            }),
            "127.0.0.1",
            "tr-r1",
        )
        .await?
        .session_id
        .ok_or_else(|| AppError::Config(String::from("missing Mcp-Session-Id")))?;

    let tpl_list = stack
        .mcp_hub
        .handle_post(
            &pat,
            Some(&sid),
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/templates/list" }),
            "127.0.0.1",
            "tr-r2",
        )
        .await?
        .body
        .unwrap_or_default()
        .to_string();
    assert!(tpl_list.contains("axpoint://templates/{id}"));
    assert!(tpl_list.contains("axpoint://subgraphs/{slug}"));

    let tpl_read = stack
        .mcp_hub
        .handle_post(
            &pat,
            Some(&sid),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "resources/read",
                "params": { "uri": "axpoint://templates/supply_chain" }
            }),
            "127.0.0.1",
            "tr-r3",
        )
        .await?
        .body
        .unwrap_or_default()
        .to_string();
    assert!(tpl_read.contains("供应链协同"));

    let sg_read = stack
        .mcp_hub
        .handle_post(
            &pat,
            Some(&sid),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "resources/read",
                "params": { "uri": "axpoint://subgraphs/main" }
            }),
            "127.0.0.1",
            "tr-r4",
        )
        .await?
        .body
        .unwrap_or_default()
        .to_string();
    assert!(sg_read.contains("企业主图"));
    Ok(())
}

#[tokio::test]
async fn mcp_axbuilder_sandbox_pii_masking_and_agent_publish_guard() -> Result<(), AppError> {
    use axpoint_domain::enums::TokenProfile;

    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;

    let mut builder_pat = make_principal(
        TokenKind::ApiToken,
        &[
            Permission::DataRead,
            Permission::DataSync,
            Permission::StudioEdit,
            Permission::AgentEdit,
        ],
    );
    builder_pat.token_profile = TokenProfile::Builder;

    let publish_err = stack
        .runtime
        .publish_agent(&builder_pat, Uuid::new_v4())
        .await;
    assert!(matches!(
        publish_err,
        Err(AppError::Forbidden("BUILDER_TOKEN_PROD_FORBIDDEN"))
    ));

    let sid = stack
        .mcp_hub
        .handle_post(
            &builder_pat,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            }),
            "127.0.0.1",
            "tr-sbx1",
        )
        .await?
        .session_id
        .ok_or_else(|| AppError::Config(String::from("missing Mcp-Session-Id")))?;

    let run_rpc = stack
        .mcp_hub
        .handle_post(
            &builder_pat,
            Some(&sid),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "axbuilder_run_dev_subgraph",
                    "arguments": { "subgraph_slug": "dev_supply_chain", "limit_rows": 10 }
                }
            }),
            "127.0.0.1",
            "tr-sbx2",
        )
        .await?
        .body
        .unwrap_or_default();
    let run_val = decode_mcp_tool_text(&run_rpc)?;
    let dumped = run_val.to_string();
    assert!(!dumped.contains("13812345678"));
    assert!(dumped.contains("138****5678"));
    Ok(())
}

async fn spawn_mock_remote_mcp_server() -> Result<String, AppError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| AppError::Config(format!("bind mock mcp tcp: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| AppError::Config(format!("local_addr: {e}")))?;

    tokio::spawn(async move {
        for _ in 0..8_u8 {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0_u8; 8192];
            let Ok(n) = socket.read(&mut buf).await else {
                continue;
            };
            let req_text = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[]));
            let ok_hdr = req_text
                .to_ascii_lowercase()
                .contains("x-vendor-key: sec-99");
            let body_json = if !ok_hdr {
                serde_json::json!({"jsonrpc":"2.0","id":1,"error":{"code":-32001,"message":"missing secret"}})
            } else if req_text.contains("\"initialize\"") {
                serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{}}})
            } else if req_text.contains("\"tools/list\"") {
                serde_json::json!({"jsonrpc":"2.0","id":2,"result":{"tools":[
                    {"name":"erp_stock","description":"ERP 库存","inputSchema":{"type":"object"}},
                    {"name":"erp_drop_db","description":"危险","inputSchema":{"type":"object"}}
                ]}})
            } else {
                serde_json::json!({"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"erp_available\":640}"}],"isError":false}})
            };
            let payload = body_json.to_string();
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: rsid\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            if let Err(err) = socket.write_all(http.as_bytes()).await {
                tracing::debug!(error = %err, "mock MCP write closed");
            }
        }
    });
    Ok(format!("http://{addr}/mcp"))
}

fn mcp_req(name: &str, url: &str, private: bool) -> CreateMcpClientRequest {
    CreateMcpClientRequest {
        name: String::from(name),
        url: String::from(url),
        headers: None,
        allowed_tools: Vec::new(),
        allow_private_network: private,
        enabled: true,
    }
}

#[tokio::test]
async fn mcp_client_manager_enforces_ssrf_guard_and_invokes_sealed_remote_tool()
-> Result<(), AppError> {
    let remote_url = spawn_mock_remote_mcp_server().await?;
    let cassette = CassetteModelAdapter::from_entries(vec![
        cassette_rule(
            vec!["[MCP-EXT]"],
            Some("user"),
            AgentModelTurn {
                content: String::new(),
                tool_calls: vec![ToolCallSpec {
                    id: String::from("c1"),
                    name: String::from("erp_stock"),
                    arguments: serde_json::json!({"sku":"SKU-1"}),
                }],
                prompt_tokens: 20,
                completion_tokens: 10,
            },
        ),
        cassette_rule(
            vec!["[MCP-EXT]"],
            Some("tool"),
            AgentModelTurn {
                content: String::from("外部 ERP 可用库存为 640 件。"),
                tool_calls: Vec::new(),
                prompt_tokens: 40,
                completion_tokens: 12,
            },
        ),
    ]);
    let stack = HarnessStack::build(Arc::new(cassette), None).await?;
    let editor = make_principal(TokenKind::Access, &[Permission::AgentEdit]);
    let admin = make_principal(TokenKind::Access, &[]);

    let ssrf_err = stack
        .mcp_clients
        .create_from_request(&admin, mcp_req("blocked_erp", &remote_url, false))
        .await;
    assert_eq!(ssrf_err.err().map(|e| e.error_code()), Some("SSRF_BLOCKED"));

    let guard_err = stack
        .mcp_clients
        .create_from_request(&editor, mcp_req("unauth_erp", &remote_url, true))
        .await;
    assert!(matches!(guard_err, Err(AppError::Forbidden(_))));

    let mut hdrs = BTreeMap::new();
    hdrs.insert(String::from("X-Vendor-Key"), String::from("sec-99"));
    let client = stack
        .mcp_clients
        .create_from_request(
            &admin,
            CreateMcpClientRequest {
                name: String::from("vendor_erp"),
                url: remote_url,
                headers: Some(hdrs),
                allowed_tools: vec![String::from("erp_stock")],
                allow_private_network: true,
                enabled: true,
            },
        )
        .await?;
    let synced = stack
        .mcp_clients
        .sync_client_tools(&admin, client.id)
        .await?;
    assert_eq!(
        synced.tools_cache.first().map(|t| t.name.as_str()),
        Some("erp_stock")
    );

    let agent_id = create_and_publish_agent(
        &stack,
        "erp_agent",
        default_agent_spec(vec![ToolRef::Mcp {
            client: String::from("vendor_erp"),
            tool: String::from("erp_stock"),
        }]),
    )
    .await?;
    let resp = stack
        .runtime
        .chat(
            &admin,
            agent_id,
            AgentChatRequest {
                message: String::from("[MCP-EXT] 查询库存"),
                conversation_id: None,
                draft: false,
            },
        )
        .await?;
    assert!(resp.answer.contains("640"));
    Ok(())
}

#[tokio::test]
async fn runtime_executes_tool_batches_concurrently_and_unlocks_sim_port() -> Result<(), AppError> {
    let entries = vec![
        cassette_rule(
            vec!["[CONC-3]"],
            Some("user"),
            AgentModelTurn {
                content: String::new(),
                tool_calls: vec![
                    ToolCallSpec {
                        id: String::from("tc1"),
                        name: String::from("get_node"),
                        arguments: serde_json::json!({ "node_id": WAREHOUSE_W1_ID }),
                    },
                    ToolCallSpec {
                        id: String::from("tc2"),
                        name: String::from("list_alerts"),
                        arguments: serde_json::json!({ "status": "OPEN" }),
                    },
                    ToolCallSpec {
                        id: String::from("tc3"),
                        name: String::from("run_simulation"),
                        arguments: serde_json::json!({ "scenario_name": "expedite" }),
                    },
                ],
                prompt_tokens: 30,
                completion_tokens: 20,
            },
        ),
        cassette_rule(
            vec!["[CONC-3]"],
            Some("tool"),
            AgentModelTurn {
                content: String::from("三路并发完成（stockout_risk_delta = -0.42）。"),
                tool_calls: Vec::new(),
                prompt_tokens: 60,
                completion_tokens: 18,
            },
        ),
    ];
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(entries)),
        Some(Arc::new(MockSimPort)),
    )
    .await?;
    let admin = make_principal(TokenKind::Access, &[]);

    stack.store.inner.lock().await.slow_node_delay_ms = 80;
    let agent_id = create_and_publish_agent(
        &stack,
        "conc_agent",
        default_agent_spec(vec![
            ToolRef::Builtin {
                name: String::from("get_node"),
            },
            ToolRef::Builtin {
                name: String::from("list_alerts"),
            },
            ToolRef::Builtin {
                name: String::from("run_simulation"),
            },
        ]),
    )
    .await?;
    let t0 = Instant::now();
    let resp = stack
        .runtime
        .chat(
            &admin,
            agent_id,
            AgentChatRequest {
                message: String::from("[CONC-3] 执行并发工具"),
                conversation_id: None,
                draft: false,
            },
        )
        .await?;
    let elapsed = t0.elapsed();
    stack.store.inner.lock().await.slow_node_delay_ms = 0;
    assert!(
        elapsed < Duration::from_millis(155),
        "elapsed = {elapsed:?}"
    );
    assert!(resp.answer.contains("-0.42"));
    Ok(())
}

#[tokio::test]
async fn runtime_wraps_up_on_max_steps_and_repairs_structured_output() -> Result<(), AppError> {
    let entries = vec![
        cassette_rule(
            vec!["[MAX-STEP]", "已达到最大工具调用步数上限"],
            None,
            AgentModelTurn {
                content: String::from("收敛回答：W-1 库存 320。"),
                tool_calls: Vec::new(),
                prompt_tokens: 50,
                completion_tokens: 14,
            },
        ),
        cassette_rule(
            vec!["[MAX-STEP]"],
            None,
            AgentModelTurn {
                content: String::new(),
                tool_calls: vec![ToolCallSpec {
                    id: String::from("tc_loop"),
                    name: String::from("get_node"),
                    arguments: serde_json::json!({ "node_id": WAREHOUSE_W1_ID }),
                }],
                prompt_tokens: 25,
                completion_tokens: 12,
            },
        ),
        cassette_rule(
            vec!["[REPAIR]", "上一条回复不符合要求的 JSON Schema"],
            Some("user"),
            AgentModelTurn {
                content: String::from("{\"score\":95}"),
                tool_calls: Vec::new(),
                prompt_tokens: 35,
                completion_tokens: 8,
            },
        ),
        cassette_rule(
            vec!["[REPAIR]"],
            Some("user"),
            AgentModelTurn {
                content: String::from("NOT_VALID_JSON"),
                tool_calls: Vec::new(),
                prompt_tokens: 20,
                completion_tokens: 6,
            },
        ),
    ];
    let stack =
        HarnessStack::build(Arc::new(CassetteModelAdapter::from_entries(entries)), None).await?;
    let admin = make_principal(TokenKind::Access, &[]);

    let mut step1_spec = default_agent_spec(vec![ToolRef::Builtin {
        name: String::from("get_node"),
    }]);
    step1_spec.max_steps = 1;
    let step_id = create_and_publish_agent(&stack, "step1_agent", step1_spec).await?;
    let step_resp = stack
        .runtime
        .chat(
            &admin,
            step_id,
            AgentChatRequest {
                message: String::from("[MAX-STEP] 测试"),
                conversation_id: None,
                draft: false,
            },
        )
        .await?;
    assert!(step_resp.answer.contains("收敛回答"));

    let mut repair_spec = default_agent_spec(Vec::new());
    repair_spec.output_schema = Some(serde_json::json!({
        "type": "object",
        "required": ["score"],
        "properties": { "score": { "type": "integer" } },
        "additionalProperties": false
    }));
    let rep_id = create_and_publish_agent(&stack, "repair_agent", repair_spec).await?;
    let rep_resp = stack
        .runtime
        .chat(
            &admin,
            rep_id,
            AgentChatRequest {
                message: String::from("[REPAIR] 评分"),
                conversation_id: None,
                draft: false,
            },
        )
        .await?;
    assert_eq!(
        rep_resp.structured,
        Some(serde_json::json!({ "score": 95 }))
    );
    Ok(())
}

fn approval_cassette_entries() -> Vec<AgentCassetteEntry> {
    vec![
        cassette_rule(
            vec!["[APPROVE-FLOW]", "用户拒绝执行"],
            Some("tool"),
            AgentModelTurn {
                content: String::from("收到审批拒绝：已取消启动补货流程。"),
                tool_calls: Vec::new(),
                prompt_tokens: 40,
                completion_tokens: 14,
            },
        ),
        cassette_rule(
            vec!["[APPROVE-FLOW]"],
            Some("tool"),
            AgentModelTurn {
                content: String::from("审批已通过，紧急补货流程已成功启动（RUNNING）。"),
                tool_calls: Vec::new(),
                prompt_tokens: 45,
                completion_tokens: 16,
            },
        ),
        cassette_rule(
            vec!["[APPROVE-FLOW]"],
            Some("user"),
            AgentModelTurn {
                content: String::new(),
                tool_calls: vec![ToolCallSpec {
                    id: String::from("call_sf_1"),
                    name: String::from("start_flow"),
                    arguments: serde_json::json!({ "flow_id": FLOW_F1_ID }),
                }],
                prompt_tokens: 30,
                completion_tokens: 15,
            },
        ),
    ]
}

#[tokio::test]
async fn runtime_human_approval_pauses_and_resumes_idempotently_via_outbox() -> Result<(), AppError>
{
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(
            approval_cassette_entries(),
        )),
        None,
    )
    .await?;
    let admin = make_principal(TokenKind::Access, &[]);

    let mut gated_spec = default_agent_spec(vec![ToolRef::Builtin {
        name: String::from("start_flow"),
    }]);
    gated_spec.approval_required_tools = vec![String::from("start_flow")];
    let agent_id = create_and_publish_agent(&stack, "gated_ops", gated_spec).await?;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(32);
    let paused = stack
        .runtime
        .chat_stream(
            &admin,
            agent_id,
            AgentChatRequest {
                message: String::from("[APPROVE-FLOW] 启动补货"),
                conversation_id: None,
                draft: false,
            },
            tx,
        )
        .await?;
    assert_eq!(paused.status, ConversationStatus::WaitingApproval);
    let approval_id = paused
        .approval_id
        .ok_or_else(|| AppError::Config(String::from("expected approval_id")))?;
    let mut saw_sse = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, AgentEvent::ApprovalRequired { .. }) {
            saw_sse = true;
        }
    }
    assert!(saw_sse);

    let _ = stack
        .store
        .decide_approval(
            "default",
            DecideApprovalParams {
                approval_id,
                actor_user_id: admin.user_id,
                actor_roles: admin.roles.clone(),
                decision: ApprovalDecision::Approve,
                comment: String::from("批准"),
            },
        )
        .await?;
    let sub = AgentApprovalDecidedSubscriber::new(stack.runtime.clone());
    let ev = OutboxEvent {
        id: 1,
        tenant_id: String::from("default"),
        topic: OutboxTopic::ApprovalDecided,
        payload: serde_json::json!({ "approval_id": approval_id }),
        claimed_by: None,
        claimed_until: None,
        created_at: chrono::Utc::now(),
    };
    sub.on_event(&ev).await?;
    sub.on_event(&ev).await?;
    let msgs = stack
        .runtime
        .list_conversation_messages(&admin, paused.conversation_id)
        .await?;
    assert!(msgs.iter().any(|m| m.content.contains("审批已通过")));
    Ok(())
}

#[tokio::test]
async fn runtime_human_approval_rejection_records_denial_and_completes() -> Result<(), AppError> {
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(
            approval_cassette_entries(),
        )),
        None,
    )
    .await?;
    let admin = make_principal(TokenKind::Access, &[]);

    let mut gated_spec = default_agent_spec(vec![ToolRef::Builtin {
        name: String::from("start_flow"),
    }]);
    gated_spec.approval_required_tools = vec![String::from("start_flow")];
    let agent_id = create_and_publish_agent(&stack, "gated_rej_ops", gated_spec).await?;

    let paused_rej = stack
        .runtime
        .chat(
            &admin,
            agent_id,
            AgentChatRequest {
                message: String::from("[APPROVE-FLOW] 再次申请"),
                conversation_id: None,
                draft: false,
            },
        )
        .await?;
    let rej_id = paused_rej.approval_id.unwrap_or_else(Uuid::nil);
    let _ = stack
        .store
        .decide_approval(
            "default",
            DecideApprovalParams {
                approval_id: rej_id,
                actor_user_id: admin.user_id,
                actor_roles: admin.roles,
                decision: ApprovalDecision::Reject,
                comment: String::from("暂缓"),
            },
        )
        .await?;
    let resumed_rej = stack
        .runtime
        .resume_after_approval("default", rej_id)
        .await?
        .ok_or_else(|| AppError::Config(String::from("expected resumed rejection")))?;
    assert!(resumed_rej.answer.contains("已取消启动补货流程"));
    Ok(())
}

#[tokio::test]
async fn flow_agent_task_port_suspends_with_flow_source_and_reuses_idempotently()
-> Result<(), AppError> {
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(
            approval_cassette_entries(),
        )),
        None,
    )
    .await?;
    let admin = make_principal(TokenKind::Access, &[]);
    let mut gated_spec = default_agent_spec(vec![ToolRef::Builtin {
        name: String::from("start_flow"),
    }]);
    gated_spec.approval_required_tools = vec![String::from("start_flow")];
    let agent_id = create_and_publish_agent(&stack, "flow_gated_agent", gated_spec).await?;

    let run_id = Uuid::new_v4();
    let task_in = AgentTaskInput {
        tenant_id: String::from("default"),
        run_id,
        node_id: String::from("agent_gate"),
        idempotency_key: format!("{run_id}:agent_gate"),
        agent_id,
        version: None,
        instruction: String::from("[APPROVE-FLOW] 流程触发审批"),
        input: serde_json::json!({ "warehouse": "W-1" }),
        output_schema: None,
        max_steps: Some(4),
        user_id: admin.user_id,
        username: admin.username.clone(),
        dept_id: admin.dept_id.clone(),
        roles: admin.roles.clone(),
        permissions: admin.permissions.clone(),
    };
    let AgentTaskOutcome::WaitingApproval {
        approval_id: flow_app_id,
        ..
    } = stack
        .runtime
        .execute_flow_agent_task(task_in.clone())
        .await?
    else {
        return Err(AppError::Config(String::from("expected WaitingApproval")));
    };
    let stored = stack
        .store
        .get_approval("default", flow_app_id)
        .await?
        .ok_or_else(|| AppError::Config(String::from("missing approval")))?;
    assert_eq!(stored.source_kind, ApprovalSourceKind::Flow);

    let _ = stack
        .store
        .decide_approval(
            "default",
            DecideApprovalParams {
                approval_id: flow_app_id,
                actor_user_id: admin.user_id,
                actor_roles: admin.roles.clone(),
                decision: ApprovalDecision::Approve,
                comment: String::new(),
            },
        )
        .await?;
    let _ = stack
        .runtime
        .resume_after_approval("default", flow_app_id)
        .await?;
    let reused = stack.runtime.execute_flow_agent_task(task_in).await?;
    assert!(matches!(reused, AgentTaskOutcome::Completed { .. }));
    Ok(())
}

#[tokio::test]
async fn multi_agent_depth_and_cycle_validation_rejects_invalid_graphs() -> Result<(), AppError> {
    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;
    let admin = make_principal(TokenKind::Access, &[]);

    let leaf_id =
        create_and_publish_agent(&stack, "leaf_expert", default_agent_spec(Vec::new())).await?;
    let mid_id = create_and_publish_agent(
        &stack,
        "mid_specialist",
        default_agent_spec(vec![ToolRef::Agent {
            agent_id: leaf_id,
            as_tool_name: String::from("leaf_expert"),
        }]),
    )
    .await?;

    let too_deep = stack
        .runtime
        .create_agent(
            &admin,
            CreateAgentRequest {
                name: String::from("root_too_deep"),
                description: String::new(),
                spec: default_agent_spec(vec![ToolRef::Agent {
                    agent_id: mid_id,
                    as_tool_name: String::from("mid_specialist"),
                }]),
            },
        )
        .await;
    assert_eq!(
        too_deep.err().map(|e| e.error_code()),
        Some("INVALID_ARGUMENT")
    );

    let cycle_res = stack
        .runtime
        .update_agent(
            &admin,
            leaf_id,
            UpdateAgentRequest {
                name: None,
                description: None,
                spec: default_agent_spec(vec![ToolRef::Agent {
                    agent_id: mid_id,
                    as_tool_name: String::from("mid_specialist"),
                }]),
            },
        )
        .await;
    assert_eq!(
        cycle_res.err().map(|e| e.error_code()),
        Some("INVALID_ARGUMENT")
    );
    Ok(())
}

fn emb_msg(text: &str, conv_id: Option<Uuid>) -> AgentChatRequest {
    AgentChatRequest {
        message: String::from(text),
        conversation_id: conv_id,
        draft: false,
    }
}

fn emb_meta(origin: &str, ip: &str, trace: &str) -> EmbedRequestMeta {
    EmbedRequestMeta {
        origin: Some(String::from(origin)),
        host: None,
        client_ip: String::from(ip),
        trace_id: String::from(trace),
    }
}

#[tokio::test]
async fn embed_service_enforces_origin_rate_limit_and_404_after_revocation() -> Result<(), AppError>
{
    let cassette = CassetteModelAdapter::from_entries(vec![cassette_rule(
        vec!["智能助手"],
        Some("user"),
        AgentModelTurn {
            content: String::from("您好，我是嵌入式供应链助手！"),
            tool_calls: Vec::new(),
            prompt_tokens: 18,
            completion_tokens: 10,
        },
    )]);
    let stack = HarnessStack::build(Arc::new(cassette), None).await?;
    let admin = make_principal(TokenKind::Access, &[]);
    let agent_id =
        create_and_publish_agent(&stack, "embed_assistant", default_agent_spec(Vec::new())).await?;

    let created = stack
        .embed
        .create_embed(
            &admin,
            agent_id,
            CreateAgentEmbedRequest {
                allowed_origins: vec![String::from("https://portal.example.com")],
                permissions: vec![Permission::DataRead, Permission::AgentRun],
                expires_at: None,
                rate_limit_rpm: 2,
            },
            "127.0.0.1",
            "tr-emb-1",
        )
        .await?;
    assert!(created.token.starts_with("axe_"));

    let bad_origin = stack
        .embed
        .chat_via_embed(
            &created.token,
            emb_msg("你好", None),
            emb_meta("https://evil.example.org", "203.0.113.9", "tr-bad"),
            None,
        )
        .await;
    assert!(matches!(bad_origin, Err(AppError::Forbidden(_))));

    let good = emb_meta("https://portal.example.com", "203.0.113.9", "tr-ok");
    let t1 = stack
        .embed
        .chat_via_embed(&created.token, emb_msg("你好", None), good.clone(), None)
        .await?;
    let t2 = stack
        .embed
        .chat_via_embed(
            &created.token,
            emb_msg("继续", Some(t1.conversation_id)),
            good.clone(),
            None,
        )
        .await?;
    assert_eq!(t2.conversation_id, t1.conversation_id);

    let t3 = stack
        .embed
        .chat_via_embed(
            &created.token,
            emb_msg("超限", Some(t1.conversation_id)),
            good,
            None,
        )
        .await;
    assert!(matches!(t3, Err(AppError::RateLimited)));

    stack.embed.revoke_embed(&admin, created.id).await?;
    let revoked = stack
        .embed
        .chat_via_embed(
            &created.token,
            emb_msg("你好", None),
            emb_meta("https://portal.example.com", "203.0.113.77", "tr-rev"),
            None,
        )
        .await;
    assert!(matches!(revoked, Err(AppError::NotFound(_))));
    Ok(())
}

#[tokio::test]
async fn mcp_axbuilder_22_tools_eval_expr_and_main_isolation() -> Result<(), AppError> {
    use axpoint_domain::builder::BUILDER_DEFAULT_SCOPES;
    use axpoint_domain::enums::TokenProfile;

    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;

    let mut builder = make_principal(TokenKind::ApiToken, BUILDER_DEFAULT_SCOPES);
    builder.token_profile = TokenProfile::Builder;

    let sid = stack
        .mcp_hub
        .handle_post(
            &builder,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            }),
            "127.0.0.1",
            "tr-s29-1",
        )
        .await?
        .session_id
        .ok_or_else(|| AppError::Config(String::from("missing Mcp-Session-Id")))?;

    let list_body = stack
        .mcp_hub
        .handle_post(
            &builder,
            Some(&sid),
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            "127.0.0.1",
            "tr-s29-2",
        )
        .await?
        .body
        .unwrap_or_default();
    let tools = list_body
        .pointer("/result/tools")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(tools, 22);

    // Deterministic expression evaluation without LLM
    let eval_rpc = stack
        .mcp_hub
        .handle_post(
            &builder,
            Some(&sid),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "axbuilder_eval_expression",
                    "arguments": {
                        "expression": "stock_qty - safety_stock",
                        "metrics": { "stock_qty": 120.0, "safety_stock": 45.0 }
                    }
                }
            }),
            "127.0.0.1",
            "tr-s29-3",
        )
        .await?
        .body
        .unwrap_or_default();
    let eval_val = decode_mcp_tool_text(&eval_rpc)?;
    assert_eq!(eval_val.get("valid"), Some(&serde_json::json!(true)));
    assert_eq!(eval_val.get("value"), Some(&serde_json::json!(75.0)));

    // Reject dryrun_action_spec on PROD main subgraph
    let main_dryrun = stack
        .mcp_hub
        .handle_post(
            &builder,
            Some(&sid),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "axbuilder_dryrun_action_spec",
                    "arguments": {
                        "subgraph_slug": "main",
                        "type_name": "Warehouse",
                        "node_key": "W-1",
                        "action_name": "restock"
                    }
                }
            }),
            "127.0.0.1",
            "tr-s29-4",
        )
        .await?
        .body
        .unwrap_or_default();
    assert_eq!(
        main_dryrun.pointer("/result/isError"),
        Some(&serde_json::json!(true))
    );
    Ok(())
}

async fn init_builder_mcp_session(
    stack: &HarnessStack,
    builder: &axpoint_security::AuthPrincipal,
) -> Result<String, AppError> {
    stack
        .mcp_hub
        .handle_post(
            builder,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            }),
            "127.0.0.1",
            "tr-rt-init",
        )
        .await?
        .session_id
        .ok_or_else(|| AppError::Config(String::from("missing Mcp-Session-Id")))
}

async fn call_builder_mcp_tool(
    stack: &HarnessStack,
    builder: &axpoint_security::AuthPrincipal,
    sid: &str,
    name: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    Ok(stack
        .mcp_hub
        .handle_post(
            builder,
            Some(sid),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }),
            "127.0.0.1",
            "tr-rt-call",
        )
        .await?
        .body
        .unwrap_or_default())
}

async fn setup_redteam_dev_sandbox_session(
    schema_doc: serde_json::Value,
    dev_node: axpoint_domain::topology::BlueNode,
) -> Result<(HarnessStack, axpoint_security::AuthPrincipal, String), AppError> {
    use axpoint_domain::builder::BUILDER_DEFAULT_SCOPES;
    use axpoint_domain::enums::TokenProfile;
    use axpoint_domain::ports::StudioRepository;
    use axpoint_domain::studio::CreateSubgraphRequest;

    let stack = HarnessStack::build(
        Arc::new(CassetteModelAdapter::from_entries(Vec::new())),
        None,
    )
    .await?;
    let mut builder = make_principal(TokenKind::ApiToken, BUILDER_DEFAULT_SCOPES);
    builder.token_profile = TokenProfile::Builder;
    assert!(!builder.has(Permission::PiiRawRead));

    stack
        .store
        .create_subgraph(
            "default",
            None,
            CreateSubgraphRequest {
                slug: String::from("dev_sbx"),
                display_name: String::from("开发沙盒"),
                description: String::new(),
                owner_dept: String::new(),
                is_dev_sandbox: true,
                seed_from_main: false,
                seed_node_types: None,
                source_template_id: None,
            },
        )
        .await?;

    let sid = init_builder_mcp_session(&stack, &builder).await?;
    let _push = call_builder_mcp_tool(
        &stack,
        &builder,
        &sid,
        "axbuilder_push_schema_draft",
        serde_json::json!({
            "subgraph_slug": "dev_sbx",
            "title": "红队验证草案",
            "schema_doc": schema_doc
        }),
    )
    .await?;

    {
        let mut guard = stack.store.inner.lock().await;
        guard
            .sandbox_nodes
            .insert(String::from("dev_sbx"), vec![dev_node]);
        guard.get_node_main_calls = 0;
    }
    Ok((stack, builder, sid))
}

#[tokio::test]
async fn mcp_axbuilder_redteam_eval_expression_subgraph_isolation_and_pii_masking()
-> Result<(), AppError> {
    let dev_c1_id = Uuid::from_u128(0x9901);
    let dev_node = serde_json::from_value(serde_json::json!({
        "id": dev_c1_id,
        "type_name": "DevCustomer",
        "node_key": "C-1",
        "title": "客户一号",
        "properties": { "contact_phone": "13900009999" },
        "metrics": {},
        "row_version": 1
    }))
    .map_err(|e| AppError::Config(format!("dev_node deserialize: {e}")))?;

    let (stack, builder, sid) = setup_redteam_dev_sandbox_session(
        serde_json::json!({
            "node_types": [{
                "name": "DevCustomer",
                "display_name": "沙盒客户",
                "properties": [{
                    "name": "contact_phone",
                    "display_name": "联系手机",
                    "data_type": "STRING",
                    "kind": "ATTRIBUTE",
                    "required": false,
                    "pii_category": "PHONE"
                }]
            }],
            "edge_types": [],
            "mappings": []
        }),
        dev_node,
    )
    .await?;

    // 1. Passing a PROD main node_id with subgraph_slug = "dev_sbx" must return 404 NODE_NOT_FOUND with 0 main reads
    let cross_rpc = call_builder_mcp_tool(
        &stack,
        &builder,
        &sid,
        "axbuilder_eval_expression",
        serde_json::json!({
            "expression": "prop.manager_phone",
            "node_id": WAREHOUSE_W1_ID,
            "subgraph_slug": "dev_sbx"
        }),
    )
    .await?;
    assert_eq!(
        cross_rpc.pointer("/result/isError"),
        Some(&serde_json::json!(true))
    );
    let cross_txt = cross_rpc.to_string();
    assert!(cross_txt.contains("NODE_NOT_FOUND"));
    assert!(!cross_txt.contains("13812345678"));
    assert_eq!(stack.store.inner.lock().await.get_node_main_calls, 0);

    // 2. Evaluating on main masks main PII (prop.manager_phone)
    let main_rpc = call_builder_mcp_tool(
        &stack,
        &builder,
        &sid,
        "axbuilder_eval_expression",
        serde_json::json!({
            "expression": "prop.manager_phone",
            "node_id": WAREHOUSE_W1_ID,
            "subgraph_slug": "main"
        }),
    )
    .await?;
    let main_val = decode_mcp_tool_text(&main_rpc)?;
    let masked_main = main_val
        .get("value")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert!(masked_main.contains("****"));
    assert_ne!(masked_main, "13812345678");

    // 3. Evaluating on dev_sbx masks DRAFT-only PII (prop.contact_phone)
    let dev_rpc = call_builder_mcp_tool(
        &stack,
        &builder,
        &sid,
        "axbuilder_eval_expression",
        serde_json::json!({
            "expression": "prop.contact_phone",
            "node_id": dev_c1_id,
            "subgraph_slug": "dev_sbx"
        }),
    )
    .await?;
    let dev_val = decode_mcp_tool_text(&dev_rpc)?;
    let masked_dev = dev_val
        .get("value")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert!(masked_dev.contains("****"));
    assert_ne!(masked_dev, "13900009999");
    Ok(())
}

fn seed_redteam_dev_hub_schema() -> serde_json::Value {
    serde_json::json!({
        "node_types": [{
            "name": "DevHub",
            "display_name": "沙盒枢纽",
            "properties": [
                { "name": "manager_phone", "display_name": "手机", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "PHONE" },
                { "name": "stock_qty", "display_name": "库存", "data_type": "FLOAT64", "kind": "METRIC", "required": false },
                { "name": "turnover_qty", "display_name": "周转", "data_type": "FLOAT64", "kind": "METRIC", "required": false }
            ],
            "derived_rules": [
                { "target_metric": "turnover_qty", "expression": "stock_qty * 0.5", "enabled": true }
            ],
            "actions": [
                {
                    "name": "dispatch_batch",
                    "display_name": "批次出库",
                    "maturity": "PROPOSED",
                    "side_effect_tier": "SELF_ONLY",
                    "parameters": [{ "name": "amount", "display_name": "数量", "data_type": "FLOAT64", "required": true }],
                    "precondition_expr": "stock_qty >= param.amount",
                    "mutations": [{ "target": "self.stock_qty", "expression": "stock_qty - param.amount" }]
                },
                {
                    "name": "write_protected",
                    "display_name": "违规改写派生指标",
                    "maturity": "PROPOSED",
                    "side_effect_tier": "SELF_ONLY",
                    "parameters": [],
                    "mutations": [{ "target": "self.turnover_qty", "expression": "999.0" }]
                }
            ]
        }],
        "edge_types": [],
        "mappings": []
    })
}

#[tokio::test]
async fn mcp_axbuilder_redteam_dryrun_action_spec_proposed_draft_pii_and_precondition()
-> Result<(), AppError> {
    let hub_node = serde_json::from_value(serde_json::json!({
        "id": Uuid::from_u128(0x9902),
        "type_name": "DevHub",
        "node_key": "DH-1",
        "title": "沙盒一号枢纽",
        "properties": { "manager_phone": "13788886666" },
        "metrics": { "stock_qty": 200.0, "turnover_qty": 100.0 },
        "row_version": 1
    }))
    .map_err(|e| AppError::Config(format!("hub_node deserialize: {e}")))?;

    let (stack, builder, sid) =
        setup_redteam_dev_sandbox_session(seed_redteam_dev_hub_schema(), hub_node).await?;

    // 1. PROPOSED draft ActionSpec dry-run mutates in memory, recomputes derived rule, masks PII, and leaves store untouched
    let ok_rpc = call_builder_mcp_tool(
        &stack,
        &builder,
        &sid,
        "axbuilder_dryrun_action_spec",
        serde_json::json!({
            "subgraph_slug": "dev_sbx",
            "type_name": "DevHub",
            "node_key": "DH-1",
            "action_name": "dispatch_batch",
            "inputs": { "amount": 60.0 }
        }),
    )
    .await?;
    let ok_val = decode_mcp_tool_text(&ok_rpc)?;
    assert_eq!(ok_val.get("status"), Some(&serde_json::json!("DRY_RUN")));
    assert_eq!(
        ok_val.pointer("/node/metrics/stock_qty"),
        Some(&serde_json::json!(140.0))
    );
    assert_eq!(
        ok_val.pointer("/node/metrics/turnover_qty"),
        Some(&serde_json::json!(70.0))
    );
    let masked_phone = ok_val
        .pointer("/node/properties/manager_phone")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert!(masked_phone.contains("****"));
    assert_ne!(masked_phone, "13788886666");
    {
        let guard = stack.store.inner.lock().await;
        assert_eq!(guard.get_node_main_calls, 0);
        let stored_qty = guard
            .sandbox_nodes
            .get("dev_sbx")
            .and_then(|v| v.first())
            .and_then(|n| n.metrics.get("stock_qty"))
            .copied();
        assert_eq!(stored_qty, Some(200.0));
    }

    // 2. Precondition violation returns 422 ACTION_PRECONDITION_FAILED
    let pre_rpc = call_builder_mcp_tool(
        &stack,
        &builder,
        &sid,
        "axbuilder_dryrun_action_spec",
        serde_json::json!({
            "subgraph_slug": "dev_sbx",
            "type_name": "DevHub",
            "node_key": "DH-1",
            "action_name": "dispatch_batch",
            "inputs": { "amount": 500.0 }
        }),
    )
    .await?;
    assert_eq!(
        pre_rpc.pointer("/result/isError"),
        Some(&serde_json::json!(true))
    );
    assert!(pre_rpc.to_string().contains("ACTION_PRECONDITION_FAILED"));

    // 3. Direct mutation of derived metric returns DERIVED_METRIC_WRITE_FORBIDDEN
    let prot_rpc = call_builder_mcp_tool(
        &stack,
        &builder,
        &sid,
        "axbuilder_dryrun_action_spec",
        serde_json::json!({
            "subgraph_slug": "dev_sbx",
            "type_name": "DevHub",
            "node_key": "DH-1",
            "action_name": "write_protected",
            "inputs": {}
        }),
    )
    .await?;
    assert_eq!(
        prot_rpc.pointer("/result/isError"),
        Some(&serde_json::json!(true))
    );
    assert!(
        prot_rpc
            .to_string()
            .contains("DERIVED_METRIC_WRITE_FORBIDDEN")
    );
    Ok(())
}
