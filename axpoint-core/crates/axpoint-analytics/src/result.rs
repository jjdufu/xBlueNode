//! Arrow `RecordBatch` to `QueryResult` serializer, truncation handler, and dynamic PII masker (`05-STEP04` §04.3).

use std::fmt::Write as _;
use std::str::FromStr;

use arrow::array::{
    Array, ArrayRef, BinaryArray, BinaryViewArray, BooleanArray, Date32Array, Date64Array,
    Decimal128Array, FixedSizeBinaryArray, FixedSizeListArray, Float16Array, Float32Array,
    Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, LargeBinaryArray, LargeListArray,
    LargeStringArray, ListArray, RecordBatch, StringArray, StringViewArray, StructArray,
    TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow::datatypes::{DataType as ArrowDataType, Field, SchemaRef, TimeUnit};
use arrow::util::display::{ArrayFormatter, FormatOptions};
use axpoint_domain::AppError;
use axpoint_domain::analytics::{QueryColumn, QueryResult};
use axpoint_domain::enums::{DataType, PiiCategory};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};

use crate::sql_guard::{GuardedSql, OutputColumnOrigin, TenantCatalog};

/// Maximum signed integer representable without precision loss in IEEE-754 `f64` (`2^53 - 1`).
const MAX_SAFE_JSON_INT: i64 = 9_007_199_254_740_991;

/// Minimum signed integer representable without precision loss in IEEE-754 `f64` (`-(2^53 - 1)`).
const MIN_SAFE_JSON_INT: i64 = -9_007_199_254_740_991;

/// Maximum unsigned integer representable without precision loss in IEEE-754 `f64` (`2^53 - 1`).
const MAX_SAFE_JSON_UINT: u64 = 9_007_199_254_740_991;

/// Days from Common Era (`0001-01-01`) to Unix epoch (`1970-01-01`).
const UNIX_EPOCH_CE_DAYS: i32 = 719_163;

/// Options controlling [`batches_to_query_result`].
#[derive(Debug, Clone)]
pub struct ResultSerializeOptions<'a> {
    /// Effective row cap (`rows > effective_max_rows` triggers truncation).
    pub effective_max_rows: u32,
    /// Guarded SQL string echoed into [`QueryResult::sql`].
    pub sql: String,
    /// Query execution duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the caller explicitly requested unmasked PII values.
    pub reveal_pii: bool,
    /// Whether the caller holds `Permission::PiiRawRead`.
    pub has_pii_raw_read: bool,
    /// Active tenant schema catalog for wildcard PII lookup.
    pub catalog: &'a TenantCatalog,
    /// Guard output containing `output_lineage` and `referenced_datasets`.
    pub guarded: &'a GuardedSql,
}

/// Serialized [`QueryResult`] paired with an audit flag indicating whether raw PII was revealed.
#[derive(Debug, Clone)]
pub struct SerializedQueryOutput {
    /// JSON-ready query result payload.
    pub result: QueryResult,
    /// `true` when `reveal_pii` was requested and permitted (`PiiRawRead`).
    pub revealed_pii: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnMaskPlan {
    Plain,
    Category(PiiCategory),
    RedactDerived(PiiCategory),
}

/// Maps an Arrow [`ArrowDataType`] to the canonical domain [`DataType`].
#[must_use]
pub fn map_arrow_data_type(dt: &ArrowDataType) -> DataType {
    match dt {
        ArrowDataType::Boolean => DataType::Boolean,
        ArrowDataType::Int8
        | ArrowDataType::Int16
        | ArrowDataType::Int32
        | ArrowDataType::Int64
        | ArrowDataType::UInt8
        | ArrowDataType::UInt16
        | ArrowDataType::UInt32
        | ArrowDataType::UInt64 => DataType::Int64,
        ArrowDataType::Float16 | ArrowDataType::Float32 | ArrowDataType::Float64 => {
            DataType::Float64
        }
        ArrowDataType::Decimal128(_, _) | ArrowDataType::Decimal256(_, _) => DataType::Decimal,
        ArrowDataType::Date32 | ArrowDataType::Date64 => DataType::Date,
        ArrowDataType::Timestamp(_, _) => DataType::Timestamp,
        ArrowDataType::Binary
        | ArrowDataType::LargeBinary
        | ArrowDataType::FixedSizeBinary(_)
        | ArrowDataType::BinaryView => DataType::Binary,
        ArrowDataType::List(_)
        | ArrowDataType::LargeList(_)
        | ArrowDataType::FixedSizeList(_, _)
        | ArrowDataType::ListView(_)
        | ArrowDataType::LargeListView(_) => DataType::List,
        ArrowDataType::Struct(_) | ArrowDataType::Map(_, _) => DataType::Struct,
        ArrowDataType::Dictionary(_, value_type) => map_arrow_data_type(value_type),
        _ => DataType::String,
    }
}

/// Masks a string slice according to `05-STEP04` §04.3 dynamic PII rules.
#[must_use]
pub fn mask_pii_str(raw: &str, category: PiiCategory) -> String {
    match category {
        PiiCategory::None => String::from(raw),
        PiiCategory::Phone => mask_phone(raw),
        PiiCategory::IdCard => mask_id_card(raw),
        PiiCategory::Email => mask_email(raw),
        PiiCategory::BankCard => mask_bank_card(raw),
        PiiCategory::Name => mask_person_name(raw),
        PiiCategory::Address => mask_address(raw),
        PiiCategory::Secret => String::from("[REDACTED]"),
    }
}

fn mask_phone(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let len = chars.len();
    if len < 7 {
        return String::from("****");
    }
    let prefix: String = chars.iter().take(3).copied().collect();
    let suffix: String = chars.iter().skip(len.saturating_sub(4)).copied().collect();
    format!("{prefix}****{suffix}")
}

fn mask_id_card(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let len = chars.len();
    if len < 7 {
        return String::from("***********");
    }
    let prefix: String = chars.iter().take(3).copied().collect();
    let suffix: String = chars.iter().skip(len.saturating_sub(4)).copied().collect();
    format!("{prefix}***********{suffix}")
}

fn mask_email(raw: &str) -> String {
    let Some((local, domain)) = raw.split_once('@') else {
        return String::from("***");
    };
    if domain.is_empty() {
        return String::from("***");
    }
    let first = local.chars().next().unwrap_or('*');
    format!("{first}***@{domain}")
}

fn mask_bank_card(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let len = chars.len();
    if len < 4 {
        return String::from("************");
    }
    let suffix: String = chars.iter().skip(len.saturating_sub(4)).copied().collect();
    format!("************{suffix}")
}

fn mask_person_name(raw: &str) -> String {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return String::from("*");
    };
    let rest_count = chars.count();
    if rest_count == 0 {
        return String::from("*");
    }
    let stars = "*".repeat(rest_count);
    format!("{first}{stars}")
}

