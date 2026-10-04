//! Search symbol descriptions by meaning: `annatar search` (5.2).
//!
//! The query is embedded with the configured embedding model and compared
//! with the vectors the embeddings stage stored in `symbol_vectors`. Without
//! a filter the vector index answers ([`schema::VECTOR_INDEX`] through
//! `vector_top_k`); with a kind, role or path filter the filtered rows are scanned
//! exactly with `vector_distance_cos`, because `vector_top_k` cannot filter
//! and returns at most about 200 rows. Every hit is scored with the exact
//! cosine similarity, highest first.
//!
//! Search reads `index.db` only. The query's embedding is cached in memory
//! for the call, never in `cache.db`, so a search creates and writes no file.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use libsql::{Builder, Connection, Database, Value, params};

use crate::config::OllamaConfig;
use crate::llm::{LlmBackend, LlmClient, OllamaBackend};
use crate::schema;
use crate::symbols::{Role, SymbolKind};
use crate::walk;

/// Hits shown when `--limit` is not given.
pub const DEFAULT_LIMIT: usize = 10;

/// Largest accepted `--limit`, well below the ≈ 200 rows `vector_top_k`
/// returns at most.
pub const MAX_LIMIT: usize = 100;

/// Which symbols a search may return; empty lists allow everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Any of these kinds.
    pub kinds: Vec<SymbolKind>,
    /// Any of these roles: a type's own role, a method's or constructor's
    /// enclosing type's role.
    pub roles: Vec<Role>,
    /// Only symbols in this file or under this directory, relative to the
    /// repository with `/` separators (`--path`).
    pub path: Option<String>,
}

impl Filter {
    fn is_empty(&self) -> bool {
        self.kinds.is_empty() && self.roles.is_empty() && self.path.is_none()
    }
}

/// [`Filter::path`] for `--path <prefix>`: the prefix relative to `repo`,
/// with `/` separators like `symbols.file`, normalised like `index --path`
/// ([`walk::relative_prefix`]). The repository itself (`.`, its absolute
/// path) is no filter; a prefix outside the repository is an error.
pub fn path_filter(repo: &Path, prefix: &Path) -> Result<Option<String>> {
    let Some(relative) = walk::relative_prefix(repo, prefix) else {
        bail!(
            "--path {} is outside the repository {}",
            prefix.display(),
            repo.display()
        );
    };
    let path = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Ok((!path.is_empty()).then_some(path))
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Cosine similarity of the query and the symbol's vector, 1 is identical.
    pub score: f64,
    pub fqn: String,
    pub kind: String,
    /// The symbol's own role (types only).
    pub role: Option<String>,
    pub file: String,
    pub start_line: i64,
    pub end_line: i64,
    pub description: Option<String>,
}

/// What the index's vectors were built with (`index_meta`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingMeta {
    pub model: String,
    pub dim: usize,
}

/// The query side of a search: an [`LlmClient`] whose embedding cache lives
/// in memory, so embedding a query writes nothing to disk.
pub struct QueryEmbedder {
    client: LlmClient,
    // Anchors the in-memory cache connection.
    _db: Database,
}

impl QueryEmbedder {
    /// An embedder calling `backend` with `config.embedding_model`.
    pub async fn new(backend: Arc<dyn LlmBackend>, config: &OllamaConfig) -> Result<Self> {
        let db = Builder::new_local(":memory:")
            .build()
            .await
            .context("opening the in-memory embedding cache")?;
        let cache = db
            .connect()
            .context("connecting to the in-memory embedding cache")?;
        schema::create_cache(&cache).await?;
        Ok(Self {
            client: LlmClient::new(backend, cache, config),
            _db: db,
        })
    }

    /// An embedder over [`OllamaBackend`] at `config.url`.
    pub async fn from_config(config: &OllamaConfig) -> Result<Self> {
        Self::new(Arc::new(OllamaBackend::new(&config.url)?), config).await
    }

    /// The configured embedding model.
    pub fn model(&self) -> &str {
        self.client.embedding_model()
    }

    async fn embed(&self, query: &str) -> Result<Vec<f32>> {
        let mut vectors = self
            .client
            .embed(&[query])
            .await
            .with_context(|| format!("embedding the query with {:?}", self.model()))?;
        vectors
            .pop()
            .context("the embedding model returned no vector")
    }
}

