//! A typed, cached LLM client over Ollama's OpenAI-compatible `/v1` API.
//!
//! [`LlmClient::complete`] turns a prompt into a `T: DeserializeOwned +
//! JsonSchema`: the `schemars` schema of `T` is sent as the structured-output
//! format (`response_format` of type `json_schema`), and the reply must
//! deserialize into `T` and pass the caller's check
//! ([`LlmClient::complete_with`]). An invalid reply is retried once, with the
//! bad reply and the error appended to the conversation; a second invalid
//! reply is an [`InvalidOutput`] error and nothing is cached. A reply cut off
//! at the token limit is logged as a warning and named in that error.
//! [`LlmClient::embed`] returns one vector per text, sending only the texts the
//! cache does not hold; all vectors of one call must have the same length.
//!
//! Response types should carry `#[serde(deny_unknown_fields)]`: schemars then
//! sends `additionalProperties: false`, and a reply with extra fields is
//! invalid instead of silently accepted.
//!
//! Both go through `cache.db`: `llm_cache` keyed by a hash of chat model,
//! reasoning effort, temperature, canonical schema and prompt (the prompt is
//! the full text, input included), `embedding_cache` keyed by a hash of embedding model and
//! text. A changed key simply misses. Cache faults degrade: an unreadable,
//! undecodable or no longer valid row is a miss, a failed write warns and keeps
//! the result. The client is shared by reference across concurrent tasks; give
//! it its own connection ([`crate::store::Store::connect_cache`]) so its
//! writes never join another stage's transaction. Its writes are single
//! statements, serialised by the client.
//!
//! [`LlmBackend`] is the seam the client calls through; [`OllamaBackend`]
//! implements it with `reqwest` and retries a 503 or a failed connection once,
//! tests use a fake. [`LlmStats`] counts backend calls and cache hits, so a
//! caller can show that a rerun made no LLM calls.
//!
//! The client is also the run's circuit breaker for the chat model (see
//! [`LlmClient`]): once tripped, or with `--no-llm`, every stage gets cached
//! completions only. [`LlmClient::check_chat_model`] fails a run whose chat
//! model does not exist.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use libsql::{Connection, params, params_from_iter};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use crate::config::OllamaConfig;

/// Per-request timeout; a large local model answering a long prompt is slow.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Timeout for establishing the connection, so an unreachable host fails
/// fast instead of using the whole request timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How much of a non-success response body is kept for the error message.
const MAX_ERROR_BODY: usize = 4 * 1024;

/// HTTP attempts per backend request: the first and one retry after a
/// transient failure (503 or no connection).
const HTTP_ATTEMPTS: usize = 2;

/// Wait before retrying a transient failure.
const TRANSIENT_BACKOFF: Duration = Duration::from_secs(2);

/// Chat attempts per completion: the first and one retry on invalid output.
pub const MAX_ATTEMPTS: usize = 2;

/// Texts per embedding request.
pub const EMBED_BATCH: usize = 64;

/// Completions in a row that end in [`InvalidOutput`] before the client stops
/// calling the chat model for the run.
pub const MAX_CONSECUTIVE_INVALID: usize = 5;

/// One chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user",
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant",
            content: content.into(),
        }
    }
}

/// Everything a backend needs for one structured chat completion.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    /// Name of the response format; `[A-Za-z0-9_-]` only.
    pub schema_name: String,
    /// JSON schema the reply must match.
    pub schema: Value,
    /// `reasoning_effort` to send, if any.
    pub reasoning_effort: Option<String>,
    /// Sampling temperature.
    pub temperature: f64,
}

/// One chat completion as the backend returned it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatReply {
    /// The assistant's reply text.
    pub content: String,
    /// `stop`, `length` (cut off at the token limit), ...
    pub finish_reason: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

impl ChatReply {
    /// A complete (`stop`) reply without usage.
    pub fn stop(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            finish_reason: Some("stop".to_string()),
            ..Self::default()
        }
    }

    /// The reply was cut off at the token limit.
    pub fn truncated(&self) -> bool {
        self.finish_reason.as_deref() == Some("length")
    }
}

/// The future an [`LlmBackend`] method returns: boxed so the trait stays
/// object-safe.
pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// The calls [`LlmClient`] makes. No caching, no validation, no retries on
/// invalid output.
pub trait LlmBackend: Send + Sync {
    /// The assistant's reply for `request`.
    fn chat<'a>(&'a self, request: &'a ChatRequest) -> BackendFuture<'a, ChatReply>;

    /// One vector per text, in input order. `texts` is never empty.
    fn embed<'a>(&'a self, model: &'a str, texts: &'a [String])
    -> BackendFuture<'a, Vec<Vec<f32>>>;

    /// Whether the server has `model`: `Ok(false)` when it answers that the
    /// model does not exist, `Err` when it cannot tell (unreachable, other
    /// errors).
    fn has_model<'a>(&'a self, model: &'a str) -> BackendFuture<'a, bool>;
}

/// Ollama's OpenAI-compatible API: `/v1/chat/completions` and
/// `/v1/embeddings`, plus the native `/api/show` to check that a model exists
/// (it accepts every model name, including ones with a `/`).
pub struct OllamaBackend {
    http: reqwest::Client,
    base_url: String,
    retry_backoff: Duration,
}

impl OllamaBackend {
    /// A backend for `url` (the server root; a trailing `/v1` is accepted).
    /// Fails when `url` is not an absolute http(s) URL.
    pub fn new(url: &str) -> Result<Self> {
        let base_url = url.trim().trim_end_matches('/');
        let base_url = base_url.strip_suffix("/v1").unwrap_or(base_url);
        let parsed = reqwest::Url::parse(base_url)
            .with_context(|| format!("ollama.url {url:?} is not a URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            bail!("ollama.url {url:?} must start with http:// or https://");
        }
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .context("building the HTTP client")?;
        Ok(Self {
            http,
            base_url: base_url.to_string(),
            retry_backoff: TRANSIENT_BACKOFF,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/{path}", self.base_url)
    }

    /// `POST` `body` to `url` and return the JSON of a 2xx answer; any other
    /// status is an [`HttpError`] carrying Ollama's message. A transient
    /// failure (see [`transient_status`]) is retried once after a short
    /// backoff.
    async fn post(&self, url: &str, body: &Value) -> Result<Value> {
        let mut attempt = 1;
        loop {
            match self.post_once(url, body).await {
                Err((err, true)) if attempt < HTTP_ATTEMPTS => {
                    tracing::warn!(attempt, "transient Ollama failure, retrying: {err:#}");
                    tokio::time::sleep(self.retry_backoff).await;
                    attempt += 1;
                }
                result => return result.map_err(|(err, _)| err),
            }
        }
    }

    /// One `POST`; an error comes with whether it is transient.
    async fn post_once(&self, url: &str, body: &Value) -> Result<Value, (anyhow::Error, bool)> {
        let mut response = match self.http.post(url).json(body).send().await {
            Ok(response) => response,
            Err(err) => {
                let transient = err.is_connect();
                return Err((anyhow!(err).context(format!("calling {url}")), transient));
            }
        };
        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .with_context(|| format!("reading the response of {url}"))
                .map_err(|err| (err, false));
        }
        let mut error_body = Vec::new();
        while error_body.len() < MAX_ERROR_BODY {
            let Ok(Some(chunk)) = response.chunk().await else {
                break;
            };
            let take = chunk.len().min(MAX_ERROR_BODY - error_body.len());
            error_body.extend_from_slice(&chunk[..take]);
        }
        Err((
            HttpError {
                url: url.to_string(),
                status: status.as_u16(),
                message: error_message(&error_body),
            }
            .into(),
            transient_status(status),
        ))
    }
}

/// A non-success HTTP answer from Ollama.
#[derive(Debug)]
pub struct HttpError {
    pub url: String,
    pub status: u16,
    /// Ollama's error message, else the start of the body.
    pub message: String,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} answered HTTP {}: {}",
            self.url, self.status, self.message
        )
    }
}

impl std::error::Error for HttpError {}

/// Statuses worth one retry: Ollama answers 503 while it is overloaded or
/// loading. Client errors (4xx) and other server errors are not retried.
fn transient_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::SERVICE_UNAVAILABLE
}

impl LlmBackend for OllamaBackend {
    fn chat<'a>(&'a self, request: &'a ChatRequest) -> BackendFuture<'a, ChatReply> {
        Box::pin(async move {
            let response = self
                .post(&self.url("chat/completions"), &chat_body(request))
                .await?;
            parse_chat_response(&response)
        })
    }