fn mask_address(raw: &str) -> String {
    for (byte_idx, ch) in raw.char_indices() {
        if matches!(ch, '市' | '州' | '盟' | '区') {
            let end = byte_idx.saturating_add(ch.len_utf8());
            if let Some(prefix) = raw.get(..end) {
                return format!("{prefix}***");
            }
        }
    }
    let chars: Vec<char> = raw.chars().collect();
    if chars.len() > 6 {
        let prefix: String = chars.into_iter().take(6).collect();
        format!("{prefix}***")
    } else {
        String::from("***")
    }
}

/// Converts Arrow [`RecordBatch`] slices into a [`SerializedQueryOutput`], enforcing `effective_max_rows`
/// truncation and dynamic PII masking (`05-STEP04` §04.3).
///
/// # Errors
/// - Returns `AppError::Forbidden("PERMISSION_DENIED")` if `opts.reveal_pii` is `true` while
///   `opts.has_pii_raw_read` is `false`.
/// - Returns `AppError::Database` if Arrow value formatting fails unexpectedly.
pub fn batches_to_query_result(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    opts: ResultSerializeOptions<'_>,
) -> Result<SerializedQueryOutput, AppError> {
    if opts.reveal_pii && !opts.has_pii_raw_read {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    let revealed_pii = opts.reveal_pii && opts.has_pii_raw_read;
    let max_rows_usize = usize::try_from(opts.effective_max_rows).unwrap_or(usize::MAX);

    let (columns, mask_plans) = build_columns_and_plans(
        schema.fields().as_ref(),
        opts.catalog,
        opts.guarded,
        revealed_pii,
    );

    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    let truncated = total_rows > max_rows_usize;
    let target_rows = total_rows.min(max_rows_usize);

    let mut rows = Vec::with_capacity(target_rows);
    for batch in batches {
        if rows.len() >= target_rows {
            break;
        }
        let take_in_batch = batch.num_rows().min(target_rows.saturating_sub(rows.len()));
        append_batch_rows(batch, take_in_batch, &columns, &mask_plans, &mut rows)?;
    }

    let row_count = rows.len();
    Ok(SerializedQueryOutput {
        result: QueryResult {
            columns,
            rows,
            row_count,
            truncated,
            sql: opts.sql,
            duration_ms: opts.duration_ms,
        },
        revealed_pii,
    })
}

fn build_columns_and_plans(
    fields: &[std::sync::Arc<Field>],
    catalog: &TenantCatalog,
    guarded: &GuardedSql,
    revealed_pii: bool,
) -> (Vec<QueryColumn>, Vec<ColumnMaskPlan>) {
    let mut columns = Vec::with_capacity(fields.len());
    let mut plans = Vec::with_capacity(fields.len());

    for (idx, field) in fields.iter().enumerate() {
        let (origin, in_wildcard_region) =
            resolve_field_origin(&guarded.output_lineage, fields.len(), idx, field.name());

        let (mut pii_category, is_derived) = match origin {
            Some(OutputColumnOrigin::Direct { pii_category, .. }) => (*pii_category, false),
            Some(OutputColumnOrigin::DerivedFromPii { pii_category, .. }) => (*pii_category, true),
            Some(OutputColumnOrigin::NonSensitive { .. }) => (PiiCategory::None, false),
            Some(OutputColumnOrigin::Wildcard) | None => {
                (lookup_wildcard_pii(field.name(), catalog, guarded), false)
            }
        };
        if in_wildcard_region && pii_category == PiiCategory::None {
            pii_category = lookup_wildcard_pii(field.name(), catalog, guarded);
        }

        let (masked, plan) = if pii_category == PiiCategory::None || revealed_pii {
            (false, ColumnMaskPlan::Plain)
        } else if is_derived {
            (true, ColumnMaskPlan::RedactDerived(pii_category))
        } else {
            (true, ColumnMaskPlan::Category(pii_category))
        };

        columns.push(QueryColumn {
            name: field.name().clone(),
            data_type: map_arrow_data_type(field.data_type()),
            pii_category,
            masked,
        });
        plans.push(plan);
    }

    (columns, plans)
}

fn resolve_field_origin<'a>(
    lineage: &'a [OutputColumnOrigin],
    field_count: usize,
    field_idx: usize,
    field_name: &str,
) -> (Option<&'a OutputColumnOrigin>, bool) {
    let first_wildcard = lineage
        .iter()
        .position(|o| matches!(o, OutputColumnOrigin::Wildcard));
    let Some(first_w) = first_wildcard else {
        if lineage.len() == field_count {
            return (lineage.get(field_idx), false);
        }
        if let Some(named) = find_named_lineage(lineage, field_name) {
            return (Some(named), false);
        }
        let unnamed_derived = lineage.iter().find(|o| {
            matches!(
                o,
                OutputColumnOrigin::DerivedFromPii {
                    output_name: None,
                    ..
                }
            )
        });
        return (unnamed_derived, unnamed_derived.is_none());
    };

    let last_w = lineage
        .iter()
        .rposition(|o| matches!(o, OutputColumnOrigin::Wildcard))
        .unwrap_or(first_w);
    let non_wild_count = lineage
        .iter()
        .filter(|o| !matches!(o, OutputColumnOrigin::Wildcard))
        .count();

    if field_count >= non_wild_count {
        if field_idx < first_w {
            return (lineage.get(field_idx), false);
        }
        let suffix_len = lineage.len().saturating_sub(1).saturating_sub(last_w);
        let suffix_start = field_count.saturating_sub(suffix_len);
        if field_idx >= suffix_start {
            let offset_from_end = field_count.saturating_sub(field_idx);
            let lin_idx = lineage.len().saturating_sub(offset_from_end);
            return (lineage.get(lin_idx), false);
        }
        let mid_slice = lineage.get(first_w..=last_w).unwrap_or(&[]);
        return (find_named_lineage(mid_slice, field_name), true);
    }

    (find_named_lineage(lineage, field_name), true)
}

