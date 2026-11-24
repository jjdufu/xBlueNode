//! Comprehensive unit and `proptest` suite for `axpoint-domain::expr` (`>= 150` cases) and `axpoint-domain::rollup` (`06-STEP05-06` §06.1 & §06.3).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::enums::{AggFn, Cardinality, DataType, PiiCategory};
use axpoint_domain::expr::{
    BinaryOp, EvalEnv, EvalWarningCode, Expr, ExprType, ExprTypeEnv, ExprValue, MAX_AST_DEPTH,
    MAX_CALL_ARGS, MAX_EXPR_CHARS, UnaryOp, eval, parse, typecheck, validate_for_node_type,
};
use axpoint_domain::rollup::{
    DirtyInstance, GraphAggregateJobPayload, MapMaterializeJobPayload, MetricDependencyPlan,
    MetricRuleStep, MetricVertex, RuntimeLinkEdge, plan_dirty_propagation,
};
use axpoint_domain::studio::{
    AggRuleSpec, DerivedRuleSpec, EdgeTypeSpec, LinkDirection, NodeTypeSpec, PropertyKind,
    PropertySpec, SchemaDoc,
};
use proptest::prelude::*;
use uuid::Uuid;

fn sample_eval_env() -> EvalEnv {
    let metrics = BTreeMap::from([
        (String::from("inventory"), 150.0),
        (String::from("total_demand"), 100.0),
        (String::from("demand"), 25.0),
        (String::from("zero_m"), 0.0),
        (String::from("neg_m"), -16.0),
    ]);
    let mut props_json = serde_json::Map::new();
    props_json.insert(
        String::from("status"),
        serde_json::Value::String(String::from("ACTIVE")),
    );
    props_json.insert(
        String::from("region"),
        serde_json::Value::String(String::from("East-China")),
    );
    props_json.insert(String::from("tier"), serde_json::json!(2));
    props_json.insert(String::from("active"), serde_json::Value::Bool(true));
    props_json.insert(String::from("empty_attr"), serde_json::Value::Null);
    EvalEnv::from_node(&metrics, &props_json)
}

fn mk_prop(name: &str, dt: DataType, kind: PropertyKind) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        unit: None,
        data_type: dt,
        kind,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
    }
}

fn sample_node_type() -> NodeTypeSpec {
    NodeTypeSpec {
        id: None,
        name: String::from("Warehouse"),
        display_name: String::from("仓库"),
        description: String::new(),
        icon: String::from("warehouse"),
        color: String::from("#2563eb"),
        title_template: String::from("{node_key}"),
        properties: vec![
            mk_prop("inventory", DataType::Float64, PropertyKind::Metric),
            mk_prop("total_demand", DataType::Float64, PropertyKind::Metric),
            mk_prop("demand", DataType::Float64, PropertyKind::Metric),
            mk_prop("zero_m", DataType::Float64, PropertyKind::Metric),
            mk_prop("neg_m", DataType::Float64, PropertyKind::Metric),
            mk_prop("missing_m", DataType::Float64, PropertyKind::Metric),
            mk_prop("status", DataType::String, PropertyKind::Attribute),
            mk_prop("region", DataType::String, PropertyKind::Attribute),
            mk_prop("tier", DataType::Int64, PropertyKind::Attribute),
            mk_prop("active", DataType::Boolean, PropertyKind::Attribute),
            mk_prop("empty_attr", DataType::String, PropertyKind::Attribute),
            mk_prop("created_on", DataType::Date, PropertyKind::Attribute),
            mk_prop("extra_json", DataType::Json, PropertyKind::Attribute),
        ],
        agg_rules: Vec::new(),
        derived_rules: Vec::new(),
        sync_to_knowledge: false,
        pos_x: 0.0,
        pos_y: 0.0,
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
        semantic_layer_tier: None,
        domain_group: None,
        actions: Vec::new(),
    }
}

fn assert_num_case(expr_str: &str, env: &EvalEnv, expected: f64) {
    let ast = parse(expr_str);
    assert!(ast.is_ok(), "failed to parse '{expr_str}': {ast:?}");
    let Ok(ast) = ast else { return };
    let rep = eval(&ast, env);
    match rep.value {
        ExprValue::Number(actual) => {
            let diff = (actual - expected).abs();
            assert!(
                diff <= 1e-9,
                "case '{expr_str}': expected {expected}, got {actual} (diff={diff})"
            );
        }
        other => assert_eq!(
            other,
            ExprValue::Number(expected),
            "case '{expr_str}' unexpected value"
        ),
    }
    assert!(
        rep.warnings.is_empty(),
        "case '{expr_str}' unexpected warnings: {:?}",
        rep.warnings
    );
}

