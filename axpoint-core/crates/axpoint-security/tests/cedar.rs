//! Comprehensive `Cedar` `ABAC` matrix integration tests (`>= 40` authorization cases covering
//! sensitivity lattice, owner/parent department hierarchy, cycle safety, cross-department grants,
//! `DATA_ADMIN` / `GUARD_ADMIN` `SECRET` bypass, custom `forbid` overrides, and `SCHEMA_INVALID`
//! validation diagnostics, `11-STEP13` §13.6.1).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::enums::{Permission, Sensitivity};
use axpoint_domain::guard::{
    CedarPolicyRecord, DepartmentRecord, GrantResourceKind, GuardPrincipalProfile, ResourceRef,
    UserAccountStatus,
};
use axpoint_security::cedar_authz::{CedarAuthorizer, CedarContext, dept_lineage};
use chrono::Utc;
use uuid::Uuid;

fn org_departments() -> Vec<DepartmentRecord> {
    vec![
        DepartmentRecord {
            tenant_id: String::from("default"),
            dept_id: String::from("hq"),
            name: String::from("总部"),
            parent_id: None,
        },
        DepartmentRecord {
            tenant_id: String::from("default"),
            dept_id: String::from("east"),
            name: String::from("华东"),
            parent_id: Some(String::from("hq")),
        },
        DepartmentRecord {
            tenant_id: String::from("default"),
            dept_id: String::from("east_ops"),
            name: String::from("华东运营"),
            parent_id: Some(String::from("east")),
        },
        DepartmentRecord {
            tenant_id: String::from("default"),
            dept_id: String::from("south"),
            name: String::from("华南"),
            parent_id: Some(String::from("hq")),
        },
    ]
}

fn make_profile(
    dept_id: &str,
    clearance: Sensitivity,
    perms: &[Permission],
) -> GuardPrincipalProfile {
    GuardPrincipalProfile {
        user_id: Uuid::new_v4(),
        tenant_id: String::from("default"),
        username: format!("u_{dept_id}"),
        display_name: String::from("Test User"),
        email: String::new(),
        dept_id: String::from(dept_id),
        clearance,
        status: UserAccountStatus::Active,
        tokens_valid_after: chrono::DateTime::UNIX_EPOCH,
        roles: vec![String::from("ANALYST")],
        permissions: perms.iter().copied().collect::<BTreeSet<_>>(),
        attributes: BTreeMap::new(),
    }
}

fn make_resource(owner_dept: &str, sensitivity: Sensitivity) -> ResourceRef {
    ResourceRef {
        kind: GrantResourceKind::Dataset,
        id: String::from("demo_orders"),
        owner_dept: String::from(owner_dept),
        sensitivity,
    }
}

fn make_custom_policy(name: &str, text: &str, enabled: bool) -> CedarPolicyRecord {
    CedarPolicyRecord {
        id: Uuid::new_v4(),
        tenant_id: String::from("default"),
        name: String::from(name),
        description: String::new(),
        policy_text: String::from(text),
        enabled,
        version: 1,
        updated_by: None,
        updated_at: Utc::now(),
    }
}

#[test]
fn cedar_sensitivity_lattice_same_and_cross_dept_32_combinations() -> Result<(), AppError> {
    let authz = CedarAuthorizer::new()?;
    let depts = org_departments();
    let levels = [
        Sensitivity::Public,
        Sensitivity::Internal,
        Sensitivity::Confidential,
        Sensitivity::Secret,
    ];

    let mut executed_cases = 0_usize;

    // 16 cases: Same department (`east` -> `east`) across all 4x4 clearance x resource sensitivity pairs.
    for &user_clr in &levels {
        for &res_sens in &levels {
            let user = make_profile("east", user_clr, &[Permission::DataRead]);
            let res = make_resource("east", res_sens);
            let sim = authz.evaluate(
                &user,
                Permission::DataRead,
                &res,
                &CedarContext::default(),
                &depts,
                &[],
            );
            // Rule 2 allows sensitivity <= INTERNAL (1) regardless of clearance; Rule 1 requires clearance >= sensitivity.
            let expected = res_sens.level() <= 1 || user_clr.level() >= res_sens.level();
            assert_eq!(
                sim.allowed, expected,
                "same-dept mismatch for user_clr={user_clr:?}, res_sens={res_sens:?}"
            );
            executed_cases = executed_cases.saturating_add(1);
        }
    }

    // 16 cases: Cross-department (`south` -> `east`) without active grant across all 4x4 pairs.
    for &user_clr in &levels {
        for &res_sens in &levels {
            let user = make_profile("south", user_clr, &[Permission::DataRead]);
            let res = make_resource("east", res_sens);
            let sim = authz.evaluate(
                &user,
                Permission::DataRead,
                &res,
                &CedarContext::default(),
                &depts,
                &[],
            );
            // Without grant, cross-dept only allows PUBLIC (0) and INTERNAL (1).
            let expected = res_sens.level() <= 1;
            assert_eq!(
                sim.allowed, expected,
                "cross-dept ungranted mismatch for user_clr={user_clr:?}, res_sens={res_sens:?}"
            );
            executed_cases = executed_cases.saturating_add(1);
        }
    }

    assert_eq!(executed_cases, 32);
    Ok(())
}

