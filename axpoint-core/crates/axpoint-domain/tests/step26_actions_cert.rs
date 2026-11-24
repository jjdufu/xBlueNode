//! Unit tests for Step 26 `ActionSpec` contracts, 4-path edge dynamics (`V->V`, `V->E`, `E->V`, `E->E`),
//! `DEAD_KNOWLEDGE` closure scanning, and L0–L4 runtime readiness certification.

use axpoint_domain::enums::{
    ActionSideEffectTier, AggFn, Cardinality, DataType, DesignMaturity, Permission, PiiCategory,
    PropagationPathKind, RuntimeCertLevel,
};
use axpoint_domain::rollup::{FourPathDependencyPlan, PropagationVertex};
use axpoint_domain::studio::{
    ActionParamSpec, ActionSpec, AggRuleSpec, DerivedRuleSpec, EdgeAggRuleSpec,
    EdgeDerivedRuleSpec, EdgeTypeSpec, IssueSeverity, LinkDirection, MutationStepSpec,
    NodeTypeSpec, PropertyKind, PropertySpec, SchemaDoc, ValidationContext,
    evaluate_cert_tier_and_score, resolve_action_permission, scan_dead_knowledge, validate,
    validate_with_metrics,
};

fn metric_prop(name: &str) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        unit: Some(String::from("u")),
        data_type: DataType::Float64,
        kind: PropertyKind::Metric,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
        maturity: DesignMaturity::Confirmed,
    }
}

#[test]
fn validate_forbids_mutating_derived_or_aggregated_metrics_and_enforces_tier() {
    let bad_derived_doc = SchemaDoc {
        node_types: vec![NodeTypeSpec {
            name: String::from("Step26Wh"),
            display_name: String::from("测试仓"),
            properties: vec![metric_prop("stock"), metric_prop("safety_gap")],
            derived_rules: vec![DerivedRuleSpec {
                target_metric: String::from("safety_gap"),
                expression: String::from("100 - stock"),
                enabled: true,
                maturity: DesignMaturity::Confirmed,
            }],
            actions: vec![ActionSpec {
                name: String::from("tamper_derived"),
                display_name: String::from("违规覆写"),
                side_effect_tier: ActionSideEffectTier::SelfOnly,
                mutations: vec![MutationStepSpec {
                    target: String::from("self.safety_gap"),
                    expression: String::from("0"),
                    filter: None,
                }],
                ..ActionSpec::default()
            }],
            ..NodeTypeSpec::default()
        }],
        edge_types: Vec::new(),
        mappings: Vec::new(),
    };

    let issues = validate(&bad_derived_doc, &ValidationContext::default());
    assert!(
        issues
            .iter()
            .any(|i| i.severity == IssueSeverity::Error
                && i.code == "DERIVED_METRIC_WRITE_FORBIDDEN")
    );

    let tier_violation_doc = SchemaDoc {
        node_types: vec![
            NodeTypeSpec {
                name: String::from("Hub"),
                display_name: String::from("枢纽"),
                properties: vec![metric_prop("reserve")],
                actions: vec![ActionSpec {
                    name: String::from("push_downstream"),
                    display_name: String::from("下推"),
                    side_effect_tier: ActionSideEffectTier::SelfOnly,
                    mutations: vec![MutationStepSpec {
                        target: String::from("via(LANE, OUT).stock"),
                        expression: String::from("target.stock + 10"),
                        filter: None,
                    }],
                    ..ActionSpec::default()
                }],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Depot"),
                display_name: String::from("前置仓"),
                properties: vec![metric_prop("stock")],
                ..NodeTypeSpec::default()
            },
        ],
        edge_types: vec![EdgeTypeSpec {
            name: String::from("LANE"),
            src_type_name: String::from("Hub"),
            dst_type_name: String::from("Depot"),
            cardinality: Cardinality::OneToMany,
            ..EdgeTypeSpec::default()
        }],
        mappings: Vec::new(),
    };

    let tier_issues = validate(&tier_violation_doc, &ValidationContext::default());
    assert!(
        tier_issues
            .iter()
            .any(|i| i.severity == IssueSeverity::Error && i.code == "SIDE_EFFECT_TIER_VIOLATION")
    );
}

