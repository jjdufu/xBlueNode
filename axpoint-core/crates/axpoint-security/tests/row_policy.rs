//! Golden vector and policy-resolution integration tests for `row_policy` loading
//! `tests/data/row_policy_cases.json` (`11-STEP13` §13.6.2, §13.6.3).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use axpoint_domain::AppError;
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, MaskRule, Permission, PiiCategory, Sensitivity};
use axpoint_domain::guard::{
    ColumnPolicyRecord, GrantRecord, GrantResourceKind, GrantStatus, GranteeKind,
    GuardPrincipalProfile, PolicyTargetKind, RowPolicyRecord, SubjectSelector, UserAccountStatus,
};
use axpoint_security::row_policy::{
    DatasetPolicyInput, NodeTypePolicyInput, compile_predicate, compile_predicate_with_ttl,
    resolve_dataset_policy, resolve_node_type_policy, validate_predicate_syntax,
    validate_predicate_syntax_for_target, validate_ttl_config,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct RowPolicyFixture {
    subject: FixtureSubject,
    dataset_columns: Vec<String>,
    node_properties: Vec<String>,
    valid_cases: Vec<ValidCase>,
    invalid_cases: Vec<InvalidCase>,
}

#[derive(Debug, Deserialize)]
struct FixtureSubject {
    user_id: Uuid,
    username: String,
    dept: String,
    attributes: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ValidCase {
    name: String,
    target_kind: PolicyTargetKind,
    predicate: String,
    expected_duckdb: Option<String>,
    expected_pg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InvalidCase {
    name: String,
    target_kind: PolicyTargetKind,
    predicate: String,
}

#[test]
fn golden_row_policy_cases_json_passes_100_percent() -> Result<(), AppError> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/data/row_policy_cases.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|err| AppError::Config(format!("failed reading {}: {err}", path.display())))?;
    let fixture: RowPolicyFixture = serde_json::from_str(&raw)
        .map_err(|err| AppError::Config(format!("invalid row_policy_cases.json: {err}")))?;

    let profile = GuardPrincipalProfile {
        user_id: fixture.subject.user_id,
        tenant_id: String::from("default"),
        username: fixture.subject.username,
        display_name: String::from("East Analyst"),
        email: String::new(),
        dept_id: fixture.subject.dept,
        clearance: Sensitivity::Confidential,
        status: UserAccountStatus::Active,
        tokens_valid_after: chrono::DateTime::UNIX_EPOCH,
        roles: vec![String::from("ANALYST")],
        permissions: BTreeSet::from([Permission::DataRead]),
        attributes: fixture.subject.attributes,
    };

    let ds_cols: BTreeSet<String> = fixture.dataset_columns.into_iter().collect();
    let node_props: BTreeSet<String> = fixture.node_properties.into_iter().collect();

    for vc in &fixture.valid_cases {
        let allowed = match vc.target_kind {
            PolicyTargetKind::Dataset => &ds_cols,
            PolicyTargetKind::NodeType => &node_props,
        };
        if vc.target_kind == PolicyTargetKind::Dataset {
            validate_predicate_syntax(&vc.predicate, Some(allowed))?;
        }
        let compiled = compile_predicate(&vc.predicate, vc.target_kind, Some(allowed), &profile)?;
        if let Some(ref exp_duck) = vc.expected_duckdb {
            assert_eq!(
                &compiled.duckdb_sql, exp_duck,
                "DuckDB SQL mismatch on valid case '{}'",
                vc.name
            );
        }
        if let Some(ref exp_pg) = vc.expected_pg {
            assert_eq!(
                &compiled.pg_sql, exp_pg,
                "PostgreSQL SQL mismatch on valid case '{}'",
                vc.name
            );
        }
    }

    for ic in &fixture.invalid_cases {
        let allowed = match ic.target_kind {
            PolicyTargetKind::Dataset => &ds_cols,
            PolicyTargetKind::NodeType => &node_props,
        };
        let res = compile_predicate(&ic.predicate, ic.target_kind, Some(allowed), &profile);
        assert!(
            matches!(
                res,
                Err(AppError::BadRequest {
                    code: "INVALID_PREDICATE",
                    ..
                })
            ),
            "expected INVALID_PREDICATE for case '{}', got {res:?}",
            ic.name
        );
    }

    Ok(())
}

fn make_south_analyst() -> GuardPrincipalProfile {
    GuardPrincipalProfile {
        user_id: Uuid::new_v4(),
        tenant_id: String::from("default"),
        username: String::from("analyst_south"),
        display_name: String::from("South Analyst"),
        email: String::new(),
        dept_id: String::from("south"),
        clearance: Sensitivity::Confidential,
        status: UserAccountStatus::Active,
        tokens_valid_after: chrono::DateTime::UNIX_EPOCH,
        roles: vec![String::from("ANALYST")],
        permissions: BTreeSet::from([Permission::DataRead]),
        attributes: BTreeMap::new(),
    }
}

fn demo_dataset_columns(ds_id: Uuid) -> Vec<ColumnMeta> {
    vec![
        ColumnMeta {
            dataset_id: ds_id,
            column_name: String::from("warehouse_code"),
            data_type: DataType::String,
            pii_category: PiiCategory::None,
            pii_confirmed: false,
            description: String::new(),
            business_term: String::new(),
            unit: String::new(),
            ordinal: 0,
        },
        ColumnMeta {
            dataset_id: ds_id,
            column_name: String::from("phone"),
            data_type: DataType::String,
            pii_category: PiiCategory::Phone,
            pii_confirmed: true,
            description: String::new(),
            business_term: String::new(),
            unit: String::new(),
            ordinal: 1,
        },
    ]
}

#[test]
fn dataset_policy_resolver_handles_grants_or_union_and_reveal_permission() -> Result<(), AppError> {
    let ds_id = Uuid::new_v4();
    let cols = demo_dataset_columns(ds_id);
    let south_profile = make_south_analyst();
    let south_lineage = vec![String::from("south")];

    let east_rp = RowPolicyRecord {
        id: Uuid::new_v4(),
        tenant_id: String::from("default"),
        name: String::from("east_warehouses"),
        target_kind: PolicyTargetKind::Dataset,
        target_ref: ds_id.to_string(),
        applies_to: SubjectSelector {
            roles: vec![String::from("ANALYST")],
            depts: Vec::new(),
            users: Vec::new(),
        },
        predicate: String::from("warehouse_code IN {{user.attr.warehouses}}"),
        enabled: true,
        policy_mode: axpoint_domain::enums::RowPolicyMode::StaticAttr,
        ttl_timestamp_field: None,
        ttl_max_age_hours: None,
        created_at: Utc::now(),
    };

    let south_grant = GrantRecord {
        id: Uuid::new_v4(),
        tenant_id: String::from("default"),
        resource_kind: GrantResourceKind::Dataset,
        resource_ref: ds_id.to_string(),
        grantee_kind: GranteeKind::Dept,
        grantee_ref: String::from("south"),
        actions: vec![Permission::DataRead],
        row_predicate: Some(String::from("warehouse_code = 'W-2'")),
        reason: String::from("cross-dept approval"),
        status: GrantStatus::Active,
        approval_id: None,
        requested_by: south_profile.user_id,
        expires_at: Utc::now() + Duration::days(1),
        created_at: Utc::now(),
        decided_at: Some(Utc::now()),
    };

    let (spec, exempt_hit) = resolve_dataset_policy(&DatasetPolicyInput {
        profile: &south_profile,
        dept_lineage: &south_lineage,
        dataset_id: ds_id,
        dataset_name: "demo_orders",
        dataset_columns: &cols,
        row_policies: std::slice::from_ref(&east_rp),
        default_deny: false,
        column_policies: &[],
        active_grants: &[south_grant],
        reveal_pii: false,
    })?;
    assert!(!exempt_hit);
    assert_eq!(
        spec.duckdb_row_filter.as_deref(),
        Some("((FALSE) OR (\"warehouse_code\" = 'W-2'))")
    );
    assert_eq!(spec.column_masks.get("phone"), Some(&MaskRule::MaskPhone));

    let denied = resolve_dataset_policy(&DatasetPolicyInput {
        profile: &south_profile,
        dept_lineage: &south_lineage,
        dataset_id: ds_id,
        dataset_name: "demo_orders",
        dataset_columns: &cols,
        row_policies: &[],
        default_deny: false,
        column_policies: &[],
        active_grants: &[],
        reveal_pii: true,
    });
    assert!(matches!(
        denied,
        Err(AppError::Forbidden("PERMISSION_DENIED"))
    ));
    Ok(())
}

#[test]
fn dataset_and_node_resolvers_handle_default_deny_admin_bypass_and_exemptions()
-> Result<(), AppError> {
    let ds_id = Uuid::new_v4();
    let cols = demo_dataset_columns(ds_id);
    let south_profile = make_south_analyst();
    let south_lineage = vec![String::from("south")];

    let (deny_spec, _) = resolve_dataset_policy(&DatasetPolicyInput {
        profile: &south_profile,
        dept_lineage: &south_lineage,
        dataset_id: ds_id,
        dataset_name: "demo_orders",
        dataset_columns: &cols,
        row_policies: &[],
        default_deny: true,
        column_policies: &[],
        active_grants: &[],
        reveal_pii: false,
    })?;
    assert_eq!(deny_spec.duckdb_row_filter.as_deref(), Some("FALSE"));

    let mut admin_profile = south_profile.clone();
    admin_profile.permissions.insert(Permission::DataAdmin);
    let (admin_spec, _) = resolve_dataset_policy(&DatasetPolicyInput {
        profile: &admin_profile,
        dept_lineage: &south_lineage,
        dataset_id: ds_id,
        dataset_name: "demo_orders",
        dataset_columns: &cols,
        row_policies: &[],
        default_deny: true,
        column_policies: &[],
        active_grants: &[],
        reveal_pii: false,
    })?;
    assert!(admin_spec.duckdb_row_filter.is_none());

    let node_cp = ColumnPolicyRecord {
        id: Uuid::new_v4(),
        tenant_id: String::from("default"),
        target_kind: PolicyTargetKind::NodeType,
        target_ref: String::from("Customer"),
        column_name: String::from("mobile"),
        mask_rule: MaskRule::MaskPhone,
        exempt: SubjectSelector {
            roles: Vec::new(),
            depts: vec![String::from("south")],
            users: Vec::new(),
        },
    };
    let node_props = vec![(String::from("mobile"), PiiCategory::Phone)];
    let (nt_spec, nt_exempt) = resolve_node_type_policy(&NodeTypePolicyInput {
        profile: &south_profile,
        dept_lineage: &south_lineage,
        type_name: "Customer",
        properties: &node_props,
        row_policies: &[],
        default_deny: false,
        column_policies: &[node_cp],
        active_grants: &[],
        reveal_pii: true,
    })?;
    assert!(nt_exempt);
    assert!(nt_spec.property_masks.is_empty());
    Ok(())
}

#[test]
fn step27_state_predicates_roles_double_eq_and_ttl_compile_dual_dialect() -> Result<(), AppError> {
    let mut profile = make_south_analyst();
    profile.dept_id = String::from("risk_obs");
    profile.roles = vec![String::from("ANALYST"), String::from("INCIDENT_CMD")];

    let allowed = BTreeSet::from([
        String::from("region"),
        String::from("inventory"),
        String::from("severity"),
        String::from("opened_at"),
    ]);

    let pred = "properties.region == {{user.dept}} OR (metrics.inventory < 100 AND \"INCIDENT_CMD\" IN {{user.roles}})";
    validate_predicate_syntax_for_target(pred, PolicyTargetKind::NodeType, Some(&allowed))?;
    validate_ttl_config(
        PolicyTargetKind::NodeType,
        Some("updated_at"),
        Some(72),
        Some(&allowed),
    )?;

    let compiled = compile_predicate_with_ttl(
        pred,
        PolicyTargetKind::NodeType,
        Some(&allowed),
        &profile,
        Some("updated_at"),
        Some(72),
    )?;
    assert!(
        compiled
            .duckdb_sql
            .contains("json_extract_string(properties, '$.region') = 'risk_obs'")
    );
    assert!(
        compiled
            .duckdb_sql
            .contains("TRY_CAST(json_extract_string(metrics, '$.inventory') AS DOUBLE) < 100")
    );
    assert!(
        compiled
            .duckdb_sql
            .contains("'INCIDENT_CMD' IN ('ANALYST', 'INCIDENT_CMD')")
    );
    assert!(
        compiled
            .duckdb_sql
            .contains("\"updated_at\" >= (CURRENT_TIMESTAMP - INTERVAL '72 hours')")
    );

    assert!(
        compiled
            .pg_sql
            .contains("(properties->>'region') = 'risk_obs'")
    );
    assert!(
        compiled
            .pg_sql
            .contains("THEN (metrics->>'inventory')::double precision END) < 100")
    );
    assert!(
        compiled
            .pg_sql
            .contains("updated_at >= (NOW() - INTERVAL '72 hours')")
    );
    Ok(())
}

#[test]
fn step27_safe_pg_casts_for_custom_property_ttl() -> Result<(), AppError> {
    let profile = make_south_analyst();
    let allowed = BTreeSet::from([String::from("inventory"), String::from("opened_at")]);
    let prop_ttl = compile_predicate_with_ttl(
        "metrics.inventory < 100",
        PolicyTargetKind::NodeType,
        Some(&allowed),
        &profile,
        Some("opened_at"),
        Some(24),
    )?;
    assert!(
        prop_ttl
            .pg_sql
            .contains("(0[13578]|1[02])-(0[1-9]|[12][0-9]|3[01])")
    );
    assert!(
        prop_ttl
            .pg_sql
            .contains("THEN (properties->>'opened_at')::timestamptz END")
    );

    let alt_pred = "node.prop.opened_at == '2026-01-01'";
    let alt = compile_predicate(
        alt_pred,
        PolicyTargetKind::NodeType,
        Some(&allowed),
        &profile,
    )?;
    assert!(
        alt.pg_sql
            .contains("(properties->>'opened_at') = '2026-01-01'")
    );

    assert!(
        validate_predicate_syntax_for_target(
            "properties.unknown_col == 'x'",
            PolicyTargetKind::NodeType,
            Some(&allowed),
        )
        .is_err()
    );
    assert!(
        validate_ttl_config(
            PolicyTargetKind::NodeType,
            Some("updated_at"),
            Some(0),
            Some(&allowed),
        )
        .is_err()
    );
    assert!(
        validate_predicate_syntax_for_target(
            "properties.region == 'east'; DROP TABLE xb_nodes",
            PolicyTargetKind::NodeType,
            Some(&allowed),
        )
        .is_err()
    );
    Ok(())
}
