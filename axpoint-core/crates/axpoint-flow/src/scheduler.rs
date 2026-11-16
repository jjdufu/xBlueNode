//! Cron expression parsing, timezone offset resolution, minute-slot deduplication,
//! and periodic `AxFlow` cron + overdue-approval scheduler (`07-STEP07-08` §07.5).

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::enums::{Environment, TriggerType};
use axpoint_domain::flow::StartRunParams;
use axpoint_domain::ports::{DatasetRepository, FlowRepository};
use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, Offset as _, Timelike as _, Utc};
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

/// Validates a 5-field (`min hour dom mon dow`) or 6/7-field cron expression and returns its 6/7-field form.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if field count or syntax is invalid.
pub fn to_six_field_cron(expr: &str) -> Result<String, AppError> {
    let trimmed = expr.trim();
    let field_count = trimmed.split_whitespace().count();
    let normalized = match field_count {
        5 => format!("0 {trimmed}"),
        6 | 7 => String::from(trimmed),
        _ => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("cron expression '{trimmed}' must have 5 or 6 fields"),
            ));
        }
    };
    let _ = cron::Schedule::from_str(&normalized).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid cron expression '{trimmed}': {err}"),
        )
    })?;
    Ok(normalized)
}

/// Resolves a fixed UTC offset (`+HH:MM`, `-HH:MM`, `UTC`, or common IANA timezone names).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `tz` is unrecognized or out of bounds.
pub fn parse_timezone_offset(tz: &str) -> Result<FixedOffset, AppError> {
    let cleaned = tz.trim();
    if let Some(secs) = lookup_iana_offset_secs(cleaned) {
        return FixedOffset::east_opt(secs).ok_or_else(|| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid timezone '{cleaned}'"))
        });
    }
    if cleaned.starts_with('+') || cleaned.starts_with('-') {
        return FixedOffset::from_str(cleaned).map_err(|_| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid UTC offset '{cleaned}'; expected +HH:MM or -HH:MM"),
            )
        });
    }
    Err(AppError::bad(
        "INVALID_ARGUMENT",
        format!("unsupported timezone '{cleaned}'"),
    ))
}

fn lookup_iana_offset_secs(tz: &str) -> Option<i32> {
    match tz {
        "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Europe/London" => Some(0),
        "Europe/Berlin" | "Europe/Paris" | "Europe/Madrid" | "Europe/Rome" => Some(3_600),
        "Europe/Moscow" | "Asia/Riyadh" => Some(3 * 3_600),
        "Asia/Dubai" => Some(4 * 3_600),
        "Asia/Kolkata" => Some(5 * 3_600 + 1_800),
        "Asia/Bangkok" | "Asia/Jakarta" | "Asia/Ho_Chi_Minh" => Some(7 * 3_600),
        "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Harbin" | "Asia/Urumqi" | "Asia/Hong_Kong"
        | "Asia/Macau" | "Asia/Taipei" | "Asia/Singapore" | "Asia/Kuala_Lumpur"
        | "Australia/Perth" | "PRC" => Some(8 * 3_600),
        "Asia/Tokyo" | "Asia/Seoul" => Some(9 * 3_600),
        "Australia/Sydney" | "Australia/Melbourne" => Some(10 * 3_600),
        "Pacific/Auckland" => Some(12 * 3_600),
        "America/Sao_Paulo" => Some(-3 * 3_600),
        "America/New_York" | "America/Toronto" | "US/Eastern" => Some(-5 * 3_600),
        "America/Chicago" | "US/Central" => Some(-6 * 3_600),
        "America/Denver" | "US/Mountain" => Some(-7 * 3_600),
        "America/Los_Angeles" | "America/Vancouver" | "US/Pacific" => Some(-8 * 3_600),
        _ => None,
    }
}

