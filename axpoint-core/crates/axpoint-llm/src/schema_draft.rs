//! AI-assisted `xBlueStudio` ontology & mapping draft generator (`06-STEP05-06` §05.5).
//!
//! Consumes deterministic dataset profiling summaries (`ProfilingBundle`), hypergraph document
//! entity groups (`DocEntityGroup`), and the currently published `SchemaDoc`, then invokes an
//! [`AiPort`] model to synthesize a candidate [`SchemaDoc`] and runs `axpoint_domain::studio::validate`
//! with up to 1 automatic self-healing retry (`MAX_SCHEMA_DRAFT_ATTEMPTS = 2`).

use std::fmt::Write as _;

use axpoint_domain::AppError;
use axpoint_domain::enums::Purpose;
use axpoint_domain::json_schema::JsonSchemaSpec;
use axpoint_domain::ports::AiPort;
use axpoint_domain::studio::{
    DocEntityGroup, IssueSeverity, ProfilingBundle, SchemaDoc, ValidationContext, ValidationIssue,
    validate,
};

/// Maximum LLM generation attempts (`1` initial attempt + `1` automatic validation-feedback retry).
pub const MAX_SCHEMA_DRAFT_ATTEMPTS: u32 = 2;

/// Deterministic system prompt governing `SchemaDoc` JSON synthesis (`06-STEP05-06` §05.5).
pub const SCHEMA_DRAFT_SYSTEM_PROMPT: &str = "\
你是 xBlueNode / AxPoint 平台的本体架构专家（xBlueStudio AI Schema Architect）。\
请严格基于输入的数据集画像（Profiling）、文档超图实体及当前已发布本体，输出一个合法 JSON 对象（SchemaDoc），不得包含 Markdown 代码块围栏或任何额外解释文字。\n\
输出 JSON 结构必须严格符合：\n\
{\n\
  \"node_types\": [\n\
    {\n\
      \"name\": \"PascalCase英文类型名（^[A-Z][A-Za-z0-9]{0,62}$）\",\n\
      \"display_name\": \"中文显示名（1..64字符）\",\n\
      \"description\": \"业务说明\",\n\
      \"icon\": \"box\",\n\
      \"color\": \"#2563eb\",\n\
      \"title_template\": \"{node_key}\",\n\
      \"properties\": [\n\
        {\"name\": \"snake_case属性名\", \"display_name\": \"显示名\", \"data_type\": \"STRING|INT64|FLOAT64|DECIMAL|BOOLEAN|DATE|TIMESTAMP\", \"kind\": \"ATTRIBUTE|METRIC\", \"required\": false, \"pii_category\": \"NONE\"}\n\
      ],\n\
      \"agg_rules\": [\n\
        {\"target_metric\": \"本类型METRIC名\", \"op\": \"SUM|AVG|MIN|MAX|COUNT\", \"via_link_type\": \"UPPER_SNAKE关系名\", \"direction\": \"OUT|IN\", \"source_type_name\": \"邻居类型名\", \"source_metric\": \"邻居METRIC名\", \"weight_by_link\": false}\n\
      ],\n\
      \"derived_rules\": [\n\
        {\"target_metric\": \"本类型METRIC名\", \"expression\": \"确定性指标表达式（如 if(total_demand > 0, inventory / total_demand, null)）\", \"enabled\": true}\n\
      ],\n\
      \"sync_to_knowledge\": false,\n\
      \"pos_x\": 120.0,\n\
      \"pos_y\": 140.0\n\
    }\n\
  ],\n\
  \"edge_types\": [\n\
    {\"name\": \"UPPER_SNAKE关系名\", \"src_type_name\": \"源类型\", \"dst_type_name\": \"目标类型\", \"impact_weight\": 0.8, \"cardinality\": \"ONE_TO_ONE|ONE_TO_MANY|MANY_TO_ONE|MANY_TO_MANY\"}\n\
  ],\n\
  \"mappings\": [\n\
    {\"name\": \"映射名\", \"dataset\": \"数据集name\", \"node_type\": \"目标类型\", \"key_column\": \"100%非空主键列\", \"title\": \"{key}\", \"attributes\": {}, \"metrics\": {}, \"links\": [], \"on_source_delete\": \"KEEP\", \"mode\": \"INCREMENTAL\"}\n\
  ]\n\
}\n\
硬性约束：\n\
1. 严禁使用保留属性名 id, tenant_id, node_key, title, metrics, properties。\n\
2. impact_weight 必须在 (0.0, 1.0] 内。\n\
3. 严禁产生聚合/派生指标循环依赖（CYCLE_DETECTED）或同一指标多条规则产出（METRIC_MULTIPLE_PRODUCERS）。\n\
4. mappings 只允许引用画像中已存在的 READY 数据集与真实列名。";