fn find_named_lineage<'a>(
    lineage: &'a [OutputColumnOrigin],
    col_name: &str,
) -> Option<&'a OutputColumnOrigin> {
    let mut fallback: Option<&'a OutputColumnOrigin> = None;
    for item in lineage {
        let matches_name = match item {
            OutputColumnOrigin::Direct { output_name, .. } => {
                output_name.eq_ignore_ascii_case(col_name)
            }
            OutputColumnOrigin::DerivedFromPii {
                output_name: Some(alias),
                ..
            }
            | OutputColumnOrigin::NonSensitive {
                output_name: Some(alias),
            } => alias.eq_ignore_ascii_case(col_name),
            OutputColumnOrigin::DerivedFromPii {
                output_name: None, ..
            }
            | OutputColumnOrigin::NonSensitive { output_name: None }
            | OutputColumnOrigin::Wildcard => false,
        };
        if matches_name {
            match item {
                OutputColumnOrigin::Direct { pii_category, .. }
                | OutputColumnOrigin::DerivedFromPii { pii_category, .. }
                    if *pii_category != PiiCategory::None =>
                {
                    return Some(item);
                }
                _ => {
                    if fallback.is_none() {
                        fallback = Some(item);
                    }
                }
            }
        }
    }
    fallback
}

fn lookup_wildcard_pii(
    col_name: &str,
    catalog: &TenantCatalog,
    guarded: &GuardedSql,
) -> PiiCategory {
    let lower = col_name.trim().to_ascii_lowercase();
    if guarded.referenced_datasets.is_empty() {
        for ds in catalog.datasets.values() {
            if let Some(meta) = ds.columns.get(&lower)
                && meta.pii_category != PiiCategory::None
            {
                return meta.pii_category;
            }
        }
        return PiiCategory::None;
    }

    for ds_id in &guarded.referenced_datasets {
        if let Some(ds) = catalog.dataset_by_id(*ds_id)
            && let Some(meta) = ds.columns.get(&lower)
            && meta.pii_category != PiiCategory::None
        {
            return meta.pii_category;
        }
    }
    PiiCategory::None
}

