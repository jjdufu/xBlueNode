//! Offline `JSONL` cassette replay, live evaluation recording, and deterministic embeddings.

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use axpoint_domain::AppError;
use axpoint_domain::agent::ToolCallSpec;
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

/// Recorded assistant turn (text content and optional tool calls) returned by [`Cassette::lookup_tool_turn`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CassetteTurn {
    /// Recorded assistant text response.
    pub content: String,
    /// Recorded structured tool invocations.
    pub tool_calls: Vec<ToolCallSpec>,
}

/// Single `JSONL` record stored in a cassette file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CassetteEntry {
    /// Optional lowercase hex `SHA-256` digest of the canonical multi-turn request transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_sha256: Option<String>,
    /// Lowercase hex `SHA-256` digest of `"{system_prompt}\n---\n{user_prompt}"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_sha256: Option<String>,
    /// Optional lowercase hex `SHA-256` digest of `"{user_prompt}"` for fallback matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_sha256: Option<String>,
    /// Optional deterministic substring fallback matched against `"{system_prompt}\n---\n{user_prompt}"`
    /// only after `prompt_sha256` and `user_sha256` both miss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_substring: Option<String>,
    /// Substrings that must ALL appear in the canonical request transcript for this entry to match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_all: Vec<String>,
    /// Substrings where NONE may appear in the canonical request transcript for this entry to match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_any: Vec<String>,
    /// Optional structured output `JsonSchemaSpec.name` disambiguating schema-guided turns (`01-STEP15` §15.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema_name: Option<String>,
    /// Entry category (defaults to `"chat"`).
    #[serde(default = "default_entry_kind")]
    pub kind: String,
    /// Recorded assistant text response.
    #[serde(default)]
    pub response: String,
    /// Optional recorded assistant tool calls (`08-STEP09`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallSpec>,
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

#[derive(Debug, Clone)]
struct MatchRule {
    response_schema_name: Option<String>,
    match_all: Vec<String>,
    exclude_any: Vec<String>,
    turn: CassetteTurn,
}

impl MatchRule {
    fn matches(&self, transcript: &str) -> bool {
        !self.match_all.is_empty()
            && self
                .match_all
                .iter()
                .all(|pat| transcript.contains(pat.as_str()))
            && !self
                .exclude_any
                .iter()
                .any(|pat| transcript.contains(pat.as_str()))
    }
}

#[derive(Debug, Default)]
struct CassetteState {
    by_request_sha256: HashMap<String, CassetteTurn>,
    by_prompt_sha256: HashMap<(Option<String>, String), CassetteTurn>,
    by_user_sha256: HashMap<(Option<String>, String), CassetteTurn>,
    by_substring: Vec<(Option<String>, String, CassetteTurn)>,
    by_match_rules: Vec<MatchRule>,
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
        let mut state = CassetteState::default();