/// Embed `query` and return up to `limit` symbols of `conn`'s index that
/// match `filter`, most similar first. Fails before calling the model when
/// the index has no vectors or was embedded with another model, and when the
/// query's vector has another length than the index's.
pub async fn search(
    conn: &Connection,
    embedder: &QueryEmbedder,
    query: &str,
    filter: &Filter,
    limit: usize,
) -> Result<Vec<Hit>> {
    let query = query.trim();
    if query.is_empty() {
        bail!("the query is empty");
    }
    let meta = embedding_meta(conn).await?;
    if meta.model != embedder.model() {
        bail!(
            "the index was embedded with {:?} but ollama.embedding_model is {:?}; search with the same model or rebuild the index",
            meta.model,
            embedder.model()
        );
    }
    let started = std::time::Instant::now();
    let vector = embedder.embed(query).await?;
    tracing::info!(elapsed_ms = started.elapsed().as_millis(), "embedded query");
    if vector.len() != meta.dim {
        bail!(
            "the query embedding has {} values but the index's vectors have {}; did the embedding model {:?} change? Rebuild the index",
            vector.len(),
            meta.dim,
            meta.model
        );
    }
    let started = std::time::Instant::now();
    let hits = nearest(conn, &vector, filter, limit).await?;
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        exact = !filter.is_empty(),
        "queried vectors"
    );
    Ok(hits)
}

/// The embedding model and dimension the index's vectors were built with.
/// An index without `symbol_vectors` (nothing was embedded) or without its
/// `index_meta` rows is an error saying so.
pub async fn embedding_meta(conn: &Connection) -> Result<EmbeddingMeta> {
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('symbol_vectors', 'index_meta')",
            (),
        )
        .await
        .context("reading the index's tables")?;
    let mut tables = Vec::new();
    while let Some(row) = rows.next().await.context("reading sqlite_master row")? {
        tables.push(row.get::<String>(0).context("reading sqlite_master.name")?);
    }
    if !tables.iter().any(|table| table == "symbol_vectors") {
        bail!(
            "the index has no vectors; run `annatar index` with an [ollama] section and a reachable embedding model"
        );
    }
    let missing = || {
        anyhow::anyhow!(
            "the index has vectors but no embedding metadata; rebuild it with `annatar index`"
        )
    };
    if !tables.iter().any(|table| table == "index_meta") {
        return Err(missing());
    }
    let model = meta_value(conn, schema::META_EMBEDDING_MODEL)
        .await?
        .ok_or_else(missing)?;
    let dim = meta_value(conn, schema::META_EMBEDDING_DIM)
        .await?
        .ok_or_else(missing)?;
    let dim = dim
        .parse()
        .with_context(|| format!("index_meta {} is {dim:?}", schema::META_EMBEDDING_DIM))?;
    Ok(EmbeddingMeta { model, dim })
}

async fn meta_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query("SELECT value FROM index_meta WHERE key = ?1", params![key])
        .await
        .with_context(|| format!("reading index_meta {key}"))?;
    match rows.next().await.context("reading index_meta row")? {
        Some(row) => Ok(Some(row.get(0).context("reading index_meta.value")?)),
        None => Ok(None),
    }
}

