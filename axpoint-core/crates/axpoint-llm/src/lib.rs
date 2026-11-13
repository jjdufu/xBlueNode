//! OpenAI-compatible LLM and multi-dimension Embedding adapter for `AxPoint`.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axpoint_domain::{AppError, ExtractedTriple, FusedHit};
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
    #[must_use]
    pub fn new(llm: LlmConfig, embed: EmbeddingConfig) -> Self {
        let initial_dim = embed.configured_dim.unwrap_or(0);
        Self {
            http: Client::new(),
            llm,
            embed,
            detected_dim: Arc::new(AtomicUsize::new(initial_dim)),
        }
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