fn append_batch_rows(
    batch: &RecordBatch,
    take_rows: usize,
    columns: &[QueryColumn],
    plans: &[ColumnMaskPlan],
    out_rows: &mut Vec<serde_json::Map<String, serde_json::Value>>,
) -> Result<(), AppError> {
    let mut row_maps: Vec<serde_json::Map<String, serde_json::Value>> = (0..take_rows)
        .map(|_| serde_json::Map::with_capacity(columns.len()))
        .collect();

    for ((col_idx, col_meta), plan) in columns.iter().enumerate().zip(plans.iter()) {
        let Some(array) = batch.columns().get(col_idx) else {
            return Err(AppError::Database(format!(
                "missing Arrow column at index {col_idx}"
            )));
        };
        for (row_idx, row_map) in row_maps.iter_mut().enumerate() {
            let raw_val = array_value_to_json(array, row_idx)?;
            let final_val = apply_mask_plan(raw_val, *plan);
            row_map.insert(col_meta.name.clone(), final_val);
        }
    }

    out_rows.extend(row_maps);
    Ok(())
}

fn apply_mask_plan(val: serde_json::Value, plan: ColumnMaskPlan) -> serde_json::Value {
    if val.is_null() {
        return serde_json::Value::Null;
    }
    match plan {
        ColumnMaskPlan::Plain => val,
        ColumnMaskPlan::RedactDerived(_) => serde_json::Value::String(String::from("[REDACTED]")),
        ColumnMaskPlan::Category(cat) => {
            let masked = match &val {
                serde_json::Value::String(s) => mask_pii_str(s, cat),
                other => mask_pii_str(&other.to_string(), cat),
            };
            serde_json::Value::String(masked)
        }
    }
}

fn array_value_to_json(array: &ArrayRef, row: usize) -> Result<serde_json::Value, AppError> {
    if array.is_null(row) {
        return Ok(serde_json::Value::Null);
    }
    match array.data_type() {
        ArrowDataType::Null => Ok(serde_json::Value::Null),
        ArrowDataType::Boolean => downcast_err::<BooleanArray>(array, "Boolean")
            .map(|a| serde_json::Value::Bool(a.value(row))),
        ArrowDataType::Int8 => downcast_err::<Int8Array>(array, "Int8")
            .map(|a| serde_json::Value::Number(serde_json::Number::from(i64::from(a.value(row))))),
        ArrowDataType::Int16 => downcast_err::<Int16Array>(array, "Int16")
            .map(|a| serde_json::Value::Number(serde_json::Number::from(i64::from(a.value(row))))),
        ArrowDataType::Int32 => downcast_err::<Int32Array>(array, "Int32")
            .map(|a| serde_json::Value::Number(serde_json::Number::from(i64::from(a.value(row))))),
        ArrowDataType::Int64 => {
            downcast_err::<Int64Array>(array, "Int64").map(|a| serialize_i64(a.value(row)))
        }
        ArrowDataType::UInt8 => downcast_err::<UInt8Array>(array, "UInt8")
            .map(|a| serde_json::Value::Number(serde_json::Number::from(u64::from(a.value(row))))),
        ArrowDataType::UInt16 => downcast_err::<UInt16Array>(array, "UInt16")
            .map(|a| serde_json::Value::Number(serde_json::Number::from(u64::from(a.value(row))))),
        ArrowDataType::UInt32 => downcast_err::<UInt32Array>(array, "UInt32")
            .map(|a| serde_json::Value::Number(serde_json::Number::from(u64::from(a.value(row))))),
        ArrowDataType::UInt64 => {
            downcast_err::<UInt64Array>(array, "UInt64").map(|a| serialize_u64(a.value(row)))
        }
        ArrowDataType::Float16 => downcast_err::<Float16Array>(array, "Float16")
            .map(|a| serialize_f64(f64::from(a.value(row)))),
        ArrowDataType::Float32 => downcast_err::<Float32Array>(array, "Float32")
            .map(|a| serialize_f64(f64::from(a.value(row)))),
        ArrowDataType::Float64 => {
            downcast_err::<Float64Array>(array, "Float64").map(|a| serialize_f64(a.value(row)))
        }
        ArrowDataType::Decimal128(_, scale) => downcast_err::<Decimal128Array>(array, "Decimal128")
            .map(|a| serialize_decimal128(a.value(row), *scale)),
        ArrowDataType::Date32 => {
            downcast_err::<Date32Array>(array, "Date32").map(|a| serialize_date32(a.value(row)))
        }
        ArrowDataType::Date64 => {
            downcast_err::<Date64Array>(array, "Date64").map(|a| serialize_date64(a.value(row)))
        }
        ArrowDataType::Timestamp(unit, _) => serialize_timestamp_array(array, *unit, row),
        ArrowDataType::Utf8 => downcast_err::<StringArray>(array, "Utf8")
            .map(|a| serde_json::Value::String(String::from(a.value(row)))),
        ArrowDataType::LargeUtf8 => downcast_err::<LargeStringArray>(array, "LargeUtf8")
            .map(|a| serde_json::Value::String(String::from(a.value(row)))),
        ArrowDataType::Utf8View => downcast_err::<StringViewArray>(array, "Utf8View")
            .map(|a| serde_json::Value::String(String::from(a.value(row)))),
        ArrowDataType::Binary
        | ArrowDataType::LargeBinary
        | ArrowDataType::FixedSizeBinary(_)
        | ArrowDataType::BinaryView => serialize_binary_array(array, row),
        ArrowDataType::List(_)
        | ArrowDataType::LargeList(_)
        | ArrowDataType::FixedSizeList(_, _) => serialize_list_array(array, row),
        ArrowDataType::Struct(_) => serialize_struct_array(array, row),
        _ => format_via_arrow_display(array, row),
    }
}