const NUMERIC_CASES: &[(&str, f64)] = &[
    ("0", 0.0),
    ("42", 42.0),
    ("3.5", 3.5),
    ("1.25e2", 125.0),
    ("2.5E-1", 0.25),
    ("1e+3", 1000.0),
    ("-0", 0.0),
    ("-42", -42.0),
    ("--7", 7.0),
    ("1 + 2", 3.0),
    ("10 - 3 - 2", 5.0),
    ("24 / 4 / 2", 3.0),
    ("2 + 3 * 4", 14.0),
    ("(2 + 3) * 4", 20.0),
    ("20 - 4 * 3 + 2", 10.0),
    ("17 % 5", 2.0),
    ("-17 % 5", -2.0),
    ("2 ^ 3", 8.0),
    ("2 ^ 3 ^ 2", 512.0),
    ("(2 ^ 3) ^ 2", 64.0),
    ("-2 ^ 2", -4.0),
    ("(-2) ^ 2", 4.0),
    ("2 ^ -3", 0.125),
    ("3 * 2 ^ 3", 24.0),
    ("inventory", 150.0),
    ("total_demand", 100.0),
    ("inventory / total_demand", 1.5),
    ("inventory - total_demand + demand", 75.0),
    ("prop.tier * 10", 20.0),
    ("abs(-12.75)", 12.75),
    ("abs(12.75)", 12.75),
    ("abs(0)", 0.0),
    ("abs(neg_m)", 16.0),
    ("floor(3.9)", 3.0),
    ("floor(-3.1)", -4.0),
    ("floor(5.0)", 5.0),
    ("ceil(3.1)", 4.0),
    ("ceil(-3.9)", -3.0),
    ("ceil(5.0)", 5.0),
    ("round(2.5)", 3.0),
    ("round(3.5)", 4.0),
    ("round(-2.5)", -3.0),
    ("round(-3.5)", -4.0),
    ("round(2.49)", 2.0),
    ("round(-2.49)", -2.0),
    ("round(2.345, 2)", 2.35),
    ("round(-2.345, 2)", -2.35),
    ("round(1250, -2)", 1300.0),
    ("round(-1250, -2)", -1300.0),
    ("round(12.3456, 0)", 12.0),
    ("sqrt(0)", 0.0),
    ("sqrt(16)", 4.0),
    ("sqrt(2.25)", 1.5),
    ("pow(3, 4)", 81.0),
    ("pow(9, 0.5)", 3.0),
    ("pow(2, 0)", 1.0),
    ("exp(0)", 1.0),
    ("ln(1)", 0.0),
    ("ln(exp(2.5))", 2.5),
    ("log10(1)", 0.0),
    ("log10(1000)", 3.0),
    ("log10(0.01)", -2.0),
    ("min(10, 20)", 10.0),
    ("min(10, -5, 3, 8)", -5.0),
    ("min(null, 42, 17, null)", 17.0),
    ("min(missing_m, inventory, total_demand)", 100.0),
    ("max(10, 20)", 20.0),
    ("max(-10, -5, -30)", -5.0),
    ("max(null, 42, 99, null)", 99.0),
    ("max(missing_m, inventory, total_demand)", 150.0),
    ("clamp(5, 0, 10)", 5.0),
    ("clamp(-3, 0, 10)", 0.0),
    ("clamp(25, 0, 10)", 10.0),
    ("clamp(5, 5, 5)", 5.0),
    ("safe_div(150, 100)", 1.5),
    ("safe_div(150, 100, 0)", 1.5),
    ("safe_div(150, 0, 0)", 0.0),
    ("safe_div(150, zero_m, -1)", -1.0),
    ("if(total_demand > 0, inventory / total_demand, 0)", 1.5),
    ("if(zero_m > 0, inventory / zero_m, 99)", 99.0),
    ("if(missing_m > 0, 1, 7)", 7.0),
    ("coalesce(missing_m, 88)", 88.0),
    ("coalesce(null, null, inventory, 0)", 150.0),
    ("coalesce(demand, 0)", 25.0),
];

