//! Offline `JSONL` cassette replay, live evaluation recording, and deterministic embeddings.

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use axpoint_domain::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tracing::warn;

/// Environment variable pointing to an offline cassette `.jsonl` file or directory.
pub const CASSETTE_ENV_VAR: &str = "AXPOINT_LLM_CASSETTE";

/// Environment variable enabling live recording (`"1"`) into [`CASSETTE_ENV_VAR`].
pub const EVAL_RECORD_ENV_VAR: &str = "AXPOINT_EVAL_RECORD";

/// Default vector dimension used for deterministic offline embeddings when unconfigured.
pub const DEFAULT_OFFLINE_EMBED_DIM: usize = 64;

/// Operating mode when an offline/recording cassette path is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CassetteMode {
    /// Offline replay only: never performs network requests; misses return `UPSTREAM_LLM`.
    Replay,
    /// Live recording (`AXPOINT_EVAL_RECORD=1`): calls upstream LLM and appends `JSONL` entries.
    Record,
}

/// Single `JSONL` record stored in a cassette file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CassetteEntry {
    /// Lowercase hex `SHA-256` digest of `"{system_prompt}\n---\n{user_prompt}"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_sha256: Option<String>,
    /// Optional lowercase hex `SHA-256` digest of `"{user_prompt}"` for fallback matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_sha256: Option<String>,
    /// Entry category (defaults to `"chat"`).
    #[serde(default = "default_entry_kind")]
    pub kind: String,
    /// Recorded assistant text response.
    pub response: String,
}

fn default_entry_kind() -> String {
    String::from("chat")
}

/// Computes the lowercase hex-encoded `SHA-256` digest of `input`.
#[must_use]
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Computes the canonical prompt hash `hex(SHA-256("{system_prompt}\n---\n{user_prompt}"))`.
#[must_use]
pub fn compute_prompt_sha256(system_prompt: &str, user_prompt: &str) -> String {
    let combined = format!("{system_prompt}\n---\n{user_prompt}");
    sha256_hex(&combined)
}

/// Generates a deterministic L2-normalized unit `f32` vector of length `dim` from `SHA-256` blocks.
///
/// Identical `text` inputs always yield identical unit vectors (cosine similarity `1.0`),
/// using `f32::from(i16)` without lossy `as` casts.
#[must_use]
pub fn deterministic_embedding(text: &str, dim: usize) -> Vec<f32> {
    if dim == 0 {
        return Vec::new();
    }

    let mut values = Vec::with_capacity(dim);
    let mut block_counter: u32 = 0;

    while values.len() < dim {
        let mut hasher = Sha256::new();
        hasher.update(block_counter.to_le_bytes());
        hasher.update(b":");
        hasher.update(text.as_bytes());
        let digest = hasher.finalize();

        for &pair in digest.as_chunks::<2>().0 {
            if values.len() >= dim {
                break;
            }
            let sample = i16::from_le_bytes(pair);
            values.push(f32::from(sample) / 32_768.0_f32);
        }
        block_counter = block_counter.wrapping_add(1);
    }

    let norm_sq: f32 = values.iter().map(|&v| v * v).sum();
    let norm = norm_sq.sqrt();

    if norm <= f32::EPSILON {
        let mut fallback = vec![0.0_f32; dim];
        if let Some(first) = fallback.first_mut() {
            *first = 1.0_f32;
        }
        return fallback;
    }

    for v in &mut values {
        *v /= norm;
    }
    values
}

#[derive(Debug)]
struct CassetteState {
    by_prompt_sha256: HashMap<String, String>,
    by_user_sha256: HashMap<String, String>,
    load_error: Option<String>,
}

/// Thread-safe offline replay and recording cassette store.
#[derive(Debug, Clone)]
pub struct Cassette {
    mode: CassetteMode,
    record_target: PathBuf,
    state: Arc<Mutex<CassetteState>>,
}

impl Cassette {
    /// Opens a cassette from a `.jsonl` file path or a directory of `*.jsonl` files.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if `mode` is [`CassetteMode::Replay`] and `path` does
    /// not exist, cannot be read, or contains invalid `JSONL` records.
    pub fn open(path: &Path, mode: CassetteMode) -> Result<Self, AppError> {
        let record_target = resolve_record_target(path);
        let mut by_prompt_sha256 = HashMap::new();
        let mut by_user_sha256 = HashMap::new();

        if path.exists() {
            if path.is_dir() {
                let mut files = Vec::new();
                collect_jsonl_files(path, &mut files)?;
                for file in files {
                    load_jsonl_file(&file, &mut by_prompt_sha256, &mut by_user_sha256)?;
                }
            } else {
                load_jsonl_file(path, &mut by_prompt_sha256, &mut by_user_sha256)?;
            }
        } else if mode == CassetteMode::Replay {
            return Err(AppError::Llm(format!(
                "cassette path does not exist: {}",
                path.display()
            )));
        }

        Ok(Self {
            mode,
            record_target,
            state: Arc::new(Mutex::new(CassetteState {
                by_prompt_sha256,
                by_user_sha256,
                load_error: None,
            })),
        })
    }

    /// Constructs a [`Cassette`] from `AXPOINT_LLM_CASSETTE` and `AXPOINT_EVAL_RECORD` if set.
    ///
    /// If loading fails during environment initialization, logs a warning and retains an
    /// offline [`Cassette`] so subsequent calls fail deterministically without network access.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let raw_path = env::var(CASSETTE_ENV_VAR).ok()?;
        let trimmed = raw_path.trim();
        if trimmed.is_empty() {
            return None;
        }

        let is_record = matches!(env::var(EVAL_RECORD_ENV_VAR), Ok(val) if val.trim() == "1");
        let mode = if is_record {
            CassetteMode::Record
        } else {
            CassetteMode::Replay
        };