    fn embed<'a>(
        &'a self,
        model: &'a str,
        texts: &'a [String],
    ) -> BackendFuture<'a, Vec<Vec<f32>>> {
        Box::pin(async move {
            let body = json!({ "model": model, "input": texts });
            let response = self.post(&self.url("embeddings"), &body).await?;
            parse_embeddings_response(&response, texts.len())
        })
    }

    fn has_model<'a>(&'a self, model: &'a str) -> BackendFuture<'a, bool> {
        Box::pin(async move {
            let url = format!("{}/api/show", self.base_url);
            match self.post(&url, &json!({ "model": model })).await {
                Ok(_) => Ok(true),
                Err(err)
                    if err
                        .downcast_ref::<HttpError>()
                        .is_some_and(|err| err.status == 404) =>
                {
                    Ok(false)
                }
                Err(err) => Err(err),
            }
        })
    }
}

/// The `/v1/chat/completions` request body.
fn chat_body(request: &ChatRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "messages": request.messages,
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": request.schema_name,
                "strict": true,
                "schema": request.schema,
            },
        },
        "stream": false,
        "temperature": request.temperature,
    });
    if let Some(effort) = &request.reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    body
}

/// The reply of a chat completion. A thinking model's reasoning arrives in
/// `message.reasoning` and is ignored. A missing or `null` content is an empty
/// reply: invalid output the client retries, not a backend failure.
fn parse_chat_response(response: &Value) -> Result<ChatReply> {
    let choice = response
        .pointer("/choices/0")
        .ok_or_else(|| anyhow!("chat response has no choices"))?;
    let content = choice
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(ChatReply {
        content: content.to_string(),
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt_tokens: response
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64),
        completion_tokens: response
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64),
    })
}

/// One vector per input, ordered by each item's `index`.
fn parse_embeddings_response(response: &Value, expected: usize) -> Result<Vec<Vec<f32>>> {
    let data = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("embeddings response has no data"))?;
    if data.len() != expected {
        bail!(
            "embeddings response has {} vectors for {expected} texts",
            data.len()
        );
    }
    let mut vectors: Vec<Option<Vec<f32>>> = vec![None; expected];
    for (position, item) in data.iter().enumerate() {
        let index = match item.get("index") {
            Some(index) => index
                .as_u64()
                .ok_or_else(|| anyhow!("embedding index is not a number"))?
                as usize,
            None => position,
        };
        let vector = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("embedding {index} has no vector"))?
            .iter()
            .map(|value| value.as_f64().map(|value| value as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| anyhow!("embedding {index} has a non-number"))?;
        if vector.is_empty() {
            bail!("embedding {index} is empty");
        }
        let slot = vectors
            .get_mut(index)
            .ok_or_else(|| anyhow!("embedding index {index} is out of range"))?;
        if slot.replace(vector).is_some() {
            bail!("embedding index {index} appears twice");
        }
    }
    Ok(vectors.into_iter().map(Option::unwrap_or_default).collect())
}

/// Ollama's `error.message` (or `error` as a string), else the raw body.
fn error_message(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return text.trim().to_string();
    };
    json.pointer("/error/message")
        .or_else(|| json.get("error"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| text.trim().to_string())
}

/// The model's reply still did not deserialize into the requested type, or
/// failed the caller's check, after [`MAX_ATTEMPTS`] attempts. Nothing was
/// cached.
#[derive(Debug)]
pub struct InvalidOutput {
    pub type_name: String,
    pub error: String,
    pub output: String,
    /// The last reply's finish reason; `length` means it was cut off.
    pub finish_reason: Option<String>,
}

impl fmt::Display for InvalidOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the model's reply is not a valid {} after {MAX_ATTEMPTS} attempts: {}",
            self.type_name, self.error
        )?;
        if self.finish_reason.as_deref() == Some("length") {
            write!(
                f,
                " (the reply was cut off at the token limit, finish_reason \"length\")"
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for InvalidOutput {}

/// The completion is not in the LLM cache and the client makes no chat calls
/// this run: `--no-llm`, or its circuit breaker tripped.
#[derive(Debug)]
pub struct LlmUnavailable;

impl fmt::Display for LlmUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "not in the LLM cache, and the chat model is not called this run"
        )
    }
}

impl std::error::Error for LlmUnavailable {}

/// Counters for one [`LlmClient`]. `chat_calls` and `embed_calls` are backend
/// requests; a fully cached rerun leaves both at zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LlmStats {
    /// Chat requests sent, retries included.
    pub chat_calls: usize,
    /// Completions answered from `llm_cache`.
    pub chat_hits: usize,
    /// Retries after an invalid reply.
    pub chat_retries: usize,
    /// Embedding requests sent.
    pub embed_calls: usize,
    /// Texts sent for embedding.
    pub embed_texts: usize,
    /// Input texts answered from `embedding_cache`.
    pub embed_hits: usize,
}

impl LlmStats {
    /// What happened between `earlier` (a snapshot of the same client) and
    /// `self`.
    pub fn since(&self, earlier: &LlmStats) -> LlmStats {
        LlmStats {
            chat_calls: self.chat_calls - earlier.chat_calls,
            chat_hits: self.chat_hits - earlier.chat_hits,
            chat_retries: self.chat_retries - earlier.chat_retries,
            embed_calls: self.embed_calls - earlier.embed_calls,
            embed_texts: self.embed_texts - earlier.embed_texts,
            embed_hits: self.embed_hits - earlier.embed_hits,
        }
    }
}

#[derive(Default)]
struct Counters {
    chat_calls: AtomicUsize,
    chat_hits: AtomicUsize,
    chat_retries: AtomicUsize,
    embed_calls: AtomicUsize,
    embed_texts: AtomicUsize,
    embed_hits: AtomicUsize,
}

/// The cached, typed client. Share it by reference (or in an `Arc`) across
/// concurrent tasks; cache writes are serialised internally.
///
/// It carries the run's circuit breaker: after a backend failure of a chat
/// call (the backend already retried a transient one) or
/// [`MAX_CONSECUTIVE_INVALID`] invalid completions in a row it warns once and
/// becomes cache-only, like `--no-llm` ([`LlmClient::set_cache_only`]): every
/// later completion, in any stage, is answered from the cache or fails with
/// [`LlmUnavailable`].
pub struct LlmClient {
    backend: Arc<dyn LlmBackend>,
    cache: Connection,
    write_lock: Mutex<()>,
    chat_model: String,
    embedding_model: String,
    reasoning_effort: Option<String>,
    temperature: f64,
    counters: Counters,
    cache_only: AtomicBool,
    consecutive_invalid: AtomicUsize,
}

impl LlmClient {
    /// A client calling `backend` with the models and reasoning effort from
    /// `config`, caching in `cache`: a connection of its own,
    /// [`crate::store::Store::connect_cache`], not the shared
    /// [`crate::store::Store::cache`].
    pub fn new(backend: Arc<dyn LlmBackend>, cache: Connection, config: &OllamaConfig) -> Self {
        Self {
            backend,
            cache,
            write_lock: Mutex::new(()),
            chat_model: config.chat_model.clone(),
            embedding_model: config.embedding_model.clone(),
            reasoning_effort: Some(config.reasoning_effort.clone())
                .filter(|effort| !effort.is_empty()),
            temperature: normalized_temperature(config.temperature),
            counters: Counters::default(),
            cache_only: AtomicBool::new(false),
            consecutive_invalid: AtomicUsize::new(0),
        }
    }

    /// A client over [`OllamaBackend`] at `config.url`.
    pub fn from_config(config: &OllamaConfig, cache: Connection) -> Result<Self> {
        let backend = OllamaBackend::new(&config.url)?;
        Ok(Self::new(Arc::new(backend), cache, config))
    }

    /// A snapshot of the counters.
    pub fn stats(&self) -> LlmStats {
        let c = &self.counters;
        LlmStats {
            chat_calls: c.chat_calls.load(Ordering::Relaxed),
            chat_hits: c.chat_hits.load(Ordering::Relaxed),
            chat_retries: c.chat_retries.load(Ordering::Relaxed),
            embed_calls: c.embed_calls.load(Ordering::Relaxed),
            embed_texts: c.embed_texts.load(Ordering::Relaxed),
            embed_hits: c.embed_hits.load(Ordering::Relaxed),
        }
    }

    /// Answer completions from the cache only from now on; no chat calls.
    pub fn set_cache_only(&self) {
        self.cache_only.store(true, Ordering::Relaxed);
    }

    /// Whether the client makes no chat calls (`--no-llm` or a tripped
    /// breaker).
    pub fn is_cache_only(&self) -> bool {
        self.cache_only.load(Ordering::Relaxed)
    }

