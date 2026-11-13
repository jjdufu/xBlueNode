//! Integration tests for `axpoint-security::keyring::CredentialKeyring`.

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_security::CredentialKeyring;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use uuid::Uuid;

#[test]
fn seal_and_open_roundtrip_with_distinct_nonces() -> Result<(), AppError> {
    let key_v1 = [0x42_u8; 32];
    let keyring = CredentialKeyring::from_keys(BTreeMap::from([(1_i16, key_v1)]))?;
    assert_eq!(keyring.active_version(), 1);
    assert_eq!(keyring.active_version_i32(), 1);

    let conn_id = Uuid::new_v4();
    let aad = CredentialKeyring::aad_for_connection("tenant_alpha", conn_id);
    let secret_json = br#"{"password":"axpoint_super_secret_pw","ssl_key":"PEM_DATA"}"#;

    let (ver_a, nonce_a, ct_a) = keyring.seal(&aad, secret_json)?;
    let (ver_b, nonce_b, ct_b) = keyring.seal(&aad, secret_json)?;

    assert_eq!(ver_a, 1);
    assert_eq!(ver_b, 1);
    assert_eq!(nonce_a.len(), 12);
    assert_eq!(nonce_b.len(), 12);
    assert_ne!(
        nonce_a, nonce_b,
        "CSPRNG must produce distinct 12-byte nonces"
    );
    assert_ne!(ct_a, ct_b);
    assert!(!String::from_utf8_lossy(&ct_a).contains("axpoint_super_secret_pw"));

    let opened_a = keyring.open(ver_a, &nonce_a, &aad, &ct_a)?;
    let opened_b = keyring.open(ver_b, &nonce_b, &aad, &ct_b)?;
    assert_eq!(opened_a.as_slice(), secret_json);
    assert_eq!(opened_b.as_slice(), secret_json);

    // Debug output must never leak key material.
    let debug_repr = format!("{keyring:?}");
    assert!(debug_repr.contains("[REDACTED]"));
    assert!(!debug_repr.contains("66"));

    Ok(())
}