const BOOL_CASES: &[(&str, bool)] = &[
    ("true", true),
    ("TRUE", true),
    ("false", false),
    ("FALSE", false),
    ("not false", true),
    ("!true", false),
    ("not not true", true),
    ("!!false", false),
    ("1 == 1", true),
    ("1 == 2", false),
    ("1 != 2", true),
    ("3 < 5", true),
    ("5 < 3", false),
    ("5 <= 5", true),
    ("6 > 5", true),
    ("5 >= 5", true),
    ("'abc' == 'abc'", true),
    ("'abc' != 'def'", true),
    ("'abc' < 'abd'", true),
    ("'b' > 'a'", true),
    ("'it''s' == 'it''s'", true),
    ("true == true", true),
    ("true != false", true),
    ("prop.status == 'ACTIVE'", true),
    ("prop.status != 'CANCELED'", true),
    ("prop.active and prop.tier == 2", true),
    ("false and true or true", true),
    ("true or false and false", true),
    ("(true or false) and false", false),
    ("not 1 == 2 and 3 == 3", true),
    ("false and (missing_m > 0)", false),
    ("(missing_m > 0) and false", false),
    ("false && null", false),
    ("null && false", false),
    ("true or (missing_m > 0)", true),
    ("(missing_m > 0) or true", true),
    ("true || null", true),
    ("null || true", true),
    ("false and (1 / 0 > 0)", false),
    ("true or (1 / 0 > 0)", true),
    ("is_null(null)", true),
    ("is_null(missing_m)", true),
    ("is_null(prop.empty_attr)", true),
    ("is_null(0)", false),
    ("is_null('')", false),
    ("is_null(inventory)", false),
    ("contains(prop.region, 'China')", true),
    ("contains(prop.region, 'West')", false),
    ("starts_with(prop.region, 'East')", true),
    ("starts_with(prop.region, 'North')", false),
];

const TEXT_CASES: &[(&str, &str)] = &[
    ("'hello'", "hello"),
    ("''", ""),
    ("'a''b''c'", "a'b'c"),
    ("lower('East-CHINA')", "east-china"),
    ("upper('East-China')", "EAST-CHINA"),
    ("if(prop.active, lower(prop.status), 'none')", "active"),
    ("coalesce(prop.empty_attr, prop.region)", "East-China"),
];

const NULL_CLEAN_CASES: &[&str] = &[
    "null",
    "NULL",
    "missing_m",
    "missing_m + 10",
    "10 - missing_m",
    "missing_m * 0",
    "missing_m / 5",
    "missing_m ^ 2",
    "-missing_m",
    "not (missing_m > 0)",
    "true and (missing_m > 0)",
    "(missing_m > 0) and true",
    "false or (missing_m > 0)",
    "(missing_m > 0) or false",
    "null and null",
    "null or null",
    "missing_m == 0",
    "missing_m != 0",
    "null == null",
    "null != null",
    "abs(missing_m)",
    "round(missing_m, 2)",
    "sqrt(missing_m)",
    "clamp(missing_m, 0, 10)",
    "min(null, missing_m)",
    "max(null, missing_m)",
    "safe_div(10, 0)",
    "safe_div(10, 0, null)",
    "safe_div(missing_m, 5, 0)",
    "safe_div(10, missing_m, 0)",
    "lower(prop.empty_attr)",
    "upper(prop.empty_attr)",
    "contains(prop.empty_attr, 'x')",
    "starts_with('abc', prop.empty_attr)",
    "coalesce(null, missing_m)",
];

#[test]
fn expr_numeric_precedence_and_builtin_functions_matrix() {
    let env = sample_eval_env();
    for &(expr_str, expected) in NUMERIC_CASES {
        assert_num_case(expr_str, &env, expected);
    }
}

