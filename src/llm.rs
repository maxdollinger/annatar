//! A typed, cached LLM client over Ollama's OpenAI-compatible `/v1` API.
//!
//! [`LlmClient::complete`] turns a prompt into a `T: DeserializeOwned +
//! JsonSchema`: the `schemars` schema of `T` is sent as the structured-output
//! format (`response_format` of type `json_schema`), and the reply must
//! deserialize into `T`. An invalid reply is retried once, with the bad reply
//! and the error appended to the conversation; a second invalid reply is an
//! [`InvalidOutput`] error and nothing is cached. [`LlmClient::embed`] returns
//! one vector per text, sending only the texts the cache does not hold.
//!
//! Both go through `cache.db`: `llm_cache` keyed by a hash of chat model,
//! reasoning effort, schema and prompt (the prompt is the full text, input
//! included), `embedding_cache` keyed by a hash of embedding model and text.
//! A changed key simply misses. Cache faults degrade: an unreadable or
//! undecodable row is a miss, a failed write warns and keeps the result.
//!
//! [`LlmBackend`] is the seam the client calls through; [`OllamaBackend`]
//! implements it with `reqwest`, tests use a fake. [`LlmStats`] counts backend
//! calls and cache hits, so a caller can show that a rerun made no LLM calls.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use libsql::{Connection, params};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::config::OllamaConfig;

/// Per-request timeout; a large local model answering a long prompt is slow.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Timeout for establishing the connection, so an unreachable host fails
/// fast instead of using the whole request timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How much of a non-success response body is kept for the error message.
const MAX_ERROR_BODY: usize = 4 * 1024;

/// Chat attempts per completion: the first and one retry on invalid output.
pub const MAX_ATTEMPTS: usize = 2;

/// Texts per embedding request.
pub const EMBED_BATCH: usize = 64;

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
}

/// The future an [`LlmBackend`] method returns: boxed so the trait stays
/// object-safe.
pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// The calls [`LlmClient`] makes. No caching, no validation, no retries.
pub trait LlmBackend: Send + Sync {
    /// The assistant's reply text for `request`.
    fn chat<'a>(&'a self, request: &'a ChatRequest) -> BackendFuture<'a, String>;

    /// One vector per text, in input order. `texts` is never empty.
    fn embed<'a>(&'a self, model: &'a str, texts: &'a [String])
    -> BackendFuture<'a, Vec<Vec<f32>>>;
}

/// Ollama's OpenAI-compatible API: `/v1/chat/completions` and
/// `/v1/embeddings`.
pub struct OllamaBackend {
    http: reqwest::Client,
    base_url: String,
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
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/{path}", self.base_url)
    }

    /// `POST` `body` to `path` and return the JSON of a 2xx answer; any other
    /// status is an error carrying Ollama's message.
    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let url = self.url(path);
        let mut response = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("calling {url}"))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .with_context(|| format!("reading the response of {url}"));
        }
        let mut error_body = Vec::new();
        while error_body.len() < MAX_ERROR_BODY {
            let Ok(Some(chunk)) = response.chunk().await else {
                break;
            };
            let take = chunk.len().min(MAX_ERROR_BODY - error_body.len());
            error_body.extend_from_slice(&chunk[..take]);
        }
        bail!(
            "{url} answered HTTP {}: {}",
            status.as_u16(),
            error_message(&error_body)
        )
    }
}

impl LlmBackend for OllamaBackend {
    fn chat<'a>(&'a self, request: &'a ChatRequest) -> BackendFuture<'a, String> {
        Box::pin(async move {
            let response = self.post("chat/completions", &chat_body(request)).await?;
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
            let response = self.post("embeddings", &body).await?;
            parse_embeddings_response(&response, texts.len())
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
    });
    if let Some(effort) = &request.reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    body
}

/// The reply text of a chat completion. A thinking model's reasoning arrives
/// in `message.reasoning` and is ignored.
fn parse_chat_response(response: &Value) -> Result<String> {
    let choice = response
        .pointer("/choices/0")
        .ok_or_else(|| anyhow!("chat response has no choices"))?;
    let content = choice
        .pointer("/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("chat response has no message content"))?;
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        tracing::debug!("chat reply was cut off at the token limit");
    }
    Ok(content.to_string())
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

/// The model's reply still did not deserialize into the requested type after
/// [`MAX_ATTEMPTS`] attempts. Nothing was cached.
#[derive(Debug)]
pub struct InvalidOutput {
    pub type_name: String,
    pub error: String,
    pub output: String,
}

impl fmt::Display for InvalidOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the model's reply is not a valid {} after {MAX_ATTEMPTS} attempts: {}",
            self.type_name, self.error
        )
    }
}