    /// Trip the circuit breaker: cache-only from now on, one warning.
    fn trip(&self, reason: &str) {
        if !self.cache_only.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "{reason}; no further chat calls this run, completions come from the LLM cache only"
            );
        }
    }

    /// Check once, before the first LLM stage, that the server has the chat
    /// model. A missing model is an error (a misconfigured run would
    /// otherwise finish with no summaries at all); an unreachable server or
    /// another failure trips the breaker and the run goes on cache-only. A
    /// cache-only client makes no request.
    pub async fn check_chat_model(&self) -> Result<()> {
        if self.is_cache_only() {
            return Ok(());
        }
        match self.backend.has_model(&self.chat_model).await {
            Ok(true) => Ok(()),
            Ok(false) => bail!(
                "the chat model {:?} does not exist on the Ollama server; pull it (`ollama pull {}`) or fix ollama.chat_model",
                self.chat_model,
                self.chat_model
            ),
            Err(err) => {
                self.trip(&format!("checking the chat model failed: {err:#}"));
                Ok(())
            }
        }
    }

    /// Answer `prompt` with a `T`, from the cache or the chat model. See the
    /// module docs for retries and caching. A reply that is still invalid
    /// after the retry is an [`InvalidOutput`] error; backend failures are
    /// returned as is (the backend retries a transient one itself) and trip
    /// the breaker. A cache miss on a cache-only client is [`LlmUnavailable`].
    pub async fn complete<T: DeserializeOwned + JsonSchema>(&self, prompt: &str) -> Result<T> {
        self.complete_with(prompt, accept::<T>).await
    }

    /// [`LlmClient::complete`] with a check on the parsed reply, such as a
    /// blank or overlong field. An `Err` message makes the reply invalid: it
    /// is sent back to the model for the retry, and nothing is cached. A
    /// cached reply that fails the check is a miss.
    pub async fn complete_with<T, F>(&self, prompt: &str, validate: F) -> Result<T>
    where
        T: DeserializeOwned + JsonSchema,
        F: Fn(&T) -> Result<(), String>,
    {
        let (schema, key) = self.schema_and_key::<T>(prompt)?;
        let type_name = T::schema_name();
        let decode = |output: &str| decode_checked(output, &validate);
        let cached = self.cached_output(&key).await;
        if let Some(value) = self.decode_hit(cached, &type_name, decode) {
            return Ok(value);
        }
        if self.is_cache_only() {
            return Err(LlmUnavailable.into());
        }

        let mut request = ChatRequest {
            model: self.chat_model.clone(),
            messages: vec![Message::user(prompt)],
            schema_name: schema_name(&type_name),
            schema,
            reasoning_effort: self.reasoning_effort.clone(),
            temperature: self.temperature,
        };
        let mut attempt = 1;
        loop {
            self.counters.chat_calls.fetch_add(1, Ordering::Relaxed);
            let started = Instant::now();
            let reply = match self.backend.chat(&request).await {
                Ok(reply) => reply,
                Err(err) => {
                    self.trip(&format!("the chat model failed: {err:#}"));
                    return Err(err.context("chat completion"));
                }
            };
            tracing::debug!(
                r#type = %type_name,
                attempt,
                prompt_tokens = ?reply.prompt_tokens,
                completion_tokens = ?reply.completion_tokens,
                finish_reason = ?reply.finish_reason,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "chat reply"
            );
            if reply.truncated() {
                tracing::warn!(
                    r#type = %type_name,
                    attempt,
                    completion_tokens = ?reply.completion_tokens,
                    "chat reply was cut off at the token limit"
                );
            }
            match decode(&reply.content) {
                Ok(value) => {
                    self.consecutive_invalid.store(0, Ordering::Relaxed);
                    if let Err(err) = self.store_completion(&key, reply.content.trim()).await {
                        tracing::warn!("writing llm cache: {err:#}");
                    }
                    return Ok(value);
                }
                Err(err) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!(r#type = %type_name, attempt, "invalid model reply, retrying: {err}");
                    self.counters.chat_retries.fetch_add(1, Ordering::Relaxed);
                    request.messages.push(Message::assistant(reply.content));
                    request.messages.push(Message::user(retry_prompt(&err)));
                    attempt += 1;
                }
                Err(err) => {
                    let invalid = self.consecutive_invalid.fetch_add(1, Ordering::Relaxed) + 1;
                    if invalid >= MAX_CONSECUTIVE_INVALID {
                        self.trip(&format!(
                            "{invalid} completions in a row were invalid after a retry"
                        ));
                    }
                    return Err(InvalidOutput {
                        type_name: type_name.into_owned(),
                        error: err,
                        output: reply.content,
                        finish_reason: reply.finish_reason,
                    }
                    .into());
                }
            }
        }
    }

    /// The cached answer to `prompt` that passes `validate`, without ever
    /// calling the backend: the cache-only half of
    /// [`LlmClient::complete_with`]. A hit counts in `chat_hits`; a miss (or
    /// an unreadable or no longer valid row) is `None`.
    pub async fn cached_with<T, F>(&self, prompt: &str, validate: F) -> Result<Option<T>>
    where
        T: DeserializeOwned + JsonSchema,
        F: Fn(&T) -> Result<(), String>,
    {
        let (_, key) = self.schema_and_key::<T>(prompt)?;
        let cached = self.cached_output(&key).await;
        Ok(self.decode_hit(cached, &T::schema_name(), |output| {
            decode_checked(output, &validate)
        }))
    }

    /// The JSON schema of `T` and the `llm_cache` key of `prompt` for it.
    fn schema_and_key<T: JsonSchema>(&self, prompt: &str) -> Result<(Value, String)> {
        let schema =
            serde_json::to_value(schemars::schema_for!(T)).context("serializing schema")?;
        let key = completion_key(
            &self.chat_model,
            self.reasoning_effort.as_deref(),
            self.temperature,
            &canonical_schema(&schema),
            prompt,
        );
        Ok((schema, key))
    }

    /// The cached row for `key`; a read fault warns and is a miss.
    async fn cached_output(&self, key: &str) -> Option<String> {
        self.cached_completion(key).await.unwrap_or_else(|err| {
            tracing::warn!("reading llm cache, treating as a miss: {err:#}");
            None
        })
    }

    /// `output` from the cache decoded by `decode`, counted as a hit; an
    /// invalid row warns and is a miss.
    fn decode_hit<T>(
        &self,
        output: Option<String>,
        type_name: &str,
        decode: impl Fn(&str) -> Result<T, String>,
    ) -> Option<T> {
        match decode(&output?) {
            Ok(value) => {
                self.counters.chat_hits.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(r#type = %type_name, "llm cache hit");
                Some(value)
            }
            Err(err) => {
                tracing::warn!(r#type = %type_name, "llm cache row is not valid, treating as a miss: {err}");
                None
            }
        }
    }

    /// One vector per text, in input order. Cached texts are not sent;
    /// repeated texts are sent once; the rest go out in batches of
    /// [`EMBED_BATCH`]. Vectors of different lengths (a changed model behind
    /// the same name, a bad cache row) are an error, and such a batch is not
    /// cached.
    pub async fn embed<S: AsRef<str>>(&self, texts: &[S]) -> Result<Vec<Vec<f32>>> {
        let mut vectors: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut dim: Option<usize> = None;
        let mut missing: Vec<(&str, String)> = Vec::new();
        let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
        for (position, text) in texts.iter().enumerate() {
            let text = text.as_ref();
            if let Some(seen) = positions.get_mut(text) {
                seen.push(position);
                continue;
            }
            positions.insert(text, vec![position]);
            let key = embedding_key(&self.embedding_model, text);
            match self.cached_embedding(&key).await {
                Ok(Some(vector)) => {
                    check_dim(&mut dim, vector.len())?;
                    vectors[position] = Some(vector);
                }
                Ok(None) => missing.push((text, key)),
                Err(err) => {
                    tracing::warn!("reading embedding cache, treating as a miss: {err:#}");
                    missing.push((text, key));
                }
            }
        }

        for batch in missing.chunks(EMBED_BATCH) {
            let inputs: Vec<String> = batch.iter().map(|(text, _)| text.to_string()).collect();
            self.counters.embed_calls.fetch_add(1, Ordering::Relaxed);
            self.counters
                .embed_texts
                .fetch_add(inputs.len(), Ordering::Relaxed);
            let embedded = self
                .backend
                .embed(&self.embedding_model, &inputs)
                .await
                .context("embedding texts")?;
            if embedded.len() != inputs.len() {
                bail!(
                    "the embedding backend returned {} vectors for {} texts",
                    embedded.len(),
                    inputs.len()
                );
            }
            for vector in &embedded {
                check_dim(&mut dim, vector.len())?;
            }
            let rows: Vec<(&str, &[f32])> = batch
                .iter()
                .zip(&embedded)
                .map(|((_, key), vector)| (key.as_str(), vector.as_slice()))
                .collect();
            if let Err(err) = self.store_embeddings(&rows).await {
                tracing::warn!("writing embedding cache: {err:#}");
            }
            for ((text, _), vector) in batch.iter().zip(embedded) {
                vectors[positions[text][0]] = Some(vector);
            }
        }

        let missed: HashSet<&str> = missing.iter().map(|(text, _)| *text).collect();
        let mut hits = 0;
        for (text, seen) in &positions {
            if !missed.contains(text) {
                hits += seen.len();
            }
            for &position in &seen[1..] {
                vectors[position] = vectors[seen[0]].clone();
            }
        }
        self.counters.embed_hits.fetch_add(hits, Ordering::Relaxed);
        vectors
            .into_iter()
            .map(|vector| vector.ok_or_else(|| anyhow!("a text has no embedding")))
            .collect()
    }

    async fn cached_completion(&self, key: &str) -> Result<Option<String>> {
        let mut rows = self
            .cache
            .query("SELECT output FROM llm_cache WHERE key = ?1", params![key])
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row.get::<String>(0)?)),
            None => Ok(None),
        }
    }

    async fn store_completion(&self, key: &str, output: &str) -> Result<()> {
        let _write = self.write_lock.lock().await;
        self.cache
            .execute(
                "INSERT OR REPLACE INTO llm_cache (key, model, output, created_at)
                 VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
                params![key, self.chat_model.as_str(), output],
            )
            .await?;
        Ok(())
    }

    async fn cached_embedding(&self, key: &str) -> Result<Option<Vec<f32>>> {
        let mut rows = self
            .cache
            .query(
                "SELECT dim, vector FROM embedding_cache WHERE key = ?1",
                params![key],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(None);
        };
        let dim = row.get::<i64>(0)?;
        let vector = decode_vector(&row.get::<Vec<u8>>(1)?)?;
        if i64::try_from(vector.len()).ok() != Some(dim) {
            bail!("cached embedding has {} values but dim {dim}", vector.len());
        }
        Ok(Some(vector))
    }

    /// One multi-row `INSERT`: atomic without an explicit transaction.
    async fn store_embeddings(&self, rows: &[(&str, &[f32])]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let placeholders = vec!["(?, ?, ?, ?)"; rows.len()].join(", ");
        let sql = format!(
            "INSERT OR REPLACE INTO embedding_cache (key, model, dim, vector) VALUES {placeholders}"
        );
        let values: Vec<libsql::Value> = rows
            .iter()
            .flat_map(|(key, vector)| {
                [
                    libsql::Value::Text(key.to_string()),
                    libsql::Value::Text(self.embedding_model.clone()),
                    libsql::Value::Integer(vector.len() as i64),
                    libsql::Value::Blob(encode_vector(vector)),
                ]
            })
            .collect();
        let _write = self.write_lock.lock().await;
        self.cache.execute(&sql, params_from_iter(values)).await?;
        Ok(())
    }
}

