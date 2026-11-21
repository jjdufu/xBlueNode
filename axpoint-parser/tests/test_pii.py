from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import polars as pl
import pytest
import spacy
from fastapi.testclient import TestClient

from main import app
from parsers.profiler import detect_pii_category, infer_and_convert_column, profile_polars_df
from pii import (
    MASK_KEYS_ENV_VAR,
    OfflineSpacyNlpEngine,
    PiiEngine,
    analyze_texts,
    anonymize_texts,
    apply_mask_rule,
    is_valid_gb11643_id_card,
    is_valid_luhn_bank_card,
)
from schemas import PiiAnalyzeRequest, PiiAnonymizeRequest
from security import SidecarError

TEST_INTERNAL_TOKEN = "axp-step13-internal-token-0123456789abcdef"
MASK_CASES_PATH = Path(__file__).resolve().parents[2] / "tests" / "data" / "mask_cases.json"


def _make_gb11643_id(prefix17: str, *, lower_x: bool = False) -> str:
    weights = (7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2)
    chars = "10X98765432"
    chk = chars[sum(int(prefix17[i]) * weights[i] for i in range(17)) % 11]
    if lower_x and chk == "X":
        chk = "x"
    return prefix17 + chk


def _make_luhn_card(prefix: str) -> str:
    for d in range(10):
        cand = f"{prefix}{d}"
        if is_valid_luhn_bank_card(cand):
            return cand
    raise AssertionError("Failed to construct valid Luhn card")


def test_all_82_shared_mask_cases_golden_vectors() -> None:
    assert MASK_CASES_PATH.is_file(), f"Missing golden vectors at {MASK_CASES_PATH}"
    payload = json.loads(MASK_CASES_PATH.read_text(encoding="utf-8"))
    mask_keys_env: str = payload["meta"]["mask_keys_env"]
    tenant_id: str = payload["meta"]["tenant_id"]
    cases: list[dict[str, Any]] = payload["cases"]

    assert len(cases) == 82

    for idx, case in enumerate(cases):
        rule = case["rule"]
        raw_input = case["input"]
        expected = case["expected"]
        actual = apply_mask_rule(
            raw_input,
            rule,
            tenant_id=tenant_id,
            mask_keys_env=mask_keys_env,
        )
        assert actual == expected, (
            f"Vector #{idx} failed for rule={rule!r}, input={raw_input!r}: "
            f"expected {expected!r}, got {actual!r}"
        )


def test_hmac_hash_key_rotation_and_tenant_isolation(monkeypatch: pytest.MonkeyPatch) -> None:
    import base64

    k1 = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="
    k2_b64 = base64.b64encode(b"fedcba9876543210fedcba9876543210").decode("ascii")

    h_v1_default = apply_mask_rule(
        "13812345678", "HMAC_HASH", tenant_id="default", mask_keys_env=f"1:{k1}"
    )
    h_v1_east = apply_mask_rule(
        "13812345678", "HMAC_HASH", tenant_id="east", mask_keys_env=f"1:{k1}"
    )
    assert h_v1_default == "8afb4ccda60213a1"
    assert h_v1_east is not None and len(h_v1_east) == 16
    assert h_v1_default != h_v1_east

    # Multi-key keyring selects highest version (v2 > v1)
    h_rotated = apply_mask_rule(
        "13812345678",
        "HMAC_HASH",
        tenant_id="default",
        mask_keys_env=f"1:{k1},2:{k2_b64}",
    )
    h_v2_only = apply_mask_rule(
        "13812345678",
        "HMAC_HASH",
        tenant_id="default",
        mask_keys_env=f"2:{k2_b64}",
    )
    assert h_rotated == h_v2_only
    assert h_rotated != h_v1_default

    # Production mode without AXPOINT_MASK_KEYS raises SidecarError
    monkeypatch.delenv(MASK_KEYS_ENV_VAR, raising=False)
    monkeypatch.setenv("AXPOINT_ENV", "production")
    with pytest.raises(SidecarError) as exc_info:
        apply_mask_rule("13812345678", "HMAC_HASH", tenant_id="default")
    assert exc_info.value.status_code == 500

    assert apply_mask_rule("keep-raw", "NONE") == "keep-raw"
    assert apply_mask_rule(None, "NONE") is None


