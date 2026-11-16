//! `HTTP_CALL` operator invoking external HTTP endpoints with `OutboundUrlGuardPort`, credential unsealing,
//! `1 MiB` response cap, no auto-redirect, and `DRY_RUN` in `TEST` (`07-STEP07-08` §07.4).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::enums::{Environment, FlowOperator};
use axpoint_domain::flow::{
    ActionKind, ActionLogStatus, ActionSourceKind, NewActionLogInput, Operator, OperatorContext,
    OperatorOutcome,
};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{FlowCredentialUnsealPort, FlowRepository, OutboundUrlGuardPort};
use base64::Engine as _;

use super::{
    parse_optional_bool, parse_optional_u64, parse_optional_uuid, parse_required_string,
    templated_bool_schema, templated_int_schema, templated_uuid_schema,
};

/// Studio palette display name for `HTTP_CALL`.
pub const DISPLAY_NAME: &str = "外部 HTTP 调用";

/// Studio palette short description for `HTTP_CALL`.
pub const DESCRIPTION: &str = "经 SSRF 防护与密钥环解密凭证调用外部 HTTP 接口，响应截断上限 1 MiB";

const MAX_RESPONSE_BODY_BYTES: usize = 1_048_576;

/// Returns the JSON Schema validating `HTTP_CALL` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["url"],
        "properties": {
            "method": {
                "oneOf": [
                    { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"] },
                    { "type": "string", "pattern": "^\\s*\\{\\{.+\\}\\}\\s*$" }
                ]
            },
            "url": { "type": "string", "minLength": 1 },
            "headers": {
                "type": "object",
                "additionalProperties": { "type": "string" }
            },
            "body": {},
            "json": {},
            "credential_id": templated_uuid_schema(),
            "allow_private_network": templated_bool_schema(),
            "timeout_s": templated_int_schema(1, 300),
            "expected_status": {
                "type": "array",
                "items": { "type": "integer", "minimum": 100, "maximum": 599 }
            },
            "dry_run": templated_bool_schema()
        },
        "additionalProperties": false
    })
}

/// `HTTP_CALL` operator implementation generic over [`FlowRepository`], [`OutboundUrlGuardPort`], and [`FlowCredentialUnsealPort`].
pub struct HttpCallOperator<F, U, C> {
    flow_repo: Arc<F>,
    url_guard: Arc<U>,
    cred_unsealer: Option<Arc<C>>,
    http: reqwest::Client,
}

