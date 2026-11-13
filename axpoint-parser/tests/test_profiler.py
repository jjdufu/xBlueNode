from __future__ import annotations

import polars as pl

from parsers.profiler import (
    infer_and_convert_column,
    is_valid_bank_card,
    is_valid_cn_id_card,
    is_valid_cn_phone,
    mask_pii_value,
    profile_polars_df,
)


def _make_valid_id_card(prefix17: str) -> str:
    weights = (7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2)
    cmap = "10X98765432"
    chk = cmap[sum(int(prefix17[i]) * weights[i] for i in range(17)) % 11]
    return prefix17 + chk


def _make_luhn_card(prefix: str) -> str:
    # Append check digit so total Luhn mod 10 == 0
    for d in range(10):
        cand = f"{prefix}{d}"
        if is_valid_bank_card(cand):
            return cand
    raise AssertionError("unreachable")


def test_all_11_datatypes_inference_and_cleaning() -> None:
    _, p_bool, _ = infer_and_convert_column("flag", 0, ["true", "FALSE", "yes", None])
    assert p_bool.data_type == "BOOLEAN"

    _, p_int, _ = infer_and_convert_column("qty", 1, ["1,200", "¥3,500", "42", None])
    assert p_int.data_type == "INT64"

    _, p_dec, _ = infer_and_convert_column("amount", 2, ["¥1,234.50", "$99.00", "0.25"])
    assert p_dec.data_type == "DECIMAL"

    _, p_flt, _ = infer_and_convert_column("ratio", 3, ["98.5%", "12.345", "1e-3"])
    assert p_flt.data_type == "FLOAT64"

    _, p_date, _ = infer_and_convert_column("dt", 4, ["2026-01-15", "2026年02月20日", "2026/03/01"])
    assert p_date.data_type == "DATE"

    s_ts, p_ts, _ = infer_and_convert_column(
        "created_at",
        5,
        ["2026-01-15 08:30:00", "2026-01-15T00:30:00Z"],
        timezone_str="Asia/Shanghai",
    )
    assert p_ts.data_type == "TIMESTAMP"
    assert s_ts.dtype == pl.Datetime("us", "UTC")

    _, p_json, _ = infer_and_convert_column("meta", 6, ['{"a": 1}', '{"b": [1, 2]}'])
    assert p_json.data_type == "JSON"

    _, p_bin, _ = infer_and_convert_column("payload", 7, [b"\x00\x01", b"\x02\x03"])
    assert p_bin.data_type == "BINARY"

    _, p_list, _ = infer_and_convert_column("tags", 8, [["a", "b"], ["c"]])
    assert p_list.data_type == "LIST"

    df_struct = pl.DataFrame({"info": [{"k": "v"}, {"k": "w"}]})
    profs = profile_polars_df(df_struct)
    assert profs[0].data_type == "STRUCT"

    _, p_str, _ = infer_and_convert_column("code", 10, ["00123", "00456"])
    assert p_str.data_type == "STRING"


def test_pii_checksum_detection_and_sample_masking() -> None:
    assert is_valid_cn_phone("+86 138-1234-5678")
    assert not is_valid_cn_phone("12812345678")

    valid_id = _make_valid_id_card("11010119900307123")
    invalid_id = valid_id[:-1] + ("0" if valid_id[-1] != "0" else "1")
    assert is_valid_cn_id_card(valid_id)
    assert not is_valid_cn_id_card(invalid_id)

    valid_card = _make_luhn_card("622202020011223344")
    assert is_valid_bank_card(valid_card)
    assert not is_valid_bank_card(
        "6222020200112233449" if valid_card[-1] != "9" else "6222020200112233448"
    )

    _, p_phone, _ = infer_and_convert_column("phone", 0, ["13812345678", "13987654321", "N/A"])
    assert p_phone.pii_category == "PHONE"
    assert p_phone.data_type == "STRING"
    assert p_phone.sample_values[0] == "138****5678"

    _, p_id, _ = infer_and_convert_column("id_no", 1, [valid_id, valid_id])
    assert p_id.pii_category == "ID_CARD"
    assert "********" in p_id.sample_values[0]

    _, p_bank, _ = infer_and_convert_column("card_no", 2, [valid_card, valid_card])
    assert p_bank.pii_category == "BANK_CARD"
    assert p_bank.sample_values[0].endswith(valid_card[-4:])

    _, p_mail, _ = infer_and_convert_column("mail", 3, ["zhangsan@example.com", "lisi@test.cn"])
    assert p_mail.pii_category == "EMAIL"
    assert p_mail.sample_values[0] == "z*******@example.com"

    _, p_sec, _ = infer_and_convert_column("api_key", 4, ["plain_val_1", "plain_val_2"])
    assert p_sec.pii_category == "SECRET"
    assert p_sec.sample_values[0] == "[REDACTED]"

    assert mask_pii_value("欧阳娜娜", "NAME") == "欧阳**"
    assert mask_pii_value("张三", "NAME") == "张*"
    assert mask_pii_value("江苏省苏州市工业园区星湖街328号", "ADDRESS").endswith("****")