/// Up to `limit` symbols matching `filter`, nearest to `vector` first (ties
/// by fqn). Unfiltered: the vector index's top `limit`; filtered: an exact
/// scan of the matching rows.
pub async fn nearest(
    conn: &Connection,
    vector: &[f32],
    filter: &Filter,
    limit: usize,
) -> Result<Vec<Hit>> {
    const COLUMNS: &str = "s.fqn, s.kind, s.role, s.file, s.start_line, s.end_line, s.description,
        vector_distance_cos(v.embedding, vector32(?1)) AS distance";
    let sql = if filter.is_empty() {
        format!(
            "SELECT {COLUMNS}
             FROM vector_top_k('{}', vector32(?1), ?2) AS t
             JOIN symbol_vectors v ON v.symbol_id = t.id
             JOIN symbols s ON s.id = t.id
             ORDER BY distance, s.fqn",
            schema::VECTOR_INDEX
        )
    } else {
        format!(
            "SELECT {COLUMNS}
             FROM symbol_vectors v
             JOIN symbols s ON s.id = v.symbol_id
             LEFT JOIN symbols p ON p.id = s.parent_id
             WHERE {}
             ORDER BY distance, s.fqn
             LIMIT ?2",
            filter_sql(filter)
        )
    };
    let blob: Vec<u8> = vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let limit = i64::try_from(limit).context("limit too large")?;
    let mut values = vec![Value::Blob(blob), Value::Integer(limit)];
    if let Some(path) = &filter.path {
        values.push(Value::Text(path.clone()));
    }
    let mut rows = conn
        .query(&sql, values)
        .await
        .context("querying symbol vectors")?;
    let mut hits = Vec::new();
    while let Some(row) = rows.next().await.context("reading search row")? {
        let distance: f64 = row.get(7).context("reading the distance")?;
        hits.push(Hit {
            score: 1.0 - distance,
            fqn: row.get(0).context("reading symbols.fqn")?,
            kind: row.get(1).context("reading symbols.kind")?,
            role: row.get(2).context("reading symbols.role")?,
            file: row.get(3).context("reading symbols.file")?,
            start_line: row.get(4).context("reading symbols.start_line")?,
            end_line: row.get(5).context("reading symbols.end_line")?,
            description: row.get(6).context("reading symbols.description")?,
        });
    }
    Ok(hits)
}