fn downcast_err<'a, T: 'static>(array: &'a ArrayRef, label: &str) -> Result<&'a T, AppError> {
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| AppError::Database(format!("failed to downcast Arrow {label} array")))
}

fn serialize_i64(v: i64) -> serde_json::Value {
    if (MIN_SAFE_JSON_INT..=MAX_SAFE_JSON_INT).contains(&v) {
        serde_json::Value::Number(serde_json::Number::from(v))
    } else {
        serde_json::Value::String(v.to_string())
    }
}

fn serialize_u64(v: u64) -> serde_json::Value {
    if v <= MAX_SAFE_JSON_UINT {
        serde_json::Value::Number(serde_json::Number::from(v))
    } else {
        serde_json::Value::String(v.to_string())
    }
}

fn serialize_f64(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

fn format_decimal128(raw: i128, scale: i8) -> String {
    if raw == 0 && scale <= 0 {
        return String::from("0");
    }
    let sign = if raw < 0 { "-" } else { "" };
    let abs_digits = raw.unsigned_abs().to_string();
    if scale <= 0 {
        let trailing_zeros = usize::from(i16::from(scale).unsigned_abs());
        return format!("{sign}{abs_digits}{}", "0".repeat(trailing_zeros));
    }
    let frac_len = usize::try_from(scale).unwrap_or(0);
    if abs_digits.len() <= frac_len {
        let pad = "0".repeat(frac_len.saturating_sub(abs_digits.len()));
        format!("{sign}0.{pad}{abs_digits}")
    } else {
        let split_at = abs_digits.len().saturating_sub(frac_len);
        let int_part = abs_digits.get(..split_at).unwrap_or("0");
        let frac_part = abs_digits.get(split_at..).unwrap_or("0");
        format!("{sign}{int_part}.{frac_part}")
    }
}

fn serialize_decimal128(raw: i128, scale: i8) -> serde_json::Value {
    let formatted = format_decimal128(raw, scale);
    let sig_digits = raw.unsigned_abs().to_string().len();
    if sig_digits <= 15
        && let Ok(num) = serde_json::Number::from_str(&formatted)
    {
        return serde_json::Value::Number(num);
    }
    serde_json::Value::String(formatted)
}

fn serialize_date32(days: i32) -> serde_json::Value {
    NaiveDate::from_num_days_from_ce_opt(days.saturating_add(UNIX_EPOCH_CE_DAYS))
        .map_or(serde_json::Value::Null, |d| {
            serde_json::Value::String(d.format("%Y-%m-%d").to_string())
        })
}

fn serialize_date64(ms: i64) -> serde_json::Value {
    DateTime::<Utc>::from_timestamp_millis(ms).map_or(serde_json::Value::Null, |dt| {
        serde_json::Value::String(dt.date_naive().format("%Y-%m-%d").to_string())
    })
}

fn serialize_timestamp_array(
    array: &ArrayRef,
    unit: TimeUnit,
    row: usize,
) -> Result<serde_json::Value, AppError> {
    let dt_opt = match unit {
        TimeUnit::Second => {
            let a = downcast_err::<TimestampSecondArray>(array, "TimestampSecond")?;
            DateTime::<Utc>::from_timestamp(a.value(row), 0)
        }
        TimeUnit::Millisecond => {
            let a = downcast_err::<TimestampMillisecondArray>(array, "TimestampMillisecond")?;
            DateTime::<Utc>::from_timestamp_millis(a.value(row))
        }
        TimeUnit::Microsecond => {
            let a = downcast_err::<TimestampMicrosecondArray>(array, "TimestampMicrosecond")?;
            DateTime::<Utc>::from_timestamp_micros(a.value(row))
        }
        TimeUnit::Nanosecond => {
            let a = downcast_err::<TimestampNanosecondArray>(array, "TimestampNanosecond")?;
            Some(DateTime::<Utc>::from_timestamp_nanos(a.value(row)))
        }
    };
    Ok(dt_opt.map_or(serde_json::Value::Null, |dt| {
        serde_json::Value::String(dt.to_rfc3339_opts(SecondsFormat::AutoSi, true))
    }))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        let _fmt_ok = write!(out, "{b:02x}");
    }
    out
}