#[test]
fn expr_three_valued_logic_text_and_null_propagation_matrix() {
    let env = sample_eval_env();
    for &(expr_str, expected) in BOOL_CASES {
        let ast = parse(expr_str);
        assert!(ast.is_ok(), "failed to parse '{expr_str}': {ast:?}");
        let Ok(ast) = ast else { continue };
        let rep = eval(&ast, &env);
        assert_eq!(rep.value, ExprValue::Bool(expected), "bool '{expr_str}'");
        assert!(rep.warnings.is_empty(), "warnings on '{expr_str}'");
    }

    for &(expr_str, expected) in TEXT_CASES {
        let ast = parse(expr_str);
        assert!(ast.is_ok(), "failed to parse '{expr_str}': {ast:?}");
        let Ok(ast) = ast else { continue };
        let rep = eval(&ast, &env);
        assert_eq!(
            rep.value,
            ExprValue::Text(String::from(expected)),
            "text '{expr_str}'"
        );
    }

    for &expr_str in NULL_CLEAN_CASES {
        let ast = parse(expr_str);
        assert!(ast.is_ok(), "failed to parse '{expr_str}': {ast:?}");
        let Ok(ast) = ast else { continue };
        let rep = eval(&ast, &env);
        assert_eq!(rep.value, ExprValue::Null, "null '{expr_str}'");
        assert!(rep.warnings.is_empty(), "null warnings on '{expr_str}'");
    }

    let total = NUMERIC_CASES.len() + BOOL_CASES.len() + TEXT_CASES.len() + NULL_CLEAN_CASES.len();
    assert!(total >= 160, "expected >= 160 matrix cases, got {total}");
}

#[test]
fn expr_domain_and_div_by_zero_warnings_produce_null_without_panicking() {
    let env = sample_eval_env();

    let div_zero_cases = [
        "10 / 0",
        "inventory / zero_m",
        "15 % 0",
        "inventory % zero_m",
    ];
    for expr_str in div_zero_cases {
        let ast = parse(expr_str);
        assert!(ast.is_ok());
        let Ok(ast) = ast else { continue };
        let rep = eval(&ast, &env);
        assert_eq!(rep.value, ExprValue::Null, "div-zero '{expr_str}'");
        assert_eq!(rep.warnings, vec![EvalWarningCode::DivByZero]);
        assert_eq!(rep.value.to_json_value(), serde_json::Value::Null);
    }

    let domain_cases = [
        "sqrt(-1)",
        "sqrt(neg_m)",
        "ln(0)",
        "ln(-5)",
        "log10(0)",
        "log10(-10)",
        "pow(-2, 0.5)",
        "(-2) ^ 0.5",
        "exp(1000)",
        "1e308 * 10",
        "clamp(5, 10, 2)",
    ];
    for expr_str in domain_cases {
        let ast = parse(expr_str);
        assert!(ast.is_ok(), "parse '{expr_str}'");
        let Ok(ast) = ast else { continue };
        let rep = eval(&ast, &env);
        assert_eq!(rep.value, ExprValue::Null, "domain '{expr_str}'");
        assert_eq!(rep.warnings, vec![EvalWarningCode::DomainError]);
    }
}

#[test]
fn expr_error_columns_limits_and_static_typecheck() {
    let err_eq = parse("inventory = 10").err();
    assert_eq!(err_eq.as_ref().and_then(|e| e.column), Some(11));

    let err_chain = parse("1 < 2 < 3").err();
    assert_eq!(err_chain.as_ref().and_then(|e| e.column), Some(7));

    let err_unterm = parse("prop.status == 'open").err();
    assert_eq!(err_unterm.as_ref().and_then(|e| e.column), Some(16));

    let err_dot = parse("other.field").err();
    assert_eq!(err_dot.as_ref().and_then(|e| e.column), Some(1));

    let err_prop = parse("prop + 1").err();
    assert_eq!(err_prop.as_ref().and_then(|e| e.column), Some(1));

    let long_expr = "1 + ".repeat(501) + "1";
    assert!(long_expr.chars().count() > MAX_EXPR_CHARS);
    let err_long = parse(&long_expr).err();
    assert_eq!(
        err_long.as_ref().and_then(|e| e.column),
        Some(MAX_EXPR_CHARS + 1)
    );

    let deep_parens = format!(
        "{}1{}",
        "(".repeat(MAX_AST_DEPTH + 2),
        ")".repeat(MAX_AST_DEPTH + 2)
    );
    assert!(parse(&deep_parens).is_err());

    let deep_chain = vec!["1"; MAX_AST_DEPTH + 2].join(" + ");
    assert!(parse(&deep_chain).is_err());

    let ok_args = vec!["1"; MAX_CALL_ARGS].join(", ");
    assert!(parse(&format!("min({ok_args})")).is_ok());
    let too_many_args = vec!["1"; MAX_CALL_ARGS + 1].join(", ");
    assert!(parse(&format!("min({too_many_args})")).is_err());

    let nt = sample_node_type();
    let ok_resp = validate_for_node_type(
        &nt,
        "if(total_demand > 0 and prop.status != 'CANCELED', safe_div(inventory, total_demand, 0), null)",
    );
    assert!(ok_resp.valid, "expected valid: {ok_resp:?}");
    assert_eq!(ok_resp.inferred_type, Some(ExprType::Number));

    let bad_cases = [
        "unknown_metric + 1",
        "prop.unknown_attr == 'x'",
        "prop.created_on == '2026-01-01'",
        "prop.extra_json == 'x'",
        "inventory + 'abc'",
        "prop.active < true",
        "if(inventory, 1, 2)",
        "if(prop.active, 1, 'two')",
        "coalesce(1, 'two')",
        "min(1)",
        "unknown_fn(1)",
    ];
    let tenv = ExprTypeEnv::from_node_type(&nt);
    for bad in bad_cases {
        let res = parse(bad).and_then(|ast| typecheck(&ast, &tenv));
        assert!(res.is_err(), "expected typecheck failure for '{bad}'");
    }
}