/// Deserialize `output` into a `T` that passes `validate`.
fn decode_checked<T, F>(output: &str, validate: &F) -> Result<T, String>
where
    T: DeserializeOwned,
    F: Fn(&T) -> Result<(), String>,
{
    let value = serde_json::from_str::<T>(output).map_err(|err| err.to_string())?;
    validate(&value)?;
    Ok(value)
}

/// The check of [`LlmClient::complete`]: any parsed reply is valid.
fn accept<T>(_: &T) -> Result<(), String> {
    Ok(())
}

/// Record the first vector length seen in `dim`; a different one is an error.
fn check_dim(dim: &mut Option<usize>, len: usize) -> Result<()> {
    match *dim {
        Some(expected) if expected != len => bail!(
            "embedding vectors differ in length ({expected} and {len}); did the embedding model change?"
        ),
        Some(_) => Ok(()),
        None => {
            *dim = Some(len);
            Ok(())
        }
    }
}

/// The follow-up message after an invalid reply.
fn retry_prompt(error: &str) -> String {
    format!(
        "Your previous reply was not valid: {error}. Reply again with only a JSON value that matches the requested schema."
    )
}

/// A response-format name OpenAI-compatible servers accept.
fn schema_name(type_name: &str) -> String {
    let name: String = type_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        "response".to_string()
    } else {
        name
    }
}

/// blake3 over length-prefixed parts, so no two part lists collide by
/// concatenation.
fn hash_parts(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

/// The schema as it goes into the cache key: object keys sorted recursively
/// (independent of `serde_json`'s `preserve_order` feature) and the top-level
/// `$schema` dialect URI dropped.
fn canonical_schema(schema: &Value) -> String {
    let mut schema = schema.clone();
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
    }
    sorted_keys(&schema).to_string()
}

fn sorted_keys(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), sorted_keys(&object[key])))
                    .collect::<Map<String, Value>>(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted_keys).collect()),
        other => other.clone(),
    }
}

/// `temperature` with `-0.0` as `0.0`: the same request, one cache key.
fn normalized_temperature(temperature: f64) -> f64 {
    if temperature == 0.0 { 0.0 } else { temperature }
}

/// `llm_cache` key: chat model, reasoning effort, temperature, canonical
/// schema and prompt.
fn completion_key(
    model: &str,
    reasoning_effort: Option<&str>,
    temperature: f64,
    schema: &str,
    prompt: &str,
) -> String {
    hash_parts(&[
        "completion",
        model,
        reasoning_effort.unwrap_or(""),
        &normalized_temperature(temperature).to_string(),
        schema,
        prompt,
    ])
}

/// `embedding_cache` key: embedding model and text.
fn embedding_key(model: &str, text: &str) -> String {
    hash_parts(&["embedding", model, text])
}