def test_cn_id_card_checksum_and_recognizer() -> None:
    valid_num = _make_gb11643_id("11010119900307123")
    # Find a prefix whose GB 11643 check char is 'X'
    valid_x = ""
    for seq in range(100, 200):
        cand = _make_gb11643_id(f"31011519881122{seq}", lower_x=True)
        if cand.endswith("x"):
            valid_x = cand
            break
    assert valid_x.endswith("x")

    wrong_chk = valid_num[:-1] + ("9" if valid_num[-1] != "9" else "8")
    bad_date = "110101199002301234"

    assert is_valid_gb11643_id_card(valid_num)
    assert is_valid_gb11643_id_card(valid_x)
    assert not is_valid_gb11643_id_card(wrong_chk)
    assert not is_valid_gb11643_id_card(bad_date)

    resp = analyze_texts(
        PiiAnalyzeRequest(
            texts=[
                f"身份证号 {valid_num} 与小写 {valid_x}",
                f"错误校验码 {wrong_chk} 及非法日期 {bad_date}",
            ]
        )
    )
    id_spans = [s for s in resp.results[0] if s.category == "ID_CARD"]
    assert len(id_spans) == 2
    assert resp.results[0][0].start == 5
    assert resp.results[0][0].end == 23
    assert [s for s in resp.results[1] if s.category == "ID_CARD"] == []


def test_bank_card_luhn_and_non_overlap_with_id_card() -> None:
    card16 = _make_luhn_card("622588011234567")
    card19 = _make_luhn_card("622202020011223344")
    bad_card = card16[:-1] + ("0" if card16[-1] != "0" else "1")

    assert is_valid_luhn_bank_card(card16)
    assert is_valid_luhn_bank_card(card19)
    assert not is_valid_luhn_bank_card(bad_card)

    # Construct an 18-digit valid CN_ID_CARD starting with '3'..'6' that also satisfies Luhn
    dual_id = ""
    for seq in range(100, 999):
        cand = _make_gb11643_id(f"31011519900307{seq}")
        if cand[-1].isdigit():
            # Check raw Luhn math directly
            total = 0
            for idx, ch in enumerate(reversed(cand)):
                d = ord(ch) - 48
                if idx % 2 == 1:
                    d = d * 2 - (9 if d * 2 > 9 else 0)
                total += d
            if total % 10 == 0:
                dual_id = cand
                break
    assert dual_id != ""
    assert not is_valid_luhn_bank_card(dual_id)

    resp = analyze_texts(
        PiiAnalyzeRequest(
            texts=[
                f"卡号 {card16} 和 {card19}，无效卡 {bad_card}",
                f"证件 {dual_id}",
            ]
        )
    )
    bank_spans = [s for s in resp.results[0] if s.category == "BANK_CARD"]
    assert len(bank_spans) == 2
    assert resp.results[1] == [
        resp.results[1][0]
    ]
    assert resp.results[1][0].category == "ID_CARD"