fn arb_expr() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        (0_u32..10_000_u32).prop_map(|n| Expr::Literal(ExprValue::Number(f64::from(n)))),
        (1_u32..1_000_u32).prop_map(|n| Expr::Literal(ExprValue::Number(f64::from(n) / 8.0))),
        "[a-zA-Z0-9 _'-]{0,12}".prop_map(|s| Expr::Literal(ExprValue::Text(s))),
        any::<bool>().prop_map(|b| Expr::Literal(ExprValue::Bool(b))),
        Just(Expr::Literal(ExprValue::Null)),
        prop_oneof![
            Just(String::from("inventory")),
            Just(String::from("total_demand")),
            Just(String::from("demand")),
        ]
        .prop_map(Expr::MetricRef),
        prop_oneof![
            Just(String::from("status")),
            Just(String::from("region")),
            Just(String::from("tier")),
        ]
        .prop_map(Expr::PropRef),
    ];

    leaf.prop_recursive(4, 32, 4, |inner| {
        prop_oneof![
            (
                prop_oneof![Just(UnaryOp::Not), Just(UnaryOp::Neg)],
                inner.clone()
            )
                .prop_map(|(op, e)| Expr::Unary {
                    op,
                    expr: Box::new(e),
                }),
            (
                prop_oneof![
                    Just(BinaryOp::Or),
                    Just(BinaryOp::And),
                    Just(BinaryOp::Eq),
                    Just(BinaryOp::Ne),
                    Just(BinaryOp::Lt),
                    Just(BinaryOp::Le),
                    Just(BinaryOp::Gt),
                    Just(BinaryOp::Ge),
                    Just(BinaryOp::Add),
                    Just(BinaryOp::Sub),
                    Just(BinaryOp::Mul),
                    Just(BinaryOp::Div),
                    Just(BinaryOp::Mod),
                    Just(BinaryOp::Pow),
                ],
                inner.clone(),
                inner.clone()
            )
                .prop_map(|(op, l, r)| Expr::Binary {
                    op,
                    left: Box::new(l),
                    right: Box::new(r),
                }),
            (
                prop_oneof![
                    Just(String::from("abs")),
                    Just(String::from("sqrt")),
                    Just(String::from("is_null")),
                ],
                inner.clone()
            )
                .prop_map(|(name, a)| Expr::Call {
                    name,
                    args: vec![a],
                }),
            (
                prop_oneof![
                    Just(String::from("min")),
                    Just(String::from("max")),
                    Just(String::from("coalesce")),
                    Just(String::from("safe_div")),
                ],
                inner.clone(),
                inner.clone()
            )
                .prop_map(|(name, a, b)| Expr::Call {
                    name,
                    args: vec![a, b],
                }),
            (inner.clone(), inner.clone(), inner).prop_map(|(c, t, e)| Expr::Call {
                name: String::from("if"),
                args: vec![c, t, e],
            }),
        ]
    })
}

