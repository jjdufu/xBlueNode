//! OpenAI-compatible LLM and multi-dimension Embedding adapter for `AxPoint`.

pub mod cassette;
pub mod schema_draft;

use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use axpoint_domain::{AppError, ExtractedTriple, FusedHit};
pub use cassette::{
    CASSETTE_ENV_VAR, Cassette, CassetteEntry, CassetteMode, DEFAULT_OFFLINE_EMBED_DIM,
    EVAL_RECORD_ENV_VAR, compute_prompt_sha256, deterministic_embedding, sha256_hex,
};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, StatusCode};
pub use schema_draft::{
    GeneratedSchemaDraft, MAX_SCHEMA_DRAFT_ATTEMPTS, SCHEMA_DRAFT_SYSTEM_PROMPT, SchemaDraftInput,
    build_schema_draft_user_prompt, generate_schema_draft, parse_schema_doc_lenient,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Maximum number of texts sent in a single `/embeddings` HTTP request (`04-STEP03` §03.0).
pub const EMBED_BATCH_CHUNK_SIZE: usize = 64;

/// Default maximum retry attempts for transient HTTP `429` / `503` embedding responses.
pub const DEFAULT_EMBED_MAX_RETRIES: u32 = 3;

/// Default initial exponential backoff duration in milliseconds (`200ms * 2^attempt`).
pub const DEFAULT_EMBED_BASE_BACKOFF_MS: u64 = 200;

/// Upper cap in seconds honored from an upstream `Retry-After` HTTP header.
const MAX_RETRY_AFTER_SECS: f64 = 5.0;

/// Configuration for an OpenAI-compatible chat/reasoning model.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Base URL, e.g., `https://dashscope.aliyuncs.com/compatible-mode/v1`.
    pub api_base: String,
    /// Bearer API key.
    pub api_key: String,
    /// Model identifier, e.g., `qwen-max`.
    pub model: String,
}

/// Configuration for the multi-dimension embedding adapter.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Base URL for embeddings endpoint.
    pub api_base: String,
    /// Bearer API key.
    pub api_key: String,
    /// Embedding model name, e.g., `text-embedding-v4` or `bge-m3`.
    pub model: String,
    /// Optional explicit dimension; `None` enables automatic detection.
    pub configured_dim: Option<usize>,
}

/// Unified client for LLM chat completions and multi-dimension embeddings.
#[derive(Debug, Clone)]
pub struct AiGateway {
    http: Client,
    llm: LlmConfig,
    embed: EmbeddingConfig,
    detected_dim: Arc<AtomicUsize>,
    cassette: Option<Cassette>,
    max_retries: u32,
    base_backoff: Duration,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageResp,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResp {
    content: String,
}

#[derive(Debug, Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedItem>,
}

#[derive(Debug, Deserialize)]
struct EmbedItem {
    #[serde(default)]
    index: Option<usize>,
    embedding: Vec<f32>,
}

/// Runtime-agnostic non-blocking delay future used only when backing off from HTTP `429`/`503`.
struct DelayFuture {
    duration: Duration,
    started: bool,
    state: Arc<Mutex<(bool, Option<Waker>)>>,
}

impl DelayFuture {
    fn new(duration: Duration) -> Self {
        Self {
            duration,
            started: false,
            state: Arc::new(Mutex::new((false, None))),
        }
    }
}

impl Future for DelayFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if this.duration.is_zero() {
            return Poll::Ready(());
        }

        let should_spawn = {
            let mut guard = this.state.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.0 {
                return Poll::Ready(());
            }
            guard.1 = Some(cx.waker().clone());
            if this.started {
                false
            } else {
                this.started = true;
                true
            }
        };

        if should_spawn {
            let state = Arc::clone(&this.state);
            let sleep_for = this.duration;
            std::thread::spawn(move || {
                std::thread::sleep(sleep_for);
                let waker = {
                    let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
                    guard.0 = true;
                    guard.1.take()
                };
                if let Some(w) = waker {
                    w.wake();
                }
            });
        }

        Poll::Pending
    }
}

