//! OpenAI-compatible LLM and multi-dimension Embedding adapter for `AxPoint`.

pub mod cassette;

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axpoint_domain::{AppError, ExtractedTriple, FusedHit};
pub use cassette::{
    CASSETTE_ENV_VAR, Cassette, CassetteEntry, CassetteMode, DEFAULT_OFFLINE_EMBED_DIM,
    EVAL_RECORD_ENV_VAR, compute_prompt_sha256, deterministic_embedding, sha256_hex,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};

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
    embedding: Vec<f32>,
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
        })
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

    /// Generates embeddings for a batch of texts and records the detected dimension.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if the HTTP request fails, returns a non-success
    /// status, or yields malformed JSON / empty embeddings.
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
        let body = EmbedRequest {
            model: &self.embed.model,
            input: inputs,
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.embed.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| AppError::Llm(format!("embedding request failed: {err}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!(
                "embedding API returned {status}: {text}"
            )));
        }

        let parsed: EmbedResponse = resp
            .json()
            .await
            .map_err(|err| AppError::Llm(format!("invalid embedding JSON: {err}")))?;

        let vectors: Vec<Vec<f32>> = parsed.data.into_iter().map(|d| d.embedding).collect();
        let dim = vectors
            .first()
            .map(Vec::len)
            .ok_or_else(|| AppError::Llm(String::from("embedding API returned empty list")))?;

        if dim == 0 {
            return Err(AppError::Llm(String::from(
                "embedding API returned zero-dimension vector",
            )));
        }

        self.detected_dim.store(dim, Ordering::Relaxed);
        Ok((vectors, dim))
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
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axpoint_domain::split_paragraphs;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

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
}