        if path.exists() {
            if path.is_dir() {
                let mut files = Vec::new();
                collect_jsonl_files(path, &mut files)?;
                for file in files {
                    load_jsonl_file(&file, &mut state)?;
                }
            } else {
                load_jsonl_file(path, &mut state)?;
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
            state: Arc::new(Mutex::new(state)),
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
                        load_error: Some(err.to_string()),
                        ..CassetteState::default()
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

    /// Looks up a recorded chat response by `prompt_sha256` with fallback to `user_sha256`,
    /// deterministic `match_substring`, and text-only `match_all` + `exclude_any` rules.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] (`UPSTREAM_LLM`) on cassette miss.
    pub fn lookup_chat(&self, system_prompt: &str, user_prompt: &str) -> Result<String, AppError> {
        self.lookup_chat_with_schema(system_prompt, user_prompt, None)
    }

    /// Looks up a recorded chat response, prioritizing entries whose `response_schema_name`
    /// equals `schema_name` before falling back to schema-agnostic (`None`) entries (`01-STEP15`).
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] (`UPSTREAM_LLM`) on cassette miss.
    pub fn lookup_chat_with_schema(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        schema_name: Option<&str>,
    ) -> Result<String, AppError> {
        let prompt_sha256 = compute_prompt_sha256(system_prompt, user_prompt);
        let user_sha256 = sha256_hex(user_prompt);
        let combined = format!("{system_prompt}\n---\n{user_prompt}");
        let norm_schema = normalize_schema_name(schema_name);

        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(ref target) = norm_schema
            && let Some(hit) = search_chat_tier(
                &guard,
                &prompt_sha256,
                &user_sha256,
                &combined,
                Some(target.as_str()),
            )
        {
            return Ok(hit);
        }
        if let Some(hit) = search_chat_tier(&guard, &prompt_sha256, &user_sha256, &combined, None) {
            return Ok(hit);
        }

        Err(miss_error(
            &prompt_sha256,
            &user_sha256,
            guard.load_error.as_deref(),
        ))
    }

    /// Looks up a recorded tool-capable assistant turn in priority order:
    /// (1) `request_sha256`, (2) `match_all` (non-empty) + `exclude_any` against `request_transcript`,
    /// and (3) `prompt_sha256` / `user_sha256` / `match_substring` when no tool result is present
    /// in `request_transcript`.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] (`UPSTREAM_LLM`) on cassette miss.
    pub fn lookup_tool_turn(
        &self,
        request_transcript: &str,
        system_prompt: &str,
        last_user_prompt: &str,
    ) -> Result<CassetteTurn, AppError> {
        self.lookup_tool_turn_with_schema(request_transcript, system_prompt, last_user_prompt, None)
    }

    /// Looks up a recorded tool-capable or schema-guided assistant turn, prioritizing entries
    /// whose `response_schema_name` matches `schema_name` before falling back to `None` entries.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] (`UPSTREAM_LLM`) on cassette miss.
    pub fn lookup_tool_turn_with_schema(
        &self,
        request_transcript: &str,
        system_prompt: &str,
        last_user_prompt: &str,
        schema_name: Option<&str>,
    ) -> Result<CassetteTurn, AppError> {
        let req_sha256 = sha256_hex(request_transcript);
        let norm_schema = normalize_schema_name(schema_name);
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(turn) = guard.by_request_sha256.get(&req_sha256) {
            return Ok(turn.clone());
        }
        if let Some(ref target) = norm_schema
            && let Some(turn) = search_tool_turn_tier(
                &guard,
                request_transcript,
                system_prompt,
                last_user_prompt,
                Some(target.as_str()),
            )
        {
            return Ok(turn);
        }
        if let Some(turn) = search_tool_turn_tier(
            &guard,
            request_transcript,
            system_prompt,
            last_user_prompt,
            None,
        ) {
            return Ok(turn);
        }

        let user_sha256 = sha256_hex(last_user_prompt);
        Err(miss_error(
            &req_sha256,
            &user_sha256,
            guard.load_error.as_deref(),
        ))
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
            request_sha256: None,
            prompt_sha256: Some(prompt_sha256.clone()),
            user_sha256: Some(user_sha256.clone()),
            match_substring: None,
            match_all: Vec::new(),
            exclude_any: Vec::new(),
            response_schema_name: None,
            kind: String::from("chat"),
            response: response.to_owned(),
            tool_calls: Vec::new(),
        };

        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        append_entry_to_disk(&self.record_target, &entry)?;

        let turn = CassetteTurn {
            content: response.to_owned(),
            tool_calls: Vec::new(),
        };
        guard
            .by_prompt_sha256
            .insert((None, prompt_sha256), turn.clone());
        guard.by_user_sha256.insert((None, user_sha256), turn);
        Ok(())
    }

    /// Appends a live tool-capable assistant turn keyed by `request_sha256` to the target `.jsonl` cassette.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if serializing or writing to disk fails.
    pub fn record_tool_turn(
        &self,
        request_transcript: &str,
        turn: &CassetteTurn,
    ) -> Result<(), AppError> {
        self.record_tool_turn_with_schema(request_transcript, turn, None)
    }

    /// Appends a live tool-capable or schema-guided turn keyed by `request_sha256` and optional `schema_name`.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if serializing or writing to disk fails.
    pub fn record_tool_turn_with_schema(
        &self,
        request_transcript: &str,
        turn: &CassetteTurn,
        schema_name: Option<&str>,
    ) -> Result<(), AppError> {
        let req_sha256 = sha256_hex(request_transcript);
        let entry = CassetteEntry {
            request_sha256: Some(req_sha256.clone()),
            prompt_sha256: None,
            user_sha256: None,
            match_substring: None,
            match_all: Vec::new(),
            exclude_any: Vec::new(),
            response_schema_name: normalize_schema_name(schema_name),
            kind: String::from("chat"),
            response: turn.content.clone(),
            tool_calls: turn.tool_calls.clone(),
        };

        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        append_entry_to_disk(&self.record_target, &entry)?;
        guard.by_request_sha256.insert(req_sha256, turn.clone());
        Ok(())
    }
}