/// Formats the canonical minute-grained cron run idempotency key `cron:YYYY-MM-DDTHH:MMZ` (`07-STEP07-08` §07.5).
#[must_use]
pub fn cron_idempotency_key(slot_utc: DateTime<Utc>) -> String {
    format!("cron:{}", slot_utc.format("%Y-%m-%dT%H:%MZ"))
}

/// Computes the latest scheduled minute slot in `(last_check_utc, now_utc]`, if `cron_expr` fired.
///
/// # Errors
/// Returns [`AppError::BadRequest`] if `cron_expr` is invalid.
pub fn due_cron_minute_slot(
    cron_expr: &str,
    tz_offset: FixedOffset,
    last_check_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, AppError> {
    if now_utc <= last_check_utc {
        return Ok(None);
    }
    let six = to_six_field_cron(cron_expr)?;
    let schedule = cron::Schedule::from_str(&six).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid cron '{cron_expr}': {err}"),
        )
    })?;

    let start_local = last_check_utc.with_timezone(&tz_offset);
    let mut latest_due: Option<DateTime<Utc>> = None;

    for candidate_local in schedule.after(&start_local).take(128) {
        let candidate_utc = candidate_local.with_timezone(&Utc);
        if candidate_utc <= now_utc {
            let truncated = candidate_utc
                .with_second(0)
                .and_then(|dt| dt.with_nanosecond(0))
                .unwrap_or(candidate_utc);
            latest_due = Some(truncated);
        } else {
            break;
        }
    }

    Ok(latest_due)
}

/// Periodic `AxFlow` cron trigger and approval-expiration scheduler (`07-STEP07-08` §07.5).
pub struct FlowCronScheduler<R> {
    repo: Arc<R>,
    last_checked: Mutex<HashMap<(String, Uuid), DateTime<Utc>>>,
}

impl<R> FlowCronScheduler<R>
where
    R: FlowRepository + DatasetRepository + 'static,
{
    /// Constructs a new [`FlowCronScheduler`].
    #[must_use]
    pub fn new(repo: Arc<R>) -> Self {
        Self {
            repo,
            last_checked: Mutex::new(HashMap::new()),
        }
    }

    /// Executes one evaluation tick at `now_utc`: expires overdue approvals and triggers due `PROD` cron flows.
    ///
    /// Returns the number of newly created cron runs (`existing == false`).
    ///
    /// # Errors
    /// Returns [`AppError`] if querying scheduled flows or expiring approvals fails.
    pub async fn tick_once(&self, now_utc: DateTime<Utc>) -> Result<usize, AppError> {
        let tenants = self.repo.list_active_tenants().await?;
        for tid in &tenants {
            let _ = self.repo.expire_overdue_approvals(tid).await?;
        }

        let scheduled = self.repo.list_scheduled_flows().await?;
        if scheduled.is_empty() {
            return Ok(0);
        }

        let mut tenant_tz_cache: HashMap<String, String> = HashMap::new();
        let mut started_count: usize = 0;

        for item in scheduled {
            let Some(cron_expr) = item
                .trigger_cfg
                .cron
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            else {
                continue;
            };

            let tz_name = resolve_item_timezone(
                &*self.repo,
                &item.tenant_id,
                item.trigger_cfg.timezone.as_deref(),
                &mut tenant_tz_cache,
            )
            .await;
            let tz_offset = parse_timezone_offset(&tz_name)
                .or_else(|_| FixedOffset::east_opt(8 * 3_600).ok_or(()))
                .unwrap_or_else(|()| Utc.fix());

            let key = (item.tenant_id.clone(), item.flow_id);
            let prev_utc = {
                let guard = self
                    .last_checked
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard
                    .get(&key)
                    .copied()
                    .or(item.last_run_at)
                    .unwrap_or_else(|| now_utc - ChronoDuration::seconds(60))
            };

            let catch_up = item.trigger_cfg.catch_up.unwrap_or(false);
            let Ok(slots) = due_cron_slots(cron_expr, tz_offset, prev_utc, now_utc, catch_up)
            else {
                continue;
            };

            {
                let mut guard = self
                    .last_checked
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.insert(key, now_utc);
            }

            for slot_utc in slots {
                if trigger_cron_slot(
                    &*self.repo,
                    &item.tenant_id,
                    item.flow_id,
                    item.version,
                    slot_utc,
                )
                .await
                {
                    started_count = started_count.saturating_add(1);
                }
            }
        }

        Ok(started_count)
    }

    /// Runs the scheduler loop until `shutdown_rx` signals `true`.
    pub async fn run_loop(
        &self,
        poll_interval: Duration,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut ticker = tokio::time::interval(poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        while !*shutdown_rx.borrow_and_update() {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = shutdown_rx.changed() => break,
            }
            if let Err(err) = self.tick_once(Utc::now()).await {
                tracing::warn!(error = %err, "FlowCronScheduler tick failed");
            }
        }
    }
}