impl AiGateway {
    /// Creates a new [`AiGateway`] with shared HTTP connection pooling.
    ///
    /// Automatically activates offline cassette replay or live recording when
    /// `AXPOINT_LLM_CASSETTE` (and optionally `AXPOINT_EVAL_RECORD=1`) is set.
    #[must_use]
    pub fn new(llm: LlmConfig, embed: EmbeddingConfig) -> Self {
        let initial_dim = embed.configured_dim.unwrap_or(0);
        let cassette = Cassette::from_env();
        Self {
            http: Client::new(),
            llm,
            embed,
            detected_dim: Arc::new(AtomicUsize::new(initial_dim)),
            cassette,
            max_retries: DEFAULT_EMBED_MAX_RETRIES,
            base_backoff: Duration::from_millis(DEFAULT_EMBED_BASE_BACKOFF_MS),
        }
    }

    /// Creates an [`AiGateway`] bound to an explicit cassette `.jsonl` file or directory.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if `mode` is [`CassetteMode::Replay`] and `cassette_path`
    /// cannot be read or contains malformed `JSONL` records.
    pub fn with_cassette(
        llm: LlmConfig,
        embed: EmbeddingConfig,
        cassette_path: impl AsRef<Path>,
        mode: CassetteMode,
    ) -> Result<Self, AppError> {
        let initial_dim = embed.configured_dim.unwrap_or(0);
        let cassette = Cassette::open(cassette_path.as_ref(), mode)?;
        Ok(Self {
            http: Client::new(),
            llm,
            embed,
            detected_dim: Arc::new(AtomicUsize::new(initial_dim)),
            cassette: Some(cassette),
            max_retries: DEFAULT_EMBED_MAX_RETRIES,
            base_backoff: Duration::from_millis(DEFAULT_EMBED_BASE_BACKOFF_MS),
        })
    }

    /// Configures maximum retry attempts and initial exponential backoff duration for transient HTTP `429`/`503` responses.
    #[must_use]
    pub const fn with_retry_policy(mut self, max_retries: u32, base_backoff: Duration) -> Self {
        self.max_retries = max_retries;
        self.base_backoff = base_backoff;
        self
    }

    /// Returns the configured embedding model name.
    #[must_use]
    pub fn embedding_model_name(&self) -> &str {
        &self.embed.model
    }

    /// Returns the currently known embedding dimension, if detected or configured.
    #[must_use]
    pub fn current_embedding_dim(&self) -> Option<usize> {
        let dim = self.detected_dim.load(Ordering::Relaxed);
        if dim == 0 { None } else { Some(dim) }
    }

    /// Generates embeddings for a batch of texts in chunks of [`EMBED_BATCH_CHUNK_SIZE`] (64)
    /// with exponential backoff retry on HTTP `429 Too Many Requests` (and `503`), recording
    /// the detected vector dimension.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if any chunk HTTP request fails after retries, returns a
    /// non-success status, or yields malformed JSON / inconsistent / empty embeddings.
    pub async fn embed_batch(&self, inputs: &[String]) -> Result<(Vec<Vec<f32>>, usize), AppError> {
        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Replay
        {
            let dim = self
                .embed
                .configured_dim
                .unwrap_or(DEFAULT_OFFLINE_EMBED_DIM);
            if dim == 0 {
                return Err(AppError::Llm(String::from(
                    "embedding configuration returned zero-dimension vector",
                )));
            }
            if inputs.is_empty() {
                self.detected_dim.store(dim, Ordering::Relaxed);
                return Ok((Vec::new(), dim));
            }
            let vectors: Vec<Vec<f32>> = inputs
                .iter()
                .map(|text| deterministic_embedding(text, dim))
                .collect();
            self.detected_dim.store(dim, Ordering::Relaxed);
            return Ok((vectors, dim));
        }

        if inputs.is_empty() {
            let dim = self.detected_dim.load(Ordering::Relaxed);
            return Ok((Vec::new(), dim));
        }

        let url = format!("{}/embeddings", self.embed.api_base.trim_end_matches('/'));
        let mut all_vectors: Vec<Vec<f32>> = Vec::with_capacity(inputs.len());
        let mut batch_dim: Option<usize> = None;

        for chunk in inputs.chunks(EMBED_BATCH_CHUNK_SIZE) {
            let chunk_vectors = self.embed_chunk_with_retry(&url, chunk).await?;
            if chunk_vectors.len() != chunk.len() {
                return Err(AppError::Llm(format!(
                    "embedding API returned {} vectors for chunk of {} inputs",
                    chunk_vectors.len(),
                    chunk.len()
                )));
            }

            for vec in chunk_vectors {
                let vec_dim = vec.len();
                if vec_dim == 0 {
                    return Err(AppError::Llm(String::from(
                        "embedding API returned zero-dimension vector",
                    )));
                }
                if let Some(expected_dim) = batch_dim {
                    if vec_dim != expected_dim {
                        return Err(AppError::Llm(format!(
                            "inconsistent embedding dimensions across batch: expected {expected_dim}, got {vec_dim}"
                        )));
                    }
                } else {
                    batch_dim = Some(vec_dim);
                }
                all_vectors.push(vec);
            }
        }

        let dim = batch_dim
            .ok_or_else(|| AppError::Llm(String::from("embedding API returned empty list")))?;
        self.detected_dim.store(dim, Ordering::Relaxed);
        Ok((all_vectors, dim))
    }