def test_phone_email_address_person_and_secret_recognizers() -> None:
    text_phone = (
        "联系电话：13812345678，国际格式 +86-13987654321，非手机号 12812345678 或 138123456789"
    )
    spans_phone = analyze_texts(PiiAnalyzeRequest(texts=[text_phone])).results[0]
    matched_phones = [text_phone[s.start : s.end] for s in spans_phone if s.category == "PHONE"]
    assert matched_phones == ["13812345678", "+86-13987654321"]
    assert all(s.score >= 0.85 for s in spans_phone if s.category == "PHONE")

    text_mail = "请发送邮件至 ops.team+1@axpoint.io，忽略 bad@@example..com"
    spans_mail = analyze_texts(PiiAnalyzeRequest(texts=[text_mail])).results[0]
    assert [text_mail[s.start : s.end] for s in spans_mail if s.category == "EMAIL"] == [
        "ops.team+1@axpoint.io"
    ]

    text_addr = (
        "收货地址：上海市浦东新区张江高科技园区科苑路88号，"
        "备选位于浙江省杭州市西湖区文三路90号东部软件园。"
    )
    spans_addr = analyze_texts(PiiAnalyzeRequest(texts=[text_addr])).results[0]
    matched_addrs = [text_addr[s.start : s.end] for s in spans_addr if s.category == "ADDRESS"]
    assert matched_addrs == [
        "上海市浦东新区张江高科技园区科苑路88号",
        "浙江省杭州市西湖区文三路90号东部软件园",
    ]

    text_person = "联系人：张三，复姓专家欧阳明日与司马相如出席会议。"
    spans_person = analyze_texts(PiiAnalyzeRequest(texts=[text_person])).results[0]
    matched_names = [text_person[s.start : s.end] for s in spans_person if s.category == "NAME"]
    assert "张三" in matched_names
    assert "欧阳明日" in matched_names
    assert "司马相如" in matched_names

    secrets_samples = [
        "-----BEGIN RSA PRIVATE KEY-----",
        "key is sk-abcdefghijklmnopqrstuvwxyz123456",
        "aws AKIAIOSFODNN7EXAMPLE end",
        "pat axp_Ab12Cd34_0123456789abcdef0123456789abcdef end",
        "hdr Bearer eyJhbGciOiJFZERTQSIsInR5cCI6IkpXVCJ9.payload",
    ]
    sec_resp = analyze_texts(PiiAnalyzeRequest(texts=secrets_samples))
    for i, res_list in enumerate(sec_resp.results):
        assert any(s.category == "SECRET" for s in res_list), f"Missed secret in sample {i}"