proptest! {
    #[test]
    fn expr_proptest_display_parse_roundtrip_is_exact(ast in arb_expr()) {
        let formatted = ast.to_string();
        let reparsed = parse(&formatted);
        prop_assert!(reparsed.is_ok(), "failed to reparse '{formatted}': {reparsed:?}");
        if let Ok(parsed_ast) = reparsed {
            prop_assert_eq!(&parsed_ast, &ast);
        }

        let env = sample_eval_env();
        let rep = eval(&ast, &env);
        if let ExprValue::Number(v) = rep.value {
            prop_assert!(v.is_finite());
        }
    }

    #[test]
    fn expr_proptest_arbitrary_strings_never_panic(raw in ".*") {
        if let Ok(ast) = parse(&raw) {
            let env = sample_eval_env();
            let rep = eval(&ast, &env);
            if let ExprValue::Number(v) = rep.value {
                prop_assert!(v.is_finite());
            }
        }
    }
}

fn build_node_type(
    name: &str,
    metrics: &[&str],
    agg_rules: Vec<AggRuleSpec>,
    derived_rules: Vec<DerivedRuleSpec>,
) -> NodeTypeSpec {
    NodeTypeSpec {
        id: None,
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        icon: String::from("box"),
        color: String::from("#2563eb"),
        title_template: String::from("{node_key}"),
        properties: metrics
            .iter()
            .map(|m| mk_prop(m, DataType::Float64, PropertyKind::Metric))
            .collect(),
        agg_rules,
        derived_rules,
        sync_to_knowledge: false,
        pos_x: 0.0,
        pos_y: 0.0,
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
        semantic_layer_tier: None,
        domain_group: None,
        actions: Vec::new(),
    }
}

fn three_tier_supply_chain_doc() -> SchemaDoc {
    let order = build_node_type("Order", &["demand"], Vec::new(), Vec::new());
    let warehouse = build_node_type(
        "Warehouse",
        &["inventory", "total_demand", "fill_rate"],
        vec![AggRuleSpec {
            target_metric: String::from("total_demand"),
            op: AggFn::Sum,
            via_link_type: String::from("FULFILLS"),
            direction: LinkDirection::Out,
            source_type_name: String::from("Order"),
            source_metric: String::from("demand"),
            filter: None,
            weight_by_link: false,
            maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
        }],
        vec![DerivedRuleSpec {
            target_metric: String::from("fill_rate"),
            expression: String::from("if(total_demand > 0, inventory / total_demand, null)"),
            enabled: true,
            maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
        }],
    );
    let factory = build_node_type(
        "Factory",
        &["network_demand", "avg_fill_rate"],
        vec![
            AggRuleSpec {
                target_metric: String::from("network_demand"),
                op: AggFn::Sum,
                via_link_type: String::from("SUPPLIES"),
                direction: LinkDirection::Out,
                source_type_name: String::from("Warehouse"),
                source_metric: String::from("total_demand"),
                filter: None,
                weight_by_link: false,
                maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            },
            AggRuleSpec {
                target_metric: String::from("avg_fill_rate"),
                op: AggFn::Avg,
                via_link_type: String::from("SUPPLIES"),
                direction: LinkDirection::Out,
                source_type_name: String::from("Warehouse"),
                source_metric: String::from("fill_rate"),
                filter: None,
                weight_by_link: false,
                maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            },
        ],
        Vec::new(),
    );

    SchemaDoc {
        node_types: vec![factory, warehouse, order],
        edge_types: vec![
            EdgeTypeSpec {
                id: None,
                name: String::from("SUPPLIES"),
                src_type_name: String::from("Factory"),
                dst_type_name: String::from("Warehouse"),
                impact_weight: 1.0,
                cardinality: Cardinality::OneToMany,
                description: String::new(),
                properties: Vec::new(),
                propagation: serde_json::json!({}),
                maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                derived_rules: Vec::new(),
                agg_rules: Vec::new(),
            },
            EdgeTypeSpec {
                id: None,
                name: String::from("FULFILLS"),
                src_type_name: String::from("Warehouse"),
                dst_type_name: String::from("Order"),
                impact_weight: 1.0,
                cardinality: Cardinality::OneToMany,
                description: String::new(),
                properties: Vec::new(),
                propagation: serde_json::json!({}),
                maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                derived_rules: Vec::new(),
                agg_rules: Vec::new(),
            },
        ],
        mappings: Vec::new(),
    }
}

fn mk_link_edge(
    link_type: &str,
    src_id: Uuid,
    src_type: &str,
    dst_id: Uuid,
    dst_type: &str,
) -> RuntimeLinkEdge {
    RuntimeLinkEdge {
        link_type: String::from(link_type),
        src_node_id: src_id,
        src_type_name: String::from(src_type),
        dst_node_id: dst_id,
        dst_type_name: String::from(dst_type),
    }
}