    async fn embed_chunk_with_retry(
        &self,
        url: &str,
        chunk: &[String],
    ) -> Result<Vec<Vec<f32>>, AppError> {
        let body = EmbedRequest {
            model: &self.embed.model,
            input: chunk,
        };

        let mut attempt: u32 = 0;
        loop {
            let resp = self
                .http
                .post(url)
                .bearer_auth(&self.embed.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|err| AppError::Llm(format!("embedding request failed: {err}")))?;

            let status = resp.status();
            if status.is_success() {
                let mut parsed: EmbedResponse = resp
                    .json()
                    .await
                    .map_err(|err| AppError::Llm(format!("invalid embedding JSON: {err}")))?;

                if parsed.data.iter().all(|item| item.index.is_some()) {
                    parsed.data.sort_by_key(|item| item.index.unwrap_or(0));
                }

                let vectors = parsed
                    .data
                    .into_iter()
                    .map(|item| item.embedding)
                    .collect::<Vec<_>>();
                return Ok(vectors);
            }

            let is_retryable = status == StatusCode::TOO_MANY_REQUESTS
                || status == StatusCode::SERVICE_UNAVAILABLE;
            if is_retryable && attempt < self.max_retries {
                let delay = self.compute_backoff_delay(attempt, resp.headers());
                let _ = resp.bytes().await;
                warn!(
                    "embedding chunk returned {status}; retry {}/{} in {delay:?}",
                    attempt.saturating_add(1),
                    self.max_retries
                );
                DelayFuture::new(delay).await;
                attempt = attempt.saturating_add(1);
                continue;
            }

            let text = resp.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!(
                "embedding API returned {status}: {text}"
            )));
        }
    }

    fn compute_backoff_delay(&self, attempt: u32, headers: &HeaderMap) -> Duration {
        let factor = 1_u32.checked_shl(attempt).unwrap_or(u32::MAX);
        let exp_delay = self.base_backoff.saturating_mul(factor);

        let retry_after_delay = headers
            .get(RETRY_AFTER)
            .and_then(|val| val.to_str().ok())
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|secs| secs.is_finite() && *secs >= 0.0)
            .and_then(|secs| Duration::try_from_secs_f64(secs.min(MAX_RETRY_AFTER_SECS)).ok());

        match retry_after_delay {
            Some(header_dur) => exp_delay.max(header_dur),
            None => exp_delay,
        }
    }

    /// Generates an embedding vector for a single string.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if the underlying batch embedding call fails.
    pub async fn embed_one(&self, text: &str) -> Result<(Vec<f32>, usize), AppError> {
        let inputs = vec![text.to_owned()];
        let (mut vecs, dim) = self.embed_batch(&inputs).await?;
        let first = vecs
            .drain(..)
            .next()
            .ok_or_else(|| AppError::Llm(String::from("missing embedding vector")))?;
        Ok((first, dim))
    }

    /// Calls the chat completion API and returns the assistant message text.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if the request fails or returns no choices.
    pub async fn chat(&self, system_prompt: &str, user_prompt: &str) -> Result<String, AppError> {
        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Replay
        {
            return cassette.lookup_chat(system_prompt, user_prompt);
        }

        let content = self.chat_live(system_prompt, user_prompt).await?;

        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Record
        {
            cassette.record_chat(system_prompt, user_prompt, &content)?;
        }

        Ok(content)
    }

    async fn chat_live(&self, system_prompt: &str, user_prompt: &str) -> Result<String, AppError> {
        let url = format!(
            "{}/chat/completions",
            self.llm.api_base.trim_end_matches('/')
        );
        let body = ChatRequest {
            model: &self.llm.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system_prompt,
                },
                ChatMessage {
                    role: "user",
                    content: user_prompt,
                },
            ],
            temperature: 0.1,
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.llm.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| AppError::Llm(format!("chat request failed: {err}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!("chat API returned {status}: {text}")));
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|err| AppError::Llm(format!("invalid chat JSON: {err}")))?;

        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| AppError::Llm(String::from("chat API returned zero choices")))
    }

    /// Extracts knowledge triples from a paragraph via LLM.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if the chat API call fails.
    pub async fn extract_triples(&self, paragraph: &str) -> Result<Vec<ExtractedTriple>, AppError> {
        let system = "You are a precision knowledge graph extractor. Output ONLY a valid JSON array of objects with fields: subject, subject_type, predicate, object, object_type, fact, confidence (0.0-1.0). No markdown fences.";
        let prompt =
            format!("Extract key entity-relation triples from this paragraph:\n\n{paragraph}");
        let raw = self.chat(system, &prompt).await?;
        Ok(parse_json_array_lenient::<ExtractedTriple>(&raw))
    }

    /// Extracts seed entity names from a user question for graph-link recall.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if the chat API call fails.
    pub async fn extract_query_entities(&self, query: &str) -> Result<Vec<String>, AppError> {
        let system = "Extract up to 5 key entity or concept names from the user query. Output ONLY a JSON array of strings.";
        let raw = self.chat(system, query).await?;
        Ok(parse_json_array_lenient::<String>(&raw))
    }

    /// Synthesizes a grounded answer from fused five-way evidence hits.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if the chat API call fails.
    pub async fn synthesize_answer(
        &self,
        query: &str,
        hits: &[FusedHit],
    ) -> Result<String, AppError> {
        let mut context = String::new();
        for (idx, hit) in hits.iter().enumerate() {
            let num = idx.saturating_add(1);
            let _ = write!(
                context,
                "[{num}] 文档《{}》(RRF={:.4}):\n{}\n\n",
                hit.doc_title, hit.rrf_score, hit.content
            );
        }

        let system = "你是 xBlueNode / AxPoint 本体智能助手。请严格基于提供的多路检索证据用中文回答问题，并在关键结论后标注引用编号如 [1]。若证据不足请明确说明。";
        let user = format!("问题：{query}\n\n检索证据：\n{context}");
        self.chat(system, &user).await
    }
}

