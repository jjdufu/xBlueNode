//! HTTP client adapter (`HttpSidecarClient`) implementing [`SidecarPort`] against `axpoint-parser`.

use std::collections::BTreeMap;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{
    CommitStagingRequest, ConnectionTestResult, ConnectorSpecDto, DiscoveredObject,
    LakeMaintenanceRequest, LakeMaintenanceResult, LakePreviewRequest, LakeSnapshotInfo,
    PreviewResponse, SidecarConnectorRequest, SidecarPreviewRequest, SidecarSyncRequest,
    SidecarSyncResult, UnifiedParseResponse,
};
use axpoint_domain::ports::SidecarPort;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::telemetry::outbound_traceparent;

#[derive(Debug, Deserialize)]
struct SidecarErrorEnvelope {
    #[serde(default)]
    error_code: String,
    #[serde(default)]
    message: String,
}

/// Internal HTTP client communicating with the Python `axpoint-parser` Sidecar.
#[derive(Debug, Clone)]
pub(crate) struct HttpSidecarClient {
    base_url: String,
    http: reqwest::Client,
}

impl HttpSidecarClient {
    /// Creates a new [`HttpSidecarClient`] targeting `base_url` using a preconfigured [`reqwest::Client`].
    pub(crate) fn new(base_url: &str, http: reqwest::Client) -> Self {
        let normalized = base_url.trim_end_matches('/').to_owned();
        Self {
            base_url: normalized,
            http,
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn decode_response<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, AppError> {
        let status = resp.status();
        if status.is_success() {
            return resp
                .json::<T>()
                .await
                .map_err(|err| AppError::Parser(format!("invalid sidecar JSON payload: {err}")));
        }

        let raw_text = resp.text().await.unwrap_or_default();
        Err(map_sidecar_error(status, &raw_text))
    }
}

fn map_sidecar_error(status: StatusCode, raw_body: &str) -> AppError {
    let parsed = serde_json::from_str::<SidecarErrorEnvelope>(raw_body).ok();
    let code = parsed.as_ref().map_or_default(|e| e.error_code.trim());
    let msg = parsed
        .as_ref()
        .map(|e| e.message.trim())
        .filter(|m| !m.is_empty())
        .map_or_else(|| raw_body.trim().to_owned(), str::to_owned);

    match status {
        StatusCode::BAD_REQUEST => match code {
            "SCHEMA_INVALID" => AppError::bad("SCHEMA_INVALID", msg),
            "SSRF_BLOCKED" => AppError::bad("SSRF_BLOCKED", msg),
            _ => AppError::bad("INVALID_ARGUMENT", msg),
        },
        StatusCode::UNAUTHORIZED => AppError::Unauthorized("SIDECAR_UNAUTHORIZED"),
        StatusCode::FORBIDDEN => match code {
            "SSRF_BLOCKED" => AppError::Forbidden("SSRF_BLOCKED"),
            _ => AppError::Forbidden("PERMISSION_DENIED"),
        },
        StatusCode::NOT_FOUND => AppError::NotFound("RESOURCE_NOT_FOUND"),
        StatusCode::CONFLICT => AppError::Conflict("CONFLICT"),
        StatusCode::PAYLOAD_TOO_LARGE => AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => AppError::Unsupported("UNSUPPORTED_FORMAT"),
        StatusCode::UNPROCESSABLE_ENTITY => match code {
            "SCHEMA_BREAKING_CHANGE" => AppError::Unprocessable {
                code: "SCHEMA_BREAKING_CHANGE",
                message: msg,
            },
            "QUALITY_BLOCKED" => AppError::Unprocessable {
                code: "QUALITY_BLOCKED",
                message: msg,
            },
            "CONNECTOR_DRIVER_UNAVAILABLE" => AppError::Unprocessable {
                code: "CONNECTOR_DRIVER_UNAVAILABLE",
                message: msg,
            },
            _ => AppError::Unprocessable {
                code: "PARSE_FAILED",
                message: msg,
            },
        },
        StatusCode::TOO_MANY_REQUESTS => AppError::RateLimited,
        StatusCode::GATEWAY_TIMEOUT => AppError::Timeout("SIDECAR_TIMEOUT"),
        _ => AppError::Parser(format!("sidecar HTTP {status}: {msg}")),
    }
}

impl SidecarPort for HttpSidecarClient {
    async fn parse_unified(
        &self,
        tenant_id: &str,
        timezone: &str,
        filename: &str,
        bytes: Vec<u8>,
        options: &BTreeMap<String, String>,
    ) -> Result<UnifiedParseResponse, AppError> {
        let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_owned());
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if !options.is_empty() {
            let opts_json = serde_json::to_string(options).map_err(|err| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid parse options JSON: {err}"),
                )
            })?;
            form = form.text("options", opts_json);
        }