impl std::error::Error for InvalidOutput {}

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

#[derive(Default)]
struct Counters {
    chat_calls: AtomicUsize,
    chat_hits: AtomicUsize,
    chat_retries: AtomicUsize,
    embed_calls: AtomicUsize,
    embed_texts: AtomicUsize,
    embed_hits: AtomicUsize,
}

/// The cached, typed client. Cheap to share by reference across tasks.
pub struct LlmClient {
    backend: Arc<dyn LlmBackend>,
    cache: Connection,
    chat_model: String,
    embedding_model: String,
    reasoning_effort: Option<String>,
    counters: Counters,
}

impl LlmClient {
    /// A client calling `backend` with the models and reasoning effort from
    /// `config`, caching in `cache` (typically [`crate::store::Store::cache`]).
    pub fn new(backend: Arc<dyn LlmBackend>, cache: Connection, config: &OllamaConfig) -> Self {
        Self {
            backend,
            cache,
            chat_model: config.chat_model.clone(),
            embedding_model: config.embedding_model.clone(),
            reasoning_effort: Some(config.reasoning_effort.clone())
                .filter(|effort| !effort.is_empty()),
            counters: Counters::default(),
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

    /// Answer `prompt` with a `T`, from the cache or the chat model. See the
    /// module docs for retries and caching. A reply that is still invalid
    /// after the retry is an [`InvalidOutput`] error; backend failures are
    /// returned as is and not retried.
    pub async fn complete<T: DeserializeOwned + JsonSchema>(&self, prompt: &str) -> Result<T> {
        let schema =
            serde_json::to_value(schemars::schema_for!(T)).context("serializing schema")?;
        let schema_text = serde_json::to_string(&schema).context("serializing schema")?;
        let type_name = T::schema_name();
        let key = completion_key(
            &self.chat_model,
            self.reasoning_effort.as_deref(),
            &schema_text,
            prompt,
        );

        match self.cached_completion(&key).await {
            Ok(Some(output)) => match serde_json::from_str::<T>(&output) {
                Ok(value) => {
                    self.counters.chat_hits.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(r#type = %type_name, "llm cache hit");
                    return Ok(value);
                }
                Err(err) => {
                    tracing::warn!(r#type = %type_name, "llm cache row does not decode, treating as a miss: {err}");
                }
            },
            Ok(None) => {}
            Err(err) => tracing::warn!("reading llm cache, treating as a miss: {err:#}"),
        }

        let mut request = ChatRequest {
            model: self.chat_model.clone(),
            messages: vec![Message::user(prompt)],
            schema_name: schema_name(&type_name),
            schema,
            reasoning_effort: self.reasoning_effort.clone(),
        };
        let mut attempt = 1;
        loop {
            self.counters.chat_calls.fetch_add(1, Ordering::Relaxed);
            let output = self
                .backend
                .chat(&request)
                .await
                .context("chat completion")?;
            match serde_json::from_str::<T>(&output) {
                Ok(value) => {
                    if let Err(err) = self.store_completion(&key, output.trim()).await {
                        tracing::warn!("writing llm cache: {err:#}");
                    }
                    return Ok(value);
                }
                Err(err) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!(r#type = %type_name, attempt, "invalid model reply, retrying: {err}");
                    self.counters.chat_retries.fetch_add(1, Ordering::Relaxed);
                    request.messages.push(Message::assistant(output));
                    request.messages.push(Message::user(retry_prompt(&err)));
                    attempt += 1;
                }
                Err(err) => {
                    return Err(InvalidOutput {
                        type_name: type_name.into_owned(),
                        error: err.to_string(),
                        output,
                    }
                    .into());
                }
            }
        }
    }

    /// One vector per text, in input order. Cached texts are not sent;
    /// repeated texts are sent once; the rest go out in batches of
    /// [`EMBED_BATCH`].
    pub async fn embed<S: AsRef<str>>(&self, texts: &[S]) -> Result<Vec<Vec<f32>>> {
        let mut vectors: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
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
                Ok(Some(vector)) => vectors[position] = Some(vector),
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
                "SELECT vector FROM embedding_cache WHERE key = ?1",
                params![key],
            )
            .await?;
        match rows.next().await? {
            Some(row) => decode_vector(&row.get::<Vec<u8>>(0)?).map(Some),
            None => Ok(None),
        }
    }

    async fn store_embeddings(&self, rows: &[(&str, &[f32])]) -> Result<()> {
        let tx = self.cache.transaction().await?;
        for (key, vector) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO embedding_cache (key, model, dim, vector)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    *key,
                    self.embedding_model.as_str(),
                    vector.len() as i64,
                    encode_vector(vector)
                ],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// The follow-up message after an invalid reply.
fn retry_prompt(error: &serde_json::Error) -> String {
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

/// `llm_cache` key: chat model, reasoning effort, schema and prompt.
fn completion_key(
    model: &str,
    reasoning_effort: Option<&str>,
    schema: &str,
    prompt: &str,
) -> String {
    hash_parts(&[
        "completion",
        model,
        reasoning_effort.unwrap_or(""),
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
    /// script is an error) and embeds a text as `[chars, first byte, 1.0]`.
    /// It records every chat request and embedding batch.
    #[derive(Default)]
    pub struct FakeBackend {
        replies: Mutex<VecDeque<String>>,
        chats: Mutex<Vec<ChatRequest>>,
        batches: Mutex<Vec<Vec<String>>>,
    }

    impl FakeBackend {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub fn reply(&self, replies: &[&str]) {
            self.replies
                .lock()
                .unwrap()
                .extend(replies.iter().map(|reply| reply.to_string()));
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
        fn chat<'a>(&'a self, request: &'a ChatRequest) -> BackendFuture<'a, String> {
            Box::pin(async move {
                self.chats.lock().unwrap().push(request.clone());
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
                Ok(texts.iter().map(|text| vector_for(text)).collect())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde::Deserialize;

    use super::fake::{FakeBackend, vector_for};
    use super::*;
    use crate::config::Config;
    use crate::store::Store;

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
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
        }
    }

    async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        (dir, store)
    }

    fn client(backend: &Arc<FakeBackend>, store: &Store, config: &OllamaConfig) -> LlmClient {
        LlmClient::new(backend.clone(), store.cache().clone(), config)
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
        assert_eq!(request.reasoning_effort.as_deref(), Some("none"));

        let body = chat_body(request);
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"],
            request.schema
        );
        assert_eq!(body["reasoning_effort"], "none");
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
    async fn changed_model_prompt_schema_or_effort_misses() {
        let (_dir, store) = store().await;
        let backend = FakeBackend::new();
        backend.reply(&[SUMMARY, SUMMARY, r#"{"ok": true}"#, SUMMARY, SUMMARY]);
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

        assert_eq!(backend.chats().len(), 5);
        assert_eq!(count(&store, "llm_cache").await, 5);
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
            completion_key("ab", None, "s", "p"),
            completion_key("a", Some("b"), "s", "p")
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
            "{\n  \"sentence\": \"The sky is blue.\"\n}"
        );
        assert!(parse_chat_response(&json!({"choices": []})).is_err());
        assert!(parse_chat_response(&json!({"choices": [{"message": {}}]})).is_err());
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
        let llm = LlmClient::from_config(&config, store.cache().clone()).unwrap();
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
        let llm = LlmClient::from_config(&config, store.cache().clone()).unwrap();
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