fn serialize_binary_array(array: &ArrayRef, row: usize) -> Result<serde_json::Value, AppError> {
    let hex_str = match array.data_type() {
        ArrowDataType::Binary => {
            bytes_to_hex(downcast_err::<BinaryArray>(array, "Binary")?.value(row))
        }
        ArrowDataType::LargeBinary => {
            bytes_to_hex(downcast_err::<LargeBinaryArray>(array, "LargeBinary")?.value(row))
        }
        ArrowDataType::FixedSizeBinary(_) => {
            bytes_to_hex(downcast_err::<FixedSizeBinaryArray>(array, "FixedSizeBinary")?.value(row))
        }
        ArrowDataType::BinaryView => {
            bytes_to_hex(downcast_err::<BinaryViewArray>(array, "BinaryView")?.value(row))
        }
        _ => return format_via_arrow_display(array, row),
    };
    Ok(serde_json::Value::String(hex_str))
}

fn serialize_list_array(array: &ArrayRef, row: usize) -> Result<serde_json::Value, AppError> {
    let child: ArrayRef = match array.data_type() {
        ArrowDataType::List(_) => downcast_err::<ListArray>(array, "List")?.value(row),
        ArrowDataType::LargeList(_) => {
            downcast_err::<LargeListArray>(array, "LargeList")?.value(row)
        }
        ArrowDataType::FixedSizeList(_, _) => {
            downcast_err::<FixedSizeListArray>(array, "FixedSizeList")?.value(row)
        }
        _ => return format_via_arrow_display(array, row),
    };
    let mut items = Vec::with_capacity(child.len());
    for i in 0..child.len() {
        items.push(array_value_to_json(&child, i)?);
    }
    Ok(serde_json::Value::Array(items))
}

fn serialize_struct_array(array: &ArrayRef, row: usize) -> Result<serde_json::Value, AppError> {
    let struct_arr = downcast_err::<StructArray>(array, "Struct")?;
    let mut obj = serde_json::Map::with_capacity(struct_arr.num_columns());
    for (field, col) in struct_arr.fields().iter().zip(struct_arr.columns().iter()) {
        obj.insert(field.name().clone(), array_value_to_json(col, row)?);
    }
    Ok(serde_json::Value::Object(obj))
}