def test_person_spacy_ner_boost_and_unicode_codepoint_offsets(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Verify Unicode codepoint offsets match Rust .chars() even with 4-byte UTF-8 emojis
    emoji_text = "😀客户电话 13812345678"
    spans = analyze_texts(PiiAnalyzeRequest(texts=[emoji_text])).results[0]
    phone_sp = [s for s in spans if s.category == "PHONE"][0]
    assert emoji_text[phone_sp.start : phone_sp.end] == "13812345678"
    assert phone_sp.start == 6
    assert phone_sp.end == 17

    # Simulate zh_core_web_sm spaCy NER entities (PERSON / NR) without network access
    @dataclass
    class _FakeEnt:
        label_: str
        start_char: int
        end_char: int

    base_zh = spacy.blank("zh")

    def _inject_ner(doc: Any) -> Any:
        idx = doc.text.find("令狐冲")
        if idx >= 0:
            doc.ents = ()
        return doc

    base_zh.add_pipe("sentencizer")
    _ = _inject_ner

    # Directly verify CnPersonRecognizer boosts spaCy NR/PERSON entities with known surnames
    from pii.recognizers import CnPersonRecognizer

    rec = CnPersonRecognizer(supported_language="zh")

    @dataclass
    class _StubArtifacts:
        entities: list[_FakeEnt]

    sample_txt = "少侠令狐冲在此"
    ner_res = rec.analyze(
        sample_txt,
        entities=["NAME"],
        nlp_artifacts=_StubArtifacts(entities=[_FakeEnt("NR", 2, 5)]),  # type: ignore[arg-type]
    )
    assert any(r.start == 2 and r.end == 5 and r.score >= 0.88 for r in ner_res)

    # Verify OfflineSpacyNlpEngine gracefully falls back when spacy.load raises OSError
    def _raise_os_error(_name: str) -> Any:
        raise OSError("model wheel not installed")

    monkeypatch.setattr(spacy, "load", _raise_os_error)
    offline_nlp = OfflineSpacyNlpEngine()
    engine = PiiEngine(nlp_engine=offline_nlp)
    assert engine.using_fallback_blank is True
    fallback_res = engine.analyze_texts(
        PiiAnalyzeRequest(texts=["负责人：欧阳明日，员工张三"])
    ).results[0]
    names = {("负责人：欧阳明日，员工张三")[s.start : s.end] for s in fallback_res}
    assert {"欧阳明日", "张三"} <= names


def test_pii_fastapi_endpoints_and_auth(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_INTERNAL_TOKEN)
    payload = json.loads(MASK_CASES_PATH.read_text(encoding="utf-8"))
    monkeypatch.setenv(MASK_KEYS_ENV_VAR, payload["meta"]["mask_keys_env"])

    client = TestClient(app)

    # Unauthorized without internal token
    unauth = client.post("/pii/analyze", json={"texts": ["13812345678"]})
    assert unauth.status_code == 401
    assert unauth.json()["error_code"] == "TOKEN_INVALID"

    headers = {
        "x-axpoint-internal-token": TEST_INTERNAL_TOKEN,
        "x-axpoint-tenant": "default",
    }

    an_resp = client.post(
        "/pii/analyze",
        headers=headers,
        json={
            "texts": ["联系人：欧阳明日，手机 13812345678，邮箱 zhangsan@example.com"],
            "language": "zh",
            "score_threshold": 0.6,
        },
    )
    assert an_resp.status_code == 200
    spans = an_resp.json()["results"][0]
    cats = [s["category"] for s in spans]
    assert cats == ["NAME", "PHONE", "EMAIL"]

    mask_resp = client.post(
        "/pii/anonymize",
        headers=headers,
        json={
            "texts": [
                "联系人：欧阳明日，手机 13812345678，密钥 sk-abcdefghijklmnopqrstuvwxyz1234"
            ],
            "rules": {"PHONE": "HMAC_HASH"},
        },
    )
    assert mask_resp.status_code == 200
    masked_txt = mask_resp.json()["texts"][0]
    assert "欧阳**" in masked_txt
    assert "8afb4ccda60213a1" in masked_txt
    assert "[REDACTED]" in masked_txt
    assert "13812345678" not in masked_txt

    # Explicit header/body tenant mismatch -> 403 PERMISSION_DENIED
    mismatch = client.post(
        "/pii/anonymize",
        headers={
            "x-axpoint-internal-token": TEST_INTERNAL_TOKEN,
            "x-axpoint-tenant": "east",
        },
        json={"texts": ["13812345678"], "tenant_id": "south"},
    )
    assert mismatch.status_code == 403
    assert mismatch.json()["error_code"] == "PERMISSION_DENIED"

    # Anonymize helper function directly
    direct = anonymize_texts(
        PiiAnonymizeRequest(
            texts=["电话 13812345678"],
            rules={"PHONE": "NONE"},
        ),
        tenant_id="default",
    )
    assert direct.texts == ["电话 13812345678"]


def test_profiler_200_sample_30_percent_threshold() -> None:
    # 59 / 200 = 29.5% (< 30% threshold) -> NONE
    below_200 = ["13812345678"] * 59 + ["ORD-0001"] * 141
    cat_below, ratio_below = detect_pii_category("col_misc", below_200)
    assert cat_below == "NONE"
    assert ratio_below == 0.0

    # 60 / 200 = 30.0% (>= 30% threshold) -> PHONE with confidence 0.3
    at_200 = ["13812345678"] * 60 + ["ORD-0001"] * 140
    cat_at, ratio_at = detect_pii_category("col_misc", at_200)
    assert cat_at == "PHONE"
    assert ratio_at == 0.3

    # Verify sampling caps at first 200 non-null values (rows > 200 do not inflate head)
    capped_300 = [None, ""] * 20 + ["ORD-0001"] * 150 + ["13812345678"] * 150
    _, prof_capped, _ = infer_and_convert_column("raw_field", 0, capped_300)
    # First 200 non-null values contain 150 ORD-0001 + 50 phones = 25% < 30%
    assert prof_capped.pii_category == "NONE"

    # Unlabeled Chinese person name column detected via PiiEngine at >= 30%
    name_col = ["张三", "欧阳明日", "司马相如", "王小明"] * 20 + ["未知"] * 120
    df = pl.DataFrame({"buyer_col": name_col, "addr_col": [
        "上海市浦东新区张江高科技园区科苑路88号" if i < 70 else "暂无"
        for i in range(200)
    ]})
    profiles = {p.name: p for p in profile_polars_df(df)}
    assert profiles["buyer_col"].pii_category == "NAME"
    assert profiles["buyer_col"].pii_hit_ratio == 0.4
    assert profiles["addr_col"].pii_category == "ADDRESS"
    assert profiles["addr_col"].pii_hit_ratio == 0.35