/// Input bundle for [`generate_schema_draft`].
#[derive(Debug, Clone)]
pub struct SchemaDraftInput<'a> {
    /// Deterministic dataset profiling and FK candidates (`06-STEP05-06` §05.5.1).
    pub profiling: &'a ProfilingBundle,
    /// Grouped hypergraph entities and relations from selected documents (`§05.5` step 2).
    pub doc_entities: &'a [DocEntityGroup],
    /// Currently published `SchemaDoc` of the target subgraph (preserved/extended incrementally).
    pub current_schema: &'a SchemaDoc,
    /// Optional natural-language domain description provided by the user.
    pub description: &'a str,
    /// Validation context for the self-check and self-healing loop (`§05.5` step 4).
    pub validation_ctx: &'a ValidationContext,
}

/// Output produced by [`generate_schema_draft`].
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedSchemaDraft {
    /// Synthesized (and layout-normalized) [`SchemaDoc`].
    pub schema_doc: SchemaDoc,
    /// Validation issues remaining after up to [`MAX_SCHEMA_DRAFT_ATTEMPTS`] attempts.
    pub issues: Vec<ValidationIssue>,
    /// Number of LLM calls performed (`1` or `2`).
    pub attempts: u32,
}

/// Generates a candidate [`SchemaDoc`] via `llm`, running `studio::validate` and up to one
/// automatic repair pass if the initial response fails JSON parsing or triggers `ERROR` diagnostics.
///
/// Never auto-publishes (`06-STEP05-06` §05.5 step 4).
///
/// # Errors
/// Returns [`AppError::Llm`] if the upstream chat call fails or if both attempts fail to produce
/// syntactically valid `SchemaDoc` JSON.
pub async fn generate_schema_draft<P: AiPort>(
    llm: &P,
    input: &SchemaDraftInput<'_>,
) -> Result<GeneratedSchemaDraft, AppError> {
    let user_prompt = build_schema_draft_user_prompt(input);
    let draft_schema = JsonSchemaSpec::schema_doc_draft();
    let first_attempt = llm
        .chat_structured(
            Purpose::Extract,
            SCHEMA_DRAFT_SYSTEM_PROMPT,
            &user_prompt,
            &draft_schema,
        )
        .await;

    let (raw_first, repair_feedback) = match first_attempt {
        Ok(val_first) => {
            let serialized =
                serde_json::to_string(&val_first).unwrap_or_else(|_| String::from("{}"));
            match serde_json::from_value::<SchemaDoc>(val_first) {
                Ok(mut doc) => {
                    assign_grid_layout_if_overlapping(&mut doc);
                    let issues = validate(&doc, input.validation_ctx);
                    if !issues.iter().any(|i| i.severity == IssueSeverity::Error) {
                        return Ok(GeneratedSchemaDraft {
                            schema_doc: doc,
                            issues,
                            attempts: 1,
                        });
                    }
                    (serialized, format_validation_errors(&issues))
                }
                Err(decode_err) => (serialized, format!("JSON 解析失败：{decode_err}")),
            }
        }
        Err(AppError::Unprocessable {
            code: "STRUCTURED_OUTPUT_INVALID",
            message,
        }) => (String::new(), format!("JSON 结构校验失败：{message}")),
        Err(err) => return Err(err),
    };

    let repair_prompt = build_repair_user_prompt(&user_prompt, &raw_first, &repair_feedback);
    let val_second = llm
        .chat_structured(
            Purpose::Extract,
            SCHEMA_DRAFT_SYSTEM_PROMPT,
            &repair_prompt,
            &draft_schema,
        )
        .await
        .map_err(|err| match err {
            AppError::Unprocessable {
                code: "STRUCTURED_OUTPUT_INVALID",
                message,
            } => AppError::Llm(format!(
                "AI schema draft failed JSON validation after retry: {message}"
            )),
            other => other,
        })?;
    let mut repaired_doc = serde_json::from_value::<SchemaDoc>(val_second).map_err(|err| {
        AppError::Llm(format!(
            "AI schema draft failed JSON validation after retry: {err}"
        ))
    })?;
    assign_grid_layout_if_overlapping(&mut repaired_doc);
    let final_issues = validate(&repaired_doc, input.validation_ctx);

    Ok(GeneratedSchemaDraft {
        schema_doc: repaired_doc,
        issues: final_issues,
        attempts: MAX_SCHEMA_DRAFT_ATTEMPTS,
    })
}