#[test]
fn dead_knowledge_detects_isolated_derived_metric_and_clears_when_action_consumes_it() {
    let mut doc = SchemaDoc {
        node_types: vec![NodeTypeSpec {
            name: String::from("CleanNode"),
            display_name: String::from("节点"),
            description: String::from("测试"),
            properties: vec![metric_prop("base_load"), metric_prop("headroom")],
            derived_rules: vec![DerivedRuleSpec {
                target_metric: String::from("headroom"),
                expression: String::from("100 - base_load"),
                enabled: true,
                maturity: DesignMaturity::Confirmed,
            }],
            ..NodeTypeSpec::default()
        }],
        edge_types: Vec::new(),
        mappings: Vec::new(),
    };

    let dk_before = scan_dead_knowledge(&doc);
    assert_eq!(dk_before.len(), 1);
    let summary_before = validate_with_metrics(&doc, &ValidationContext::default());
    assert!(
        summary_before
            .issues
            .iter()
            .any(|i| i.code == "DEAD_KNOWLEDGE")
    );

    if let Some(nt) = doc.node_types.first_mut() {
        nt.actions.push(ActionSpec {
            name: String::from("add_load"),
            display_name: String::from("加载"),
            parameters: vec![ActionParamSpec {
                name: String::from("delta"),
                display_name: String::from("增量"),
                data_type: DataType::Float64,
                required: true,
                default_value: None,
                description: String::new(),
            }],
            precondition_expr: Some(String::from("headroom >= param.delta")),
            side_effect_tier: ActionSideEffectTier::SelfOnly,
            mutations: vec![MutationStepSpec {
                target: String::from("self.base_load"),
                expression: String::from("base_load + param.delta"),
                filter: None,
            }],
            ..ActionSpec::default()
        });
    }

    let dk_after = scan_dead_knowledge(&doc);
    assert_eq!(dk_after.len(), 0);
    let summary_after = validate_with_metrics(&doc, &ValidationContext::default());
    assert!(
        !summary_after
            .issues
            .iter()
            .any(|i| i.code == "DEAD_KNOWLEDGE")
    );
}

#[test]
fn four_path_plan_orders_v2e_e2e_e2v_and_v2v_and_evaluates_cert_tiers() {
    let doc = SchemaDoc {
        node_types: vec![
            NodeTypeSpec {
                name: String::from("Hub"),
                display_name: String::from("枢纽"),
                properties: vec![metric_prop("output_rate")],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Transit"),
                display_name: String::from("中转"),
                properties: vec![metric_prop("congestion")],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Depot"),
                display_name: String::from("前置仓"),
                properties: vec![metric_prop("inbound_flux"), metric_prop("net_margin")],
                agg_rules: vec![AggRuleSpec {
                    target_metric: String::from("inbound_flux"),
                    op: AggFn::Sum,
                    via_link_type: String::from("LAST_MILE"),
                    direction: LinkDirection::In,
                    source_type_name: String::from("LAST_MILE"),
                    source_metric: String::from("relayed_flux"),
                    filter: None,
                    weight_by_link: false,
                    maturity: DesignMaturity::Confirmed,
                }],
                derived_rules: vec![DerivedRuleSpec {
                    target_metric: String::from("net_margin"),
                    expression: String::from("inbound_flux * 0.9"),
                    enabled: true,
                    maturity: DesignMaturity::Confirmed,
                }],
                ..NodeTypeSpec::default()
            },
        ],
        edge_types: vec![
            EdgeTypeSpec {
                name: String::from("TRUNK_LANE"),
                src_type_name: String::from("Hub"),
                dst_type_name: String::from("Transit"),
                properties: vec![metric_prop("effective_flux")],
                derived_rules: vec![EdgeDerivedRuleSpec {
                    target_metric: String::from("effective_flux"),
                    expression: String::from("src.output_rate * (1 - dst.congestion)"),
                    path_kind: PropagationPathKind::VertexToEdge,
                    enabled: true,
                    maturity: DesignMaturity::Confirmed,
                }],
                ..EdgeTypeSpec::default()
            },
            EdgeTypeSpec {
                name: String::from("LAST_MILE"),
                src_type_name: String::from("Transit"),
                dst_type_name: String::from("Depot"),
                properties: vec![metric_prop("relayed_flux")],
                agg_rules: vec![EdgeAggRuleSpec {
                    target_metric: String::from("relayed_flux"),
                    op: AggFn::Sum,
                    via_link_type: String::from("TRUNK_LANE"),
                    direction: LinkDirection::In,
                    source_metric: String::from("effective_flux"),
                    filter: None,
                    maturity: DesignMaturity::Confirmed,
                }],
                ..EdgeTypeSpec::default()
            },
        ],
        mappings: Vec::new(),
    };

    let val_issues = validate(&doc, &ValidationContext::default());
    assert_eq!(
        val_issues.len(),
        0,
        "unexpected validation issues: {val_issues:?}"
    );

    let plan = FourPathDependencyPlan::from_schema(&doc);
    assert_eq!(plan.steps.len(), 4);
    let chain = plan.build_rule_chain(&PropagationVertex::node("Depot", "net_margin"));
    assert_eq!(chain.len(), 4);

    assert_eq!(
        evaluate_cert_tier_and_score(&[], false, true, 0, 0, true),
        (RuntimeCertLevel::L4Certified, 100)
    );
    assert_eq!(
        evaluate_cert_tier_and_score(&[], false, true, 1, 0, true).0,
        RuntimeCertLevel::L3Verified
    );
    assert_eq!(
        evaluate_cert_tier_and_score(&[], true, true, 0, 0, false).0,
        RuntimeCertLevel::L1Configured
    );
    assert_eq!(
        evaluate_cert_tier_and_score(&[], false, false, 0, 0, false).0,
        RuntimeCertLevel::L0Invalid
    );
}

