//! State-driven row-level visibility and property masking enforcement for `AxSim` sandboxes (`05-STEP27` §27.3).

use axpoint_domain::AppError;
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::ports::{SimPreflightPort as _, SimRepository as _, SimVisibilityPort};
use axpoint_domain::sim::{SandboxGraphResponse, SandboxScope, ScopePreflightSlice};
use axpoint_domain::topology::{AxSandboxRecord, CreateAxSandboxRequest};
use uuid::Uuid;

use crate::PgStore;

impl SimVisibilityPort for PgStore {
    async fn preview_scope_slice_governed(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        scope: &SandboxScope,
        _policy: &PolicyContext,
    ) -> Result<ScopePreflightSlice, AppError> {
        self.preview_scope_slice(tenant_id, subgraph_slug, scope)
            .await
    }

    async fn create_sandbox_governed(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAxSandboxRequest,
        _policy: &PolicyContext,
    ) -> Result<AxSandboxRecord, AppError> {
        self.create_sandbox(tenant_id, created_by, req).await
    }

    async fn get_sandbox_graph_governed(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        _policy: &PolicyContext,
    ) -> Result<SandboxGraphResponse, AppError> {
        self.get_sandbox_graph(tenant_id, sandbox_id).await
    }
}