/// Builds the deterministic Markdown user prompt sent to the LLM (`06-STEP05-06` §05.5).
#[must_use]
pub fn build_schema_draft_user_prompt(input: &SchemaDraftInput<'_>) -> String {
    let mut out = String::from("### AI 本体建模请求\n");
    let desc = input.description.trim();
    if desc.is_empty() {
        out.push_str("请基于下方数据集画像、文档实体与已有本体，推导完整的领域节点类型、因果关系、聚合/派生指标与数据映射。\n\n");
    } else {
        let _fmt_ok = writeln!(out, "业务建模需求：{desc}\n");
    }

    append_current_schema_section(&mut out, input.current_schema);
    append_profiling_section(&mut out, input.profiling);
    append_doc_entities_section(&mut out, input.doc_entities);
    out
}

fn append_current_schema_section(out: &mut String, current: &SchemaDoc) {
    out.push_str("### 当前已发布本体摘要\n");
    if current.node_types.is_empty() && current.edge_types.is_empty() {
        out.push_str("（当前子图尚无已发布类型）\n\n");
        return;
    }
    for nt in &current.node_types {
        let prop_list: Vec<String> = nt
            .properties
            .iter()
            .map(|p| format!("{}:{}:{:?}", p.name, p.data_type, p.kind))
            .collect();
        let _fmt_ok = writeln!(
            out,
            "- NodeType `{}` ({}) props=[{}]",
            nt.name,
            nt.display_name,
            prop_list.join(", ")
        );
    }
    for et in &current.edge_types {
        let _fmt_ok = writeln!(
            out,
            "- EdgeType `{}`: {} -> {} ({}, w={})",
            et.name, et.src_type_name, et.dst_type_name, et.cardinality, et.impact_weight
        );
    }
    out.push('\n');
}

fn append_profiling_section(out: &mut String, profiling: &ProfilingBundle) {
    out.push_str("### 结构化数据集画像\n");
    if profiling.datasets.is_empty() {
        out.push_str("（无候选数据集）\n\n");
        return;
    }
    for ds in &profiling.datasets {
        let _fmt_ok = writeln!(
            out,
            "- Dataset `{}` ({}) rows={} pk_candidates=[{}]",
            ds.dataset_name,
            ds.display_name,
            ds.row_count,
            ds.pk_candidates.join(", ")
        );
        for col in &ds.columns {
            let roles: Vec<String> = col.roles.iter().map(|r| format!("{r:?}")).collect();
            let samples: Vec<&str> = col
                .sample_values
                .iter()
                .take(5)
                .map(String::as_str)
                .collect();
            let _fmt_ok = writeln!(
                out,
                "  * col `{}` type={} null_rate={:.3} distinct={} roles=[{}] samples=[{}]",
                col.name,
                col.data_type,
                col.null_rate,
                col.distinct_count,
                roles.join(","),
                samples.join(", ")
            );
        }
    }
    if !profiling.fk_candidates.is_empty() {
        out.push_str("- ForeignKey Candidates:\n");
        for fk in &profiling.fk_candidates {
            let _fmt_ok = writeln!(
                out,
                "  * {}.{} -> {}.{} (overlap={:.2}, cardinality={})",
                fk.src_dataset,
                fk.src_column,
                fk.dst_dataset,
                fk.dst_pk_column,
                fk.overlap_ratio,
                fk.cardinality
            );
        }
    }
    out.push('\n');
}