fn parse_json_array_lenient<T: for<'de> Deserialize<'de>>(raw: &str) -> Vec<T> {
    let trimmed = raw.trim();
    if let Ok(items) = serde_json::from_str::<Vec<T>>(trimmed) {
        return items;
    }
    if let (Some(start), Some(end)) = (trimmed.find('['), trimmed.rfind(']'))
        && let Some(slice) = trimmed.get(start..=end)
        && let Ok(items) = serde_json::from_str::<Vec<T>>(slice)
    {
        return items;
    }
    Vec::new()
}

impl axpoint_domain::ports::AiPort for AiGateway {
    fn embedding_model_name(&self) -> &str {
        Self::embedding_model_name(self)
    }

    fn current_embedding_dim(&self) -> Option<usize> {
        Self::current_embedding_dim(self)
    }

    async fn embed_batch(&self, inputs: &[String]) -> Result<(Vec<Vec<f32>>, usize), AppError> {
        Self::embed_batch(self, inputs).await
    }

    async fn embed_one(&self, text: &str) -> Result<(Vec<f32>, usize), AppError> {
        Self::embed_one(self, text).await
    }

    async fn extract_triples(&self, paragraph: &str) -> Result<Vec<ExtractedTriple>, AppError> {
        Self::extract_triples(self, paragraph).await
    }

    async fn extract_query_entities(&self, query: &str) -> Result<Vec<String>, AppError> {
        Self::extract_query_entities(self, query).await
    }