        let resp = self
            .http
            .post(self.endpoint("/parse/unified"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("X-AxPoint-Timezone", timezone)
            .header("traceparent", outbound_traceparent())
            .multipart(form)
            .send()
            .await
            .map_err(|err| AppError::Parser(format!("sidecar /parse/unified failed: {err}")))?;

        Self::decode_response(resp).await
    }

    async fn list_connector_types(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ConnectorSpecDto>, AppError> {
        let resp = self
            .http
            .get(self.endpoint("/connectors/types"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("traceparent", outbound_traceparent())
            .send()
            .await
            .map_err(|err| AppError::Parser(format!("sidecar /connectors/types failed: {err}")))?;

        Self::decode_response(resp).await
    }

    async fn test_connector(
        &self,
        tenant_id: &str,
        req: &SidecarConnectorRequest,
    ) -> Result<ConnectionTestResult, AppError> {
        let resp = self
            .http
            .post(self.endpoint("/connectors/test"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("traceparent", outbound_traceparent())
            .json(req)
            .send()
            .await
            .map_err(|err| AppError::Parser(format!("sidecar /connectors/test failed: {err}")))?;

        Self::decode_response(resp).await
    }

    async fn discover_connector(
        &self,
        tenant_id: &str,
        req: &SidecarConnectorRequest,
    ) -> Result<Vec<DiscoveredObject>, AppError> {
        let resp = self
            .http
            .post(self.endpoint("/connectors/discover"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("traceparent", outbound_traceparent())
            .json(req)
            .send()
            .await
            .map_err(|err| {
                AppError::Parser(format!("sidecar /connectors/discover failed: {err}"))
            })?;

        Self::decode_response(resp).await
    }

    async fn preview_connector(
        &self,
        tenant_id: &str,
        req: &SidecarPreviewRequest,
    ) -> Result<PreviewResponse, AppError> {
        let resp = self
            .http
            .post(self.endpoint("/connectors/preview"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("traceparent", outbound_traceparent())
            .json(req)
            .send()
            .await
            .map_err(|err| {
                AppError::Parser(format!("sidecar /connectors/preview failed: {err}"))
            })?;

        Self::decode_response(resp).await
    }

    async fn sync_dataset(
        &self,
        tenant_id: &str,
        timezone: &str,
        req: &SidecarSyncRequest,
        timeout_secs: u64,
    ) -> Result<SidecarSyncResult, AppError> {
        let resp = self
            .http
            .post(self.endpoint("/connectors/sync"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("X-AxPoint-Timezone", timezone)
            .header("traceparent", outbound_traceparent())
            .timeout(Duration::from_secs(timeout_secs.max(1)))
            .json(req)
            .send()
            .await
            .map_err(|err| {
                if err.is_timeout() {
                    AppError::Timeout("SIDECAR_SYNC_TIMEOUT")
                } else {
                    AppError::Parser(format!("sidecar /connectors/sync failed: {err}"))
                }
            })?;

        Self::decode_response(resp).await
    }

    async fn commit_staging(
        &self,
        tenant_id: &str,
        timezone: &str,
        req: &CommitStagingRequest,
    ) -> Result<Vec<SidecarSyncResult>, AppError> {
        let resp = self
            .http
            .post(self.endpoint("/lake/commit_staging"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("X-AxPoint-Timezone", timezone)
            .header("traceparent", outbound_traceparent())
            .json(req)
            .send()
            .await
            .map_err(|err| {
                AppError::Parser(format!("sidecar /lake/commit_staging failed: {err}"))
            })?;

        Self::decode_response(resp).await
    }

    async fn preview_lake(
        &self,
        tenant_id: &str,
        req: &LakePreviewRequest,
    ) -> Result<PreviewResponse, AppError> {
        let resp = self
            .http
            .post(self.endpoint("/lake/preview"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("traceparent", outbound_traceparent())
            .json(req)
            .send()
            .await
            .map_err(|err| AppError::Parser(format!("sidecar /lake/preview failed: {err}")))?;

        Self::decode_response(resp).await
    }

    async fn list_snapshots(
        &self,
        tenant_id: &str,
        lake_table: &str,
    ) -> Result<Vec<LakeSnapshotInfo>, AppError> {
        let resp = self
            .http
            .get(self.endpoint("/lake/snapshots"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("traceparent", outbound_traceparent())
            .query(&[("tenant_id", tenant_id), ("lake_table", lake_table)])
            .send()
            .await
            .map_err(|err| AppError::Parser(format!("sidecar /lake/snapshots failed: {err}")))?;

        Self::decode_response(resp).await
    }

    async fn run_maintenance(
        &self,
        tenant_id: &str,
        req: &LakeMaintenanceRequest,
    ) -> Result<LakeMaintenanceResult, AppError> {
        let resp = self
            .http
            .post(self.endpoint("/lake/maintenance"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("traceparent", outbound_traceparent())
            .json(req)
            .send()
            .await
            .map_err(|err| AppError::Parser(format!("sidecar /lake/maintenance failed: {err}")))?;

        Self::decode_response(resp).await
    }

    async fn drop_lake_table(&self, tenant_id: &str, lake_table: &str) -> Result<(), AppError> {
        let resp = self
            .http
            .post(self.endpoint("/lake/drop_table"))
            .header("X-AxPoint-Tenant", tenant_id)
            .header("traceparent", outbound_traceparent())
            .json(&serde_json::json!({
                "tenant_id": tenant_id,
                "lake_table": lake_table,
            }))
            .send()
            .await
            .map_err(|err| AppError::Parser(format!("sidecar /lake/drop_table failed: {err}")))?;

        let _: serde_json::Value = Self::decode_response(resp).await?;
        Ok(())
    }
}
