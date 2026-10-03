# Annatar — State

Living document. Updated after every step. Tracks where the project stands,
the decisions taken, and the tradeoffs behind them.

- **Plan:** [`plan.md`](./plan.md)
- **Overview:** [`project.md`](./project.md)

## Current state

| | |
| --- | --- |
| Phase | 2 — History and ticket keys (complete with deviations, see open #18); Phase 2 audit remediation in progress |
| Step | R5 Shared test helpers; exact benchmark assertions — **done** |
| Last updated | 2026-10-03 |
| Toolchain | rustc 1.97.0, edition 2024 |

### Done

- **R5 Shared test helpers; exact benchmark assertions.** The four copies of
  the scratch-git helpers (`history`, `indexer`, `show`, `tests/benchmark.rs`)
  are now one file, `src/test_support.rs` (`git`, `git_ok`, `init_repo`,
  `commit` returning the new sha). The lib uses it as a `#[cfg(test)]` module;
  the benchmark includes the same file with `#[path]`, because `#[cfg(test)]`
  lib items are invisible to `tests/` — so there is one source, not two. `show`'s
  inline `git init`/config block became `init_repo`. The benchmark no longer
  asserts `warm < cold` (timing) or loose bounds: it asserts the exact counts
  (cold 0/1000, warm 1000/0, changed 900/100 hits/misses). Its doc now says the
  timings are from a debug build and overstate release time. Re-run here
  (linux, debug): cold 3.41 s, warm 0.39 s, changed 0.72 s. Resolves open #16
  and D-n (finding 24, 25). Decision D-s. 69 lib + 1 integration tests green,
  benchmark green.

- **R0 State repair and Phase 2 deviations.** Added the Phase 2 audit
  ([`audit_phase_2.md`](./audit_phase_2.md), 37 findings, all verified) and its
  remediation plan ([`audit_phase_2_plan.md`](./audit_phase_2_plan.md), items
  R0–R9 plus Phase 3 decisions J0–J9). Discarded an uncommitted formatter pass
  that had broken four code spans/table cells in this file (finding 17).
  Phase 2 is now marked **complete with deviations**: 2.2 was verified on a
  scratch repo and 2.3 measured a synthetic repo (D-p), not the target repo;
  `plan.md` 2.2/2.3 carry a *Deviation* note and both are closed by audit item
  R8 (real-repo run, open #18). Docs only; 69 lib + 1 integration tests green,
  `clippy --all-targets` clean.

- **2.3 Measure.** Recon confirmed git history is essentially the whole index
  cost (on a real 98k-commit repo `git log -L` was ~89 ms/symbol, while the same
  sources indexed structure-only in 0.02 s). Added the plan's history cache: a
  `history_cache` table in `cache.db` keyed by **fqn + content hash + the file's
  last commit sha**, storing the commits as JSON. `history::HistoryCache`
  (`get`/`put`) is consulted per symbol; a hit reuses the stored commits with no
  `git log -L`, a miss/outdated row recomputes and upserts. `history::file_last_commit`
  does one `git log -1 --format=%H -- <file>` per file (the cheap half of the
  key). Cache writes go to the cache connection, not the build transaction, so
  an aborted index cannot roll them back. `Commit` gained serde derives;
  `IndexStats` gained `history_hits`/`history_misses` (printed by `index`).
  **Measurement** (synthetic benchmark, the product owner's chosen target;
  `tests/benchmark.rs`, `#[ignore]`, 200 files × 4 methods = 1000 symbols,
  20 ticket commits, macos/18 cpus): cold **9.75 s** (0 hits / 1000 misses),
  warm **1.23 s** (1000 / 0), 10 %-rewritten **1.74 s** (900 / 100) — ~8× on a
  repeat run, which is plainly acceptable for daily iteration. Decisions D-o, D-p
  (2.3). 69 lib + 1 integration tests, benchmark ignored, all green.

- **2.2 Store history and ticket keys.** `build_index` now takes the compiled
  `ticket_regex` and, for every symbol it writes, records the commits that
  touched its span and the ticket keys those commits mention. New index tables
  `symbol_commits(symbol_id, sha, date, subject)` and
  `symbol_tickets(symbol_id, ticket_key, first_date, last_date)` (both
  `UNIQUE` per symbol, FK to `symbols`, indexed by `symbol_id`); commit subjects
  are kept as the fallback for when a ticket is unavailable. Keys come from the
  configured regex on subject **and** body via a pure `history::ticket_keys`
  (drops zero-width matches from an empty pattern, dedups). A key's `first_date`
  / `last_date` are the oldest / newest commit that mentions it, taken from git's
  newest-first order rather than comparing ISO strings (timezone-safe). `show`
  lists a `- commits:` and `- tickets:` section per symbol, omitted when empty.
  `IndexStats` gained `commits`/`tickets` counts; `main` compiles the regex and
  prints them. **Graceful degradation:** `history::is_repository` is checked
  once; a non-git `repo` warns once and builds a structure-only index, and a
  per-symbol history failure (untracked file) warns and is skipped — one bad
  file never aborts the run. `history_for_span` keeps its strict error contract.
  Verified end to end on a scratch git repo (3 symbols, 4 commit rows, 5 ticket
  rows; `show` lists both). 65 tests + 1 integration, green. Decisions D-l, D-m
  (2.2).

- **2.1 History for a span.** New `history` module: `history_for_span(repo,
  file, start_line, end_line) -> Result<Vec<Commit>>` shells out to
  `git -C <repo> log -L <start>,<end>:<file>` and parses only the format
  records, discarding the diff hunks. `Commit { sha, date, subject, body }`,
  `date` as strict ISO 8601 (`%aI`). The format (`%x1e%H%x1f%aI%x1f%s%x1f%b%x1d`)
  uses ASCII control chars as record/field/end delimiters, so commit text can
  never split a record. Records come back newest first (`git log` order), so the
  last element is the commit that introduced the lines. A non-zero git exit
  (untracked file, out-of-range span, non-repo) is an `anyhow` error carrying
  git's stderr, never an empty list. Empirically documented in the module doc:
  `git log -L` **does** follow renames via git's own rename detection (same
  machinery as `git blame`), and `--follow` is rejected by git when combined
  with `-L` (`fatal: --follow requires exactly one pathspec`) and unnecessary.
  Six unit tests on scratch git repos (no network); 57 tests, green. Decision
  D-k (2.1).

- **H8 Doc drift, test smells and minor cleanups.** Five hygiene findings closed
  with no behaviour change. (13) Decision 41's wording now describes what
  `show::render` actually does — load the whole table, build a child map,
  recurse with plain function calls — instead of the false "explicit stack"
  claim; the deep-nesting overflow risk is tracked as a new open question rather
  than reworking render. (14) `minimal_without` asserts each pattern is present
  before `str::replace` and absent after, so an edited `MINIMAL` can no longer
  make a config test pass vacuously. (15) The `store` test helper's insert now
  binds the value through `params!` (`INSERT INTO {table} VALUES (?1)`), leaving
  only the table identifier interpolated, as `CREATE TABLE` must be. (16)
  `relative_path` is now `path.to_string_lossy().replace('\\', "/")` instead of a
  component loop; the walker/indexer path tests confirm repo-relative,
  `/`-separated paths still hold. (18) `Config::load` resolves `repo`/`data_dir`
  from a clone instead of `std::mem::take`. Decision D-i (H8); 52 tests, green.

- **H9 Narrow walker pruning of `build`/`generated`.** `is_skipped_dir` no
  longer prunes a directory named in `SKIP_DIRS` anywhere: it now prunes such a
  directory only when it is **not under a source root** (no ancestor component
  named `java`). So `src/main/java/com/acme/build/Keep.java` is indexed while a
  module-root `build/`, `target/...`, `generated-sources/...`, and
  `module/build/Nested/Ignored.java` are still excluded. `java_files`' signature
  and the rest of the walk are unchanged; the `SKIP_DIRS` doc comment and
  `is_skipped_dir` gained a doc comment describing the narrowing. No
  `[index] skip_dirs` config escape hatch (explicitly out of scope). Updated the
  `returns_exactly_the_expected_production_files` fixture to move the generated
  sample out of the source root (`generated/Generated.java`) so it still proves
  generated output is pruned, and added a test that a package named `build`
  under `src/main/java` survives while root `build/Out.java` is pruned.
  Decision D-h (H9); decision D-j (H1, H9). 52 tests, green.

- **H7 Batch symbol writes in a transaction.** `build_index` now begins one
  transaction on the build connection after `begin_index` and writes every
  symbol through it, then commits the transaction before `build.commit()`
  renames the temp file over `index.db`. A write error drops the transaction
  (rollback) before the temp file is discarded, so a partial index never
  replaces a good one. `write_symbol` still takes `&Connection`; the
  `Transaction` is passed via its `Deref<Target = Connection>`. `last_insert_rowid`
  stays transaction-local and correct. Kept a local transaction in `build_index`
  rather than an `IndexBuild::transaction()` guard: ordering stays a readable
  two-liner and `store.rs` production code is untouched. New store test proves a
  dropped transaction rolls back (row invisible on the build connection) and the
  later file-level commit does not resurrect it; the existing indexer tests
  exercise the committed path and the duplicate-fqn test keeps
  `UNIQUE` + `INSERT OR IGNORE` inside a transaction green. Note: in libsql
  0.9.30 `Connection::transaction()` is `async` (contrary to the plan text), so
  the begin is `.await`ed. Decision D-g (H7). 51 tests, green.

- **H6 De-duplicate `Symbol` construction.** `src/symbols.rs` gains a single
  `build_symbol` factory used by both the type branch of `collect_symbols` and
  `member_symbol`, so the 15+ field literal lives once. The declaration's
  annotation nodes are walked once (`annotations`); the annotation texts,
  `signature_of` (now taking the precomputed node list) and `role_of` (now
  taking the already-extracted simple names) all derive from that list instead
  of re-walking/re-parsing. Deleted the dead `"record_body"` arm in `body_node`
  (`record_declaration` declares `field('body', $.class_body)`, and
  `child_by_field_name("body")` already resolves it) and the dead `"program"`
  arm in `is_type_container` (the root is passed to `collect_symbols` directly,
  never discovered via the predicate). One new test locks a record *with a body*
  (compact constructor + member) to a header-only signature. Decision D-f (H6).
  50 tests, green.

- **H5 Surface decode failures.** `show::parse_annotations` now returns
  `Result<Vec<String>>`; corrupt `annotations` JSON propagates as an error with
  the symbol's fqn in the context (`parsing annotations for `<fqn>``), and
  `write_symbol`/`render` thread the `Result` so a corrupt row fails loudly
  instead of rendering as "none". `symbols::text` stays infallible but logs a
  `tracing::warn!` with the node kind and byte range on a `utf8_text` failure,
  then returns the empty string (tree-sitter spans never split a UTF-8 char, so
  this is the proportionate signal). New `show` test inserts a row with
  `annotations = 'not json'` via `IndexBuild` and asserts the error names the
  fqn. Decision D-e (H5). 49 tests, green.

- **H4 Side-effect-free read path.** Added `store::IndexReader`
  (`connection()` + `open(data_dir)`) that opens only `index.db` with
  `OpenFlags::SQLITE_OPEN_READ_ONLY` and the per-connection
  `PRAGMA foreign_keys = ON`. Opening a reader creates neither the data
  directory nor `cache.db`; a missing index keeps the friendly "run
  `annatar index` first" error. Removed `Store::open_index`, `Store::data_dir`
  and `Store::cache_path`; `main`'s `show` arm now uses `IndexReader::open`, so
  `annatar show` creates nothing. `show::render` still takes `&Connection`. Two
  new store tests: reader-open on an absent index leaves no `data_dir`/`index.db`
  /`cache.db`, and a read after deleting `cache.db` does not recreate it.
  Decision D-d (H4). 48 tests, green.

- **H3 Enforce the declared foreign key.** `PRAGMA foreign_keys = ON` now runs
  on the build connection in `Store::begin_index` (immediately after `connect`,
  before `schema::create_index`) and on the read connection from
  `Store::open_index`, via an `enable_foreign_keys` helper. libSQL enforces it
  per connection: the build tests insert a dangling `parent_id` and observe
  `FOREIGN KEY constraint failed` with the row not stored (0 rows), while a
  parent-then-child insert still succeeds. Note: `INSERT OR IGNORE` does **not**
  swallow an FK violation here (SQLite semantics — `ON CONFLICT` does not cover
  foreign keys), so a future linking bug fails the insert loudly rather than
  silently skipping the row. Decision D-c (H3).

- **H2 Bounds-safe `content_hash`.** `content_hash` now uses
  `as_bytes().get(start..end)` and returns `anyhow::Result<String>`; an
  out-of-range span is a hard error naming the symbol fqn and the span, not a
  panic or a silent clamp. `write_symbol` `?`s the hash with
  `hashing symbol <fqn>` context, so a parser bug fails the run. Added a unit
  test that a span past the end of the source errors and names the fqn/span;
  existing content-hash test stays green. Decision D-b (H2).

- **H1 Parse contract + parser reuse.** `symbols` gains a reusable `JavaParser`
  (grammar loaded once per run) and `ParsedFile { symbols, parse_error }`;
  `parse_file` is removed. A broken file is now `parse_error: true` with empty
  symbols, while a clean symbol-less file (`package-info.java`) is
  `parse_error: false`. `IndexStats.skipped` is replaced by disjoint `empty` /
  `parse_errors` / `unreadable` buckets, with `files` still meaning "produced
  ≥ 1 symbol"; the `index` summary prints them all. Resolves open question #11.
  43 tests, green.

- **Pre-Phase-2 review.** Verified all five steps on edge-case Java (enums,
  interfaces with default methods, records, annotations, generics, varargs,
  receiver parameters) and the CLI end to end. Fixes: `build_index` now errors
  when `repo` is not a directory instead of committing an empty index; a valid
  file with no symbols (`package-info.java`) is logged at debug, not warn (the
  parse-error log stays). 41 tests, green.

- **1.5 Write symbols and `show`.** `symbols` table (id, parent_id, kind,
  role, fqn unique, file, start/end line, signature, javadoc, annotations,
  content_hash) in `INDEX_TABLES`; `indexer::build_index` walks, parses and
  writes all symbols in one temp-file build, linking `parent_id` via an in-memory
  fqn map. `Store::open_index` reads the committed index; `show::render` prints a
  symbol and its descendants recursively. `index`/`show` wired in `main`;
  `content_hash` is blake3 of javadoc + source slice; annotations stored as JSON.
  Manual run on the fixture project: 4 files, 14 symbols, show works. 40 tests
  at the time; clippy clean.

- **1.4 Signature, Javadoc, annotations, Spring role.** `Symbol` gained
  `signature` (declaration without body, annotations removed, whitespace
  collapsed), `javadoc: Option<String>` (cleaned `/** ... */` directly above the
  declaration), `annotations: Vec<String>` (as written) and `role: Option<Role>`
  for types. `Role`: controller/service/repository/component/configuration/
  entity, derived from annotations; Spring Data interfaces are repository via
  `extends` names. Precedence controller > service > repository > configuration
  > component > entity. 7 tests; 32 total, green; clippy clean.

- **1.3 Parse methods and constructors.** `symbols::parse_file` now returns a
  unified `Vec<Symbol>` (`SymbolKind`: class/interface/enum/record/annotation/
  method/constructor) in source order, replacing `TypeDef`/`parse_types`.
  Members get fqn `pkg.Type#name(params)`; constructors use `<init>`; parameter
  types are rendered as written (whitespace-normalized, no names) so overloads
  differ. Nested-type members carry the nested parent fqn; anonymous/local
  classes contribute nothing. 10 symbols tests; 25 total, green; clippy clean.

- **1.2 Parse types.** `symbols::parse_types(path, source)` returned a `TypeDef`
  per class, interface, enum, record and `@interface`, including nested types:
  package, simple name, dotted fqn, kind, 1-based line span, byte span and
  parent fqn (both renamed to `parse_file`/`Symbol` in 1.3). `tree-sitter` 0.25
  + `tree-sitter-java` 0.23; a file that does not parse is logged and skipped
  (`Ok(empty)`). Recursion is scoped to type containers so method-local and
  anonymous classes are excluded. 6 tests; 21 total, green; clippy clean.

- **1.1 File walker.** `walk::java_files(repo, path_prefix)` returns the
  production `.java` files under `repo` as repo-relative, sorted paths. Uses
  `ignore` with standard filters (so `.gitignore` applies), prunes
  `build`/`target`/`generated`/`generated-sources` anywhere in the tree, drops
  `src/test`, and applies `--path` as a literal path prefix. 6 tests
  (exact-set, gitignore, skip dirs, `src/test`, prefix forms, empty prefix);
  15 total, green; clippy clean.

- **Pre-Phase-1 rework.** Config paths (`repo`, `data_dir`) now resolve
  relative to the config file's directory, `--path` is a `PathBuf`, and the
  `jira`/`ollama` sections are optional. `Store::begin_index` now returns an
  owned `IndexBuild` with `commit()` (dropping it aborts); `Store` no longer
  tracks the in-progress build. Added the `schema` module (its table lists are
  populated from 1.5), a committed example `annatar.toml`, and framed
  `project.md` as the target with `plan.md` as its POC subset. 9 tests, green;
  `cargo clippy -D warnings` clean.

- **0.2 Database files.** `store::Store` opens `<data_dir>/cache.db` once and
  builds `<data_dir>/index.db` in a temporary file (`tempfile`, same
  directory) that is atomically renamed on `IndexBuild::commit`. An aborted run
  drops the temp file and leaves the previous index untouched; a new
  `begin_index` discards an unfinished one. 4 store tests cover replace,
  abort, cache survival and no-build finish. 9 tests total, green.
- **0.1 CLI, config, logging.** `annatar` binary with `clap` subcommands
  `index`, `show`, `search`, `serve` (stubs), global `--path`, `-v` and
  `--config`. `annatar.toml` loaded into a typed `Config` (`repo`, `data_dir`,
  `ticket_regex`, `jira`, `ollama`), secrets from environment, validation on
  load, `tracing` logging with `RUST_LOG` override. 5 config tests, green.
  `cargo fmt`, `cargo clippy -D warnings` clean.
  Realigned to the refined plan on 2026-10-03: single `database` path became
  `data_dir` (the two-file `index.db` + `cache.db` layout), and `embedding_dim`
  was removed from config (5.1 reads it from the first embedding response).

### Next

- Phase 2 audit remediation **R1**: extract a `symbol_history` helper in the
  indexer — skip a file with no history once (one warning), degrade cache
  faults to a miss, add `IndexStats.history_skipped` so
  `hits + misses + skipped == symbols`. Then R2, R3, R4, R6, R7 per
  [`audit_phase_2_plan.md`](./audit_phase_2_plan.md).

## Step log

| Step | Status | Notes |
| --- | --- | --- |
| 0.1 CLI, config, logging | done | `--help` works; 5 config tests green; realigned to refined plan (`data_dir`, no `embedding_dim`) |
| 0.2 Database files | done | two-file `Store`, temp file + atomic rename; 4 tests |
| 1.1 File walker | done | `walk::java_files`; `ignore` crate; 6 tests (15 total) |
| 1.2 Parse types | done | `symbols::parse_types` (renamed to `parse_file` in 1.3); tree-sitter 0.25 + tree-sitter-java 0.23; 6 tests (21 total) |
| 1.3 Methods/constructors | done | unified `Symbol`; fqn `Type#name(params)`, ctors `<init>`; 10 symbols tests (25 total) |
| 1.4 Text + role | done | signature/javadoc/annotations/role; 17 symbols tests (32 total) |
| 1.5 Write + show | done | `symbols` table, `indexer::build_index`, `Store::open_index`, `show::render`; 40 tests |
| Pre-Phase-2 review | done | edge-case + CLI verification; missing-repo error; quieter empty-file log; 41 tests |
| H1 Parse contract + parser reuse | done | `JavaParser`/`ParsedFile`; `IndexStats` buckets `empty`/`parse_errors`/`unreadable`; parser built once per run; resolves open #11; 43 tests |
| H2 Bounds-safe `content_hash` | done | `.get(start..end)` + `Result`; out-of-range span errors with fqn/span context; unit test; decision D-b |
| H3 Enforce foreign key | done | `PRAGMA foreign_keys = ON` per build/read connection; dangling `parent_id` errors and is not stored (libSQL verified); valid parent-child insert works; decision D-c |
| H4 Side-effect-free read path | done | `IndexReader::open` read-only; `Store::open_index`/`data_dir`/`cache_path` removed; `show` creates nothing; 2 reader tests; decision D-d; 48 tests |
| H5 Surface decode failures | done | corrupt annotations error with the fqn; `text()` warns with kind/span and returns empty; 1 show test; decision D-e; 49 tests |
| H6 De-duplicate symbol construction | done | one `build_symbol` factory; annotations walked once (texts + signature + role); dead `record_body`/`program` arms removed; 1 record-body test; decision D-f; 50 tests |
| H7 Batch symbol writes in a transaction | done | one transaction wraps all symbol writes; committed before `build.commit()`; dropped tx rolls back; local tx in `build_index` (no `IndexBuild::transaction()`); 1 store rollback test; decision D-g; 51 tests |
| H9 Narrow walker pruning of `build`/`generated` | done | prune `SKIP_DIRS` names only outside a `java` source root; `com.acme.build` survives, root/module build output still pruned; fixture moved generated sample out of source root; 1 new walker test; decision D-h, D-j; 52 tests |
| H8 Doc drift, test smells and minor cleanups | done | decision 41 wording corrected, deep-nesting open question added; `minimal_without` asserts presence/removal; store helper inserts via `params!`; `relative_path` uses `replace('\\', "/")`; `Config::load` clones instead of `mem::take`; decision D-i; 52 tests |
| 2.1 History for a span | done | `history::history_for_span` over `git log -L`; control-char `--format`; `Commit {sha,date,subject,body}`, newest first; `-L` follows renames, `--follow` rejected; 6 history tests; decision D-k; 57 tests |
| 2.2 Store history and ticket keys | done | `symbol_commits` + `symbol_tickets`; per-symbol history in the build transaction; pure `history::ticket_keys`; structure-only degradation when `repo` is not a git work tree; `show` lists commits/tickets; `IndexStats.commits/tickets`; 8 new tests; decisions D-l, D-m; 65 tests |
| 2.3 Measure | done | `history_cache` in `cache.db` keyed by fqn + content hash + file last-commit sha; `HistoryCache`, `file_last_commit`; `IndexStats.history_hits/misses`; synthetic `tests/benchmark.rs` (`#[ignore]`): cold 9.75 s → warm 1.23 s (~8×), 10 %-edit 1.74 s; decisions D-o, D-p; 69 tests |
| R0 State repair + Phase 2 deviations | done | phase 2 audit + remediation plan added; formatter damage in `state.md` discarded; Phase 2 marked complete with deviations; `plan.md` 2.2/2.3 deviation notes; docs only, 69 tests |
| R5 Shared test helpers | done | one `src/test_support.rs` shared by lib tests and `tests/benchmark.rs` (`#[path]`); exact hit/miss benchmark assertions, no timing assertion; debug-build note; resolves open #16 / D-n; decision D-s; 69 tests |

## Decisions and tradeoffs

| # | Decision | Alternatives | Rationale / consequence |
| --- | --- | --- | --- |
| 1 | `Config::parse` is pure (text in), `Config::load` reads the file and then the environment | One loader that reads env inline | Keeps config tests deterministic and free of global env state |
| 2 | Secrets (`ANNATAR_JIRA_TOKEN`, `ANNATAR_JIRA_EMAIL`) only from env; `deny_unknown_fields` rejects them in the file | Allow secrets in `annatar.toml` | Config file stays shareable/committable; a test asserts a `token` key is rejected |
| 3 | Added `--config <FILE>` (default `annatar.toml`), not in the plan | Fix the path to `annatar.toml` in the cwd | Lets tests and multiple checkouts point at their own config; cheap to remove |
| 4 | Validate `ticket_regex` compiles at load time | Validate lazily at first use | Fail fast with a clear message; an invalid regex is a config bug, not a per-file one |
| 5 | Binary + library split (`src/lib.rs` + `src/main.rs`) | Binary only | Enables unit tests now and integration tests (`tests/`) later without invoking the process |
| 6 | Stub subcommands still load config before printing "not implemented" | Stubs that parse nothing | Exercises the config path end to end and gives real errors for a missing/invalid config |
| 7 | Verbosity map `0=warn, 1=info, 2=debug, 3+=trace`; `RUST_LOG` wins | Always `info` | Sensible default silence, full detail on demand, env escape hatch |
| 8 | Jira auth modeled as optional `token` + `email`, not required at load | Require auth for every run | `index`/`show`/`search` can run before Jira is wired; only a Jira-calling command will require it |
| 9 | Defaults: ticket regex `\bGRLD-\d+\b`, Ollama URL `http://localhost:11434` | Require both explicitly | Matches the plan's stated default and local Ollama convention; fewer mandatory keys |
| 10 | `JiraConfig` has a hand-written redacting `Debug` (`token` shown as `***`) | Derive `Debug` | Prevents a secret leaking into logs the first time someone debugs the config |
| 11 | Config exposes `data_dir`, not a database file path (refined plan) | A single `database` path, or `index_db` + `cache_db` paths | Ground rules define two files with different lifecycles; 0.2's `Store` derives both names from one directory, so config stays one key |
| 12 | `embedding_dim` dropped from config (refined plan) | Keep it and validate | 5.1 takes the dimension from the first embedding response, so config would only be a second source of truth that can disagree |
| 13 | libSQL via the `libsql` crate with `default-features = false, features = ["core"]` | Default features (remote, replication, sync, tls) | Only local files are needed; avoids tonic/hyper/rustls and keeps compile times down. Vector functions still live in `libsql-sys` |
| 14 | Index temp file via `tempfile` in `data_dir`, committed with `NamedTempFile::persist` (rename) | Hand-rolled temp name + `fs::rename` | `persist` is atomic on Unix and deletes the temp file on drop, so an aborted run cleans itself up. Same directory guarantees the same filesystem |
| 15 | Rely on libSQL local leaving a single file (default `journal_mode=delete`) | WAL for the index, checkpoint + sidecar handling | Verified empirically: closed local db leaves only `index.db`. If the index ever moves to WAL, `IndexBuild::commit` must checkpoint before rename |
| 16 | `Store`/`IndexReader` are async; `tokio` is the runtime | Synchronous wrapper | The `libsql` API is async. `Store::open`/`begin_index` and `IndexReader::open` are async, `IndexBuild::commit` is sync (drop + rename). Tests use `#[tokio::test]` |
| 17 | `Store` owns no schema in 0.2; it exposes `cache()` and the `begin_index()` connection | Bundle table creation into 0.2 | Each phase adds its own tables (plan). Keeps this step to the file lifecycle and lets schema land with the data it stores |
| 18 | Drop order: connection fields before their `Database`; `_cache_db` field anchors the cache connection | Rely on `Connection` alone | Explicit and safe: the connection is closed before the database handle; the anchor field is underscore-prefixed to stay intentionally unread |
| 19 | `repo` and `data_dir` resolve relative to the config file's directory, not the cwd (`Config::load`) | Resolve against the cwd | A config file describes its own repo/data layout, so the same file works from any working directory. Absolute paths are returned unchanged |
| 20 | `--path` is a `PathBuf` | `String` | It is a filesystem path prefix; parse it as one so the walker (1.1) doesn't re-parse or re-validate |
| 21 | `jira` and `ollama` are `Option<...>` in `Config` | Keep them required | Phases 1–2 (`index`, `show`) don't talk to Jira or Ollama; requiring them taxes the `--path` iteration loop. A command that needs them will require them once wired (3.x/4.x) |
| 22 | `Store::begin_index` returns an owned `IndexBuild` with `commit()`; `Store` no longer tracks the in-progress build | Return `&Connection` from `&mut Store`, then finish on `Store` | The borrow blocked holding the connection across an async pipeline while later calling finish, and mixed a sync finish with an async build. An owned guard survives the whole run and aborting is just a drop |
| 23 | All DDL lives in one `schema` module: `INDEX_TABLES` applied by `begin_index`, `CACHE_TABLES` by `open` | Per-phase DDL scattered in each step | `index.db` is disposable, so no migrations or versioning; one file keeps the whole shape reviewable as phases 1–5 add tables |
| 24 | A working dummy `annatar.toml` is committed (data dir `.annatar`, gitignored) | No config in the repo | The binary's default config now loads out of the box; secrets still come only from the environment, so the file stays committable |
| 25 | Build/generated sources detected by directory name (`build`, `target`, `generated`, `generated-sources`), pruned only when not under a source root (no ancestor named `java`) | Prune the names anywhere; parse Maven/Gradle build files; add an `index.skip_dirs` config | Language/build-system agnostic, cheap, and testable, and now does not drop a legitimate package such as `com.acme.build`; a config escape hatch is deferred (D-h) |
| 26 | `--path` is a literal path prefix (`Path::starts_with`), not a glob | Glob/pattern matching | The plan calls it a prefix; prefix semantics cover both a directory and a single file with no pattern engine |
| 27 | Walker sets `require_git(false)` and `parents(false)` | Require a git repo / honour ancestor ignores | Temp-dir tests need no `git init` and stay deterministic; `parents(false)` also stops a parent `.gitignore` outside the repo from affecting a run |
| 28 | `tree-sitter` 0.25 + `tree-sitter-java` 0.23 (ABI-matched pair) | Other version pairs | A real parse test proves the pair loads; later bumps must keep the grammar crate compatible |
| 29 | Type recursion is scoped to type-container nodes (`program`, `*_body`, `enum_body_declarations`), never executable scopes | Filter `class_declaration` by inspecting its `parent.kind()` | `tree-sitter-java` has no `local_class_declaration`; local and anonymous classes are also `class_declaration`, so only descending type containers correctly excludes them |
| 30 | `TypeDef`/`parse_types` replaced by one unified `Symbol`/`parse_file` in 1.3 | Keep types and members as separate structs | 1.4 adds text to every symbol and 1.5 persists one `symbols` table; one model avoids parallel fields and makes the parent_id join trivial |
| 31 | Constructor fqn uses `<init>` (name field `<init>` too) | Use the type's simple name | JVM/SCIP/JDT convention; unambiguous and stable, and later SCIP usage mapping matches without translation |
| 32 | Member fqn parameter types are whitespace-stripped source text, no parameter names (`Map<String,List<X>>`) | Keep original spacing / resolve simple names | Identity only needs distinct overloads; 1.4 stores the full signature separately, so fqn can be compact and deterministic |
| 33 | Fields, initializer blocks and annotation-type elements are not symbols | Include them | The plan scopes members to methods and constructors; annotations are covered by 1.4's annotation text, not as symbols |
| 34 | `signature` drops declaration-level annotations and collapses whitespace; parameter annotations stay in it | Keep annotations in the signature / preserve source formatting | Annotations have their own field; a one-line signature is what the 4.3 prompt wants. Parameter annotations are part of the method's shape |
| 35 | Javadoc is cleaned (delimiters and leading `*` stripped) and only the immediately preceding `block_comment` counts | Store raw comment / search further back | Clean text goes straight into the LLM prompt; a comment separated by code is not a doc comment for the declaration |
| 36 | Role precedence puts `Configuration` above `Component` (plan lists component first) | Follow the plan's list order verbatim | `@Configuration` is a specialization of `@Component`, so it is the more specific role; classes carrying both are configurations |
| 37 | Spring Data repository = known base-interface list or a superinterface simple name ending in `Repository`; the type's own name does not matter | Match the type's own name too / use imports for exactness | The `extends` chain is what makes it a Spring Data repo; own-name matching would tag unrelated interfaces. Import resolution is deferred |
| 38 | `symbols` table lives in `INDEX_TABLES` with `fqn UNIQUE`, an in-memory fqn→id map for `parent_id`, and `INSERT OR IGNORE` with first-definition-wins on duplicates | Resolve parents by fqn subquery, or error on duplicates | The parser is pre-order so parents are always written first; a stray duplicate must not orphan the real symbol's children |
| 39 | `content_hash` = blake3(javadoc + `\n` + source slice); annotations stored as a JSON array string | Hash source only / newline-join annotations | A doc-only edit must invalidate later summaries; JSON survives multi-line annotation text |
| 40 | `kind`/`role` stored as lower-case strings via `as_str()`, part of the on-disk contract | Store the Rust enum via an integer/serde tag | The index is inspected by SQL and later the MCP tools; readable stable strings are the simplest contract |
| 41 | `show::render` loads the whole `symbols` table, builds an in-memory child map, then recurses with plain function calls | One query per node / recursive CTEs | POC-scale tables make this cheap and it avoids async recursion; plain recursion can overflow on pathologically deep nesting, tracked as open question #15 |
| 42 | `IndexStats.files` counts files that produced a symbol; the disjoint `empty` / `parse_errors` / `unreadable` buckets count the rest | Count every walked file as `files` | `files` + the three failure buckets = walked; a clean symbol-less file (`package-info.java`) is now distinct from a parse failure (D-a) |
| D-a (H1) | New `JavaParser`/`ParsedFile` API; `IndexStats.skipped` is replaced by `empty`/`parse_errors`/`unreadable`, `files` keeps its meaning | Keep `Ok(vec![])` for both | Distinguishes a broken file from a symbol-less one; resolves open #11; parser reused once per run (one grammar load) |
| D-j (H1, H9) | Update decision 42 (skip semantics) and decision 25 (walker pruning) | — | Decision 42 changed with H1; decision 25 changed with H9 (D-h) |
| D-b (H2) | An out-of-range symbol span is a hard error carrying the fqn and span; `content_hash` returns `Result` | Clamp the span to the source length and hash the wrong bytes | A span past the end is a parser bug, so the run must fail with context, never panic or hash a truncated slice |
| D-c (H3) | Enable `PRAGMA foreign_keys = ON` per index connection (build in `begin_index`, read in `IndexReader::open`) | Drop the `REFERENCES` clause | Keeps the schema's advertised integrity real; pre-order insert already satisfies it, so enabling is safe. SQLite/libSQL is per-connection and the pragma is a no-op inside a transaction, so it runs right after `connect`. Empirically, `INSERT OR IGNORE` does not suppress an FK violation (it errors), so a linking bug fails loudly |
| 43 | `build_index` requires `repo` to be an existing directory; zero-symbol files log at debug | Let the walker silently yield nothing on a bad path | A typo in `repo` must fail loudly, not produce an empty index; `package-info.java` is normal, so it must not warn on every run |
| D-d (H4) | `IndexReader::open(data_dir)` returns a read-only reader; `Store::open_index` is removed; `show` no longer opens `Store` | Keep opening read-write and fix only the doc; or keep both open paths | The read path must not create `cache.db`/the data dir; a single read entry point becomes the contract 6.1 reuses. `_db` anchors the connection (decision 18) |
| D-e (H5) | Corrupt annotation JSON is an error carrying the symbol fqn; `symbols::text` logs `tracing::warn!` (node kind + byte range) and returns empty | Make every text helper fallible | JSON corruption is a real, cheap-to-propagate failure; a UTF-8 failure is impossible for valid tree-sitter spans, so a log is the proportionate signal instead of rippling `Result` through every parser helper |
| D-f (H6) | One `build_symbol` factory builds every `Symbol`; a declaration's annotation nodes are walked once and the texts, signature and role all derive from that single list (`signature_of` takes the node list, `role_of` the extracted simple names) | Keep the two field literals and let `annotations_of`/`signature_of`/`role_of` each walk/parse independently | Removes the duplicated 15+ field construction and triple annotation traversal without changing any output; the existing `symbols` tests are the regression net. A record's body is a `class_body` resolved via `child_by_field_name("body")`, so the `record_body` arm and the unreachable `program` arm are dead |
| D-g (H7) | Wrap the whole build in one transaction (local to `build_index`), committed before the temp file is renamed | Per-file transactions; an `IndexBuild::transaction()` guard | Fewest round trips and the audit's first lever for the 2.3 batched baseline. A local transaction keeps the commit-before-`build.commit()` ordering explicit in one function and leaves `store.rs` production code untouched; a guard type would not really enforce ordering any harder. On error the transaction is dropped (rollback) before the temp file is discarded, so nothing partial is ever renamed over `index.db`. In libsql 0.9.30 `Connection::transaction()` is `async`, so the begin is `.await`ed (the plan text said sync) |
| D-h (H9) | Prune skip-dirs only outside source roots (no ancestor named `java`) | Prune by name anywhere (current); add `index.skip_dirs` config | Fixes the false negative with a one-function change; a config escape hatch is deferred to avoid cross-module scope creep |
| D-i (H8) | Correct decision 41's wording and track deep-nesting overflow as an open question | Rewrite `show` to an explicit stack now | Doc/behaviour drift is the actual finding; `show`'s plain recursion is fine at POC scale, and reworking rendering for a pathological input is unwarranted until a real index needs it |
| D-k (2.1) | `history_for_span` shells out to `git log -L` and parses a control-char-delimited `--format` (`%x1e`…`%x1d`), returning `Commit { sha, date (ISO `%aI`), subject, body }` newest first | A pure-Rust git implementation; `git blame` + `git show`; diff parsing | `-L` already tracks a line range backwards through history (including renames), which is exactly the per-symbol history 2.2 needs. Control chars can't appear in commit text, so diff hunks after the format record are dropped safely. Newest-first preserves git's order so 2.2 can take the last element as "introduced". A non-zero git exit is surfaced as an error rather than an empty history, so a misconfigured repo fails loudly |
| D-l (2.2) | The indexer degrades to a structure-only index when `repo` is not a git work tree (one warning), and skips a symbol whose history lookup fails (one warning naming the file); `history_for_span` itself still errors | Fail the whole run on a non-git repo or on any single file's history failure; or require git and initialize git in every unit-test fixture | Keeps the 57 existing non-git temp-dir tests meaningful without a test-wide git rewrite, and matches the product: structure is still useful without history. A single untracked file must not sink a whole index. The strict contract stays on the leaf function so a caller that truly needs history still gets an error |
| D-m (2.2) | Ticket `first_date`/`last_date` are derived from git's newest-first commit order (first sighting = most recent, later sightings move the first date back), never by comparing ISO strings | Parse dates / compare `%aI` strings lexicographically | `%aI` carries a timezone offset, so lexicographic comparison is wrong across offsets (e.g. `+01:00` vs `-05:00`); git's ordering is already correct and needs no date library. `symbol_commits`/`symbol_tickets` are `UNIQUE` per symbol so `INSERT OR IGNORE` is idempotent |
| D-n (2.2 decision, resolved by R5) | The git test helpers (`git`/`git_ok`/`init_repo`/`commit`) are currently duplicated in `history`, `indexer` and `show` test modules; consolidating into a `#[cfg(test)]` support module is queued, not done here | Extract now | Keeps step 2.2 scoped (AGENTS: one step = one small PR, no unrelated refactors). Recorded as open question #16 so the hygiene pass does not lose it |
| D-o (2.3) | `history_cache` in `cache.db`, keyed by fqn + content hash + the file's last commit sha, storing commits as JSON; one row per fqn (upsert on miss); per-file `git log -1` supplies the sha | Key by fqn + content hash only (zero git on a warm run, but blind to history rewrites); key by fqn + span only; bounded parallelism for the cold run | Follows the plan's stated key. The content hash alone would be faster, but the file's last commit sha is what catches a rewrite/revert that leaves the bytes identical (proved by a test). One `git log -1` (~22 ms on the 98k-commit repo) is ~4× cheaper than `-L` and is paid once per file, not per symbol. Parallelism would help only the one-time cold run and is a larger refactor; deferred (open #17) |
| D-p (2.3) | The measurement target is a **synthetic benchmark repo** (`tests/benchmark.rs`, `#[ignore]`), not a specific real product repo | Measure against `grld-spring-auth` (single-module) or `grld-core` (multi-module, 98k commits) | Product-owner choice: a controlled repo makes the numbers reproducible and independent of a checkout, and avoids the multi-module test-pruning gap (open #13) confounding the measurement. A real-repo end-to-end validation is deferred to Phase 6 (agent trial) or earlier if wanted. Recon numbers on real repos are recorded in the Done entry as context |
| D-s (R5) | One scratch-git helper file, `src/test_support.rs`: a `#[cfg(test)]` module for the lib, included by integration tests via `#[path = "../src/test_support.rs"]` | A second copy in `tests/common/mod.rs`; a `test-support` cargo feature exposing the helpers publicly | `#[path]` gives a single source without widening the public API or adding a feature. The constraint is that the file may depend on `std` only (no `crate::` paths), which the helpers already satisfy |

## Open questions

| # | Question | Raised at | Status |
| --- | --- | --- | --- |
| 1 | Jira auth scheme: Cloud uses email + API token (basic auth), Server/DC often uses a personal access token (bearer). Assume Cloud for now? | 0.1 | open |
| 2 | Config discovery: search parent directories, or add `ANNATAR_CONFIG`? Currently only cwd/`--config`. | 0.1 | open, defer |
| 3 | Should `jira` / `ollama` sections become optional per command (e.g. `show` may not need Jira)? | 0.1 | resolved — both sections are now `Option`; commands require them when wired |
| 4 | `--path` is currently a `String`; make it a `PathBuf` once the walker lands (1.1)? | 0.1 | resolved — now `PathBuf` |
| 5 | Should `cache.db` use WAL for concurrent reader access (MCP server)? The index stays rollback-journal so rename is single-file. | 0.2 | open, defer |
| 6 | `Store` exposes no read connection to the committed `index.db` yet. Needed by `show` (1.5) and the MCP server reopening on replace (6.1). | 0.2 | resolved — `IndexReader::open` returns a read-only reader; 6.1 will reopen on file replace |
| 7 | Is `data_dir` resolved relative to the cwd or to the config file's directory? | 0.2 | resolved — relative to the config file's directory (also applies to `repo`) |
| 8 | Index schema: a central schema module, or per-phase DDL run against `begin_index`? | 0.2 | resolved — one `schema` module (decision 23) |
| 9 | Walker uses `parents(false)`: if `repo` ever points at a subdirectory of a larger checkout, `.ignore`/`.gitignore` above it are not read. Acceptable? | 1.1 | open, defer — fine while `repo` is a repo root |
| 10 | `--path` is a literal prefix; should it accept globs (e.g. `src/main/java/com/acme/**`)? | 1.1 | open, defer — literal prefix matches the plan wording |
| 11 | A valid but symbol-less file (e.g. `package-info.java`) is counted `skipped` like a parse error. Distinguish parse-error vs empty? | 1.5 | resolved by H1 — `ParsedFile.parse_error` separates them; `IndexStats` reports `empty` / `parse_errors` / `unreadable` |
| 12 | `symbols` has no uniqueness constraint tying fqn to a file; two source roots defining the same fqn keep the first and drop the second. | 1.5 | open, defer — duplicate fqn is a compile error for a real repo |
| 13 | Test-code detection only matches a repo-root `src/test`; multi-module repos put tests at `<module>/src/test`. | 1.5 review | open, defer — multi-module handling is a "Later" item; note if the Phase 2 target is multi-module |
| 14 | Phase 2.1 assumes `repo` is a git work tree and the file paths it stores match git's root. A dirty or renamed tree may not match `HEAD` line numbers. | 1.5 review | partially resolved by 2.1 — renamed files are fine (`git log -L` follows renames by default; verified). Remaining: a **dirty** working tree's stored line numbers come from the working copy, while `-L` resolves the range against committed revisions, so uncommitted edits can mis-attribute. 2.2 runs history before/while writing symbols; decide whether to warn on a dirty tree or accept it for the POC |
| 15 | `show::render` recurses with plain function calls, so a pathologically deep symbol nesting could overflow the stack. Guard the depth, or move rendering to an explicit stack? | H8/13 | open, defer — POC-scale nesting is shallow; revisit if a real repo triggers it |
| 16 | The scratch-git test helpers are now duplicated across the `history`, `indexer`, `show` and `benchmark` test modules (four copies of `git`/`git_ok`/`init_repo`/`commit`). Extract a shared `#[cfg(test)]` support module. | 2.2 review | resolved by R5 — `src/test_support.rs`, shared with `tests/benchmark.rs` via `#[path]` (decision D-s) |
| 17 | The history cache makes *repeat* runs fast (1.2 s), but the first cold run over a huge real repo is still minutes (per-symbol `git log -L`). Add bounded parallelism (results through one writer) if cold-run time starts to hurt, or rely on the project's central-build model. | 2.3 | open, defer — the plan offered parallelism *or* the cache; the cache was chosen (daily iteration is the stated goal). Revisit when a real repo is indexed end to end. |
| 18 | Phase 2.3 measured a synthetic repo by product-owner decision; no real GRLD repo has been indexed end to end (multi-module `grld-core` would also hit the test-pruning gap of open #13). | 2.3 | open — Phase 2 is marked *complete with deviations* (R0); closed by audit item R8 (real-repo run: symbols, cold/warm time, ticket coverage) before Phase 3.2; may force a decision on #13 |