    async fn synthesize_answer(&self, query: &str, hits: &[FusedHit]) -> Result<String, AppError> {
        Self::synthesize_answer(self, query, hits).await
    }

    async fn chat(&self, system_prompt: &str, user_prompt: &str) -> Result<String, AppError> {
        Self::chat(self, system_prompt, user_prompt).await
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axpoint_domain::split_paragraphs;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    fn dead_configs(configured_dim: Option<usize>) -> (LlmConfig, EmbeddingConfig) {
        (
            LlmConfig {
                api_base: String::from("http://127.0.0.1:9"),
                api_key: String::new(),
                model: String::from("offline-test-model"),
            },
            EmbeddingConfig {
                api_base: String::from("http://127.0.0.1:9"),
                api_key: String::new(),
                model: String::from("offline-embed-model"),
                configured_dim,
            },
        )
    }

    fn live_embed_gateway(addr: std::net::SocketAddr) -> AiGateway {
        let (llm_dead, _) = dead_configs(None);
        let embed_live = EmbeddingConfig {
            api_base: format!("http://{addr}"),
            api_key: String::from("test-embed-key"),
            model: String::from("test-embed-v4"),
            configured_dim: None,
        };
        AiGateway {
            http: Client::new(),
            llm: llm_dead,
            embed: embed_live,
            detected_dim: Arc::new(AtomicUsize::new(0)),
            cassette: None,
            max_retries: DEFAULT_EMBED_MAX_RETRIES,
            base_backoff: Duration::from_millis(5),
        }
    }

    async fn read_http_json_body(socket: &mut TcpStream) -> Result<serde_json::Value, AppError> {
        let mut buf: Vec<u8> = Vec::with_capacity(4096);
        let mut chunk = [0_u8; 2048];
        let mut header_end: Option<usize> = None;
        let mut content_length: usize = 0;

        loop {
            let n = socket
                .read(&mut chunk)
                .await
                .map_err(|e| AppError::Llm(e.to_string()))?;
            if n == 0 {
                break;
            }
            if let Some(read_slice) = chunk.get(..n) {
                buf.extend_from_slice(read_slice);
            }

            if header_end.is_none()
                && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
            {
                let body_start = pos.saturating_add(4);
                header_end = Some(body_start);
                let header_text = String::from_utf8_lossy(buf.get(..pos).unwrap_or_default());
                for line in header_text.lines() {
                    if let Some((name, val)) = line.split_once(':')
                        && name.trim().eq_ignore_ascii_case("content-length")
                    {
                        content_length = val.trim().parse::<usize>().unwrap_or(0);
                    }
                }
            }

            if let Some(start) = header_end
                && buf.len() >= start.saturating_add(content_length)
            {
                let body_bytes = buf
                    .get(start..start.saturating_add(content_length))
                    .unwrap_or_default();
                return serde_json::from_slice(body_bytes)
                    .map_err(|e| AppError::Llm(e.to_string()));
            }
        }

        Err(AppError::Llm(String::from("incomplete HTTP request")))
    }

    fn repo_cassettes_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/cassettes")
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("axpoint_llm_{prefix}_{nanos}"))
    }

    #[tokio::test]
    async fn baseline_cassette_replays_seed_triples_and_query_entities_offline()
    -> Result<(), AppError> {
        let (llm, embed) = dead_configs(None);
        let gateway =
            AiGateway::with_cassette(llm, embed, repo_cassettes_dir(), CassetteMode::Replay)?;

        let seed_content = "华东仓（W-1）负责履约订单 O-1 与 O-2。晶圆一厂又称 Fab-1。";
        let chunks = split_paragraphs(seed_content, 900);
        assert_eq!(chunks.len(), 1);

        let paragraph = chunks
            .first()
            .ok_or_else(|| AppError::Llm(String::from("missing paragraph")))?;
        let triples = gateway.extract_triples(paragraph).await?;
        assert_eq!(triples.len(), 4);

        let t0 = triples
            .first()
            .ok_or_else(|| AppError::Llm(String::from("missing triple 0")))?;
        assert_eq!(t0.subject, "华东仓");
        assert_eq!(t0.subject_type, "WAREHOUSE");
        assert_eq!(t0.predicate, "ALIAS_OF");
        assert_eq!(t0.object, "W-1");
        assert_eq!(t0.object_type, "WAREHOUSE_CODE");
        assert_eq!(t0.fact, "华东仓（W-1）负责履约订单 O-1 与 O-2。");
        assert!((t0.confidence - 0.98).abs() < 1e-6);

        let t1 = triples
            .get(1)
            .ok_or_else(|| AppError::Llm(String::from("missing triple 1")))?;
        assert_eq!(t1.subject, "华东仓");
        assert_eq!(t1.predicate, "FULFILLS");
        assert_eq!(t1.object, "O-1");
        assert_eq!(t1.object_type, "ORDER");
        assert!((t1.confidence - 0.96).abs() < 1e-6);

        let t2 = triples
            .get(2)
            .ok_or_else(|| AppError::Llm(String::from("missing triple 2")))?;
        assert_eq!(t2.subject, "华东仓");
        assert_eq!(t2.predicate, "FULFILLS");
        assert_eq!(t2.object, "O-2");
        assert_eq!(t2.object_type, "ORDER");
        assert!((t2.confidence - 0.96).abs() < 1e-6);

        let t3 = triples
            .get(3)
            .ok_or_else(|| AppError::Llm(String::from("missing triple 3")))?;
        assert_eq!(t3.subject, "晶圆一厂");
        assert_eq!(t3.subject_type, "FACTORY");
        assert_eq!(t3.predicate, "ALIAS_OF");
        assert_eq!(t3.object, "Fab-1");
        assert_eq!(t3.object_type, "FACTORY_CODE");
        assert_eq!(t3.fact, "晶圆一厂又称 Fab-1。");
        assert!((t3.confidence - 0.98).abs() < 1e-6);

        let entities = gateway
            .extract_query_entities("华东仓负责履约哪些订单？")
            .await?;
        assert_eq!(entities, vec!["华东仓", "W-1", "O-1", "O-2"]);

        Ok(())
    }

    #[tokio::test]
    async fn cassette_miss_returns_upstream_llm_error_without_network() -> Result<(), AppError> {
        let (llm, embed) = dead_configs(None);
        let gateway =
            AiGateway::with_cassette(llm, embed, repo_cassettes_dir(), CassetteMode::Replay)?;

        let result = gateway
            .chat("system prompt", "unrecorded user question")
            .await;
        assert!(result.is_err());

        if let Err(err) = result {
            assert_eq!(err.error_code(), "UPSTREAM_LLM");
            assert!(matches!(err, AppError::Llm(_)));
            assert!(err.to_string().contains("cassette miss for prompt hash"));
        }

        Ok(())
    }

    #[tokio::test]
    async fn offline_embed_batch_produces_deterministic_normalized_vectors() -> Result<(), AppError>
    {
        let (llm_default, embed_default) = dead_configs(None);
        let gateway_default = AiGateway::with_cassette(
            llm_default,
            embed_default,
            repo_cassettes_dir(),
            CassetteMode::Replay,
        )?;

        assert_eq!(gateway_default.current_embedding_dim(), None);

        let inputs = vec![
            String::from("华东仓（W-1）"),
            String::from("晶圆一厂（Fab-1）"),
            String::from("华东仓（W-1）"),
        ];
        let (vectors, dim) = gateway_default.embed_batch(&inputs).await?;
        assert_eq!(dim, 64);
        assert_eq!(gateway_default.current_embedding_dim(), Some(64));
        assert_eq!(vectors.len(), 3);

        for vec in &vectors {
            assert_eq!(vec.len(), 64);
            let norm: f32 = vec.iter().map(|&x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0_f32).abs() < 1e-5_f32);
        }

        let v0 = vectors
            .first()
            .ok_or_else(|| AppError::Llm(String::from("missing vector 0")))?;
        let v1 = vectors
            .get(1)
            .ok_or_else(|| AppError::Llm(String::from("missing vector 1")))?;
        let v2 = vectors
            .get(2)
            .ok_or_else(|| AppError::Llm(String::from("missing vector 2")))?;

        assert_eq!(v0, v2);
        assert_ne!(v0, v1);

        let cosine_same: f32 = v0.iter().zip(v2.iter()).map(|(&a, &b)| a * b).sum();
        assert!((cosine_same - 1.0_f32).abs() < 1e-5_f32);

        let (llm_custom, embed_custom) = dead_configs(Some(128));
        let gateway_custom = AiGateway::with_cassette(
            llm_custom,
            embed_custom,
            repo_cassettes_dir(),
            CassetteMode::Replay,
        )?;
        let (single_vec, single_dim) = gateway_custom.embed_one("华东仓（W-1）").await?;
        assert_eq!(single_dim, 128);
        assert_eq!(single_vec.len(), 128);

        Ok(())
    }

    #[tokio::test]
    async fn cassette_directory_lexicographical_merge_and_user_fallback() -> Result<(), AppError> {
        let dir = unique_temp_dir("merge");
        fs::create_dir_all(&dir).map_err(|e| AppError::Llm(e.to_string()))?;

        let p_hash = compute_prompt_sha256("sys", "q1");
        let u_only_hash = sha256_hex("fallback-question");

        let file_a = dir.join("01_first.jsonl");
        let file_b = dir.join("02_second.jsonl");
        fs::write(
            &file_a,
            format!(
                "{{\"prompt_sha256\":\"{p_hash}\",\"kind\":\"chat\",\"response\":\"old\"}}\n\
                 {{\"user_sha256\":\"{u_only_hash}\",\"kind\":\"chat\",\"response\":\"via-user-hash\"}}\n"
            ),
        )
        .map_err(|e| AppError::Llm(e.to_string()))?;
        fs::write(
            &file_b,
            format!("{{\"prompt_sha256\":\"{p_hash}\",\"kind\":\"chat\",\"response\":\"new\"}}\n"),
        )
        .map_err(|e| AppError::Llm(e.to_string()))?;

        let (llm, embed) = dead_configs(None);
        let gateway = AiGateway::with_cassette(llm, embed, &dir, CassetteMode::Replay)?;

        let overridden = gateway.chat("sys", "q1").await?;
        assert_eq!(overridden, "new");

        let fallback = gateway
            .chat("any-other-system", "fallback-question")
            .await?;
        assert_eq!(fallback, "via-user-hash");

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[tokio::test]
    async fn cassette_record_mode_appends_recorded_jsonl_and_replays() -> Result<(), AppError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| AppError::Llm(e.to_string()))?;

        let server = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0_u8; 2048];
                let _ = socket.read(&mut buf).await;
                let payload = r#"{"choices":[{"message":{"content":"recorded-answer"}}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            }
        });

        let dir = unique_temp_dir("record");
        fs::create_dir_all(&dir).map_err(|e| AppError::Llm(e.to_string()))?;

        let llm_live = LlmConfig {
            api_base: format!("http://{addr}"),
            api_key: String::from("test-key"),
            model: String::from("test-model"),
        };
        let (_, embed_dead) = dead_configs(None);

        let recorder = AiGateway::with_cassette(llm_live, embed_dead, &dir, CassetteMode::Record)?;
        let live_ans = recorder.chat("record-sys", "record-user").await?;
        assert_eq!(live_ans, "recorded-answer");
        let _ = server.await;

        assert!(dir.join("recorded.jsonl").is_file());

        let (llm_dead, embed_dead2) = dead_configs(None);
        let replayer = AiGateway::with_cassette(llm_dead, embed_dead2, &dir, CassetteMode::Replay)?;
        let offline_ans = replayer.chat("record-sys", "record-user").await?;
        assert_eq!(offline_ans, "recorded-answer");

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[tokio::test]
    async fn live_embed_batch_chunks_150_inputs_into_three_64_item_requests() -> Result<(), AppError>
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| AppError::Llm(e.to_string()))?;

        let server = tokio::spawn(async move {
            let mut observed_sizes: Vec<usize> = Vec::new();
            for _ in 0..3 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let Ok(req_json) = read_http_json_body(&mut socket).await else {
                    break;
                };
                let items = req_json
                    .get("input")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                observed_sizes.push(items.len());

                // Encode the original global item number parsed from `"item_<idx>"` into the vector
                // so the caller can assert exact 0..150 order preservation across chunks.
                let mut data_entries = Vec::with_capacity(items.len());
                for (local_idx, item_val) in items.iter().enumerate() {
                    let s = item_val.as_str().unwrap_or_default();
                    let seq_num = s
                        .strip_prefix("item_")
                        .and_then(|num| num.parse::<u16>().ok())
                        .unwrap_or(0);
                    let marker = f32::from(seq_num);
                    data_entries.push(serde_json::json!({
                        "index": local_idx,
                        "embedding": [marker, marker + 0.5_f32, 1.0_f32, 0.0_f32]
                    }));
                }

                let payload = serde_json::json!({ "data": data_entries }).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            }
            observed_sizes
        });

        let gateway = live_embed_gateway(addr);
        let inputs: Vec<String> = (0..150_u16).map(|i| format!("item_{i}")).collect();
        let (vectors, dim) = gateway.embed_batch(&inputs).await?;

        let observed_sizes = server.await.map_err(|e| AppError::Llm(e.to_string()))?;
        assert_eq!(observed_sizes, vec![64, 64, 22]);
        assert_eq!(dim, 4);
        assert_eq!(gateway.current_embedding_dim(), Some(4));
        assert_eq!(vectors.len(), 150);

        for (idx, vec) in vectors.iter().enumerate() {
            let expected_marker = u16::try_from(idx).map_or(-1.0_f32, f32::from);
            let first = vec.first().copied().unwrap_or(-1.0_f32);
            assert!(
                (first - expected_marker).abs() < 1e-5_f32,
                "vector {idx} out of order: got {first}, expected {expected_marker}"
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn live_embed_batch_retries_http_429_and_errors_after_max_retries() -> Result<(), AppError>
    {
        // Subcase 1: first 2 attempts return HTTP 429, 3rd attempt returns 200 OK -> succeeds.
        let listener_ok = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        let addr_ok = listener_ok
            .local_addr()
            .map_err(|e| AppError::Llm(e.to_string()))?;

        let server_ok = tokio::spawn(async move {
            let mut calls: usize = 0;
            for attempt in 0..3_usize {
                let Ok((mut socket, _)) = listener_ok.accept().await else {
                    break;
                };
                let _ = read_http_json_body(&mut socket).await;
                calls = calls.saturating_add(1);

                if attempt < 2 {
                    let body = r#"{"error":"rate limit exceeded"}"#;
                    let resp = format!(
                        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0.005\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                } else {
                    let body = r#"{"data":[{"index":0,"embedding":[0.25,0.75,0.5]}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                }
            }
            calls
        });

        let gateway_ok = live_embed_gateway(addr_ok).with_retry_policy(3, Duration::from_millis(5));
        let (vecs, dim) = gateway_ok
            .embed_batch(&[String::from("retry-target")])
            .await?;
        let total_calls_ok = server_ok.await.map_err(|e| AppError::Llm(e.to_string()))?;
        assert_eq!(total_calls_ok, 3);
        assert_eq!(dim, 3);
        assert_eq!(vecs.len(), 1);

        // Subcase 2: all 4 attempts (1 initial + 3 retries) return HTTP 429 -> returns Err(AppError::Llm).
        let listener_fail = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        let addr_fail = listener_fail
            .local_addr()
            .map_err(|e| AppError::Llm(e.to_string()))?;

        let server_fail = tokio::spawn(async move {
            let mut calls: usize = 0;
            for _ in 0..4_usize {
                let Ok((mut socket, _)) = listener_fail.accept().await else {
                    break;
                };
                let _ = read_http_json_body(&mut socket).await;
                calls = calls.saturating_add(1);
                let body = r#"{"error":"persistent 429"}"#;
                let resp = format!(
                    "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            }
            calls
        });

        let gateway_fail =
            live_embed_gateway(addr_fail).with_retry_policy(3, Duration::from_millis(2));
        let result_fail = gateway_fail
            .embed_batch(&[String::from("exhaust-retries")])
            .await;
        let total_calls_fail = server_fail
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        assert_eq!(total_calls_fail, 4);
        assert!(matches!(result_fail, Err(AppError::Llm(_))));
        if let Err(err) = result_fail {
            assert_eq!(err.error_code(), "UPSTREAM_LLM");
            assert!(err.to_string().contains("429"));
        }

        Ok(())
    }
}