#[test]
fn cedar_department_hierarchy_grants_and_admin_bypass() -> Result<(), AppError> {
    let authz = CedarAuthorizer::new()?;
    let depts = org_departments();

    let sub_user = make_profile(
        "east_ops",
        Sensitivity::Confidential,
        &[Permission::DataRead],
    );
    let east_conf = make_resource("east", Sensitivity::Confidential);
    authz.authorize(
        &sub_user,
        Permission::DataRead,
        &east_conf,
        &CedarContext::default(),
        &depts,
        &[],
    )?;

    let sub_secret_user = make_profile("east_ops", Sensitivity::Secret, &[Permission::DataRead]);
    let hq_secret = make_resource("hq", Sensitivity::Secret);
    authz.authorize(
        &sub_secret_user,
        Permission::DataRead,
        &hq_secret,
        &CedarContext::default(),
        &depts,
        &[],
    )?;

    let hq_user = make_profile("hq", Sensitivity::Confidential, &[Permission::DataRead]);
    let child_conf = make_resource("east_ops", Sensitivity::Confidential);
    assert!(matches!(
        authz.authorize(
            &hq_user,
            Permission::DataRead,
            &child_conf,
            &CedarContext::default(),
            &depts,
            &[],
        ),
        Err(AppError::Forbidden("POLICY_DENIED"))
    ));

    let south_user = make_profile("south", Sensitivity::Internal, &[Permission::DataRead]);
    let granted_ctx = CedarContext {
        granted: true,
        ..CedarContext::default()
    };
    authz.authorize(
        &south_user,
        Permission::DataRead,
        &east_conf,
        &granted_ctx,
        &depts,
        &[],
    )?;
    assert!(matches!(
        authz.authorize(
            &south_user,
            Permission::ExportData,
            &east_conf,
            &granted_ctx,
            &depts,
            &[],
        ),
        Err(AppError::Forbidden("POLICY_DENIED"))
    ));

    let guard_admin = make_profile(
        "south",
        Sensitivity::Internal,
        &[Permission::GuardAdmin, Permission::DataRead],
    );
    let east_secret = make_resource("east", Sensitivity::Secret);
    authz.authorize(
        &guard_admin,
        Permission::GuardAdmin,
        &east_secret,
        &CedarContext::default(),
        &depts,
        &[],
    )?;
    authz.authorize(
        &guard_admin,
        Permission::DataRead,
        &east_secret,
        &CedarContext::default(),
        &depts,
        &[],
    )?;
    Ok(())
}

#[test]
fn cedar_custom_forbid_and_disabled_policy() -> Result<(), AppError> {
    let authz = CedarAuthorizer::new()?;
    let depts = org_departments();
    let east_conf = make_resource("east", Sensitivity::Confidential);
    let east_secret = make_resource("east", Sensitivity::Secret);

    let data_admin = make_profile(
        "south",
        Sensitivity::Internal,
        &[Permission::DataAdmin, Permission::DataRead],
    );
    authz.authorize(
        &data_admin,
        Permission::DataRead,
        &east_secret,
        &CedarContext::default(),
        &depts,
        &[],
    )?;

    let custom_forbid = make_custom_policy(
        "no_night_export",
        r#"forbid (principal, action == AxPoint::Action::"EXPORT_DATA", resource) when { context.hour < 8 || context.hour >= 20 };"#,
        true,
    );
    authz.ensure_valid_policy_text(&custom_forbid.policy_text)?;

    let exporter = make_profile(
        "east",
        Sensitivity::Secret,
        &[Permission::DataRead, Permission::ExportData],
    );
    let day_ctx = CedarContext {
        hour: 14,
        ..CedarContext::default()
    };
    let night_ctx = CedarContext {
        hour: 22,
        ..CedarContext::default()
    };
    authz.authorize(
        &exporter,
        Permission::ExportData,
        &east_conf,
        &day_ctx,
        &depts,
        std::slice::from_ref(&custom_forbid),
    )?;
    let night_sim = authz.evaluate(
        &exporter,
        Permission::ExportData,
        &east_conf,
        &night_ctx,
        &depts,
        std::slice::from_ref(&custom_forbid),
    );
    assert!(!night_sim.allowed);
    assert!(
        night_sim
            .determining_policies
            .iter()
            .any(|id| id == "custom:no_night_export")
    );

    let mut disabled_forbid = custom_forbid;
    disabled_forbid.enabled = false;
    authz.authorize(
        &exporter,
        Permission::ExportData,
        &east_conf,
        &night_ctx,
        &depts,
        &[disabled_forbid],
    )?;
    Ok(())
}

#[test]
fn cedar_schema_validation_and_cycle_safety() -> Result<(), AppError> {
    let authz = CedarAuthorizer::new()?;
    assert!(matches!(
        authz.ensure_valid_policy_text(
            r#"forbid (principal, action, resource) when { principal.nonexistent == "x" };"#,
        ),
        Err(AppError::BadRequest {
            code: "SCHEMA_INVALID",
            ..
        })
    ));
    assert!(matches!(
        authz.ensure_valid_policy_text("permit (principal, broken syntax"),
        Err(AppError::BadRequest {
            code: "SCHEMA_INVALID",
            ..
        })
    ));

    let cyclic = vec![
        DepartmentRecord {
            tenant_id: String::from("default"),
            dept_id: String::from("d1"),
            name: String::from("D1"),
            parent_id: Some(String::from("d2")),
        },
        DepartmentRecord {
            tenant_id: String::from("default"),
            dept_id: String::from("d2"),
            name: String::from("D2"),
            parent_id: Some(String::from("d1")),
        },
    ];
    assert_eq!(
        dept_lineage("d1", &cyclic),
        vec![String::from("d1"), String::from("d2")]
    );
    let cyc_user = make_profile("d1", Sensitivity::Confidential, &[Permission::DataRead]);
    let cyc_res = make_resource("d2", Sensitivity::Confidential);
    authz.authorize(
        &cyc_user,
        Permission::DataRead,
        &cyc_res,
        &CedarContext::default(),
        &cyclic,
        &[],
    )?;
    authz.invalidate_tenant("default");
    Ok(())
}
