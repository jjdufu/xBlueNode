//! Golden masking vector tests (`tests/data/mask_cases.json`), `audit_chain` tamper-detection
//! tests, and `pii_local` outbound egress redaction/restoration round-trip tests
//! (`11-STEP13` §13.7–§13.9).

use std::collections::BTreeMap;
use std::path::Path;

use axpoint_domain::AppError;
use axpoint_domain::enums::{AuditAction, MaskRule, PiiCategory};
use axpoint_domain::guard::{AuditLogEntry, PiiSpan};
use axpoint_security::audit_chain::{
    AuditHashFields, compute_audit_row_hash, truncate_to_micros, verify_audit_chain,
};
use axpoint_security::masking::{
    MaskKeyRing, apply_mask_rule, mask_json_properties, mask_text_spans,
};
use axpoint_security::pii_local::preview_llm_egress;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct MaskFixture {
    meta: MaskFixtureMeta,
    cases: Vec<MaskCase>,
}

#[derive(Debug, Deserialize)]
struct MaskFixtureMeta {
    mask_keys_env: String,
    tenant_id: String,
}

#[derive(Debug, Deserialize)]
struct MaskCase {
    rule: MaskRule,
    input: Option<String>,
    expected: Option<String>,
}

#[test]
fn golden_mask_cases_json_all_82_vectors_pass_100_percent() -> Result<(), AppError> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/data/mask_cases.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|err| AppError::Config(format!("failed reading {}: {err}", path.display())))?;
    let fixture: MaskFixture = serde_json::from_str(&raw)
        .map_err(|err| AppError::Config(format!("invalid mask_cases.json: {err}")))?;

    let keyring = MaskKeyRing::from_env_str(&fixture.meta.mask_keys_env)?;
    let tenant_key = keyring.tenant_key(&fixture.meta.tenant_id);

    assert_eq!(fixture.cases.len(), 82);
    for (idx, case) in fixture.cases.iter().enumerate() {
        let actual = case
            .input
            .as_deref()
            .map(|val| apply_mask_rule(case.rule, val, &tenant_key));
        assert_eq!(
            actual, case.expected,
            "mask_cases.json vector #{idx} failed for rule={:?}, input={:?}",
            case.rule, case.input
        );
    }
    Ok(())
}

#[test]
fn json_properties_and_text_spans_masking_work_on_unicode_offsets() {
    let tenant_key = MaskKeyRing::dev_default().tenant_key("default");
    let mut props = serde_json::Map::new();
    props.insert(String::from("phone"), json!("13812345678"));
    props.insert(String::from("null_field"), serde_json::Value::Null);
    props.insert(String::from("city"), json!("Shanghai"));

    let masks = BTreeMap::from([(String::from("phone"), MaskRule::MaskPhone)]);
    mask_json_properties(&mut props, &masks, &tenant_key);
    assert_eq!(props.get("phone"), Some(&json!("138****5678")));
    assert_eq!(props.get("null_field"), Some(&serde_json::Value::Null));
    assert_eq!(props.get("city"), Some(&json!("Shanghai")));

    let para = "联系人：欧阳娜娜，手机 13812345678。";
    let spans = vec![
        PiiSpan {
            category: PiiCategory::Name,
            start: 4,
            end: 8,
            score: 0.9,
        },
        PiiSpan {
            category: PiiCategory::Phone,
            start: 12,
            end: 23,
            score: 0.95,
        },
    ];
    let masked_para = mask_text_spans(para, &spans, &BTreeMap::new(), &tenant_key);
    assert_eq!(masked_para, "联系人：欧阳**，手机 138****5678。");
}

#[test]
fn audit_hash_chain_detects_tampering_and_verifies_restoration() {
    let base_ts = truncate_to_micros(Utc::now());
    let mut entries = Vec::new();
    let mut prev = String::new();

    for seq in 1..=4_i64 {
        let created_at = base_ts + Duration::seconds(seq);
        let detail = json!({"b": seq, "a": "ok"});
        let username = format!("user_{seq}");
        let resource_ref = format!("dataset:{seq}");
        let row_hash = compute_audit_row_hash(
            &prev,
            &AuditHashFields {
                id: seq,
                tenant_id: "default",
                user_id: Some(Uuid::from_u128(u128::try_from(seq).unwrap_or(0))),
                username: &username,
                action: AuditAction::QuerySql.as_str(),
                resource_ref: &resource_ref,
                status_code: 200,
                client_ip: "127.0.0.1",
                trace_id: "trace-xyz",
                detail_json: &detail,
                created_at,
            },
        );
        entries.push(AuditLogEntry {
            id: seq,
            tenant_id: String::from("default"),
            user_id: Some(Uuid::from_u128(u128::try_from(seq).unwrap_or(0))),
            username,
            action: AuditAction::QuerySql,
            resource_ref,
            status_code: 200,
            client_ip: String::from("127.0.0.1"),
            trace_id: String::from("trace-xyz"),
            detail_json: json!({"a": "ok", "b": seq}), // Reordered keys still canonicalize identically!
            prev_hash: prev.clone(),
            row_hash: row_hash.clone(),
            created_at,
        });
        prev = row_hash;
    }

    let ok_res = verify_audit_chain("", &entries);
    assert!(ok_res.ok);
    assert_eq!(ok_res.checked_count, 4);
    assert_eq!(ok_res.first_broken_id, None);

    // Tamper row #3
    if let Some(target) = entries.get_mut(2) {
        target.username = String::from("tampered");
    }
    let broken = verify_audit_chain("", &entries);
    assert!(!broken.ok);
    assert_eq!(broken.first_broken_id, Some(3));

    // Restore row #3
    if let Some(target) = entries.get_mut(2) {
        target.username = String::from("user_3");
    }
    let restored = verify_audit_chain("", &entries);
    assert!(restored.ok);
    assert_eq!(restored.first_broken_id, None);
}

#[test]
fn pii_local_egress_preview_strips_pii_and_restores_or_masks() {
    let tenant_key = MaskKeyRing::dev_default().tenant_key("default");
    let sample = "客户电话 13812345678，身份证 110101199003077516，邮箱 zhangsan@example.com";
    let preview = preview_llm_egress(sample, &tenant_key);

    assert!(!preview.outbound.contains("13812345678"));
    assert!(!preview.outbound.contains("110101199003077516"));
    assert!(!preview.outbound.contains("zhangsan@example.com"));
    assert!(preview.outbound.contains("⟨PHONE_1⟩"));
    assert!(preview.outbound.contains("⟨ID_CARD_1⟩"));
    assert!(preview.outbound.contains("⟨EMAIL_1⟩"));

    assert_eq!(preview.restored, sample);
    assert!(preview.masked.contains("138****5678"));
    assert!(preview.masked.contains("110101********7516"));
    assert!(preview.masked.contains("z*******@example.com"));
}
