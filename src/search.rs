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
//! 5.5 groups the hits by file ([`group_by_file`]) and prints each file
//! with its types and members ([`format_files`]), the default output of
//! `annatar search`; [`format_hits`] is the `--symbols` output.
//!
//! Search reads `index.db` only. The query's embedding is cached in memory
//! for the call, never in `cache.db`, so a search creates and writes no file.

use std::collections::{HashMap, HashSet};
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

/// The `index_meta` value of `key`, `None` when the row is missing.
pub async fn meta_value(conn: &Connection, key: &str) -> Result<Option<String>> {
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

/// Files shown when `--limit` is not given (file-level output, 5.5).
pub const DEFAULT_FILES: usize = 5;

/// Largest accepted `--limit` for the file-level output: files are ranked
/// from the [`MAX_LIMIT`] nearest symbols, which on `argus` span 33–54
/// files, so 20 files never run short of hits.
pub const MAX_FILES: usize = 20;

/// Member and nested-type lines shown per file before the rest is cut to
/// `… N more`; hits are always shown.
pub const MEMBER_LINES: usize = 40;

/// One file of a file-level search: the file of the best symbol hits.
#[derive(Debug, Clone, PartialEq)]
pub struct FileHits {
    pub file: String,
    /// The best score of the file's hits.
    pub score: f64,
    /// The file's hits, best first.
    pub hits: Vec<Hit>,
}

/// Group `hits` (best first) by file: files in the order of their best hit,
/// at most `files` of them. Hits stop at the first hit of a file beyond the
/// `files`th, so every file keeps the hits that rank above the last file's
/// best hit — the symbols a symbol search would have listed to fill
/// `files` files. When the hits run out before such a file (a narrow
/// filter, or few files among the hits), nothing bounds them, so a file
/// keeps only its best hit and those among the [`DEFAULT_LIMIT`] best
/// overall.
pub fn group_by_file(hits: &[Hit], files: usize) -> Vec<FileHits> {
    let mut groups: Vec<FileHits> = Vec::new();
    let mut cut = false;
    for hit in hits {
        match groups.iter().position(|group| group.file == hit.file) {
            Some(index) => groups[index].hits.push(hit.clone()),
            None if groups.len() == files => {
                cut = true;
                break;
            }
            None => groups.push(FileHits {
                file: hit.file.clone(),
                score: hit.score,
                hits: vec![hit.clone()],
            }),
        }
    }
    if !cut {
        let top: HashSet<&str> = hits
            .iter()
            .take(DEFAULT_LIMIT)
            .map(|hit| hit.fqn.as_str())
            .collect();
        for group in &mut groups {
            let mut best = true;
            group
                .hits
                .retain(|hit| std::mem::take(&mut best) || top.contains(hit.fqn.as_str()));
        }
    }
    groups
}

/// The files of a file-level search ([`search_files`]).
#[derive(Debug, Clone, PartialEq)]
pub struct FileSearch {
    pub groups: Vec<FileHits>,
    /// Files asked for (`-k`).
    pub files: usize,
    /// Symbol hits the files were grouped from, at most [`MAX_LIMIT`].
    pub hits: usize,
}

impl FileSearch {
    /// A note for stderr when the search found no file, or fewer files than
    /// asked for because the [`MAX_LIMIT`] hits ran out (more files may
    /// match further down).
    pub fn note(&self) -> Option<String> {
        if self.groups.is_empty() {
            Some("no symbol matches the filter".to_string())
        } else if self.groups.len() < self.files && self.hits >= MAX_LIMIT {
            Some(format!(
                "files found: {} of the {} asked for; the {MAX_LIMIT} nearest symbols are in no other file",
                self.groups.len(),
                self.files
            ))
        } else {
            None
        }
    }
}

/// One symbol of a file shown by a file-level search.
#[derive(Debug, Clone, PartialEq)]
pub struct FileSymbol {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub kind: String,
    pub role: Option<String>,
    pub fqn: String,
    pub file: String,
    pub start_line: i64,
    pub end_line: i64,
    pub description: Option<String>,
}

/// Every symbol of `files`, in no particular order.
pub async fn file_symbols(conn: &Connection, files: &[&str]) -> Result<Vec<FileSymbol>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (1..=files.len())
        .map(|n| format!("?{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let values: Vec<Value> = files
        .iter()
        .map(|file| Value::Text(file.to_string()))
        .collect();
    let mut rows = conn
        .query(
            &format!(
                "SELECT id, parent_id, kind, role, fqn, file, start_line, end_line, description
                 FROM symbols WHERE file IN ({placeholders})"
            ),
            values,
        )
        .await
        .context("reading the files' symbols")?;
    let mut symbols = Vec::new();
    while let Some(row) = rows.next().await.context("reading symbols row")? {
        symbols.push(FileSymbol {
            id: row.get(0).context("reading symbols.id")?,
            parent_id: row.get(1).context("reading symbols.parent_id")?,
            kind: row.get(2).context("reading symbols.kind")?,
            role: row.get(3).context("reading symbols.role")?,
            fqn: row.get(4).context("reading symbols.fqn")?,
            file: row.get(5).context("reading symbols.file")?,
            start_line: row.get(6).context("reading symbols.start_line")?,
            end_line: row.get(7).context("reading symbols.end_line")?,
            description: row.get(8).context("reading symbols.description")?,
        });
    }
    Ok(symbols)
}

/// Search like [`search`], but return the `files` files of the best hits:
/// [`group_by_file`] over the [`MAX_LIMIT`] nearest symbols matching
/// `filter`.
pub async fn search_files(
    conn: &Connection,
    embedder: &QueryEmbedder,
    query: &str,
    filter: &Filter,
    files: usize,
) -> Result<FileSearch> {
    let hits = search(conn, embedder, query, filter, MAX_LIMIT).await?;
    Ok(FileSearch {
        groups: group_by_file(&hits, files),
        files,
        hits: hits.len(),
    })
}

/// Every symbol of the files of `groups`, for [`format_files`].
pub async fn group_symbols(conn: &Connection, groups: &[FileHits]) -> Result<Vec<FileSymbol>> {
    let files: Vec<&str> = groups.iter().map(|group| group.file.as_str()).collect();
    file_symbols(conn, &files).await
}

/// The file-level results as text:
///
/// ```text
/// 1. 0.770 src/main/java/com/acme/token/TokenCleanup.java
///    class com.acme.token.TokenCleanup [service] :18-48 *0.758
///      Deletes expired user tokens from the database periodically.
///      #<init>(TokenRepository) :24-27
///      #deleteExpiredTokens() :32-37 *0.770
///      enum Mode :40-47
///        #isStrict() :44-46
/// ```
///
/// per file its rank, best score and path; then each top-level type in
/// source order as `kind fqn [role] :start-end` with its description on the
/// next line (whitespace collapsed); under it its members and nested types
/// in source order, each as the part of its fqn after its parent's
/// (`#name(params)`, a nested type's `kind Name [role]`) and `:start-end`,
/// without a description, indented two spaces per level. A symbol among the
/// group's hits ends with `*score`. A file with more than `member_lines`
/// member and nested-type lines shows its hits and the first lines in
/// source order up to `member_lines`; each top-level type with hidden lines
/// ends with `… N more`, N counting its own.
pub fn format_files(groups: &[FileHits], symbols: &[FileSymbol], member_lines: usize) -> String {
    let mut out = String::new();
    for (rank, group) in groups.iter().enumerate() {
        out.push_str(&format!(
            "{}. {:.3} {}\n",
            rank + 1,
            group.score,
            group.file
        ));
        format_file(group, symbols, member_lines, &mut out);
    }
    out
}

fn format_file(group: &FileHits, symbols: &[FileSymbol], member_lines: usize, out: &mut String) {
    let in_file: Vec<&FileSymbol> = symbols
        .iter()
        .filter(|symbol| symbol.file == group.file)
        .collect();
    let by_id: HashMap<i64, &FileSymbol> =
        in_file.iter().map(|symbol| (symbol.id, *symbol)).collect();
    let mut children: HashMap<i64, Vec<&FileSymbol>> = HashMap::new();
    let mut roots = Vec::new();
    for symbol in &in_file {
        match symbol.parent_id.filter(|parent| by_id.contains_key(parent)) {
            Some(parent) => children.entry(parent).or_default().push(symbol),
            None => roots.push(*symbol),
        }
    }
    let order = |symbol: &&FileSymbol| (symbol.start_line, symbol.id);
    roots.sort_by_key(order);
    for list in children.values_mut() {
        list.sort_by_key(order);
    }
    let scores: HashMap<&str, f64> = group
        .hits
        .iter()
        .map(|hit| (hit.fqn.as_str(), hit.score))
        .collect();

    let mut members = Vec::new();
    for root in &roots {
        descendants(root, &children, 1, &mut members);
    }
    let mut shown = HashSet::new();
    for (symbol, _) in &members {
        if scores.contains_key(symbol.fqn.as_str()) {
            let mut current = Some(*symbol);
            while let Some(symbol) = current {
                if roots.iter().any(|root| root.id == symbol.id) {
                    break;
                }
                shown.insert(symbol.id);
                current = symbol
                    .parent_id
                    .and_then(|parent| by_id.get(&parent).copied());
            }
        }
    }
    for (symbol, _) in &members {
        if shown.len() >= member_lines {
            break;
        }
        shown.insert(symbol.id);
    }

    let score = |symbol: &FileSymbol| {
        scores
            .get(symbol.fqn.as_str())
            .map_or(String::new(), |score| format!(" *{score:.3}"))
    };
    let role = |symbol: &FileSymbol| {
        symbol
            .role
            .as_ref()
            .map_or(String::new(), |role| format!(" [{role}]"))
    };
    for root in &roots {
        out.push_str(&format!(
            "   {} {}{} :{}-{}{}\n",
            root.kind,
            root.fqn,
            role(root),
            root.start_line,
            root.end_line,
            score(root)
        ));
        if let Some(description) = &root.description {
            let line = description.split_whitespace().collect::<Vec<_>>().join(" ");
            if !line.is_empty() {
                out.push_str(&format!("     {line}\n"));
            }
        }
        let mut list = Vec::new();
        descendants(root, &children, 1, &mut list);
        let mut hidden = 0;
        for (symbol, depth) in list {
            if !shown.contains(&symbol.id) {
                hidden += 1;
                continue;
            }
            let parent = symbol
                .parent_id
                .and_then(|parent| by_id.get(&parent))
                .map_or("", |parent| parent.fqn.as_str());
            let name = symbol.fqn.strip_prefix(parent).unwrap_or(&symbol.fqn);
            let indent = "  ".repeat(depth);
            let is_type = SymbolKind::parse(&symbol.kind).is_some_and(SymbolKind::is_type);
            if is_type {
                out.push_str(&format!(
                    "   {indent}{} {}{}",
                    symbol.kind,
                    name.strip_prefix('.').unwrap_or(name),
                    role(symbol)
                ));
            } else {
                out.push_str(&format!("   {indent}{name}"));
            }
            out.push_str(&format!(
                " :{}-{}{}\n",
                symbol.start_line,
                symbol.end_line,
                score(symbol)
            ));
        }
        if hidden > 0 {
            out.push_str(&format!("     … {hidden} more\n"));
        }
    }
}

/// `symbol`'s descendants in source order, depth first, each with its depth
/// below the top-level type (`depth` for the children).
fn descendants<'a>(
    symbol: &FileSymbol,
    children: &HashMap<i64, Vec<&'a FileSymbol>>,
    depth: usize,
    out: &mut Vec<(&'a FileSymbol, usize)>,
) {
    for child in children.get(&symbol.id).into_iter().flatten() {
        out.push((child, depth));
        descendants(child, children, depth + 1, out);
    }
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

    fn hit_in(score: f64, fqn: &str, file: &str) -> Hit {
        Hit {
            score,
            fqn: fqn.to_string(),
            kind: if fqn.contains('#') { "method" } else { "class" }.to_string(),
            role: None,
            file: file.to_string(),
            start_line: 1,
            end_line: 2,
            description: None,
        }
    }

    #[test]
    fn hits_group_by_file_in_the_order_of_their_best_hit() {
        let hits = [
            hit_in(0.9, "a.A#run()", "A.java"),
            hit_in(0.8, "a.B", "B.java"),
            hit_in(0.7, "a.A", "A.java"),
            hit_in(0.6, "a.C", "C.java"),
            hit_in(0.5, "a.B#go()", "B.java"),
        ];

        let groups = group_by_file(&hits, 2);

        assert_eq!(
            groups
                .iter()
                .map(|group| (group.file.as_str(), group.score, fqns(&group.hits)))
                .collect::<Vec<_>>(),
            [
                ("A.java", 0.9, vec!["a.A#run()", "a.A"]),
                ("B.java", 0.8, vec!["a.B"]),
            ]
        );
        let all = group_by_file(&hits, 5);
        assert_eq!(all.len(), 3);
        assert_eq!(fqns(&all[1].hits), ["a.B", "a.B#go()"]);
        assert!(group_by_file(&[], 5).is_empty());
    }

    #[test]
    fn hits_that_run_out_mark_each_files_best_and_the_overall_top() {
        let mut hits = vec![hit_in(0.99, "a.B#top()", "B.java")];
        for n in 0..12 {
            hits.push(hit_in(
                0.9 - f64::from(n) / 100.0,
                &format!("a.A#m{n}()"),
                "A.java",
            ));
        }
        hits.push(hit_in(0.5, "a.C", "C.java"));
        hits.push(hit_in(0.4, "a.B", "B.java"));

        let groups = group_by_file(&hits, 3);

        let a: Vec<String> = (0..9).map(|n| format!("a.A#m{n}()")).collect();
        assert_eq!(
            groups
                .iter()
                .map(|group| (group.file.as_str(), fqns(&group.hits)))
                .collect::<Vec<_>>(),
            [
                ("B.java", vec!["a.B#top()"]),
                ("A.java", a.iter().map(String::as_str).collect()),
                ("C.java", vec!["a.C"]),
            ]
        );
        let cut = group_by_file(&hits, 2);
        assert_eq!(cut[1].hits.len(), 12);
        assert_eq!(fqns(&cut[0].hits), ["a.B#top()"]);
    }

    #[test]
    fn tied_hits_keep_their_order_across_files() {
        let hits = [
            hit_in(0.8, "a.A", "A.java"),
            hit_in(0.8, "a.B", "B.java"),
            hit_in(0.8, "a.A#run()", "A.java"),
            hit_in(0.8, "a.C", "C.java"),
        ];

        let groups = group_by_file(&hits, 2);

        assert_eq!(
            groups
                .iter()
                .map(|group| (group.file.as_str(), fqns(&group.hits)))
                .collect::<Vec<_>>(),
            [
                ("A.java", vec!["a.A", "a.A#run()"]),
                ("B.java", vec!["a.B"])
            ]
        );
    }

    #[test]
    fn file_search_notes_no_match_and_too_few_files() {
        let group = token_group(&[(0.9, "a.Token")]);
        let found = |groups: Vec<FileHits>, files, hits| FileSearch {
            groups,
            files,
            hits,
        };

        assert_eq!(
            found(vec![], 5, 0).note().as_deref(),
            Some("no symbol matches the filter")
        );
        assert_eq!(
            found(vec![group.clone()], 5, MAX_LIMIT).note().as_deref(),
            Some("files found: 1 of the 5 asked for; the 100 nearest symbols are in no other file")
        );
        assert_eq!(found(vec![group.clone()], 5, 40).note(), None);
        assert_eq!(found(vec![group], 1, MAX_LIMIT).note(), None);
    }

    fn symbol(
        id: i64,
        parent_id: Option<i64>,
        kind: &str,
        fqn: &str,
        lines: (i64, i64),
    ) -> FileSymbol {
        FileSymbol {
            id,
            parent_id,
            kind: kind.to_string(),
            role: None,
            fqn: fqn.to_string(),
            file: "src/a/Token.java".to_string(),
            start_line: lines.0,
            end_line: lines.1,
            description: Some(format!("{fqn} does\n  things.")),
        }
    }

    /// `Token.java`: a service class with a constructor, two methods and a
    /// nested enum with a method and a nested-nested record, listed out of
    /// source order; then a second top-level class.
    fn token_file() -> Vec<FileSymbol> {
        let mut service = symbol(1, None, "class", "a.Token", (3, 60));
        service.role = Some("service".to_string());
        let mut mode = symbol(5, Some(1), "enum", "a.Token.Mode", (40, 55));
        mode.role = Some("component".to_string());
        vec![
            symbol(4, Some(1), "method", "a.Token#delete(Long)", (30, 38)),
            service,
            symbol(2, Some(1), "constructor", "a.Token#<init>(Repo)", (10, 14)),
            symbol(3, Some(1), "method", "a.Token#find(Long)", (16, 28)),
            mode,
            symbol(7, Some(5), "record", "a.Token.Mode.Pair", (50, 54)),
            symbol(6, Some(5), "method", "a.Token.Mode#strict()", (44, 48)),
            symbol(8, None, "class", "a.TokenHelper", (62, 70)),
            symbol(9, Some(8), "method", "a.TokenHelper#help()", (64, 69)),
        ]
    }

    fn token_group(hits: &[(f64, &str)]) -> FileHits {
        FileHits {
            file: "src/a/Token.java".to_string(),
            score: hits[0].0,
            hits: hits
                .iter()
                .map(|(score, fqn)| hit_in(*score, fqn, "src/a/Token.java"))
                .collect(),
        }
    }

    #[test]
    fn files_format_as_types_with_descriptions_and_members_with_lines() {
        let group = token_group(&[(0.8123, "a.Token.Mode#strict()"), (0.75, "a.Token")]);
        let mut other = symbol(10, None, "interface", "b.Other", (1, 9));
        other.file = "src/b/Other.java".to_string();
        other.description = None;
        let mut symbols = token_file();
        symbols.push(other);
        let groups = [
            group,
            FileHits {
                file: "src/b/Other.java".to_string(),
                score: 0.5,
                hits: vec![hit_in(0.5, "b.Other", "src/b/Other.java")],
            },
        ];

        assert_eq!(
            format_files(&groups, &symbols, MEMBER_LINES),
            "\
1. 0.812 src/a/Token.java
   class a.Token [service] :3-60 *0.750
     a.Token does things.
     #<init>(Repo) :10-14
     #find(Long) :16-28
     #delete(Long) :30-38
     enum Mode [component] :40-55
       #strict() :44-48 *0.812
       record Pair :50-54
   class a.TokenHelper :62-70
     a.TokenHelper does things.
     #help() :64-69
2. 0.500 src/b/Other.java
   interface b.Other :1-9 *0.500
"
        );
        assert_eq!(format_files(&[], &symbols, MEMBER_LINES), "");
    }

    #[test]
    fn long_files_keep_their_hits_and_the_first_members() {
        let group = token_group(&[(0.9, "a.Token.Mode.Pair"), (0.8, "a.TokenHelper#help()")]);

        assert_eq!(
            format_files(&[group], &token_file(), 4),
            "\
1. 0.900 src/a/Token.java
   class a.Token [service] :3-60
     a.Token does things.
     #<init>(Repo) :10-14
     enum Mode [component] :40-55
       record Pair :50-54 *0.900
     … 3 more
   class a.TokenHelper :62-70
     a.TokenHelper does things.
     #help() :64-69 *0.800
"
        );
        let group = token_group(&[(0.9, "a.Token")]);
        assert_eq!(
            format_files(&[group], &token_file(), 0),
            "\
1. 0.900 src/a/Token.java
   class a.Token [service] :3-60 *0.900
     a.Token does things.
     … 6 more
   class a.TokenHelper :62-70
     a.TokenHelper does things.
     … 1 more
"
        );
    }

    #[tokio::test]
    async fn file_search_ranks_files_by_filtered_hits_and_shows_whole_files() {
        let data = index(true, META).await;
        let reader = IndexReader::open(data.path()).await.unwrap();
        let embedder = QueryEmbedder::new(FakeBackend::new(), &ollama(MODEL))
            .await
            .unwrap();
        // vector_for("a") = [1, 97, 1]: the service file first.
        let found = search_files(reader.connection(), &embedder, "a", &Filter::default(), 5)
            .await
            .unwrap();
        assert_eq!((found.files, found.hits), (5, ROWS.len()));
        assert_eq!(found.note(), None);
        let groups = found.groups;
        let symbols = group_symbols(reader.connection(), &groups).await.unwrap();
        assert_eq!(
            groups
                .iter()
                .map(|group| (group.file.as_str(), fqns(&group.hits)))
                .collect::<Vec<_>>(),
            [
                (
                    "src/web2/UserService.java",
                    vec![
                        "com.acme.UserService",
                        "com.acme.UserService#find(Long)",
                        "com.acme.UserService#<init>()"
                    ]
                ),
                (
                    "src/web/UserController.java",
                    vec![
                        "com.acme.UserController#get(Long)",
                        "com.acme.UserController"
                    ]
                ),
            ]
        );
        assert_eq!(symbols.len(), ROWS.len());

        let controllers = Filter {
            roles: vec![Role::Controller],
            ..Filter::default()
        };
        let groups = search_files(reader.connection(), &embedder, "a", &controllers, 5)
            .await
            .unwrap()
            .groups;
        let symbols = group_symbols(reader.connection(), &groups).await.unwrap();
        let text = format_files(&groups, &symbols, MEMBER_LINES);
        assert!(
            text.starts_with("1. ") && text.contains("src/web/UserController.java\n"),
            "{text}"
        );
        assert!(!text.contains("UserService"), "{text}");

        let constructors = Filter {
            kinds: vec![SymbolKind::Constructor],
            ..Filter::default()
        };
        let groups = search_files(reader.connection(), &embedder, "a", &constructors, 5)
            .await
            .unwrap()
            .groups;
        let symbols = group_symbols(reader.connection(), &groups).await.unwrap();
        assert_eq!(fqns(&groups[0].hits), ["com.acme.UserService#<init>()"]);
        let text = format_files(&groups, &symbols, MEMBER_LINES);
        assert!(
            text.contains("     #find(Long) :4-5\n")
                && text.contains("     #<init>() :5-6 *")
                && text.contains("   class com.acme.UserService [service] :3-4\n"),
            "{text}"
        );
    }
}