fn polymorphic_supply_doc() -> SchemaDoc {
    SchemaDoc {
        node_types: vec![
            NodeTypeSpec {
                name: String::from("Hub"),
                display_name: String::from("枢纽"),
                properties: vec![metric_prop("output")],
                actions: vec![
                    ActionSpec {
                        name: String::from("tamper_poly_edge"),
                        display_name: String::from("多态边篡改"),
                        side_effect_tier: ActionSideEffectTier::RelatedNodes,
                        mutations: vec![MutationStepSpec {
                            target: String::from("edge(SUPPLY, OUT).flux"),
                            expression: String::from("42"),
                            filter: None,
                        }],
                        ..ActionSpec::default()
                    },
                    ActionSpec {
                        name: String::from("tamper_poly_neighbor"),
                        display_name: String::from("多态邻居篡改"),
                        side_effect_tier: ActionSideEffectTier::RelatedNodes,
                        mutations: vec![MutationStepSpec {
                            target: String::from("via(SUPPLY, OUT).inbound_stock"),
                            expression: String::from("99"),
                            filter: None,
                        }],
                        ..ActionSpec::default()
                    },
                    ActionSpec {
                        name: String::from("bad_perm_action"),
                        display_name: String::from("非法权限动作"),
                        required_permission: String::from("NOT_A_REAL_PERM"),
                        side_effect_tier: ActionSideEffectTier::SelfOnly,
                        mutations: vec![MutationStepSpec {
                            target: String::from("self.output"),
                            expression: String::from("10"),
                            filter: None,
                        }],
                        ..ActionSpec::default()
                    },
                ],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Store"),
                display_name: String::from("门店"),
                properties: vec![metric_prop("inbound_stock")],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Depot"),
                display_name: String::from("大仓"),
                properties: vec![metric_prop("inbound_stock")],
                derived_rules: vec![DerivedRuleSpec {
                    target_metric: String::from("inbound_stock"),
                    expression: String::from("100"),
                    enabled: true,
                    maturity: DesignMaturity::Confirmed,
                }],
                ..NodeTypeSpec::default()
            },
        ],
        edge_types: vec![
            EdgeTypeSpec {
                name: String::from("SUPPLY"),
                src_type_name: String::from("Hub"),
                dst_type_name: String::from("Store"),
                properties: vec![metric_prop("flux")],
                ..EdgeTypeSpec::default()
            },
            EdgeTypeSpec {
                name: String::from("SUPPLY"),
                src_type_name: String::from("Hub"),
                dst_type_name: String::from("Depot"),
                properties: vec![metric_prop("flux")],
                derived_rules: vec![EdgeDerivedRuleSpec {
                    target_metric: String::from("flux"),
                    expression: String::from("src.output * 0.5"),
                    path_kind: PropagationPathKind::VertexToEdge,
                    enabled: true,
                    maturity: DesignMaturity::Confirmed,
                }],
                ..EdgeTypeSpec::default()
            },
        ],
        mappings: Vec::new(),
    }
}

#[test]
fn redteam_polymorphic_edge_derived_write_and_permission_validation() {
    assert_eq!(
        resolve_action_permission("ACTION_EXECUTE"),
        Ok(Permission::FlowRun)
    );
    assert_eq!(
        resolve_action_permission("STUDIO_PUBLISH"),
        Ok(Permission::StudioPublish)
    );
    assert!(resolve_action_permission("UNKNOWN_PRIVILEGE").is_err());
    assert!(resolve_action_permission("").is_err());

    let doc = polymorphic_supply_doc();
    let issues = validate(&doc, &ValidationContext::default());
    let derived_errs = issues
        .iter()
        .filter(|i| {
            i.severity == IssueSeverity::Error && i.code == "DERIVED_METRIC_WRITE_FORBIDDEN"
        })
        .count();
    assert_eq!(
        derived_errs, 2,
        "expected both polymorphic edge and neighbor mutations rejected: {issues:?}"
    );
    assert!(
        issues
            .iter()
            .any(|i| i.severity == IssueSeverity::Error && i.code == "ACTION_PERMISSION_INVALID"),
        "expected ACTION_PERMISSION_INVALID: {issues:?}"
    );
}