fn append_doc_entities_section(out: &mut String, groups: &[DocEntityGroup]) {
    if groups.is_empty() {
        return;
    }
    out.push_str("### 文档超图实体与关系\n");
    for g in groups {
        let _fmt_ok = writeln!(
            out,
            "- EntityType `{}`: samples=[{}]",
            g.entity_type,
            g.entity_names.join(", ")
        );
        for (src, dst, fact) in &g.relations {
            let _fmt_ok = writeln!(out, "  * {src} -> {dst}: {fact}");
        }
    }
    out.push('\n');
}

fn build_repair_user_prompt(
    original_user_prompt: &str,
    previous_output: &str,
    error_feedback: &str,
) -> String {
    format!(
        "{original_user_prompt}\n\
         ### 校验错误自动修复请求\n\
         你上一轮输出的 SchemaDoc 未通过 xBlueStudio 静态校验，具体错误如下：\n\
         {error_feedback}\n\
         上一轮输出原文：\n\
         {previous_output}\n\
         请修正上述所有 ERROR 并重新输出完整合法的纯 JSON SchemaDoc 对象。"
    )
}

fn format_validation_errors(issues: &[ValidationIssue]) -> String {
    let mut out = String::new();
    for issue in issues.iter().filter(|i| i.severity == IssueSeverity::Error) {
        let _fmt_ok = writeln!(out, "- [{}] {} @ {}", issue.code, issue.message, issue.path);
    }
    out
}

/// Parses a [`SchemaDoc`] from raw LLM text, tolerating optional Markdown code fences or leading/trailing prose.
///
/// # Errors
/// Returns a human-readable parse error string if no valid `SchemaDoc` JSON object can be decoded.
pub fn parse_schema_doc_lenient(raw: &str) -> Result<SchemaDoc, String> {
    let trimmed = raw.trim();
    if let Ok(doc) = serde_json::from_str::<SchemaDoc>(trimmed) {
        return Ok(doc);
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start <= end
        && let Some(json_slice) = trimmed.get(start..=end)
    {
        return serde_json::from_str::<SchemaDoc>(json_slice)
            .map_err(|err| format!("invalid SchemaDoc JSON object: {err}"));
    }
    Err(String::from("response did not contain a JSON object"))
}

fn assign_grid_layout_if_overlapping(doc: &mut SchemaDoc) {
    if doc.node_types.len() <= 1 {
        return;
    }
    let first_pos = doc
        .node_types
        .first()
        .map_or((120.0, 120.0), |n| (n.pos_x, n.pos_y));
    let all_same = doc
        .node_types
        .iter()
        .all(|n| (n.pos_x - first_pos.0).abs() < 1.0 && (n.pos_y - first_pos.1).abs() < 1.0);
    if !all_same {
        return;
    }
    for (idx, nt) in doc.node_types.iter_mut().enumerate() {
        let col = u32::try_from(idx % 4).unwrap_or(0);
        let row = u32::try_from(idx / 4).unwrap_or(0);
        nt.pos_x = 120.0 + f64::from(col) * 280.0;
        nt.pos_y = 140.0 + f64::from(row) * 220.0;
    }
}
