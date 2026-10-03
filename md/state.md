# Annatar — State

Living document. Updated after every step. Tracks where the project stands,
the decisions taken, and the tradeoffs behind them.

- **Plan:** [`plan.md`](./plan.md)
- **Overview:** [`project.md`](./project.md)

## Current state

| | |
| --- | --- |
| Phase | 1 — Symbols |
| Step | 1.5 Write symbols and `show` — **done** (Phase 1 complete) |
| Last updated | 2026-10-03 |
| Toolchain | rustc 1.97.0, edition 2024 |

### Done

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

- 2.1 History for a span: `git log -L <start>,<end>:<file>` via
  `std::process::Command`, with a custom `--format` and record delimiter. Note
  behaviour on renamed files in the PR.

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
| 16 | `Store` is async; `tokio` is the runtime | Synchronous wrapper | The `libsql` API is async. `Store::open`/`begin_index`/`open_index` are async, `IndexBuild::commit` is sync (drop + rename). Tests use `#[tokio::test]` |
| 17 | `Store` owns no schema in 0.2; it exposes `cache()` and the `begin_index()` connection | Bundle table creation into 0.2 | Each phase adds its own tables (plan). Keeps this step to the file lifecycle and lets schema land with the data it stores |
| 18 | Drop order: connection fields before their `Database`; `_cache_db` field anchors the cache connection | Rely on `Connection` alone | Explicit and safe: the connection is closed before the database handle; the anchor field is underscore-prefixed to stay intentionally unread |
| 19 | `repo` and `data_dir` resolve relative to the config file's directory, not the cwd (`Config::load`) | Resolve against the cwd | A config file describes its own repo/data layout, so the same file works from any working directory. Absolute paths are returned unchanged |
| 20 | `--path` is a `PathBuf` | `String` | It is a filesystem path prefix; parse it as one so the walker (1.1) doesn't re-parse or re-validate |
| 21 | `jira` and `ollama` are `Option<...>` in `Config` | Keep them required | Phases 1–2 (`index`, `show`) don't talk to Jira or Ollama; requiring them taxes the `--path` iteration loop. A command that needs them will require them once wired (3.x/4.x) |
| 22 | `Store::begin_index` returns an owned `IndexBuild` with `commit()`; `Store` no longer tracks the in-progress build | Return `&Connection` from `&mut Store`, then finish on `Store` | The borrow blocked holding the connection across an async pipeline while later calling finish, and mixed a sync finish with an async build. An owned guard survives the whole run and aborting is just a drop |
| 23 | All DDL lives in one `schema` module: `INDEX_TABLES` applied by `begin_index`, `CACHE_TABLES` by `open` | Per-phase DDL scattered in each step | `index.db` is disposable, so no migrations or versioning; one file keeps the whole shape reviewable as phases 1–5 add tables |
| 24 | A working dummy `annatar.toml` is committed (data dir `.annatar`, gitignored) | No config in the repo | The binary's default config now loads out of the box; secrets still come only from the environment, so the file stays committable |
| 25 | Generated sources detected by directory name (`generated`, `generated-sources`), pruned anywhere | Parse Maven/Gradle build files | Language/build-system agnostic, cheap, and testable; revisit if a repo names its tree differently |
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
| 41 | `show::render` loads the whole `symbols` table, builds a child map in memory, recurses with an explicit stack | One query per node / recursive CTEs | POC-scale tables make this cheap and it avoids async recursion; revisit when the index grows |
| 42 | `IndexStats.files` counts files that produced a symbol; a genuinely empty or unparseable file counts as `skipped` | Count every walked file as `files` | Keeps `files + skipped` = walked, and treats "nothing to index" uniformly until the parser can report parse-error vs empty |
| 43 | `build_index` requires `repo` to be an existing directory; zero-symbol files log at debug | Let the walker silently yield nothing on a bad path | A typo in `repo` must fail loudly, not produce an empty index; `package-info.java` is normal, so it must not warn on every run |

## Open questions

| # | Question | Raised at | Status |
| --- | --- | --- | --- |
| 1 | Jira auth scheme: Cloud uses email + API token (basic auth), Server/DC often uses a personal access token (bearer). Assume Cloud for now? | 0.1 | open |
| 2 | Config discovery: search parent directories, or add `ANNATAR_CONFIG`? Currently only cwd/`--config`. | 0.1 | open, defer |
| 3 | Should `jira` / `ollama` sections become optional per command (e.g. `show` may not need Jira)? | 0.1 | resolved — both sections are now `Option`; commands require them when wired |
| 4 | `--path` is currently a `String`; make it a `PathBuf` once the walker lands (1.1)? | 0.1 | resolved — now `PathBuf` |
| 5 | Should `cache.db` use WAL for concurrent reader access (MCP server)? The index stays rollback-journal so rename is single-file. | 0.2 | open, defer |
| 6 | `Store` exposes no read connection to the committed `index.db` yet. Needed by `show` (1.5) and the MCP server reopening on replace (6.1). | 0.2 | resolved — `Store::open_index` returns a read connection; 6.1 will reopen on file replace |
| 7 | Is `data_dir` resolved relative to the cwd or to the config file's directory? | 0.2 | resolved — relative to the config file's directory (also applies to `repo`) |
| 8 | Index schema: a central schema module, or per-phase DDL run against `begin_index`? | 0.2 | resolved — one `schema` module (decision 23) |
| 9 | Walker uses `parents(false)`: if `repo` ever points at a subdirectory of a larger checkout, `.ignore`/`.gitignore` above it are not read. Acceptable? | 1.1 | open, defer — fine while `repo` is a repo root |
| 10 | `--path` is a literal prefix; should it accept globs (e.g. `src/main/java/com/acme/**`)? | 1.1 | open, defer — literal prefix matches the plan wording |
| 11 | A valid but symbol-less file (e.g. `package-info.java`) is counted `skipped` like a parse error. Distinguish parse-error vs empty? | 1.5 | open — resolve before 2.3 reports run quality; needs `parse_file` to signal a parse failure |
| 12 | `symbols` has no uniqueness constraint tying fqn to a file; two source roots defining the same fqn keep the first and drop the second. | 1.5 | open, defer — duplicate fqn is a compile error for a real repo |
| 13 | Test-code detection only matches a repo-root `src/test`; multi-module repos put tests at `<module>/src/test`. | 1.5 review | open, defer — multi-module handling is a "Later" item; note if the Phase 2 target is multi-module |
| 14 | Phase 2.1 assumes `repo` is a git work tree and the file paths it stores match git's root. A dirty or renamed tree may not match `HEAD` line numbers. | 1.5 review | open — decide handling (error, warn, or blame working tree) when 2.1 lands |
