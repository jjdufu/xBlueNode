//! `AxPoint` Core HTTP server (`Axum`) for hypergraph ingestion, five-way RAG, and `xBlueNode` UI hosting.

mod routes;
mod state;

use std::env;
use std::net::SocketAddr;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_llm::{AiGateway, EmbeddingConfig, LlmConfig};
use sqlx::postgres::PgPoolOptions;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::state::AppState;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            env::var("RUST_LOG").unwrap_or_else(|_| String::from("axpoint_server=info,info")),
        )
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| AppError::Config(String::from("DATABASE_URL must be set")))?;
    let bind_addr: SocketAddr = env::var("AXPOINT_BIND_ADDR")
        .unwrap_or_else(|_| String::from("0.0.0.0:9995"))
        .parse()
        .map_err(|err| AppError::Config(format!("invalid AXPOINT_BIND_ADDR: {err}")))?;
    let parser_base_url =
        env::var("PARSER_BASE_URL").unwrap_or_else(|_| String::from("http://127.0.0.1:8001"));
    let web_dist_dir =
        env::var("WEB_DIST_DIR").unwrap_or_else(|_| String::from("../xbluenode-web/dist"));

    let llm_cfg = LlmConfig {
        api_base: env::var("LLM_API_BASE")
            .unwrap_or_else(|_| String::from("https://dashscope.aliyuncs.com/compatible-mode/v1")),
        api_key: env::var("LLM_API_KEY").unwrap_or_default(),
        model: env::var("LLM_MODEL").unwrap_or_else(|_| String::from("qwen-max")),
    };

    let configured_dim = env::var("EMBED_DIM")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.trim()
                .parse::<usize>()
                .map_err(|err| AppError::Config(format!("invalid EMBED_DIM: {err}")))
        })
        .transpose()?;

    let embed_cfg = EmbeddingConfig {
        api_base: env::var("EMBED_API_BASE").unwrap_or_else(|_| llm_cfg.api_base.clone()),
        api_key: env::var("EMBED_API_KEY").unwrap_or_else(|_| llm_cfg.api_key.clone()),
        model: env::var("EMBED_MODEL").unwrap_or_else(|_| String::from("text-embedding-v4")),
        configured_dim,
    };

    let pool = PgPoolOptions::new()
        .max_connections(16)
        .connect(&database_url)
        .await
        .map_err(|err| AppError::Database(format!("postgres connection failed: {err}")))?;

    let state = AppState {
        store: PgStore::new(pool),
        ai: AiGateway::new(llm_cfg, embed_cfg),
        parser_base_url,
        http: reqwest::Client::new(),
    };

    let app = routes::api_router(state)
        .fallback_service(ServeDir::new(web_dist_dir))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    info!("AxPoint Core listening on http://{bind_addr}");
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|err| AppError::Config(format!("failed to bind {bind_addr}: {err}")))?;

    axum::serve(listener, app)
        .await
        .map_err(|err| AppError::Config(format!("server error: {err}")))?;

    Ok(())
}