fn format_via_arrow_display(array: &ArrayRef, row: usize) -> Result<serde_json::Value, AppError> {
    let formatter = ArrayFormatter::try_new(array.as_ref(), &FormatOptions::default())
        .map_err(|err| AppError::Database(format!("Arrow value format failed: {err}")))?;
    Ok(serde_json::Value::String(formatter.value(row).to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use arrow::datatypes::Schema;
    use axpoint_domain::axhub::ColumnMeta;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::sql_guard::{CatalogDatasetEntry, GuardOptions, guard};

    fn pii_demo_catalog() -> TenantCatalog {
        let ds_id = Uuid::from_u128(1);
        let mut ds = CatalogDatasetEntry::simple(ds_id, "demo_orders", "demo_orders");
        for (idx, (col, pii)) in [
            ("phone", PiiCategory::Phone),
            ("email", PiiCategory::Email),
            ("status", PiiCategory::None),
        ]
        .into_iter()
        .enumerate()
        {
            ds.columns.insert(
                String::from(col),
                ColumnMeta {
                    dataset_id: ds_id,
                    column_name: String::from(col),
                    data_type: DataType::String,
                    pii_category: pii,
                    pii_confirmed: pii != PiiCategory::None,
                    description: String::new(),
                    business_term: String::new(),
                    unit: String::new(),
                    ordinal: i32::try_from(idx).unwrap_or(0),
                },
            );
        }
        TenantCatalog::new("default").with_dataset(ds)
    }

    #[test]
    fn pii_mask_rules_cover_all_categories_and_edge_cases() {
        assert_eq!(
            mask_pii_str("13812345678", PiiCategory::Phone),
            "138****5678"
        );
        assert_eq!(mask_pii_str("12345", PiiCategory::Phone), "****");
        assert_eq!(
            mask_pii_str("110101199001011234", PiiCategory::IdCard),
            "110***********1234"
        );
        assert_eq!(
            mask_pii_str("alice@x.com", PiiCategory::Email),
            "a***@x.com"
        );
        assert_eq!(mask_pii_str("not-an-email", PiiCategory::Email), "***");
        assert_eq!(
            mask_pii_str("6222021234567890", PiiCategory::BankCard),
            "************7890"
        );
        assert_eq!(mask_pii_str("张三", PiiCategory::Name), "张*");
        assert_eq!(mask_pii_str("欧阳修", PiiCategory::Name), "欧**");
        assert_eq!(mask_pii_str("单", PiiCategory::Name), "*");
        assert_eq!(
            mask_pii_str("上海市浦东新区世纪大道1号", PiiCategory::Address),
            "上海市***"
        );
        assert_eq!(
            mask_pii_str("NoDelimiterStreet999", PiiCategory::Address),
            "NoDeli***"
        );
        assert_eq!(
            mask_pii_str("top-secret-key", PiiCategory::Secret),
            "[REDACTED]"
        );
        assert_eq!(mask_pii_str("plain", PiiCategory::None), "plain");
    }

    #[test]
    fn record_batch_serialization_truncates_and_sanitizes_types() -> Result<(), AppError> {
        let catalog = pii_demo_catalog();
        let guarded = guard(
            "SELECT count_n, score, amount FROM demo_orders",
            &catalog,
            &GuardOptions {
                max_rows: 2,
                allow_live: false,
                params: BTreeMap::new(),
            },
        )?;

        let schema = Arc::new(Schema::new(vec![
            Field::new("count_n", ArrowDataType::Int64, false),
            Field::new("score", ArrowDataType::Float64, true),
            Field::new("amount", ArrowDataType::Decimal128(18, 2), false),
        ]));
        let dec_arr = Decimal128Array::from(vec![351_750_i128, 10_000_i128, 999_i128])
            .with_precision_and_scale(18, 2)
            .map_err(|e| AppError::Database(e.to_string()))?;
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(vec![
                    100_i64,
                    9_007_199_254_740_999_i64,
                    3_i64,
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(f64::NAN),
                    Some(f64::INFINITY),
                    Some(1.5),
                ])),
                Arc::new(dec_arr),
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        let out = batches_to_query_result(
            &schema,
            &[batch],
            ResultSerializeOptions {
                effective_max_rows: 2,
                sql: guarded.sql.clone(),
                duration_ms: 9,
                reveal_pii: false,
                has_pii_raw_read: false,
                catalog: &catalog,
                guarded: &guarded,
            },
        )?;
        assert!(out.result.truncated);
        assert_eq!(out.result.row_count, 2);
        let [r0, r1] = out.result.rows.as_slice() else {
            return Err(AppError::bad("TEST_FAIL", "expected 2 rows"));
        };
        assert_eq!(r0.get("count_n"), Some(&json!(100)));
        assert_eq!(r0.get("score"), Some(&serde_json::Value::Null));
        assert_eq!(r0.get("amount"), Some(&json!(3517.5)));
        assert_eq!(r1.get("count_n"), Some(&json!("9007199254740999")));
        assert_eq!(r1.get("score"), Some(&serde_json::Value::Null));
        Ok(())
    }

    #[test]
    fn record_batch_pii_masking_and_reveal_permission_enforced() -> Result<(), AppError> {
        let catalog = pii_demo_catalog();
        let guarded = guard(
            "SELECT phone, substring(email, 1, 3) AS em_sub FROM demo_orders",
            &catalog,
            &GuardOptions::default(),
        )?;
        let schema = Arc::new(Schema::new(vec![
            Field::new("phone", ArrowDataType::Utf8, true),
            Field::new("em_sub", ArrowDataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(vec![Some("13812345678"), None])),
                Arc::new(StringArray::from(vec![Some("ali"), None])),
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        // Default masks even when principal holds PiiRawRead (`ADMIN`)
        let masked = batches_to_query_result(
            &schema,
            std::slice::from_ref(&batch),
            ResultSerializeOptions {
                effective_max_rows: 100,
                sql: guarded.sql.clone(),
                duration_ms: 4,
                reveal_pii: false,
                has_pii_raw_read: true,
                catalog: &catalog,
                guarded: &guarded,
            },
        )?;
        assert!(!masked.revealed_pii);
        let [r0, r1] = masked.result.rows.as_slice() else {
            return Err(AppError::bad("TEST_FAIL", "expected 2 rows"));
        };
        assert_eq!(r0.get("phone"), Some(&json!("138****5678")));
        assert_eq!(r0.get("em_sub"), Some(&json!("[REDACTED]")));
        assert_eq!(r1.get("phone"), Some(&serde_json::Value::Null));
        assert_eq!(r1.get("em_sub"), Some(&serde_json::Value::Null));

        // reveal_pii=true without PiiRawRead -> 403 PERMISSION_DENIED
        let denied = batches_to_query_result(
            &schema,
            std::slice::from_ref(&batch),
            ResultSerializeOptions {
                effective_max_rows: 100,
                sql: guarded.sql.clone(),
                duration_ms: 4,
                reveal_pii: true,
                has_pii_raw_read: false,
                catalog: &catalog,
                guarded: &guarded,
            },
        );
        assert!(matches!(
            denied,
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));

        // reveal_pii=true with PiiRawRead -> plaintext + revealed_pii=true
        let revealed = batches_to_query_result(
            &schema,
            &[batch],
            ResultSerializeOptions {
                effective_max_rows: 100,
                sql: guarded.sql.clone(),
                duration_ms: 4,
                reveal_pii: true,
                has_pii_raw_read: true,
                catalog: &catalog,
                guarded: &guarded,
            },
        )?;
        assert!(revealed.revealed_pii);
        let Some(first) = revealed.result.rows.first() else {
            return Err(AppError::bad("TEST_FAIL", "expected row"));
        };
        assert_eq!(first.get("phone"), Some(&json!("13812345678")));
        assert_eq!(first.get("em_sub"), Some(&json!("ali")));
        Ok(())
    }

    #[test]
    fn subquery_wrapped_and_cte_queries_enforce_pii_masking_and_redaction() -> Result<(), AppError>
    {
        let catalog = pii_demo_catalog();

        // 1. xBlueView global filter wrapper around aliased direct PII + derived PII
        let wrapped_guarded = guard(
            "SELECT * FROM (SELECT phone AS contact_num, substring(email, 1, 4) AS em, status FROM demo_orders) AS _w WHERE status = 'PAID'",
            &catalog,
            &GuardOptions::default(),
        )?;
        let wrapped_schema = Arc::new(Schema::new(vec![
            Field::new("contact_num", ArrowDataType::Utf8, true),
            Field::new("em", ArrowDataType::Utf8, true),
            Field::new("status", ArrowDataType::Utf8, true),
        ]));
        let wrapped_batch = RecordBatch::try_new(
            Arc::clone(&wrapped_schema),
            vec![
                Arc::new(StringArray::from(vec![Some("13812345678")])),
                Arc::new(StringArray::from(vec![Some("alic")])),
                Arc::new(StringArray::from(vec![Some("PAID")])),
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        let wrapped_out = batches_to_query_result(
            &wrapped_schema,
            &[wrapped_batch],
            ResultSerializeOptions {
                effective_max_rows: 100,
                sql: wrapped_guarded.sql.clone(),
                duration_ms: 2,
                reveal_pii: false,
                has_pii_raw_read: false,
                catalog: &catalog,
                guarded: &wrapped_guarded,
            },
        )?;
        let Some(row0) = wrapped_out.result.rows.first() else {
            return Err(AppError::bad("TEST_FAIL", "expected masked row"));
        };
        assert_eq!(row0.get("contact_num"), Some(&json!("138****5678")));
        assert_eq!(row0.get("em"), Some(&json!("[REDACTED]")));
        assert_eq!(row0.get("status"), Some(&json!("PAID")));

        // 2. CTE passthrough + outer function over aliased PII column
        let cte_guarded = guard(
            "WITH sub AS (SELECT phone AS p FROM demo_orders) SELECT p, upper(p) AS loud_p FROM sub",
            &catalog,
            &GuardOptions::default(),
        )?;
        let cte_schema = Arc::new(Schema::new(vec![
            Field::new("p", ArrowDataType::Utf8, true),
            Field::new("loud_p", ArrowDataType::Utf8, true),
        ]));
        let cte_batch = RecordBatch::try_new(
            Arc::clone(&cte_schema),
            vec![
                Arc::new(StringArray::from(vec![Some("13900001111")])),
                Arc::new(StringArray::from(vec![Some("13900001111")])),
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        let cte_out = batches_to_query_result(
            &cte_schema,
            &[cte_batch],
            ResultSerializeOptions {
                effective_max_rows: 100,
                sql: cte_guarded.sql.clone(),
                duration_ms: 2,
                reveal_pii: false,
                has_pii_raw_read: false,
                catalog: &catalog,
                guarded: &cte_guarded,
            },
        )?;
        let Some(cte_row) = cte_out.result.rows.first() else {
            return Err(AppError::bad("TEST_FAIL", "expected CTE masked row"));
        };
        assert_eq!(cte_row.get("p"), Some(&json!("139****1111")));
        assert_eq!(cte_row.get("loud_p"), Some(&json!("[REDACTED]")));
        Ok(())
    }
}