#[test]
fn multi_version_rotation_decrypts_old_and_seals_with_highest() -> Result<(), AppError> {
    let key_v1 = [0x11_u8; 32];
    let key_v2 = [0x22_u8; 32];

    let keyring_v1 = CredentialKeyring::from_keys(BTreeMap::from([(1_i16, key_v1)]))?;
    let webhook_id = Uuid::new_v4();
    let aad = CredentialKeyring::aad_for_webhook("default", webhook_id);
    let payload = br#"{"secret":"whsec_old_secret"}"#;

    let (v1_ver, v1_nonce, v1_ct) = keyring_v1.seal(&aad, payload)?;
    assert_eq!(v1_ver, 1);

    let env_raw = format!(
        "1:{}, 2:{}",
        STANDARD.encode(key_v1),
        STANDARD.encode(key_v2)
    );
    let keyring_v2 = CredentialKeyring::from_env_str(&env_raw)?;
    assert_eq!(keyring_v2.active_version(), 2);

    // Ciphertext sealed under v1 remains readable when v1+v2 are loaded.
    let opened_old = keyring_v2.open(v1_ver, &v1_nonce, &aad, &v1_ct)?;
    assert_eq!(opened_old.as_slice(), payload);

    // Re-sealing uses active version 2.
    let rotated = keyring_v2.seal_credential(webhook_id, &aad, opened_old.as_slice())?;
    assert_eq!(rotated.key_version, 2);
    let opened_rotated = keyring_v2.open_credential(&aad, &rotated)?;
    assert_eq!(opened_rotated.as_slice(), payload);

    // Keyring with only v2 rejects v1 ciphertext with CREDENTIAL_DECRYPT_FAILED.
    let keyring_only_v2 = CredentialKeyring::from_keys(BTreeMap::from([(2_i16, key_v2)]))?;
    let missing_ver_err = keyring_only_v2.open(v1_ver, &v1_nonce, &aad, &v1_ct);
    assert!(matches!(
        missing_ver_err,
        Err(AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
    ));

    Ok(())
}

#[test]
fn tampered_ciphertext_nonce_or_cross_tenant_aad_rejected() -> Result<(), AppError> {
    let keyring = CredentialKeyring::from_keys(BTreeMap::from([(1_i16, [0x7A_u8; 32])]))?;
    let resource_id = Uuid::new_v4();
    let aad_t1 = CredentialKeyring::aad_for_connection("tenant_one", resource_id);
    let aad_t2 = CredentialKeyring::aad_for_connection("tenant_two", resource_id);
    let aad_other_res = CredentialKeyring::aad_for_connection("tenant_one", Uuid::new_v4());

    let (ver, nonce, ciphertext) = keyring.seal(&aad_t1, b"top-secret")?;

    // 1. Tamper 1 byte of ciphertext.
    let mut bad_ct = ciphertext.clone();
    if let Some(first_byte) = bad_ct.first_mut() {
        *first_byte ^= 0x01;
    }
    assert!(matches!(
        keyring.open(ver, &nonce, &aad_t1, &bad_ct),
        Err(AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
    ));

    // 2. Tamper 1 byte of nonce.
    let mut bad_nonce = nonce.clone();
    if let Some(first_byte) = bad_nonce.first_mut() {
        *first_byte ^= 0x80;
    }
    assert!(matches!(
        keyring.open(ver, &bad_nonce, &aad_t1, &ciphertext),
        Err(AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
    ));

    // 3. Invalid nonce length.
    assert!(matches!(
        keyring.open(ver, &[0_u8; 8], &aad_t1, &ciphertext),
        Err(AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
    ));

    // 4. Cross-tenant AAD substitution fails authentication.
    assert!(matches!(
        keyring.open(ver, &nonce, &aad_t2, &ciphertext),
        Err(AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
    ));

    // 5. Cross-resource AAD substitution within same tenant fails authentication.
    assert!(matches!(
        keyring.open(ver, &nonce, &aad_other_res, &ciphertext),
        Err(AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
    ));

    Ok(())
}

#[test]
fn invalid_keyring_configurations_return_config_error() -> Result<(), AppError> {
    // Empty map / empty string.
    assert!(matches!(
        CredentialKeyring::from_keys(BTreeMap::new()),
        Err(AppError::Config(_))
    ));
    assert!(matches!(
        CredentialKeyring::from_env_str("   "),
        Err(AppError::Config(_))
    ));

    // Invalid base64.
    assert!(matches!(
        CredentialKeyring::from_env_str("1:not_valid_base64!!!"),
        Err(AppError::Config(_))
    ));

    // Wrong decoded length (16 bytes instead of 32 bytes).
    let short_b64 = STANDARD.encode([0xAA_u8; 16]);
    assert!(matches!(
        CredentialKeyring::from_env_str(&format!("1:{short_b64}")),
        Err(AppError::Config(_))
    ));

    // Duplicate version number.
    let valid_b64 = STANDARD.encode([0xBB_u8; 32]);
    assert!(matches!(
        CredentialKeyring::from_env_str(&format!("1:{valid_b64},1:{valid_b64}")),
        Err(AppError::Config(_))
    ));

    // Non-positive version number.
    assert!(matches!(
        CredentialKeyring::from_env_str(&format!("0:{valid_b64}")),
        Err(AppError::Config(_))
    ));
    assert!(matches!(
        CredentialKeyring::from_env_str(&format!("-1:{valid_b64}")),
        Err(AppError::Config(_))
    ));

    // Production requires explicit keys; development falls back to ephemeral key.
    assert!(matches!(
        CredentialKeyring::from_env_with_mode("production", None),
        Err(AppError::Config(_))
    ));
    let dev_ephemeral = CredentialKeyring::from_env_with_mode("test", None)?;
    assert_eq!(dev_ephemeral.active_version(), 1);

    Ok(())
}