#[test]
fn rollup_three_tier_dag_layers_are_topologically_ordered() {
    let doc = three_tier_supply_chain_doc();
    let plan = MetricDependencyPlan::from_schema(&doc);
    assert_eq!(plan.steps.len(), 4);

    let layers = plan.layers();
    assert_eq!(layers.len(), 3);

    let l0: BTreeSet<MetricVertex> = layers
        .first()
        .map_or_default(|s| s.iter().map(MetricRuleStep::target_vertex).collect());
    assert_eq!(
        l0,
        BTreeSet::from([MetricVertex::new("Warehouse", "total_demand")])
    );

    let l1: BTreeSet<MetricVertex> = layers
        .get(1)
        .map_or_default(|s| s.iter().map(MetricRuleStep::target_vertex).collect());
    assert_eq!(
        l1,
        BTreeSet::from([
            MetricVertex::new("Warehouse", "fill_rate"),
            MetricVertex::new("Factory", "network_demand"),
        ])
    );

    let l2: BTreeSet<MetricVertex> = layers
        .get(2)
        .map_or_default(|s| s.iter().map(MetricRuleStep::target_vertex).collect());
    assert_eq!(
        l2,
        BTreeSet::from([MetricVertex::new("Factory", "avg_fill_rate")])
    );
}

#[test]
fn rollup_dirty_propagation_deduplicates_parents_and_isolates_unaffected_nodes() {
    let doc = three_tier_supply_chain_doc();
    let plan = MetricDependencyPlan::from_schema(&doc);

    let f1 = Uuid::new_v4();
    let w1 = Uuid::new_v4();
    let w2 = Uuid::new_v4();
    let o1 = Uuid::new_v4();
    let o2 = Uuid::new_v4();
    let o3 = Uuid::new_v4();

    let links = vec![
        mk_link_edge("SUPPLIES", f1, "Factory", w1, "Warehouse"),
        mk_link_edge("SUPPLIES", f1, "Factory", w2, "Warehouse"),
        mk_link_edge("FULFILLS", w1, "Warehouse", o1, "Order"),
        mk_link_edge("FULFILLS", w1, "Warehouse", o2, "Order"),
        mk_link_edge("FULFILLS", w2, "Warehouse", o3, "Order"),
    ];

    let seeds = vec![
        DirtyInstance {
            node_id: o1,
            type_name: String::from("Order"),
            metric: String::from("demand"),
        },
        DirtyInstance {
            node_id: o2,
            type_name: String::from("Order"),
            metric: String::from("demand"),
        },
    ];

    let scheduled = plan_dirty_propagation(&plan, &seeds, &links);
    assert_eq!(scheduled.len(), 3);

    let wave0 = scheduled.first().cloned().unwrap_or_default();
    assert_eq!(wave0.len(), 1);
    assert_eq!(
        wave0.first().map(|t| &t.target_node_ids),
        Some(&BTreeSet::from([w1]))
    );

    let wave1 = scheduled.get(1).cloned().unwrap_or_default();
    assert_eq!(wave1.len(), 2);
    for task in &wave1 {
        assert!(!task.target_node_ids.contains(&w2));
    }

    let wave2 = scheduled.get(2).cloned().unwrap_or_default();
    assert_eq!(wave2.len(), 1);
    assert_eq!(
        wave2.first().map(|t| &t.target_node_ids),
        Some(&BTreeSet::from([f1]))
    );
}

#[test]
fn rollup_job_payload_hashes_and_idempotency_keys_are_deterministic() {
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    let p1 = GraphAggregateJobPayload {
        full: false,
        full_type: None,
        node_ids: vec![id_a, id_b, id_a],
        metrics: vec![String::from("demand"), String::from("inventory")],
        schema_version: Some(3),
    };
    let p2 = GraphAggregateJobPayload {
        full: false,
        full_type: None,
        node_ids: vec![id_b, id_a],
        metrics: vec![String::from("inventory"), String::from("demand")],
        schema_version: Some(3),
    };
    assert_eq!(p1.canonical_hash(), p2.canonical_hash());

    let map_id = Uuid::new_v4();
    assert_eq!(
        MapMaterializeJobPayload::idempotency_key_for_snapshot(map_id, 42),
        format!("{map_id}:42")
    );
}