/// The `WHERE` clause of a filtered search. Every kind and role is a fixed
/// [`SymbolKind`] or [`Role`] text, so they are inlined; the path is `?3`.
fn filter_sql(filter: &Filter) -> String {
    let list = |values: Vec<&str>| {
        values
            .iter()
            .map(|value| format!("'{value}'"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut clauses = Vec::new();
    if !filter.kinds.is_empty() {
        let kinds = filter.kinds.iter().map(SymbolKind::as_str).collect();
        clauses.push(format!("s.kind IN ({})", list(kinds)));
    }
    if !filter.roles.is_empty() {
        let members = SymbolKind::ALL
            .into_iter()
            .filter(|kind| !kind.is_type())
            .map(|kind| kind.as_str())
            .collect();
        let roles = filter.roles.iter().map(Role::as_str).collect();
        clauses.push(format!(
            "(CASE WHEN s.kind IN ({}) THEN p.role ELSE s.role END) IN ({})",
            list(members),
            list(roles)
        ));
    }
    if filter.path.is_some() {
        clauses.push("(s.file = ?3 OR substr(s.file, 1, length(?3) + 1) = ?3 || '/')".to_string());
    }
    clauses.join(" AND ")
}

/// The hits as text, two lines each:
///
/// ```text
/// 1. 0.734 com.acme.UserService [class] role=service src/main/java/com/acme/UserService.java:12-80
///    Manages user accounts.
/// ```
///
/// rank, cosine similarity (three decimals), fqn, kind, the role when the
/// symbol has one, `file:start-end`; then the description on one line
/// (whitespace collapsed), indented by three spaces. Empty for no hits.
pub fn format_hits(hits: &[Hit]) -> String {
    let mut out = String::new();
    for (rank, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "{}. {:.3} {} [{}]",
            rank + 1,
            hit.score,
            hit.fqn,
            hit.kind
        ));
        if let Some(role) = &hit.role {
            out.push_str(&format!(" role={role}"));
        }
        out.push_str(&format!(
            " {}:{}-{}\n",
            hit.file, hit.start_line, hit.end_line
        ));
        if let Some(description) = &hit.description {
            let line = description.split_whitespace().collect::<Vec<_>>().join(" ");
            if !line.is_empty() {
                out.push_str(&format!("   {line}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::fake::{FakeBackend, vector_for};
    use crate::store::{IndexReader, Store};

    const MODEL: &str = "embed";

    fn ollama(embedding_model: &str) -> OllamaConfig {
        OllamaConfig {
            url: "http://localhost:11434".to_string(),
            chat_model: "chat".to_string(),
            embedding_model: embedding_model.to_string(),
            reasoning_effort: "none".to_string(),
            temperature: 0.0,
            max_tokens: crate::config::DEFAULT_MAX_TOKENS,
        }
    }

    struct Row {
        id: i64,
        parent: Option<i64>,
        kind: &'static str,
        role: Option<&'static str>,
        fqn: &'static str,
        vector: [f32; 3],
    }

    const ROWS: &[Row] = &[
        Row {
            id: 1,
            parent: None,
            kind: "class",
            role: Some("controller"),
            fqn: "com.acme.UserController",
            vector: [1.0, 0.0, 0.0],
        },
        Row {
            id: 2,
            parent: Some(1),
            kind: "method",
            role: None,
            fqn: "com.acme.UserController#get(Long)",
            vector: [0.9, 0.1, 0.0],
        },
        Row {
            id: 3,
            parent: None,
            kind: "class",
            role: Some("service"),
            fqn: "com.acme.UserService",
            vector: [0.0, 1.0, 0.0],
        },
        Row {
            id: 4,
            parent: Some(3),
            kind: "method",
            role: None,
            fqn: "com.acme.UserService#find(Long)",
            vector: [0.1, 0.9, 0.0],
        },
        Row {
            id: 5,
            parent: Some(3),
            kind: "constructor",
            role: None,
            fqn: "com.acme.UserService#<init>()",
            vector: [0.0, 0.0, 1.0],
        },
    ];

    /// An index of [`ROWS`] (each described as "<fqn> does things.", line
    /// `id` to `id + 1`, controller rows in `src/web/`, service rows in
    /// `src/web2/`) with 3-dimensional vectors, unless `vectors` is
    /// off, and `meta` as its `index_meta` rows.
    async fn index(vectors: bool, meta: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        let conn = build.connection();
        if vectors {
            schema::create_vectors(conn, 3).await.unwrap();
        }
        for row in ROWS {
            conn.execute(
                "INSERT INTO symbols (id, parent_id, kind, role, fqn, file, start_line, end_line, signature, content_hash, description)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?7, ?1, ?1 + 1, '', '', ?6)",
                params![
                    row.id,
                    row.parent,
                    row.kind,
                    row.role,
                    row.fqn,
                    format!("{} does\n  things.", row.fqn),
                    if row.fqn.contains("Controller") {
                        "src/web/UserController.java"
                    } else {
                        "src/web2/UserService.java"
                    }
                ],
            )
            .await
            .unwrap();
            if vectors {
                let blob: Vec<u8> = row.vector.iter().flat_map(|v| v.to_le_bytes()).collect();
                conn.execute(
                    "INSERT INTO symbol_vectors (symbol_id, embedding) VALUES (?1, vector32(?2))",
                    params![row.id, blob],
                )
                .await
                .unwrap();
            }
        }
        for (key, value) in meta {
            conn.execute(
                "INSERT INTO index_meta (key, value) VALUES (?1, ?2)",
                params![*key, *value],
            )
            .await
            .unwrap();
        }
        build.commit().unwrap();
        dir
    }

    const META: &[(&str, &str)] = &[
        (schema::META_EMBEDDING_MODEL, MODEL),
        (schema::META_EMBEDDING_DIM, "3"),
        (schema::META_EMBEDDING_PARENT_DESCRIPTION, "false"),
    ];

    fn fqns(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|hit| hit.fqn.as_str()).collect()
    }

    #[tokio::test]
    async fn nearest_ranks_by_cosine_similarity() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();

        let hits = nearest(reader.connection(), &[1.0, 0.0, 0.0], &Filter::default(), 3)
            .await
            .unwrap();

        assert_eq!(
            fqns(&hits),
            [
                "com.acme.UserController",
                "com.acme.UserController#get(Long)",
                "com.acme.UserService#find(Long)"
            ]
        );
        assert!((hits[0].score - 1.0).abs() < 1e-6, "{hits:?}");
        assert!(hits[0].score > hits[1].score && hits[1].score > hits[2].score);
        assert_eq!(
            hits[0],
            Hit {
                score: hits[0].score,
                fqn: "com.acme.UserController".to_string(),
                kind: "class".to_string(),
                role: Some("controller".to_string()),
                file: "src/web/UserController.java".to_string(),
                start_line: 1,
                end_line: 2,
                description: Some("com.acme.UserController does\n  things.".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn kind_filter_scans_only_matching_kinds() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let filter = Filter {
            kinds: vec![SymbolKind::Method, SymbolKind::Constructor],
            ..Filter::default()
        };

        let hits = nearest(reader.connection(), &[1.0, 0.0, 0.0], &filter, 10)
            .await
            .unwrap();

        assert_eq!(
            fqns(&hits),
            [
                "com.acme.UserController#get(Long)",
                "com.acme.UserService#find(Long)",
                "com.acme.UserService#<init>()"
            ]
        );
    }

    #[tokio::test]
    async fn role_filter_matches_members_by_their_type_role() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let filter = Filter {
            kinds: vec![],
            roles: vec![Role::Service],
            ..Filter::default()
        };

        let hits = nearest(reader.connection(), &[1.0, 0.0, 0.0], &filter, 10)
            .await
            .unwrap();
        assert_eq!(
            fqns(&hits),
            [
                "com.acme.UserService#find(Long)",
                "com.acme.UserService",
                "com.acme.UserService#<init>()"
            ]
        );

        let both = Filter {
            kinds: vec![SymbolKind::Method],
            roles: vec![Role::Service, Role::Controller],
            ..Filter::default()
        };
        let hits = nearest(reader.connection(), &[0.0, 1.0, 0.0], &both, 1)
            .await
            .unwrap();
        assert_eq!(fqns(&hits), ["com.acme.UserService#find(Long)"]);
    }

    #[tokio::test]
    async fn path_filter_matches_whole_components() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();

        for (path, expected) in [
            (
                "src/web",
                &[
                    "com.acme.UserController",
                    "com.acme.UserController#get(Long)",
                ][..],
            ),
            (
                "src/web2/UserService.java",
                &[
                    "com.acme.UserService#find(Long)",
                    "com.acme.UserService",
                    "com.acme.UserService#<init>()",
                ][..],
            ),
            ("src/we", &[][..]),
        ] {
            let filter = Filter {
                path: Some(path.to_string()),
                ..Filter::default()
            };
            let hits = nearest(reader.connection(), &[1.0, 0.0, 0.0], &filter, 10)
                .await
                .unwrap();
            assert_eq!(fqns(&hits), expected, "{path}");
        }
    }

    #[tokio::test]
    async fn vector_index_and_exact_scan_rank_alike() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let every_kind = Filter {
            kinds: SymbolKind::ALL.to_vec(),
            ..Filter::default()
        };

        for query in [[1.0, 0.0, 0.0], [0.2, 0.7, 0.4], [0.0, 0.1, 1.0]] {
            let indexed = nearest(reader.connection(), &query, &Filter::default(), 5)
                .await
                .unwrap();
            let exact = nearest(reader.connection(), &query, &every_kind, 5)
                .await
                .unwrap();
            assert_eq!(indexed.len(), ROWS.len(), "{query:?}");
            assert_eq!(indexed, exact, "{query:?}");
        }
    }

    #[test]
    fn path_filter_is_repo_relative_and_empty_for_the_repo() {
        let repo = Path::new("/repo");
        for (prefix, expected) in [
            (".", None),
            ("./", None),
            ("/repo", None),
            ("/repo/", None),
            ("src/web/", Some("src/web")),
            ("./src/web", Some("src/web")),
            (
                "/repo/src/web/UserController.java",
                Some("src/web/UserController.java"),
            ),
            ("backend/../src/web", Some("src/web")),
        ] {
            assert_eq!(
                path_filter(repo, Path::new(prefix)).unwrap().as_deref(),
                expected,
                "{prefix}"
            );
        }
        for prefix in ["..", "../x", "src/../../x", "/other/src"] {
            let err = path_filter(repo, Path::new(prefix)).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("--path {prefix} is outside the repository /repo")
            );
        }
    }

    #[tokio::test]
    async fn search_embeds_the_query_with_the_configured_model() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let backend = FakeBackend::new();
        let embedder = QueryEmbedder::new(backend.clone(), &ollama(MODEL))
            .await
            .unwrap();

        // vector_for("a") = [1, 97, 1]: nearest to the service, then its method.
        let hits = search(reader.connection(), &embedder, " a ", &Filter::default(), 2)
            .await
            .unwrap();

        assert_eq!(vector_for("a"), [1.0, 97.0, 1.0]);
        assert_eq!(backend.batches(), [vec!["a".to_string()]]);
        assert_eq!(
            fqns(&hits),
            ["com.acme.UserService", "com.acme.UserService#find(Long)"]
        );
    }

    #[tokio::test]
    async fn search_refuses_another_model_before_embedding() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let backend = FakeBackend::new();
        let embedder = QueryEmbedder::new(backend.clone(), &ollama("other"))
            .await
            .unwrap();

        let err = search(reader.connection(), &embedder, "a", &Filter::default(), 2)
            .await
            .unwrap_err();

        let message = format!("{err:#}");
        assert!(
            message.contains("\"embed\"") && message.contains("\"other\""),
            "{message}"
        );
        assert!(backend.batches().is_empty());
    }

    #[tokio::test]
    async fn search_refuses_a_query_vector_of_another_length() {
        let meta = [
            (schema::META_EMBEDDING_MODEL, MODEL),
            (schema::META_EMBEDDING_DIM, "4"),
        ];
        let data = index(true, &meta).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let embedder = QueryEmbedder::new(FakeBackend::new(), &ollama(MODEL))
            .await
            .unwrap();

        let err = search(reader.connection(), &embedder, "a", &Filter::default(), 2)
            .await
            .unwrap_err();

        let message = format!("{err:#}");
        assert!(
            message.contains("3 values") && message.contains("have 4"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn search_says_when_the_index_has_no_vectors_or_metadata() {
        let embedder = QueryEmbedder::new(FakeBackend::new(), &ollama(MODEL))
            .await
            .unwrap();
        for (vectors, meta, expected) in [
            (false, &[][..], "the index has no vectors"),
            (true, &[][..], "no embedding metadata"),
            (
                true,
                &[(schema::META_EMBEDDING_MODEL, MODEL)][..],
                "no embedding metadata",
            ),
        ] {
            let data = index(vectors, meta).await;
            let reader = IndexReader::open(data.path()).await.unwrap();
            let err = search(reader.connection(), &embedder, "a", &Filter::default(), 2)
                .await
                .unwrap_err();
            assert!(format!("{err:#}").contains(expected), "{err:#}");
        }
    }

    #[tokio::test]
    async fn search_rejects_an_empty_query_and_reports_a_failed_embedding() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let backend = FakeBackend::new();
        let embedder = QueryEmbedder::new(backend.clone(), &ollama(MODEL))
            .await
            .unwrap();

        let err = search(reader.connection(), &embedder, "  ", &Filter::default(), 2)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("empty"), "{err:#}");

        backend.fail_embeddings();
        let err = search(reader.connection(), &embedder, "a", &Filter::default(), 2)
            .await
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("embedding the query with \"embed\"")
                && message.contains("embedding server down"),
            "{message}"
        );
    }

    #[test]
    fn hits_format_as_a_ranked_line_and_an_indented_description() {
        let hit =
            |score, fqn: &str, kind: &str, role: Option<&str>, description: Option<&str>| Hit {
                score,
                fqn: fqn.to_string(),
                kind: kind.to_string(),
                role: role.map(str::to_string),
                file: "src/main/java/com/acme/UserService.java".to_string(),
                start_line: 12,
                end_line: 80,
                description: description.map(str::to_string),
            };
        let hits = [
            hit(
                0.7341,
                "com.acme.UserService",
                "class",
                Some("service"),
                Some("Manages user\n  accounts. "),
            ),
            hit(0.5, "com.acme.UserService#find(Long)", "method", None, None),
            hit(
                0.25,
                "com.acme.UserService#<init>()",
                "constructor",
                None,
                Some(" "),
            ),
        ];

        assert_eq!(
            format_hits(&hits),
            "\
1. 0.734 com.acme.UserService [class] role=service src/main/java/com/acme/UserService.java:12-80
   Manages user accounts.
2. 0.500 com.acme.UserService#find(Long) [method] src/main/java/com/acme/UserService.java:12-80
3. 0.250 com.acme.UserService#<init>() [constructor] src/main/java/com/acme/UserService.java:12-80
"
        );
        assert_eq!(format_hits(&[]), "");
    }
}