/// Little-endian `f32`s, the layout of libSQL's `F32_BLOB`.
fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_vector(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        bail!("embedding blob of {} bytes is not a vector", bytes.len());
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect())
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    /// A [`LlmBackend`] that answers chats from a script (in order; an empty
    /// script is an error; a standing reply for a response type comes
    /// first) and embeds a text as `[chars, first byte, 1.0]` (without the
    /// `1.0` for a text marked [`FakeBackend::short`]).
    /// It records every chat request and embedding batch.
    #[derive(Default)]
    pub struct FakeBackend {
        model: Mutex<ModelCheck>,
        model_checks: Mutex<Vec<String>>,
        replies: Mutex<VecDeque<ChatReply>>,
        standing: Mutex<HashMap<String, String>>,
        chats: Mutex<Vec<ChatRequest>>,
        batches: Mutex<Vec<Vec<String>>>,
        short: Mutex<Vec<String>>,
    }

    /// How [`FakeBackend::has_model`] answers.
    #[derive(Debug, Clone, Copy, Default)]
    pub enum ModelCheck {
        #[default]
        Found,
        Missing,
        Fails,
    }

    impl FakeBackend {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub fn model(&self, check: ModelCheck) {
            *self.model.lock().unwrap() = check;
        }

        /// The model names checked so far.
        pub fn model_checks(&self) -> Vec<String> {
            self.model_checks.lock().unwrap().clone()
        }

        pub fn reply(&self, replies: &[&str]) {
            self.replies
                .lock()
                .unwrap()
                .extend(replies.iter().map(|reply| ChatReply::stop(*reply)));
        }

        /// Answer every request for the response type `schema_name` with
        /// `reply`, ahead of the scripted replies.
        pub fn always(&self, schema_name: &str, reply: &str) {
            self.standing
                .lock()
                .unwrap()
                .insert(schema_name.to_string(), reply.to_string());
        }

        pub fn reply_with(&self, reply: ChatReply) {
            self.replies.lock().unwrap().push_back(reply);
        }

        /// Embed `text` one value short from now on.
        pub fn short(&self, text: &str) {
            self.short.lock().unwrap().push(text.to_string());
        }

        pub fn chats(&self) -> Vec<ChatRequest> {
            self.chats.lock().unwrap().clone()
        }

        pub fn batches(&self) -> Vec<Vec<String>> {
            self.batches.lock().unwrap().clone()
        }
    }

    pub fn vector_for(text: &str) -> Vec<f32> {
        vec![
            text.chars().count() as f32,
            text.bytes().next().unwrap_or(0) as f32,
            1.0,
        ]
    }

    impl LlmBackend for FakeBackend {
        fn chat<'a>(&'a self, request: &'a ChatRequest) -> BackendFuture<'a, ChatReply> {
            Box::pin(async move {
                self.chats.lock().unwrap().push(request.clone());
                if let Some(reply) = self.standing.lock().unwrap().get(&request.schema_name) {
                    return Ok(ChatReply::stop(reply.clone()));
                }
                self.replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or_else(|| anyhow!("no scripted reply"))
            })
        }

        fn embed<'a>(
            &'a self,
            _model: &'a str,
            texts: &'a [String],
        ) -> BackendFuture<'a, Vec<Vec<f32>>> {
            Box::pin(async move {
                assert!(!texts.is_empty(), "the client never sends an empty batch");
                self.batches.lock().unwrap().push(texts.to_vec());
                let short = self.short.lock().unwrap();
                Ok(texts
                    .iter()
                    .map(|text| {
                        let mut vector = vector_for(text);
                        if short.contains(text) {
                            vector.pop();
                        }
                        vector
                    })
                    .collect())
            })
        }

        fn has_model<'a>(&'a self, model: &'a str) -> BackendFuture<'a, bool> {
            Box::pin(async move {
                self.model_checks.lock().unwrap().push(model.to_string());
                match *self.model.lock().unwrap() {
                    ModelCheck::Found => Ok(true),
                    ModelCheck::Missing => Ok(false),
                    ModelCheck::Fails => Err(anyhow!("server down")),
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde::Deserialize;

    use super::fake::{FakeBackend, ModelCheck, vector_for};
    use super::*;
    use crate::config::Config;
    use crate::store::Store;

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Summary {
        what: String,
        why: String,
    }

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
    struct Verdict {
        ok: bool,
    }

    const SUMMARY: &str = r#"{"what": "Loads users.", "why": "Login needs them."}"#;

    fn summary() -> Summary {
        Summary {
            what: "Loads users.".to_string(),
            why: "Login needs them.".to_string(),
        }
    }

    fn ollama(chat_model: &str) -> OllamaConfig {
        OllamaConfig {
            url: "http://localhost:11434".to_string(),
            chat_model: chat_model.to_string(),
            embedding_model: "embed".to_string(),
            reasoning_effort: "none".to_string(),
            temperature: 0.0,
        }
    }

    async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        (dir, store)
    }

    fn client(backend: &Arc<FakeBackend>, store: &Store, config: &OllamaConfig) -> LlmClient {
        LlmClient::new(backend.clone(), store.connect_cache().unwrap(), config)
    }

    async fn count(store: &Store, table: &str) -> i64 {
        let mut rows = store
            .cache()
            .query(&format!("SELECT COUNT(*) FROM {table}"), ())
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    }

    #[tokio::test]
    async fn repeated_completion_hits_the_cache() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));

        assert_eq!(llm.complete::<Summary>("prompt").await.unwrap(), summary());
        assert_eq!(llm.complete::<Summary>("prompt").await.unwrap(), summary());

        let fresh = client(&backend, &store, &ollama("chat"));
        assert_eq!(
            fresh.complete::<Summary>("prompt").await.unwrap(),
            summary()
        );

        assert_eq!(backend.chats().len(), 1);
        assert_eq!(
            llm.stats(),
            LlmStats {
                chat_calls: 1,
                chat_hits: 1,
                ..LlmStats::default()
            }
        );
        assert_eq!(fresh.stats().chat_calls, 0);
        assert_eq!(fresh.stats().chat_hits, 1);
        assert_eq!(count(&store, "llm_cache").await, 1);
    }

    #[tokio::test]
    async fn request_carries_model_schema_and_reasoning_effort() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));
        llm.complete::<Summary>("prompt").await.unwrap();

        let request = &backend.chats()[0];
        assert_eq!(request.model, "chat");
        assert_eq!(request.messages, vec![Message::user("prompt")]);
        assert_eq!(request.schema_name, "Summary");
        assert_eq!(request.schema["required"], json!(["what", "why"]));
        assert_eq!(request.schema["additionalProperties"], json!(false));
        assert_eq!(request.reasoning_effort.as_deref(), Some("none"));

        let body = chat_body(request);
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"],
            request.schema
        );
        assert_eq!(body["reasoning_effort"], "none");
        assert_eq!(body["temperature"], json!(0.0));
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[tokio::test]
    async fn empty_reasoning_effort_is_not_sent() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY]);
        let mut config = ollama("chat");
        config.reasoning_effort = String::new();
        let llm = client(&backend, &store, &config);
        llm.complete::<Summary>("prompt").await.unwrap();

        let request = &backend.chats()[0];
        assert_eq!(request.reasoning_effort, None);
        assert!(chat_body(request).get("reasoning_effort").is_none());
    }

    #[tokio::test]
    async fn changed_model_prompt_schema_effort_or_temperature_misses() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[
            SUMMARY,
            SUMMARY,
            r#"{"ok": true}"#,
            SUMMARY,
            SUMMARY,
            SUMMARY,
        ]);
        let llm = client(&backend, &store, &ollama("chat"));
        llm.complete::<Summary>("prompt").await.unwrap();
        llm.complete::<Summary>("other prompt").await.unwrap();
        assert_eq!(
            llm.complete::<Verdict>("prompt").await.unwrap(),
            Verdict { ok: true }
        );
        client(&backend, &store, &ollama("other-chat"))
            .complete::<Summary>("prompt")
            .await
            .unwrap();
        let mut thinking = ollama("chat");
        thinking.reasoning_effort = "high".to_string();
        client(&backend, &store, &thinking)
            .complete::<Summary>("prompt")
            .await
            .unwrap();
        let mut warmer = ollama("chat");
        warmer.temperature = 0.7;
        client(&backend, &store, &warmer)
            .complete::<Summary>("prompt")
            .await
            .unwrap();

        assert_eq!(backend.chats().len(), 6);
        assert_eq!(count(&store, "llm_cache").await, 6);
    }

    #[tokio::test]
    async fn invalid_reply_is_retried_once_with_the_error() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[r#"{"what": "Loads users."}"#, SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));

        assert_eq!(llm.complete::<Summary>("prompt").await.unwrap(), summary());

        let chats = backend.chats();
        assert_eq!(chats.len(), 2);
        let retry = &chats[1].messages;
        assert_eq!(retry.len(), 3);
        assert_eq!(retry[0], Message::user("prompt"));
        assert_eq!(retry[1], Message::assistant(r#"{"what": "Loads users."}"#));
        assert_eq!(retry[2].role, "user");
        assert!(
            retry[2].content.contains("missing field `why`"),
            "retry message should carry the error: {}",
            retry[2].content
        );
        assert_eq!(llm.stats().chat_retries, 1);
        assert_eq!(llm.stats().chat_calls, 2);

        assert_eq!(llm.complete::<Summary>("prompt").await.unwrap(), summary());
        assert_eq!(backend.chats().len(), 2, "the retried answer is cached");
    }

    #[tokio::test]
    async fn two_invalid_replies_fail_and_cache_nothing() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&["not json", r#"{"what": 1, "why": "x"}"#, SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));

        let err = llm.complete::<Summary>("prompt").await.unwrap_err();
        let invalid = err
            .downcast_ref::<InvalidOutput>()
            .expect("an InvalidOutput error");
        assert_eq!(invalid.type_name, "Summary");
        assert_eq!(invalid.output, r#"{"what": 1, "why": "x"}"#);
        assert!(err.to_string().contains("after 2 attempts"), "{err}");
        assert_eq!(backend.chats().len(), 2);
        assert_eq!(count(&store, "llm_cache").await, 0);

        assert_eq!(llm.complete::<Summary>("prompt").await.unwrap(), summary());
        assert_eq!(backend.chats().len(), 3, "a failure is not cached");
    }

    #[tokio::test]
    async fn backend_failure_is_returned_without_retry() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        let llm = client(&backend, &store, &ollama("chat"));

        let err = llm.complete::<Summary>("prompt").await.unwrap_err();
        assert!(format!("{err:#}").contains("no scripted reply"), "{err:#}");
        assert!(err.downcast_ref::<InvalidOutput>().is_none());
        assert_eq!(backend.chats().len(), 1);
        assert_eq!(count(&store, "llm_cache").await, 0);
    }

    fn is_unavailable(err: &anyhow::Error) -> bool {
        err.downcast_ref::<LlmUnavailable>().is_some()
    }

    #[tokio::test]
    async fn backend_failure_trips_the_breaker_for_every_later_completion() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));
        llm.complete::<Summary>("cached").await.unwrap();

        assert!(llm.complete::<Summary>("fails").await.is_err());
        assert!(llm.is_cache_only());
        backend.reply(&[SUMMARY]);
        let err = llm.complete::<Summary>("later").await.unwrap_err();
        assert!(is_unavailable(&err), "{err:#}");
        let err = llm.complete::<Verdict>("another stage").await.unwrap_err();
        assert!(is_unavailable(&err), "{err:#}");
        assert_eq!(
            llm.complete::<Summary>("cached").await.unwrap(),
            summary(),
            "cached completions are still served"
        );
        assert_eq!(backend.chats().len(), 2, "no chat call after the failure");
    }

    #[tokio::test]
    async fn backend_failure_on_the_retry_is_a_failure_not_invalid_output() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&["not json"]);
        let llm = client(&backend, &store, &ollama("chat"));

        let err = llm.complete::<Summary>("prompt").await.unwrap_err();

        assert!(err.downcast_ref::<InvalidOutput>().is_none(), "{err:#}");
        assert!(format!("{err:#}").contains("no scripted reply"), "{err:#}");
        assert_eq!(backend.chats().len(), 2);
        assert_eq!(llm.stats().chat_retries, 1);
        assert!(llm.is_cache_only(), "the failed retry trips the breaker");
        assert_eq!(count(&store, "llm_cache").await, 0);
    }

    #[tokio::test]
    async fn empty_reply_is_invalid_output_and_retried() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply_with(ChatReply::stop(""));
        backend.reply(&[SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));

        assert_eq!(llm.complete::<Summary>("prompt").await.unwrap(), summary());
        assert_eq!(llm.stats().chat_retries, 1);
        assert!(!llm.is_cache_only());
    }

    #[tokio::test]
    async fn consecutive_invalid_completions_trip_the_breaker() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        let llm = client(&backend, &store, &ollama("chat"));
        for round in 0..MAX_CONSECUTIVE_INVALID - 1 {
            backend.reply(&["x", "x"]);
            let err = llm
                .complete::<Summary>(&format!("bad {round}"))
                .await
                .unwrap_err();
            assert!(err.downcast_ref::<InvalidOutput>().is_some(), "{err:#}");
        }
        backend.reply(&[SUMMARY]);
        llm.complete::<Summary>("good").await.unwrap();
        assert!(!llm.is_cache_only(), "a valid reply resets the count");

        for round in 0..MAX_CONSECUTIVE_INVALID {
            assert!(!llm.is_cache_only());
            backend.reply(&["x", "x"]);
            let err = llm
                .complete::<Summary>(&format!("worse {round}"))
                .await
                .unwrap_err();
            assert!(err.downcast_ref::<InvalidOutput>().is_some(), "{err:#}");
        }
        assert!(llm.is_cache_only());
        let calls = backend.chats().len();
        let err = llm.complete::<Summary>("next").await.unwrap_err();
        assert!(is_unavailable(&err), "{err:#}");
        assert_eq!(backend.chats().len(), calls);
    }

    #[tokio::test]
    async fn cache_only_client_never_calls_the_backend() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY]);
        client(&backend, &store, &ollama("chat"))
            .complete::<Summary>("cached")
            .await
            .unwrap();
        let llm = client(&backend, &store, &ollama("chat"));
        llm.set_cache_only();
        backend.reply(&[SUMMARY]);

        assert_eq!(llm.complete::<Summary>("cached").await.unwrap(), summary());
        let err = llm.complete::<Summary>("missing").await.unwrap_err();
        assert!(is_unavailable(&err), "{err:#}");
        llm.check_chat_model().await.unwrap();
        assert!(backend.model_checks().is_empty());
        assert_eq!(backend.chats().len(), 1);
    }

    #[tokio::test]
    async fn chat_model_check_fails_on_a_missing_model_and_trips_on_errors() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        let llm = client(&backend, &store, &ollama("chat"));
        llm.check_chat_model().await.unwrap();
        assert!(!llm.is_cache_only());
        assert_eq!(backend.model_checks(), vec!["chat".to_string()]);

        backend.model(ModelCheck::Missing);
        let err = llm.check_chat_model().await.unwrap_err();
        assert!(
            format!("{err:#}").contains("the chat model \"chat\" does not exist"),
            "{err:#}"
        );

        backend.model(ModelCheck::Fails);
        llm.check_chat_model().await.unwrap();
        assert!(
            llm.is_cache_only(),
            "an unreachable server trips the breaker"
        );
    }

    #[test]
    fn negative_zero_temperature_is_the_same_key() {
        assert_eq!(
            completion_key("m", None, -0.0, "{}", "p"),
            completion_key("m", None, 0.0, "{}", "p")
        );
        assert_ne!(
            completion_key("m", None, 0.5, "{}", "p"),
            completion_key("m", None, 0.0, "{}", "p")
        );
        let mut config = ollama("chat");
        config.temperature = -0.0;
        assert!(normalized_temperature(config.temperature).is_sign_positive());
    }

    #[tokio::test]
    async fn backend_checks_a_model_with_api_show() {
        let missing = r#"{"error": "model 'nope:latest' not found"}"#;
        let (url, served) = serve(vec![(200, "{}"), (404, missing), (500, BUSY)]);
        let backend = quick_backend(&url);
        assert!(backend.has_model("chat").await.unwrap());
        assert!(!backend.has_model("nope").await.unwrap());
        let err = backend.has_model("chat").await.unwrap_err();
        assert!(format!("{err:#}").contains("HTTP 500"), "{err:#}");
        assert_eq!(served.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn undecodable_cache_row_is_a_miss() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY, SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));
        llm.complete::<Summary>("prompt").await.unwrap();
        store
            .cache()
            .execute("UPDATE llm_cache SET output = 'garbage'", ())
            .await
            .unwrap();

        assert_eq!(llm.complete::<Summary>("prompt").await.unwrap(), summary());
        assert_eq!(backend.chats().len(), 2);
        assert_eq!(llm.complete::<Summary>("prompt").await.unwrap(), summary());
        assert_eq!(backend.chats().len(), 2, "the bad row was rewritten");
    }

    fn not_blank(summary: &Summary) -> Result<(), String> {
        if summary.what.trim().is_empty() {
            return Err("`what` is blank".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn unknown_fields_and_failed_checks_are_retried() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[
            r#"{"what": "", "why": "x", "extra": 1}"#,
            r#"{"what": " ", "why": "x"}"#,
        ]);
        let llm = client(&backend, &store, &ollama("chat"));

        let err = llm
            .complete_with::<Summary, _>("prompt", not_blank)
            .await
            .unwrap_err();
        let invalid = err.downcast_ref::<InvalidOutput>().unwrap();
        assert!(invalid.error.contains("`what` is blank"), "{err}");
        let retry = &backend.chats()[1].messages[2].content;
        assert!(retry.contains("unknown field `extra`"), "{retry}");
        assert_eq!(count(&store, "llm_cache").await, 0);

        backend.reply(&[r#"{"what": "", "why": "x"}"#, SUMMARY]);
        assert_eq!(
            llm.complete_with::<Summary, _>("prompt", not_blank)
                .await
                .unwrap(),
            summary()
        );
        let retry = &backend.chats()[3].messages[2].content;
        assert!(retry.contains("`what` is blank"), "{retry}");
        assert_eq!(count(&store, "llm_cache").await, 1);
    }

    #[tokio::test]
    async fn cached_reply_failing_the_check_is_a_miss() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[r#"{"what": "", "why": "x"}"#, SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));
        llm.complete::<Summary>("prompt").await.unwrap();

        assert_eq!(
            llm.complete_with::<Summary, _>("prompt", not_blank)
                .await
                .unwrap(),
            summary()
        );
        assert_eq!(backend.chats().len(), 2);
        assert_eq!(llm.stats().chat_hits, 0);
    }

    #[tokio::test]
    async fn cached_with_never_calls_the_backend() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY, r#"{"what": "", "why": "x"}"#]);
        let llm = client(&backend, &store, &ollama("chat"));
        llm.complete::<Summary>("prompt").await.unwrap();
        llm.complete::<Summary>("blank").await.unwrap();
        let before = llm.stats();

        assert_eq!(
            llm.cached_with::<Summary, _>("prompt", not_blank)
                .await
                .unwrap(),
            Some(summary())
        );
        assert_eq!(
            llm.cached_with::<Summary, _>("missing", not_blank)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            llm.cached_with::<Summary, _>("blank", not_blank)
                .await
                .unwrap(),
            None,
            "a cached row failing the check is a miss"
        );
        let stats = llm.stats().since(&before);
        assert_eq!(stats.chat_calls, 0);
        assert_eq!(stats.chat_hits, 1);
        assert_eq!(backend.chats().len(), 2);
    }

    #[tokio::test]
    async fn truncated_reply_is_named_in_the_error() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        for _ in 0..MAX_ATTEMPTS {
            backend.reply_with(ChatReply {
                content: r#"{"what": "Loads"#.to_string(),
                finish_reason: Some("length".to_string()),
                prompt_tokens: Some(4000),
                completion_tokens: Some(96),
            });
        }
        let llm = client(&backend, &store, &ollama("chat"));

        let err = llm.complete::<Summary>("prompt").await.unwrap_err();
        let invalid = err.downcast_ref::<InvalidOutput>().unwrap();
        assert_eq!(invalid.finish_reason.as_deref(), Some("length"));
        assert!(
            err.to_string().contains("cut off at the token limit"),
            "{err}"
        );

        backend.reply(&["not json", "still not json"]);
        let err = llm.complete::<Summary>("prompt").await.unwrap_err();
        assert!(!err.to_string().contains("cut off"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_tasks_share_one_client_and_cache_everything() {
        const TASKS: usize = 16;
        const TEXTS: usize = 200;
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&vec![SUMMARY; TASKS]);
        let texts = |task: usize| -> Vec<String> {
            (0..TEXTS)
                .map(|i| format!("task {task} text {i}"))
                .collect()
        };

        let run = |llm: Arc<LlmClient>| async move {
            let handles: Vec<_> = (0..TASKS)
                .map(|task| {
                    let llm = llm.clone();
                    let texts = texts(task);
                    tokio::spawn(async move {
                        let vectors = llm.embed(&texts).await.unwrap();
                        assert_eq!(vectors[7], vector_for(&texts[7]));
                        llm.complete::<Summary>(&format!("prompt {task}"))
                            .await
                            .unwrap()
                    })
                })
                .collect();
            for handle in handles {
                assert_eq!(handle.await.unwrap(), summary());
            }
            llm.stats()
        };

        let llm = Arc::new(client(&backend, &store, &ollama("chat")));
        let cold = run(llm).await;
        assert_eq!(cold.chat_calls, TASKS);
        assert_eq!(cold.embed_texts, TASKS * TEXTS);
        assert_eq!(
            count(&store, "embedding_cache").await,
            (TASKS * TEXTS) as i64
        );
        assert_eq!(count(&store, "llm_cache").await, TASKS as i64);

        let warm = run(Arc::new(client(&backend, &store, &ollama("chat")))).await;
        assert_eq!(warm.chat_calls, 0);
        assert_eq!(warm.embed_calls, 0);
        assert_eq!(warm.chat_hits, TASKS);
        assert_eq!(warm.embed_hits, TASKS * TEXTS);
    }

    #[tokio::test]
    async fn client_writes_survive_a_rollback_on_the_shared_connection() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY]);
        let llm = client(&backend, &store, &ollama("chat"));

        let transaction = store.cache().transaction().await.unwrap();
        llm.complete::<Summary>("prompt").await.unwrap();
        llm.embed(&["alpha", "beta"]).await.unwrap();
        transaction.rollback().await.unwrap();

        assert_eq!(count(&store, "llm_cache").await, 1);
        assert_eq!(count(&store, "embedding_cache").await, 2);
    }

    #[tokio::test]
    async fn embed_caches_per_text_and_sends_only_misses() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        let llm = client(&backend, &store, &ollama("chat"));

        let first = llm.embed(&["alpha", "beta"]).await.unwrap();
        assert_eq!(first, vec![vector_for("alpha"), vector_for("beta")]);

        let second = llm
            .embed(&["gamma", "alpha", "gamma", "beta"])
            .await
            .unwrap();
        assert_eq!(
            second,
            vec![
                vector_for("gamma"),
                vector_for("alpha"),
                vector_for("gamma"),
                vector_for("beta"),
            ]
        );
        assert_eq!(
            backend.batches(),
            vec![
                vec!["alpha".to_string(), "beta".to_string()],
                vec!["gamma".to_string()],
            ]
        );
        assert_eq!(
            llm.stats(),
            LlmStats {
                embed_calls: 2,
                embed_texts: 3,
                embed_hits: 2,
                ..LlmStats::default()
            }
        );
        assert_eq!(count(&store, "embedding_cache").await, 3);

        let mut other = ollama("chat");
        other.embedding_model = "other-embed".to_string();
        client(&backend, &store, &other)
            .embed(&["alpha"])
            .await
            .unwrap();
        assert_eq!(backend.batches().len(), 3, "another model misses");
    }

    #[tokio::test]
    async fn embed_of_nothing_sends_nothing() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        let llm = client(&backend, &store, &ollama("chat"));

        let empty: [&str; 0] = [];
        assert!(llm.embed(&empty).await.unwrap().is_empty());
        assert!(backend.batches().is_empty());
        assert_eq!(llm.stats(), LlmStats::default());
    }

    #[tokio::test]
    async fn embed_batches_large_inputs() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        let llm = client(&backend, &store, &ollama("chat"));

        let texts: Vec<String> = (0..EMBED_BATCH + 1).map(|i| format!("text {i}")).collect();
        let vectors = llm.embed(&texts).await.unwrap();
        assert_eq!(vectors.len(), texts.len());
        assert_eq!(vectors[EMBED_BATCH], vector_for(&texts[EMBED_BATCH]));
        let sizes: Vec<usize> = backend.batches().iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![EMBED_BATCH, 1]);
    }

    #[tokio::test]
    async fn cached_embedding_with_a_wrong_dim_is_a_miss() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        let llm = client(&backend, &store, &ollama("chat"));
        llm.embed(&["alpha"]).await.unwrap();
        store
            .cache()
            .execute("UPDATE embedding_cache SET dim = 5", ())
            .await
            .unwrap();

        assert_eq!(
            llm.embed(&["alpha"]).await.unwrap(),
            vec![vector_for("alpha")]
        );
        assert_eq!(backend.batches().len(), 2);
        llm.embed(&["alpha"]).await.unwrap();
        assert_eq!(backend.batches().len(), 2, "the bad row was rewritten");
    }

    #[tokio::test]
    async fn embedding_lengths_must_agree() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        let llm = client(&backend, &store, &ollama("chat"));
        llm.embed(&["alpha"]).await.unwrap();
        backend.short("beta");
        backend.short("gamma");

        let err = llm.embed(&["alpha", "beta"]).await.unwrap_err();
        assert!(err.to_string().contains("differ in length"), "{err}");
        let err = llm.embed(&["delta", "gamma"]).await.unwrap_err();
        assert!(err.to_string().contains("differ in length"), "{err}");
        assert_eq!(
            count(&store, "embedding_cache").await,
            1,
            "nothing mixed is cached"
        );
    }

    #[test]
    fn schema_key_ignores_key_order_and_dialect() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {"why": {"type": "string"}, "what": {"type": "string"}},
            "required": ["what", "why"]
        });
        let reordered = json!({
            "required": ["what", "why"],
            "properties": {"what": {"type": "string"}, "why": {"type": "string"}},
            "type": "object",
            "$schema": "http://json-schema.org/draft-07/schema#"
        });
        assert_eq!(canonical_schema(&schema), canonical_schema(&reordered));
        assert_eq!(
            canonical_schema(&schema),
            r#"{"properties":{"what":{"type":"string"},"why":{"type":"string"}},"required":["what","why"],"type":"object"}"#
        );
    }

    /// A one-shot HTTP server on localhost answering one connection per
    /// `(status, body)`; returns its URL and the number of requests served.
    fn serve(responses: Vec<(u16, &'static str)>) -> (String, Arc<AtomicUsize>) {
        use std::io::{BufRead, BufReader, Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = Arc::new(AtomicUsize::new(0));
        let counter = served.clone();
        std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut request_body = vec![0; length];
                reader.read_exact(&mut request_body).unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (url, served)
    }

    fn quick_backend(url: &str) -> OllamaBackend {
        let mut backend = OllamaBackend::new(url).unwrap();
        backend.retry_backoff = Duration::ZERO;
        backend
    }

    const BUSY: &str = r#"{"error": {"message": "server busy"}}"#;
    const EMBEDDED: &str = r#"{"data": [{"embedding": [0.5], "index": 0}]}"#;

    #[tokio::test]
    async fn backend_retries_a_503_once() {
        let texts = ["x".to_string()];
        let (url, served) = serve(vec![(503, BUSY), (200, EMBEDDED)]);
        let vectors = quick_backend(&url).embed("m", &texts).await.unwrap();
        assert_eq!(vectors, vec![vec![0.5]]);
        assert_eq!(served.load(Ordering::SeqCst), 2);

        let (url, served) = serve(vec![(503, BUSY), (503, BUSY), (200, EMBEDDED)]);
        let err = quick_backend(&url).embed("m", &texts).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("HTTP 503: server busy"),
            "{err:#}"
        );
        assert_eq!(served.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn backend_does_not_retry_client_errors() {
        let texts = ["x".to_string()];
        let missing = r#"{"error": {"message": "model 'm' not found"}}"#;
        let (url, served) = serve(vec![(404, missing), (200, EMBEDDED)]);
        let err = quick_backend(&url).embed("m", &texts).await.unwrap_err();
        assert!(format!("{err:#}").contains("HTTP 404"), "{err:#}");
        assert_eq!(served.load(Ordering::SeqCst), 1);
        assert!(!transient_status(reqwest::StatusCode::BAD_REQUEST));
        assert!(!transient_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(transient_status(reqwest::StatusCode::SERVICE_UNAVAILABLE));
    }

    #[tokio::test]
    async fn backend_retries_a_refused_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let backend = quick_backend(&url);
        let err = backend
            .post_once(&backend.url("embeddings"), &json!({}))
            .await
            .unwrap_err();
        assert!(err.1, "a refused connection is transient: {:#}", err.0);
        let err = backend.embed("m", &["x".to_string()]).await.unwrap_err();
        assert!(format!("{err:#}").contains("calling"), "{err:#}");
    }

    #[test]
    fn vectors_round_trip_as_little_endian_f32() {
        let vector = vec![1.5, -0.25, f32::MIN_POSITIVE];
        let bytes = encode_vector(&vector);
        assert_eq!(bytes.len(), 12);
        assert_eq!(&bytes[..4], &1.5f32.to_le_bytes());
        assert_eq!(decode_vector(&bytes).unwrap(), vector);
        assert!(decode_vector(&bytes[..5]).is_err());
        assert!(decode_vector(&[]).is_err());
    }

    #[test]
    fn keys_separate_their_parts() {
        assert_ne!(
            completion_key("ab", None, 0.0, "s", "p"),
            completion_key("a", Some("b"), 0.0, "s", "p")
        );
        assert_ne!(embedding_key("m", "ab"), embedding_key("ma", "b"));
        assert_eq!(embedding_key("m", "t"), embedding_key("m", "t"));
    }

    #[test]
    fn schema_names_are_sanitized() {
        assert_eq!(schema_name("Summary"), "Summary");
        assert_eq!(schema_name("Vec<String>"), "Vec_String_");
        assert_eq!(schema_name(""), "response");
    }

    #[test]
    fn parses_a_chat_response_and_ignores_reasoning() {
        let response = json!({
            "id": "chatcmpl-299",
            "object": "chat.completion",
            "model": "qwen3.8:latest",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\n  \"sentence\": \"The sky is blue.\"\n}",
                    "reasoning": "The user wants a single short sentence."
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 21, "completion_tokens": 82, "total_tokens": 103}
        });
        assert_eq!(
            parse_chat_response(&response).unwrap(),
            ChatReply {
                content: "{\n  \"sentence\": \"The sky is blue.\"\n}".to_string(),
                finish_reason: Some("stop".to_string()),
                prompt_tokens: Some(21),
                completion_tokens: Some(82),
            }
        );
        let cut = json!({"choices": [{"message": {"content": "{"}, "finish_reason": "length"}]});
        let cut = parse_chat_response(&cut).unwrap();
        assert!(cut.truncated());
        assert_eq!(cut.prompt_tokens, None);
        assert!(parse_chat_response(&json!({"choices": []})).is_err());
        for message in [json!({}), json!({"content": null})] {
            let reply = parse_chat_response(&json!({"choices": [{"message": message}]}))
                .expect("no content is an empty reply, not a backend failure");
            assert_eq!(reply.content, "");
        }
    }

    #[test]
    fn parses_embeddings_in_index_order() {
        let response = json!({
            "object": "list",
            "data": [
                {"object": "embedding", "embedding": [0.5, 0.25], "index": 1},
                {"object": "embedding", "embedding": [-1.0, 2.0], "index": 0}
            ],
            "model": "bge-m3:latest"
        });
        assert_eq!(
            parse_embeddings_response(&response, 2).unwrap(),
            vec![vec![-1.0, 2.0], vec![0.5, 0.25]]
        );
        assert!(parse_embeddings_response(&response, 3).is_err());
        let duplicate = json!({"data": [
            {"embedding": [1.0], "index": 0},
            {"embedding": [1.0], "index": 0}
        ]});
        assert!(parse_embeddings_response(&duplicate, 2).is_err());
        let empty = json!({"data": [{"embedding": [], "index": 0}]});
        assert!(parse_embeddings_response(&empty, 1).is_err());
    }

    #[test]
    fn error_message_prefers_the_openai_error() {
        let not_found = br#"{"error":{"message":"model 'nope:latest' not found","type":"not_found_error","param":null,"code":null}}"#;
        assert_eq!(error_message(not_found), "model 'nope:latest' not found");
        assert_eq!(error_message(br#"{"error":"boom"}"#), "boom");
        assert_eq!(error_message(b" plain text "), "plain text");
    }

    #[test]
    fn backend_url_is_validated_and_normalized() {
        let backend = OllamaBackend::new("http://localhost:11434/v1/").unwrap();
        assert_eq!(
            backend.url("chat/completions"),
            "http://localhost:11434/v1/chat/completions"
        );
        let backend = OllamaBackend::new(" http://host:11434 ").unwrap();
        assert_eq!(backend.url("embeddings"), "http://host:11434/v1/embeddings");
        assert!(OllamaBackend::new("").is_err());
        assert!(OllamaBackend::new("ftp://host").is_err());
    }

    /// `[ollama]` from `annatar.toml`, overridden by `ANNATAR_OLLAMA_URL`,
    /// `ANNATAR_OLLAMA_CHAT_MODEL`, `ANNATAR_OLLAMA_EMBEDDING_MODEL` and
    /// `ANNATAR_OLLAMA_REASONING_EFFORT`.
    fn live_config() -> OllamaConfig {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("annatar.toml");
        let file = Config::load(&path).ok().and_then(|config| config.ollama);
        let pick = |var: &str, from_file: Option<String>| {
            std::env::var(var)
                .ok()
                .or(from_file)
                .unwrap_or_else(|| panic!("set {var} or the [ollama] section in annatar.toml"))
        };
        OllamaConfig {
            url: pick(
                "ANNATAR_OLLAMA_URL",
                file.as_ref().map(|ollama| ollama.url.clone()),
            ),
            chat_model: pick(
                "ANNATAR_OLLAMA_CHAT_MODEL",
                file.as_ref().map(|ollama| ollama.chat_model.clone()),
            ),
            embedding_model: pick(
                "ANNATAR_OLLAMA_EMBEDDING_MODEL",
                file.as_ref().map(|ollama| ollama.embedding_model.clone()),
            ),
            reasoning_effort: std::env::var("ANNATAR_OLLAMA_REASONING_EFFORT")
                .ok()
                .or(file.map(|ollama| ollama.reasoning_effort))
                .unwrap_or_else(|| crate::config::DEFAULT_REASONING_EFFORT.to_string()),
            temperature: crate::config::DEFAULT_TEMPERATURE,
        }
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    struct ClassSummary {
        /// One sentence: what the class does.
        what: String,
        /// The Spring stereotype that fits best.
        role: Role,
    }

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
    #[serde(rename_all = "lowercase")]
    enum Role {
        Controller,
        Service,
        Repository,
        Other,
    }

    #[tokio::test]
    #[ignore = "needs a running Ollama"]
    async fn completes_a_real_struct_and_caches_it() {
        let config = live_config();
        let (_dir, store) = store().await;
        let llm = LlmClient::from_config(&config, store.connect_cache().unwrap()).unwrap();
        let prompt = "Summarise this Java class for a code index.\n\n\
            @Repository\npublic interface UserRepository extends JpaRepository<User, Long> {\n\
            \x20   Optional<User> findByEmail(String email);\n}";

        let start = Instant::now();
        let first = llm.complete::<ClassSummary>(prompt).await.unwrap();
        let cold = start.elapsed();
        let start = Instant::now();
        let second = llm.complete::<ClassSummary>(prompt).await.unwrap();
        let warm = start.elapsed();

        assert!(!first.what.trim().is_empty());
        assert_eq!(first.role, Role::Repository);
        assert_eq!(second.what, first.what);
        let stats = llm.stats();
        assert_eq!(stats.chat_hits, 1);
        assert_eq!(stats.chat_calls, 1 + stats.chat_retries);
        println!(
            "model {} (reasoning_effort {:?}): {first:?}; cold {cold:?}, cached {warm:?}, {stats:?}",
            config.chat_model, config.reasoning_effort
        );
    }

    #[tokio::test]
    #[ignore = "needs a running Ollama"]
    async fn embeds_real_texts_and_caches_them() {
        let config = live_config();
        let (_dir, store) = store().await;
        let llm = LlmClient::from_config(&config, store.connect_cache().unwrap()).unwrap();
        let texts = ["Loads a user by email.", "Sends the invoice by mail.", ""];

        let start = Instant::now();
        let vectors = llm.embed(&texts).await.unwrap();
        let cold = start.elapsed();
        let start = Instant::now();
        let again = llm.embed(&texts).await.unwrap();
        let warm = start.elapsed();

        assert_eq!(vectors.len(), texts.len());
        let dim = vectors[0].len();
        assert!(dim > 0);
        assert!(vectors.iter().all(|vector| vector.len() == dim));
        assert_eq!(again, vectors);
        assert_eq!(llm.stats().embed_calls, 1);
        assert_eq!(llm.stats().embed_hits, texts.len());
        println!(
            "model {}: {} vectors of {dim} dims; cold {cold:?}, cached {warm:?}",
            config.embedding_model,
            vectors.len()
        );
    }
}