impl<F, U, C> HttpCallOperator<F, U, C>
where
    F: FlowRepository + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    /// Constructs a new [`HttpCallOperator`] with automatic redirects disabled.
    #[must_use]
    pub fn new(flow_repo: Arc<F>, url_guard: Arc<U>, cred_unsealer: Option<Arc<C>>) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            flow_repo,
            url_guard,
            cred_unsealer,
            http,
        }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        let url = parse_required_string(&ctx.config, "url")?;
        let method_str = ctx
            .config
            .get("method")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| String::from("POST"), |s| s.trim().to_ascii_uppercase());
        let method = parse_http_method(&method_str)?;
        let allow_private =
            parse_optional_bool(&ctx.config, "allow_private_network")?.unwrap_or(false);
        let timeout_s = parse_optional_u64(&ctx.config, "timeout_s")?
            .unwrap_or(30)
            .clamp(1, 300);
        let dry_run = parse_optional_bool(&ctx.config, "dry_run")?.unwrap_or(false);
        let is_dry_run = ctx.environment == Environment::Test || dry_run;

        let idem_key = format!("{}:{}:0", ctx.run_id, ctx.node_id);
        if let Some(existing) = self
            .flow_repo
            .get_action_log_by_key(&ctx.tenant_id, &idem_key)
            .await?
            && matches!(
                existing.status,
                ActionLogStatus::Done | ActionLogStatus::DryRun
            )
        {
            return Ok(OperatorOutcome::Completed(serde_json::json!({
                "dry_run": existing.status == ActionLogStatus::DryRun,
                "status_code": 200,
                "body": existing.payload.get("response_body").cloned().unwrap_or(serde_json::Value::Null),
                "action_id": existing.id,
            })));
        }

        if is_dry_run {
            validate_dry_run_url(&*self.url_guard, &url, allow_private).await?;
            let (rec, _) = self
                .flow_repo
                .record_action_log(
                    &ctx.tenant_id,
                    NewActionLogInput {
                        run_id: Some(ctx.run_id),
                        node_id: Some(ctx.node_id.clone()),
                        source_kind: ActionSourceKind::Flow,
                        action_kind: ActionKind::Http,
                        target: url,
                        payload: redacted_request_summary(&method_str, &ctx.config),
                        status: ActionLogStatus::DryRun,
                        idempotency_key: idem_key,
                        error_message: None,
                    },
                )
                .await?;
            return Ok(OperatorOutcome::Completed(serde_json::json!({
                "dry_run": true,
                "status_code": 200,
                "body": serde_json::Value::Null,
                "action_id": rec.id,
            })));
        }

        let checked_url = self
            .url_guard
            .check_outbound_url(&url, allow_private)
            .await?;

        self.send_and_record(ctx, method, &method_str, &checked_url, timeout_s, &idem_key)
            .await
    }

    async fn send_and_record(
        &self,
        ctx: &OperatorContext,
        method: reqwest::Method,
        method_str: &str,
        checked_url: &str,
        timeout_s: u64,
        idem_key: &str,
    ) -> Result<OperatorOutcome, AppError> {
        let mut req_builder = self
            .http
            .request(method, checked_url)
            .timeout(Duration::from_secs(timeout_s))
            .header("Idempotency-Key", &ctx.idempotency_key);

        if let Some(serde_json::Value::Object(hdrs)) = ctx.config.get("headers") {
            for (k, v) in hdrs {
                if let Some(s) = v.as_str() {
                    req_builder = req_builder.header(k.as_str(), s);
                }
            }
        }

        if let Some(cred_id) = parse_optional_uuid(&ctx.config, "credential_id")? {
            let unsealer = self.cred_unsealer.as_ref().ok_or_else(|| {
                AppError::Config(String::from("FlowCredentialUnsealPort is not configured"))
            })?;
            let secret_val = unsealer
                .unseal_credential_json(&ctx.tenant_id, cred_id)
                .await?;
            req_builder = apply_secret_headers(req_builder, &secret_val);
        }

        if let Some(body_val) = ctx.config.get("body").or_else(|| ctx.config.get("json"))
            && !body_val.is_null()
        {
            match body_val {
                serde_json::Value::String(raw_str) if ctx.config.get("json").is_none() => {
                    req_builder = req_builder.body(raw_str.clone());
                }
                json_body => {
                    req_builder = req_builder.json(json_body);
                }
            }
        }

        let send_res = req_builder.send().await;
        let mut resp = match send_res {
            Ok(r) => r,
            Err(err) => {
                let app_err = if err.is_timeout() {
                    AppError::Timeout("UPSTREAM_TIMEOUT")
                } else {
                    AppError::Upstream(format!("HTTP_CALL request failed: {err}"))
                };
                self.record_http_failure(ctx, method_str, checked_url, idem_key, &app_err)
                    .await;
                return Err(app_err);
            }
        };

        let status_code = resp.status().as_u16();
        let body_val = read_capped_body(&mut resp).await?;
        let ok_status = is_expected_status(&ctx.config, status_code);

        if !ok_status {
            let app_err = AppError::Upstream(format!("HTTP {status_code} from {checked_url}"));
            self.record_http_failure(ctx, method_str, checked_url, idem_key, &app_err)
                .await;
            return Err(app_err);
        }

        let mut log_payload = redacted_request_summary(method_str, &ctx.config);
        if let serde_json::Value::Object(ref mut m) = log_payload {
            m.insert(String::from("status_code"), serde_json::json!(status_code));
            m.insert(String::from("response_body"), body_val.clone());
        }

        let (rec, _) = self
            .flow_repo
            .record_action_log(
                &ctx.tenant_id,
                NewActionLogInput {
                    run_id: Some(ctx.run_id),
                    node_id: Some(ctx.node_id.clone()),
                    source_kind: ActionSourceKind::Flow,
                    action_kind: ActionKind::Http,
                    target: String::from(checked_url),
                    payload: log_payload,
                    status: ActionLogStatus::Done,
                    idempotency_key: String::from(idem_key),
                    error_message: None,
                },
            )
            .await?;

        Ok(OperatorOutcome::Completed(serde_json::json!({
            "dry_run": false,
            "status_code": status_code,
            "body": body_val,
            "action_id": rec.id,
        })))
    }

    async fn record_http_failure(
        &self,
        ctx: &OperatorContext,
        method_str: &str,
        target_url: &str,
        idem_key: &str,
        err: &AppError,
    ) {
        if let Err(log_err) = self
            .flow_repo
            .record_action_log(
                &ctx.tenant_id,
                NewActionLogInput {
                    run_id: Some(ctx.run_id),
                    node_id: Some(ctx.node_id.clone()),
                    source_kind: ActionSourceKind::Flow,
                    action_kind: ActionKind::Http,
                    target: String::from(target_url),
                    payload: redacted_request_summary(method_str, &ctx.config),
                    status: ActionLogStatus::Failed,
                    idempotency_key: String::from(idem_key),
                    error_message: Some(err.client_message()),
                },
            )
            .await
        {
            tracing::warn!(error = %log_err, "failed to record FAILED HTTP ax_action_log");
        }
    }
}