fn due_cron_slots(
    cron_expr: &str,
    tz_offset: FixedOffset,
    last_check_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
    catch_up: bool,
) -> Result<Vec<DateTime<Utc>>, AppError> {
    if !catch_up {
        return due_cron_minute_slot(cron_expr, tz_offset, last_check_utc, now_utc)
            .map(|opt| opt.into_iter().collect());
    }
    if now_utc <= last_check_utc {
        return Ok(Vec::new());
    }
    let six = to_six_field_cron(cron_expr)?;
    let schedule = cron::Schedule::from_str(&six).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid cron '{cron_expr}': {err}"),
        )
    })?;
    let start_local = last_check_utc.with_timezone(&tz_offset);
    let mut slots: Vec<DateTime<Utc>> = Vec::new();
    for candidate_local in schedule.after(&start_local).take(64) {
        let candidate_utc = candidate_local.with_timezone(&Utc);
        if candidate_utc <= now_utc {
            let truncated = candidate_utc
                .with_second(0)
                .and_then(|dt| dt.with_nanosecond(0))
                .unwrap_or(candidate_utc);
            if slots.last().copied() != Some(truncated) {
                slots.push(truncated);
            }
        } else {
            break;
        }
    }
    Ok(slots)
}

async fn resolve_item_timezone<R: DatasetRepository>(
    repo: &R,
    tenant_id: &str,
    explicit_tz: Option<&str>,
    cache: &mut HashMap<String, String>,
) -> String {
    if let Some(tz) = explicit_tz.filter(|s| !s.trim().is_empty()) {
        return String::from(tz);
    }
    if let Some(cached) = cache.get(tenant_id) {
        return cached.clone();
    }
    let resolved = repo
        .get_tenant_timezone(tenant_id)
        .await
        .unwrap_or_else(|_| String::from("Asia/Shanghai"));
    cache.insert(String::from(tenant_id), resolved.clone());
    resolved
}

async fn trigger_cron_slot<R: FlowRepository>(
    repo: &R,
    tenant_id: &str,
    flow_id: Uuid,
    version: i32,
    slot_utc: DateTime<Utc>,
) -> bool {
    let idem_key = cron_idempotency_key(slot_utc);
    match repo
        .start_flow_run(
            tenant_id,
            StartRunParams {
                flow_id,
                environment: Environment::Prod,
                explicit_version: Some(version),
                trigger_source: TriggerType::Cron,
                trigger_payload: serde_json::json!({
                    "scheduled_at": slot_utc.to_rfc3339(),
                }),
                idempotency_key: Some(idem_key),
                started_by: None,
            },
        )
        .await
    {
        Ok((_run, false)) => true,
        Ok((_run, true)) => false,
        Err(err) => {
            tracing::warn!(
                tenant_id = %tenant_id,
                flow_id = %flow_id,
                error = %err,
                "scheduled cron flow start_flow_run failed"
            );
            false
        }
    }
}