fn normalize_schema_name(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn search_chat_tier(
    guard: &CassetteState,
    prompt_sha256: &str,
    user_sha256: &str,
    combined: &str,
    target_schema: Option<&str>,
) -> Option<String> {
    let schema_key = target_schema.map(str::to_owned);
    if let Some(turn) = guard
        .by_prompt_sha256
        .get(&(schema_key.clone(), String::from(prompt_sha256)))
    {
        return Some(turn.content.clone());
    }
    if let Some(turn) = guard
        .by_user_sha256
        .get(&(schema_key, String::from(user_sha256)))
    {
        return Some(turn.content.clone());
    }
    for (entry_schema, pat, turn) in &guard.by_substring {
        if entry_schema.as_deref() == target_schema && combined.contains(pat.as_str()) {
            return Some(turn.content.clone());
        }
    }
    for rule in &guard.by_match_rules {
        if rule.response_schema_name.as_deref() == target_schema
            && rule.turn.tool_calls.is_empty()
            && rule.matches(combined)
        {
            return Some(rule.turn.content.clone());
        }
    }
    None
}

fn search_tool_turn_tier(
    guard: &CassetteState,
    request_transcript: &str,
    system_prompt: &str,
    last_user_prompt: &str,
    target_schema: Option<&str>,
) -> Option<CassetteTurn> {
    for rule in &guard.by_match_rules {
        if rule.response_schema_name.as_deref() == target_schema && rule.matches(request_transcript)
        {
            return Some(rule.turn.clone());
        }
    }
    if !transcript_has_tool_result(request_transcript) {
        let schema_key = target_schema.map(str::to_owned);
        let prompt_sha256 = compute_prompt_sha256(system_prompt, last_user_prompt);
        let user_sha256 = sha256_hex(last_user_prompt);
        if let Some(turn) = guard
            .by_prompt_sha256
            .get(&(schema_key.clone(), prompt_sha256))
        {
            return Some(turn.clone());
        }
        if let Some(turn) = guard.by_user_sha256.get(&(schema_key, user_sha256)) {
            return Some(turn.clone());
        }
        let combined = format!("{system_prompt}\n---\n{last_user_prompt}");
        for (entry_schema, pat, turn) in &guard.by_substring {
            if entry_schema.as_deref() == target_schema
                && (combined.contains(pat.as_str()) || request_transcript.contains(pat.as_str()))
            {
                return Some(turn.clone());
            }
        }
    }
    None
}

fn transcript_has_tool_result(transcript: &str) -> bool {
    transcript.contains("ROLE:tool")
        || transcript.contains("ROLE:TOOL")
        || transcript.contains("[TOOL")
        || transcript.contains("\"role\":\"tool\"")
        || transcript.contains("\"role\": \"tool\"")
        || transcript.contains("tool_call_id=")
        || transcript.contains("\"tool_call_id\"")
}

fn miss_error(primary_hash: &str, user_sha256: &str, load_error: Option<&str>) -> AppError {
    if let Some(reason) = load_error {
        return AppError::Llm(format!(
            "cassette miss for prompt hash {primary_hash} user_sha256={user_sha256} ({reason})"
        ));
    }
    AppError::Llm(format!(
        "cassette miss for prompt hash {primary_hash} user_sha256={user_sha256}"
    ))
}

fn append_entry_to_disk(record_target: &Path, entry: &CassetteEntry) -> Result<(), AppError> {
    let serialized = serde_json::to_string(entry)
        .map_err(|err| AppError::Llm(format!("failed to serialize cassette entry: {err}")))?;

    if let Some(parent) = record_target.parent()
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
        .open(record_target)
        .map_err(|err| {
            AppError::Llm(format!(
                "failed to open cassette file {} for append: {err}",
                record_target.display()
            ))
        })?;

    writeln!(file, "{serialized}").map_err(|err| {
        AppError::Llm(format!(
            "failed to append to cassette file {}: {err}",
            record_target.display()
        ))
    })?;
    file.flush().map_err(|err| {
        AppError::Llm(format!(
            "failed to flush cassette file {}: {err}",
            record_target.display()
        ))
    })
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

fn normalize_opt_hex(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
}

fn register_cassette_entry(
    file_path: &Path,
    line_no: usize,
    entry: CassetteEntry,
    state: &mut CassetteState,
) -> Result<(), AppError> {
    if !entry.kind.eq_ignore_ascii_case("chat")
        && !entry.kind.eq_ignore_ascii_case("tool")
        && !entry.kind.eq_ignore_ascii_case("agent")
    {
        return Ok(());
    }

    let req_key = normalize_opt_hex(entry.request_sha256.as_deref());
    let prompt_key = normalize_opt_hex(entry.prompt_sha256.as_deref());
    let user_key = normalize_opt_hex(entry.user_sha256.as_deref());
    let sub_key = entry
        .match_substring
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let has_match_all = entry.match_all.iter().any(|s| !s.is_empty());

    if req_key.is_none()
        && prompt_key.is_none()
        && user_key.is_none()
        && sub_key.is_none()
        && !has_match_all
    {
        return Err(AppError::Llm(format!(
            "cassette entry at {}:{line_no} must specify request_sha256, prompt_sha256, user_sha256, match_substring, or match_all",
            file_path.display()
        )));
    }

    let schema_name = normalize_schema_name(entry.response_schema_name.as_deref());
    let turn = CassetteTurn {
        content: entry.response,
        tool_calls: entry.tool_calls,
    };

    if let Some(rk) = req_key {
        state.by_request_sha256.insert(rk, turn.clone());
    }
    if has_match_all {
        let match_all = entry
            .match_all
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let exclude_any = entry
            .exclude_any
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        state.by_match_rules.push(MatchRule {
            response_schema_name: schema_name.clone(),
            match_all,
            exclude_any,
            turn: turn.clone(),
        });
    }
    if let Some(pk) = prompt_key {
        state
            .by_prompt_sha256
            .insert((schema_name.clone(), pk), turn.clone());
    }
    if let Some(uk) = user_key {
        state
            .by_user_sha256
            .insert((schema_name.clone(), uk), turn.clone());
    }
    if let Some(sk) = sub_key {
        state.by_substring.push((schema_name, sk, turn));
    }
    Ok(())
}

fn load_jsonl_file(file_path: &Path, state: &mut CassetteState) -> Result<(), AppError> {
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
        register_cassette_entry(file_path, line_no, entry, state)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use axpoint_domain::enums::Purpose;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::tools::{ToolChatMessage, ToolDefinition, canonical_request_transcript};

    fn temp_cassette_file(tag: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "axpoint_cassette_{tag}_{}.jsonl",
            Uuid::new_v4().simple()
        ))
    }

    fn write_entries(path: &Path, entries: &[CassetteEntry]) -> Result<(), AppError> {
        let mut content = String::new();
        for e in entries {
            let line = serde_json::to_string(e).map_err(|err| AppError::Llm(err.to_string()))?;
            content.push_str(&line);
            content.push('\n');
        }
        fs::write(path, content).map_err(|err| AppError::Llm(err.to_string()))
    }

    #[test]
    fn cassette_lookup_tool_turn_supports_request_sha256_and_miss() -> Result<(), AppError> {
        let path = temp_cassette_file("req_hash");
        let tools = vec![ToolDefinition::function(
            "ask_data",
            "Ask data",
            json!({"type": "object"}),
        )];
        let exact_msgs = vec![
            ToolChatMessage::system("exact-sys"),
            ToolChatMessage::user("exact-user"),
        ];
        let exact_transcript = canonical_request_transcript(Purpose::Agent, &exact_msgs, &tools);
        let exact_hash = sha256_hex(&exact_transcript);

        write_entries(
            &path,
            &[CassetteEntry {
                request_sha256: Some(exact_hash),
                prompt_sha256: None,
                user_sha256: None,
                match_substring: None,
                match_all: Vec::new(),
                exclude_any: Vec::new(),
                response_schema_name: None,
                kind: String::from("chat"),
                response: String::from("exact-hit"),
                tool_calls: Vec::new(),
            }],
        )?;

        let cassette = Cassette::open(&path, CassetteMode::Replay)?;
        let exact_res = cassette.lookup_tool_turn(&exact_transcript, "exact-sys", "exact-user")?;
        assert_eq!(exact_res.content, "exact-hit");
        assert_eq!(exact_res.tool_calls.len(), 0);

        let miss =
            cassette.lookup_tool_turn("PURPOSE:AGENT\nROLE:user\n未录制问题\n", "", "未录制问题");
        assert!(miss.is_err());
        if let Err(err) = miss {
            assert_eq!(err.error_code(), "UPSTREAM_LLM");
        }

        fs::remove_file(&path).map_err(|err| AppError::Llm(err.to_string()))?;
        Ok(())
    }

    #[test]
    fn cassette_lookup_tool_turn_supports_multi_turn_match_exclude() -> Result<(), AppError> {
        let path = temp_cassette_file("multi_turn");
        let tools = vec![ToolDefinition::function(
            "ask_data",
            "Ask data",
            json!({"type": "object"}),
        )];

        write_entries(
            &path,
            &[
                CassetteEntry {
                    request_sha256: None,
                    prompt_sha256: None,
                    user_sha256: None,
                    match_substring: None,
                    match_all: vec![String::from("已支付订单总金额是多少")],
                    exclude_any: vec![String::from("ROLE:tool")],
                    response_schema_name: None,
                    kind: String::from("chat"),
                    response: String::new(),
                    tool_calls: vec![ToolCallSpec {
                        id: String::from("call_1"),
                        name: String::from("ask_data"),
                        arguments: json!({"question": "已支付订单总金额是多少"}),
                    }],
                },
                CassetteEntry {
                    request_sha256: None,
                    prompt_sha256: None,
                    user_sha256: None,
                    match_substring: None,
                    match_all: vec![
                        String::from("已支付订单总金额是多少"),
                        String::from("ROLE:tool"),
                        String::from("33239.25"),
                    ],
                    exclude_any: Vec::new(),
                    response_schema_name: None,
                    kind: String::from("chat"),
                    response: String::from("已支付订单总金额为 33239.25 元。"),
                    tool_calls: Vec::new(),
                },
            ],
        )?;

        let cassette = Cassette::open(&path, CassetteMode::Replay)?;
        let turn1_msgs = vec![
            ToolChatMessage::system("你是运营主管"),
            ToolChatMessage::user("已支付订单总金额是多少？"),
        ];
        let turn1_tx = canonical_request_transcript(Purpose::Agent, &turn1_msgs, &tools);
        let turn1 =
            cassette.lookup_tool_turn(&turn1_tx, "你是运营主管", "已支付订单总金额是多少？")?;
        assert_eq!(turn1.tool_calls.len(), 1);
        assert_eq!(
            turn1.tool_calls.first().map(|c| c.name.as_str()),
            Some("ask_data")
        );

        let mut turn2_msgs = turn1_msgs;
        turn2_msgs.push(ToolChatMessage::assistant_with_tools("", turn1.tool_calls));
        turn2_msgs.push(ToolChatMessage::tool_result(
            "call_1",
            r#"{"rows":[{"paid_amount":33239.25}]}"#,
        ));
        let turn2_tx = canonical_request_transcript(Purpose::Agent, &turn2_msgs, &tools);
        let turn2 =
            cassette.lookup_tool_turn(&turn2_tx, "你是运营主管", "已支付订单总金额是多少？")?;
        assert_eq!(turn2.tool_calls.len(), 0);
        assert_eq!(turn2.content, "已支付订单总金额为 33239.25 元。");

        fs::remove_file(&path).map_err(|err| AppError::Llm(err.to_string()))?;
        Ok(())
    }

    #[test]
    fn cassette_lookup_prioritizes_matching_schema_and_falls_back_to_none() -> Result<(), AppError>
    {
        let path = temp_cassette_file("schema_prio");
        write_entries(
            &path,
            &[
                CassetteEntry {
                    request_sha256: None,
                    prompt_sha256: None,
                    user_sha256: None,
                    match_substring: Some(String::from("订单总金额")),
                    match_all: Vec::new(),
                    exclude_any: Vec::new(),
                    response_schema_name: None,
                    kind: String::from("chat"),
                    response: String::from(r#"{"sql":"SELECT 1","explanation":"fallback"}"#),
                    tool_calls: Vec::new(),
                },
                CassetteEntry {
                    request_sha256: None,
                    prompt_sha256: None,
                    user_sha256: None,
                    match_substring: Some(String::from("订单总金额")),
                    match_all: Vec::new(),
                    exclude_any: Vec::new(),
                    response_schema_name: Some(String::from("axpoint_ontology_slots")),
                    kind: String::from("chat"),
                    response: String::from(r#"{"route":"SINGLE_METRIC"}"#),
                    tool_calls: Vec::new(),
                },
            ],
        )?;

        let cassette = Cassette::open(&path, CassetteMode::Replay)?;
        let slots = cassette.lookup_chat_with_schema(
            "sys",
            "查询订单总金额",
            Some("axpoint_ontology_slots"),
        )?;
        assert_eq!(slots, r#"{"route":"SINGLE_METRIC"}"#);

        let fallback = cassette.lookup_chat_with_schema(
            "sys",
            "查询订单总金额",
            Some("axpoint_nl2sql_draft"),
        )?;
        assert_eq!(fallback, r#"{"sql":"SELECT 1","explanation":"fallback"}"#);

        let plain = cassette.lookup_chat("sys", "查询订单总金额")?;
        assert_eq!(plain, r#"{"sql":"SELECT 1","explanation":"fallback"}"#);

        fs::remove_file(&path).map_err(|err| AppError::Llm(err.to_string()))?;
        Ok(())
    }
}