async fn validate_dry_run_url<U: OutboundUrlGuardPort + ?Sized>(
    guard: &U,
    url: &str,
    allow_private: bool,
) -> Result<(), AppError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid url '{url}': {err}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unsupported URL scheme '{}'", parsed.scheme()),
        ));
    }
    let host_str = parsed
        .host_str()
        .unwrap_or_default()
        .trim_matches(['[', ']']);
    if host_str.parse::<IpAddr>().is_ok()
        || host_str.eq_ignore_ascii_case("localhost")
        || host_str.ends_with(".localhost")
    {
        let _ = guard.check_outbound_url(url, allow_private).await?;
    }
    Ok(())
}

fn parse_http_method(method: &str) -> Result<reqwest::Method, AppError> {
    match method {
        "GET" => Ok(reqwest::Method::GET),
        "POST" => Ok(reqwest::Method::POST),
        "PUT" => Ok(reqwest::Method::PUT),
        "PATCH" => Ok(reqwest::Method::PATCH),
        "DELETE" => Ok(reqwest::Method::DELETE),
        other => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unsupported HTTP method '{other}'"),
        )),
    }
}

fn apply_secret_headers(
    mut builder: reqwest::RequestBuilder,
    secret_val: &serde_json::Value,
) -> reqwest::RequestBuilder {
    let Some(obj) = secret_val.as_object() else {
        return builder;
    };
    if let Some(tok) = obj
        .get("bearer")
        .or_else(|| obj.get("token"))
        .and_then(serde_json::Value::as_str)
    {
        builder = builder.header("Authorization", format!("Bearer {tok}"));
    } else if let Some(auth) = obj.get("authorization").and_then(serde_json::Value::as_str) {
        builder = builder.header("Authorization", auth);
    } else if let Some(api_key) = obj.get("api_key").and_then(serde_json::Value::as_str) {
        let hdr = obj
            .get("header_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("X-API-Key");
        builder = builder.header(hdr, api_key);
    } else if let (Some(u), Some(p)) = (
        obj.get("username").and_then(serde_json::Value::as_str),
        obj.get("password").and_then(serde_json::Value::as_str),
    ) {
        let b64 = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}").as_bytes());
        builder = builder.header("Authorization", format!("Basic {b64}"));
    }
    builder
}

async fn read_capped_body(resp: &mut reqwest::Response) -> Result<serde_json::Value, AppError> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|err| AppError::Upstream(format!("read HTTP response chunk failed: {err}")))?
    {
        let remaining = MAX_RESPONSE_BODY_BYTES.saturating_sub(buf.len());
        if remaining == 0 {
            break;
        }
        let take_len = chunk.len().min(remaining);
        if let Some(slice) = chunk.get(..take_len) {
            buf.extend_from_slice(slice);
        }
        if buf.len() >= MAX_RESPONSE_BODY_BYTES {
            break;
        }
    }

    if buf.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&buf) {
        return Ok(json_val);
    }
    Ok(serde_json::Value::String(
        String::from_utf8_lossy(&buf).into_owned(),
    ))
}

fn is_expected_status(config: &serde_json::Value, status_code: u16) -> bool {
    if let Some(arr) = config
        .get("expected_status")
        .and_then(serde_json::Value::as_array)
        && !arr.is_empty()
    {
        let code_u64 = u64::from(status_code);
        return arr.iter().any(|v| v.as_u64() == Some(code_u64));
    }
    (200..300).contains(&status_code)
}

fn redacted_request_summary(method: &str, config: &serde_json::Value) -> serde_json::Value {
    let header_keys: Vec<String> = config
        .get("headers")
        .and_then(serde_json::Value::as_object)
        .map_or_else(Vec::new, |m| m.keys().cloned().collect());
    serde_json::json!({
        "method": method,
        "header_keys": header_keys,
        "has_credential": config.get("credential_id").is_some_and(|v| !v.is_null()),
    })
}

impl<F, U, C> Operator for HttpCallOperator<F, U, C>
where
    F: FlowRepository + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::HttpCall
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn config_schema(&self) -> serde_json::Value {
        config_schema()
    }

    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async move { self.run(ctx).await })
    }
}