        let path = Path::new(trimmed);
        match Self::open(path, mode) {
            Ok(cassette) => Some(cassette),
            Err(err) => {
                warn!("failed to preload cassette from {trimmed}: {err}");
                Some(Self {
                    mode,
                    record_target: resolve_record_target(path),
                    state: Arc::new(Mutex::new(CassetteState {
                        by_prompt_sha256: HashMap::new(),
                        by_user_sha256: HashMap::new(),
                        load_error: Some(err.to_string()),
                    })),
                })
            }
        }
    }

    /// Returns the active [`CassetteMode`].
    #[must_use]
    pub const fn mode(&self) -> CassetteMode {
        self.mode
    }

    /// Looks up a recorded chat response by `prompt_sha256` with fallback to `user_sha256`.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] (`UPSTREAM_LLM`) on cassette miss.
    pub fn lookup_chat(&self, system_prompt: &str, user_prompt: &str) -> Result<String, AppError> {
        let prompt_sha256 = compute_prompt_sha256(system_prompt, user_prompt);
        let user_sha256 = sha256_hex(user_prompt);

        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(resp) = guard.by_prompt_sha256.get(&prompt_sha256) {
            return Ok(resp.clone());
        }
        if let Some(resp) = guard.by_user_sha256.get(&user_sha256) {
            return Ok(resp.clone());
        }

        if let Some(reason) = &guard.load_error {
            return Err(AppError::Llm(format!(
                "cassette miss for prompt hash {prompt_sha256} ({reason})"
            )));
        }

        Err(AppError::Llm(format!(
            "cassette miss for prompt hash {prompt_sha256}"
        )))
    }

    /// Appends a live chat completion entry to the target `.jsonl` cassette file.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if serializing or writing to disk fails.
    pub fn record_chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        response: &str,
    ) -> Result<(), AppError> {
        let prompt_sha256 = compute_prompt_sha256(system_prompt, user_prompt);
        let user_sha256 = sha256_hex(user_prompt);

        let entry = CassetteEntry {
            prompt_sha256: Some(prompt_sha256.clone()),
            user_sha256: Some(user_sha256.clone()),
            kind: String::from("chat"),
            response: response.to_owned(),
        };

        let serialized = serde_json::to_string(&entry)
            .map_err(|err| AppError::Llm(format!("failed to serialize cassette entry: {err}")))?;

        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(parent) = self.record_target.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|err| {
                AppError::Llm(format!(
                    "failed to create cassette directory {}: {err}",
                    parent.display()
                ))
            })?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.record_target)
            .map_err(|err| {
                AppError::Llm(format!(
                    "failed to open cassette file {} for append: {err}",
                    self.record_target.display()
                ))
            })?;

        writeln!(file, "{serialized}").map_err(|err| {
            AppError::Llm(format!(
                "failed to append to cassette file {}: {err}",
                self.record_target.display()
            ))
        })?;
        file.flush().map_err(|err| {
            AppError::Llm(format!(
                "failed to flush cassette file {}: {err}",
                self.record_target.display()
            ))
        })?;

        guard
            .by_prompt_sha256
            .insert(prompt_sha256, response.to_owned());
        guard
            .by_user_sha256
            .insert(user_sha256, response.to_owned());
        Ok(())
    }
}

fn resolve_record_target(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.join("recorded.jsonl");
    }
    if path.is_file() {
        return path.to_path_buf();
    }
    if path.extension().and_then(OsStr::to_str) == Some("jsonl") {
        path.to_path_buf()
    } else {
        path.join("recorded.jsonl")
    }
}

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), AppError> {
    let read_dir = fs::read_dir(dir).map_err(|err| {
        AppError::Llm(format!(
            "failed to read cassette directory {}: {err}",
            dir.display()
        ))
    })?;

    let mut entries = Vec::new();
    for item in read_dir {
        let entry = item.map_err(|err| {
            AppError::Llm(format!(
                "failed to read entry in cassette directory {}: {err}",
                dir.display()
            ))
        })?;
        entries.push(entry.path());
    }

    entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()).then_with(|| a.cmp(b)));

    for entry_path in entries {
        if entry_path.is_dir() {
            collect_jsonl_files(&entry_path, out)?;
        } else if entry_path.is_file()
            && entry_path.extension().and_then(OsStr::to_str) == Some("jsonl")
        {
            out.push(entry_path);
        }
    }

    Ok(())
}

fn load_jsonl_file(
    file_path: &Path,
    by_prompt_sha256: &mut HashMap<String, String>,
    by_user_sha256: &mut HashMap<String, String>,
) -> Result<(), AppError> {
    let raw = fs::read_to_string(file_path).map_err(|err| {
        AppError::Llm(format!(
            "failed to read cassette file {}: {err}",
            file_path.display()
        ))
    })?;

    for (idx, raw_line) in raw.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line_no = idx.saturating_add(1);
        let entry: CassetteEntry = serde_json::from_str(line).map_err(|err| {
            AppError::Llm(format!(
                "invalid cassette JSON at {}:{line_no}: {err}",
                file_path.display()
            ))
        })?;

        if !entry.kind.eq_ignore_ascii_case("chat") {
            continue;
        }

        let prompt_key = entry
            .prompt_sha256
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        let user_key = entry
            .user_sha256
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);

        if prompt_key.is_none() && user_key.is_none() {
            return Err(AppError::Llm(format!(
                "cassette entry at {}:{line_no} must specify prompt_sha256 or user_sha256",
                file_path.display()
            )));
        }

        if let Some(pk) = prompt_key {
            by_prompt_sha256.insert(pk, entry.response.clone());
        }
        if let Some(uk) = user_key {
            by_user_sha256.insert(uk, entry.response);
        }
    }

    Ok(())
}
