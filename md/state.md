# Annatar — State

Living document. Updated after every step. Tracks where the project stands,
the decisions taken, and the tradeoffs behind them.

- **Plan:** [`plan.md`](./plan.md)
- **Overview:** [`project.md`](./project.md)

## Current state

| | |
| --- | --- |
| Phase | 3 — Jira (in progress); Phase 2 and its audit remediation R0–R8 complete |
| Step | 3.1 Fetch and parse — **done** (synthetic fixtures; real-token run pending, J3) |
| Last updated | 2026-10-03 |
| Toolchain | rustc 1.99.0, edition 2024 |

### Done

- **3.1 review fixes.** Body read errors are `FetchError::Transport`
  (`bytes()` then `serde_json::from_slice` → `Parse`). A 403 with
  `X-Authentication-Denied-Reason` (Server/DC CAPTCHA after failed logins) is
  `Unauthorized`, so bad credentials never look like an unavailable issue
  (J6); `status_error` now takes the headers and the auth mode, and the
  `Unauthorized { basic }` message names `ANNATAR_JIRA_EMAIL` only in basic
  mode. Request construction is a separate `JiraClient::request` (built, not
  sent) with a test for URL, `Accept` and `Authorization` (basic vs bearer).
  `html_to_text` turns off unicode strikeout, collapses runs of blank lines,
  and its doc says links keep their `[text]` brackets. `JiraClient::new`
  rejects a `base_url` that is not an absolute http(s) URL and stores it
  trimmed. New tests: epic-link object with `key`, empty parent key, long
  paragraph stays on one line (guards `TEXT_WIDTH`). `fetches_real_ticket`
  no longer requires the returned key to equal the requested one (it logs the
  difference). Nits: `#[tokio::test]`, `Auth` no longer derives
  `PartialEq, Eq`, `status_error`/`issue_url`/`html_to_text`/`REQUEST_TIMEOUT`
  private. D-x and D-z extended. 20 jira tests + 1 config test (1 ignored);
  101 tests total.

- **3.1 Fetch and parse.** New `jira` module: `Ticket { key, issue_type,
  summary, description: Option<String>, parent_key }`, a pure
  `parse_issue(&Value, epic_link_field)`, and an async `JiraClient::fetch`
  calling `GET /rest/api/2/issue/{key}?expand=renderedFields` with a 30 s
  timeout. Auth per J1 (`Auth::from_config`: email set → basic, else bearer;
  missing token is an error). HTML → text via `html2text` (no emphasis
  markers, table borders or link URLs). Parent from `fields.parent.key`, with
  the new optional `jira.epic_link_field` as fallback (J9). `FetchError` keeps
  401 (`Unauthorized`), 403/404 (`Unavailable`), 429 (`RateLimited` with
  `Retry-After` seconds) and other statuses, invalid keys, transport and
  parse failures apart for 3.2 (pure `status_error` mapping). Keys are
  checked to be `[A-Za-z0-9_-]+` before they go into the URL path.
  **Deviation (J3):** no real Jira token is available to the agent, so the
  five fixtures in `tests/fixtures/jira/` (story, bug, sub-task with parent,
  epic child via an epic-link custom field, null description) are *synthetic*,
  modelled on the REST v2 `renderedFields` shape. The product owner must
  replace them with scrubbed real captures and run the `#[ignore]`
  `jira::tests::fetches_real_ticket` (audit §3.1). Not wired into
  `annatar index` yet (3.2/R9). 12 jira tests + 1 config test (1 ignored);
  93 tests total.

- **R8 Real-repo run.** Indexed `argus` (Greenland, single-module Java
  backend, 217 Java files, 1375 commits; mounted at `/Repos/argus`), which
  avoids the multi-module gap of open #13. Numbers are from a `--release`
  build on rustc 1.99.0 (linux/aarch64).

  | | |
  | --- | --- |
  | Files / symbols | 217 / **960** (671 method, 213 class, 58 constructor, 12 interface, 4 annotation, 2 enum) |
  | Cold run | **14.97 s** — 0 hits / 960 misses / 0 skipped → **15.6 ms/symbol** |
  | Warm run | **1.92 s** (and 1.93 s again) — 960 hits / 0 misses → 2.0 ms/symbol, **~7.8×** |
  | `--path` subset | 54 files / 247 symbols warm in **0.52 s** |
  | Stored | 2920 `symbol_commits`, 1821 `symbol_tickets` rows, 79 distinct keys |
  | **Ticket coverage** | **888 / 960 symbols = 92.5 %** with ≥1 ticket |
  | Degradation | 0 empty, 0 parse errors, 0 unreadable, 0 history-skipped |
  | On disk | `index.db` 1.37 MB, `cache.db` 0.96 MB |

  The R1 invariant `hits + misses + skipped == symbols` holds on both runs
  (0+960+0 and 960+0+0). Every symbol got history; no file fell into the
  untracked/failed path.

  **The cold run is git-bound, not CPU-bound.** The same run on the debug
  binary took 15.02 s / 2.05 s — within noise of release. The cold run spends
  6.7 s of CPU across 15.0 s wall (**44 % CPU**), i.e. most of it is spent
  waiting on sequential `git log -L` subprocesses. Optimising the Rust side
  cannot move this number; only cutting or overlapping git calls can (see
  open #17).

  **Spot-checked against git, not just counted.** For
  `AuthDataCacheRest#findUsers(String)` the stored commits are exactly the four
  `git log -L` reports, same shas and order. **R3 (Javadoc span) is live on
  real data:** of 40 sampled Javadoc'd symbols, 2 have a span whose history
  differs from the declaration-only span, and in both the stored set matches
  the Javadoc-inclusive `-L` exactly. **R2 (dirty tree) is live:** shifting
  lines in one file of a clone produced exactly one summary warning, 6 misses
  for that file's 6 symbols and **zero** cache writes (960 rows before and
  after); after committing, the rows were rewritten and the next run was 960
  hits. Non-`GRLD` keys in subjects (`RDRK-2380`) are correctly not extracted.

  **Finding 5 (keys in merge commits) — measured, no action needed.** 336
  merges (229 mention a key) vs 1039 non-merges (760 mention a key), but only
  **8 of 194** distinct keys appear *exclusively* in merge subjects/bodies
  (4 %). A first-parent/merge-message pass would buy ~4 % of keys; **not**
  worth a follow-up item before Phase 4.

  **Finding 8 (type `-L` ≈ file history) — the premise does not hold here, so
  the cold-run lever on open #17 is withdrawn.** For 30 sampled top-level
  types, the stored span history equals `git log -- <file>` in only 9 cases,
  and the span yields **1.89× more** commits on average (e.g.
  `AuthDataCacheRest`: 24 vs 18). `git log -- <file>` applies history
  simplification and drops commits that are TREESAME through a merge, which
  `-L` still reports (`--full-history` gives 33). Substituting it would *lose*
  roughly a quarter of a type's commits, so it is a correctness regression,
  not a free speedup.

  **New finding (not in the audit): a `--path` run narrows the whole index.**
  `index.db` is rebuilt into a temp file and atomically renamed on every run,
  so a `--path` run leaves an index containing *only* that prefix, and
  `--path <missing-dir>` (the "missing prefix → empty list" contract kept by
  R6, D-v) leaves it with **0 symbols** — `show` then fails for every symbol
  in the repo. Nothing in `plan.md`, `AGENTS.md` or `README.md` says the flag
  is destructive; it is documented as "limits a run to part of the repo, for
  fast iteration". Logged as open #19 for R9/J4 to resolve.

  **Toolchain installed; the gates ran for the first time.** Earlier phases
  recorded test counts without `cargo` being present in the container. A
  stable toolchain is now installed (rustup 1.29.1 → rustc 1.99.0, via
  `static.rust-lang.org`; the proxy allowlist needed subdomain entries for the
  Rust domains). On rustc 1.99.0, two versions newer than the recorded 1.97.0:
  `cargo fmt --check` clean, `cargo clippy --all-targets -- -D warnings`
  clean, `cargo test` **80 lib + 1 integration passed, 0 failed**. The
  `#[ignore]` benchmark passes its R5 count assertions; on release it is cold
  2.42 s, warm 0.30 s, changed 0.52 s (the recorded 3.41 / 0.39 / 0.72 were
  debug). Every previously claimed-but-unverified green gate is now actually
  verified.

- **R7 Doc drift.** (21) `project.md` no longer contradicts the ground
  rules: storage is two libSQL files (`index.db` portable and atomically
  rebuilt, `cache.db` persistent and never shipped) with libSQL's native
  vector index instead of "one SQLite file with sqlite-vec"; symbols are
  identified by fqn (`get_symbol(fqn)`, `trace_usage(fqn, depth)`);
  `search_intent(query, kind?, role?, path?)` is a superset of the plan's
  6.1; the broken `&#91;embedded content…\]` export became a five-step list. (22)
  `AGENTS.md` step 4 now gates on `cargo clippy --all-targets -- -D
  warnings`, which also lints tests and the benchmark (already clean). (23)
  README gained a Usage section: build, `annatar index [--path]`,
  `annatar show <fqn>`, config location, secrets from the environment, and
  the `git` work-tree requirement. Deferred findings recorded: (8) a type's
  `-L` span is its whole body — noted in `plan.md` 4.4 and open #17; (10)
  `show` loads every symbol/commit/ticket row — noted on open #15 for 6.1;
  (16) `history_cache_v2` is never pruned — `plan.md` Later list. Docs only;
  80 lib + 1 integration tests green. **Remediation R0–R7 complete.**

- **R6 Small cleanups.** (12) `ticket_span` returns `Vec<TicketSpan { key,
  first_date, last_date }>` built with a key → index map, replacing the
  `(String, String, String)` tuple and the production `expect`. (14)
  `Config.ticket_regex` is now a compiled `regex::Regex`, deserialized with a
  `deserialize_with` that reports `invalid ticket_regex "<pattern>": …`
  inside the TOML error (now with line/column); `Config::validate` and
  `main`'s second compile are gone, `main` passes `&config.ticket_regex`. (9)
  `--path` now prunes the walk itself: `walk::on_prefix_path` keeps only the
  prefix, its subtree and its ancestors, so the rest of a large repo is never
  visited. The walk still starts at `repo` (not `repo/prefix`) so ancestor
  `.gitignore` files keep applying, and a missing or outside prefix still
  yields an empty list (existing contract kept). 2 new walk tests (ancestor
  `.gitignore` under a prefix; pure pruning decisions); the invalid-regex
  config test now checks the pattern is named. Findings 9, 12, 14. Decision
  D-v. 80 lib + 1 integration tests green.

- **R4 Deterministic, cheaper git output; batched cache writes.** Every
  production `git log` now passes `--no-color --no-ext-diff
  --no-show-signature` (`history::LOG_FLAGS`), and `git log -L` adds `-s`, so
  git no longer prints (and the parser no longer reads and discards) the diff
  hunks; `git diff HEAD` in `dirty_files` gets `--no-color --no-ext-diff`. The
  parser still tolerates hunks, for gits that ignore `-s` with `-L`. A file's
  history-cache writes now share one transaction on the cache connection (one
  commit per file instead of one autocommit per symbol), still separate from
  the index transaction (D-o); failing to begin or commit it only warns.
  Benchmark (linux, debug, 3 runs each): cold **~2.78 s** vs ~3.55 s before
  R4 (≈22 % faster); warm/changed unchanged (~0.4 s / ~0.6 s). Findings 6, 7,
  15. 1 new test (output unchanged with `color.ui=always`,
  `log.showSignature=true`, `diff.external=false` in the repo config — a
  regression guard); 78 lib + 1 integration tests green, benchmark green.

- **R3 Javadoc-inclusive history span.** Decision J0 taken as recommended: a
  commit that only edits a symbol's Javadoc is part of its history (the
  content hash already includes the Javadoc, and doc edits often carry the
  "why" ticket). `Symbol` gained `history_start_line` — the Javadoc comment's
  first line when there is one, else `start_line` — and `symbol_history` passes
  it to `git log -L`. `start_line` (what `show` and file:line report) is
  unchanged. `javadoc_of` became `javadoc_node`, so the comment node yields
  both the cleaned text and the span start. Because the cached *value* changed
  under an unchanged key, the cache table was renamed to `history_cache_v2` and
  `CACHE_TABLES` drops the old `history_cache` (ground rule: drop a cache table
  whose meaning changes); the first run after upgrading is a cold run.
  Finding 27. Decision D-t. 3 new tests (span lines, doc-only commit
  attributed — verified to fail with the old span —, legacy table dropped);
  77 lib + 1 integration tests green, benchmark green.

- **R2 Dirty working tree: history without caching.** New
  `history::dirty_files` runs one `git diff HEAD --name-only -z --relative
  --no-renames` per run: staged and unstaged changes, as paths relative to
  `repo` (so they match the walker even when `repo` is a subdirectory of the
  work tree; untracked files are excluded, they have no history). `build_index`
  intersects it with the walked files, logs **one** warning with the count
  (paths at debug) and passes `cacheable = false` to `symbol_history` for those
  files: they still get `-L` history (approximate, good enough for the `--path`
  loop) but never read or write `history_cache`, so a mis-attributed result can
  no longer be cached under `HEAD`'s sha and reused. If the check itself fails
  (e.g. no `HEAD` yet), every file is treated as dirty for that run. Finding 3;
  closes open #14. Decision D-r. 3 new tests (2 `dirty_files`, 1 indexer:
  dirty file uncached on two runs, cached after commit); 74 lib + 1
  integration tests green, benchmark green.

- **R1 `symbol_history` helper; skip untracked files once; degrade cache
  faults.** The cache get → `git log -L` → cache put sequence, copied twice in
  `build_index`, is now one `symbol_history` function returning a
  `HistoryOutcome` (`Hit` / `Miss` / `Failed`), which removes two nesting
  levels from the loop. A file no commit touches (or whose `git log -1` fails)
  now gets **one** warning naming the file and no `git log -L` at all; before,
  every symbol in it spawned a doomed `-L` and warned (a 40-method class: 41
  spawns, 41 warnings). Cache faults degrade instead of aborting the run: an
  unreadable or undecodable `history_cache` row warns and counts as a miss
  (and is rewritten), a failed cache write warns and keeps the commits.
  `IndexStats` derives `Default` and gained `history_skipped`, so on a git
  repo `hits + misses + skipped == symbols`; `index` prints it.
  `history_misses` is documented as "no usable cache row". Verified with the
  CLI on a scratch repo with one untracked 4-symbol file: one warning,
  `4 history skipped`. Findings 1, 2, 4, 11, 13, 26; D-l now matches the code.
  Decision D-q. 2 new tests; 71 lib + 1 integration tests green, benchmark
  green (asserts `history_skipped == 0`).

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

- **Product owner:** run the token end-to-end test
  (`cargo test -- --ignored fetches_real_ticket --nocapture`, audit §3.1),
  replace the synthetic fixtures in `tests/fixtures/jira/` with scrubbed real
  captures (open #20), and report the auth mode and parent/epic field shape.
  Confirm or override the PM defaults for J1, J2, J9 (D-w–D-y), answer J4–J8,
  and decide open #19 (`--path` narrows the whole index).
- **Agent:** R9 staged pipeline (needs J4; natural home for open #19), then
  3.2 Ticket cache. 3.2 must key the cache by the *requested* key, not
  `Ticket.key` (D-z).

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
| R1 `symbol_history` helper | done | one cache → git → cache path; untracked file: one warning, no `-L`; cache read/decode/write faults warn and degrade; `IndexStats: Default` + `history_skipped`; 2 tests (untracked + corrupt row); decision D-q; 71 tests |
| R2 Dirty working tree | done | `history::dirty_files` (`git diff HEAD --name-only --relative`), once per run; dirty files get history but bypass the cache; one summary warning; closes open #14; decision D-r; 3 tests; 74 tests |
| R3 Javadoc-inclusive history span | done | `Symbol.history_start_line` (Javadoc start or `start_line`) feeds `-L`; `history_cache` → `history_cache_v2`, old table dropped; 3 tests; decision D-t (J0); 77 tests |
| R4 Deterministic git, batched cache writes | done | `LOG_FLAGS` (`--no-color --no-ext-diff --no-show-signature`) + `-s` on `-L`; one cache transaction per file; cold ~2.78 s vs ~3.55 s (debug); 1 config-independence test; 78 tests |
| R6 Small cleanups | done | `TicketSpan` struct (no production `expect`); `Config.ticket_regex: Regex` via `deserialize_with`, compiled once; `--path` prunes the walk to the prefix path, ancestor `.gitignore` kept; 2 walk tests; decision D-v; 80 tests |
| R7 Doc drift | done | `project.md` storage/fqn/MCP signatures/broken export fixed; `AGENTS.md` clippy `--all-targets`; README usage + git requirement; findings 8/10/16 recorded as notes; docs only; 80 tests |
| R8 Real-repo run | done | `argus` (release, rustc 1.99.0): 217 files, 960 symbols, cold 14.97 s / warm 1.92 s (~7.8×), 15.6 ms/symbol, **92.5 % ticket coverage**, 0 degraded; cold run is git-bound (44 % CPU), release ≈ debug; R2/R3 verified live, history spot-checked against git; closes open #18; finding 5 measured (4 % merge-only keys → no action), finding 8 withdrawn; new open #19 (`--path` narrows the index). Toolchain installed: fmt/clippy/`cargo test` (80+1) green for the first time |
| 3.1 Fetch and parse | done | `jira` module: `Ticket`, pure `parse_issue`, async `JiraClient::fetch` (REST v2, `expand=renderedFields`, 30 s timeout), `Auth` (J1), `FetchError` + `status_error`, `html2text`; `jira.epic_link_field` (J9); `reqwest` 0.13 (`json` + `rustls`); **synthetic** fixtures (J3 deviation, open #20); `#[ignore]` `fetches_real_ticket`; decisions D-w–D-z; 93 tests |
| 3.1 review fixes | done | body read errors → `Transport`; 403 + `X-Authentication-Denied-Reason` → `Unauthorized` (J6); testable `JiraClient::request`; no unicode strikeout, blank-line runs collapsed; `base_url` validated and trimmed; 401 message per auth mode; more parse/HTML tests; D-x/D-z notes; 101 tests |

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
| D-l (2.2) | The indexer degrades to a structure-only index when `repo` is not a git work tree (one warning), and indexes a file with no commit, or whose last-commit lookup fails, without history (one warning naming the file, symbols counted in `history_skipped`; R1); `history_for_span` itself still errors | Fail the whole run on a non-git repo or on any single file's history failure; or require git and initialize git in every unit-test fixture | Keeps the 57 existing non-git temp-dir tests meaningful without a test-wide git rewrite, and matches the product: structure is still useful without history. A single untracked file must not sink a whole index. The strict contract stays on the leaf function so a caller that truly needs history still gets an error |
| D-m (2.2) | Ticket `first_date`/`last_date` are derived from git's newest-first commit order (first sighting = most recent, later sightings move the first date back), never by comparing ISO strings | Parse dates / compare `%aI` strings lexicographically | `%aI` carries a timezone offset, so lexicographic comparison is wrong across offsets (e.g. `+01:00` vs `-05:00`); git's ordering is already correct and needs no date library. `symbol_commits`/`symbol_tickets` are `UNIQUE` per symbol so `INSERT OR IGNORE` is idempotent |
| D-n (2.2 decision, resolved by R5) | The git test helpers (`git`/`git_ok`/`init_repo`/`commit`) are currently duplicated in `history`, `indexer` and `show` test modules; consolidating into a `#[cfg(test)]` support module is queued, not done here | Extract now | Keeps step 2.2 scoped (AGENTS: one step = one small PR, no unrelated refactors). Recorded as open question #16 so the hygiene pass does not lose it |
| D-o (2.3) | `history_cache` (renamed `history_cache_v2` by R3, D-t) in `cache.db`, keyed by fqn + content hash + the file's last commit sha, storing commits as JSON; one row per fqn (upsert on miss); per-file `git log -1` supplies the sha | Key by fqn + content hash only (zero git on a warm run, but blind to history rewrites); key by fqn + span only; bounded parallelism for the cold run | Follows the plan's stated key. The content hash alone would be faster, but the file's last commit sha is what catches a rewrite/revert that leaves the bytes identical (proved by a test). One `git log -1` (~22 ms on the 98k-commit repo) is ~4× cheaper than `-L` and is paid once per file, not per symbol. Parallelism would help only the one-time cold run and is a larger refactor; deferred (open #17) |
| D-p (2.3) | The measurement target is a **synthetic benchmark repo** (`tests/benchmark.rs`, `#[ignore]`), not a specific real product repo | Measure against `grld-spring-auth` (single-module) or `grld-core` (multi-module, 98k commits) | Product-owner choice: a controlled repo makes the numbers reproducible and independent of a checkout, and avoids the multi-module test-pruning gap (open #13) confounding the measurement. A real-repo end-to-end validation is deferred to Phase 6 (agent trial) or earlier if wanted. Recon numbers on real repos are recorded in the Done entry as context |
| D-s (R5) | One scratch-git helper file, `src/test_support.rs`: a `#[cfg(test)]` module for the lib, included by integration tests via `#[path = "../src/test_support.rs"]` | A second copy in `tests/common/mod.rs`; a `test-support` cargo feature exposing the helpers publicly | `#[path]` gives a single source without widening the public API or adding a feature. The constraint is that the file may depend on `std` only (no `crate::` paths), which the helpers already satisfy |
| D-q (R1) | Cache faults degrade: an unreadable/undecodable `history_cache` row is a miss (and is rewritten), a failed write warns and keeps the commits. A file with no commit is skipped once with one warning, counted in `history_skipped` | Propagate cache errors (previous behaviour); count skipped symbols as misses | The cache is disposable by the ground rules, so it must never sink an index — the same degrade-don't-abort policy D-l chose for git. A separate `history_skipped` bucket keeps `history_misses` meaning "paid for a `git log -L`", which is what the benchmark and cold-run estimates need |
| D-r (R2) | Detect files with uncommitted changes once per run (`git diff HEAD --name-only -z --relative --no-renames`); give them `-L` history but never read or write the cache for them; one summary warning. A failed check treats every file as dirty | Skip history for dirty files; map working-copy lines onto `HEAD` via `git diff`; ignore it (status quo) | Skipping would blank history in exactly the `--path` dev loop; line mapping is a lot of machinery for a POC. Not caching is the minimal fix for the real harm (a wrong result persisted under `HEAD`'s sha and reused). `git diff --relative` instead of `git status --porcelain` because its paths are relative to `repo`, not the work-tree root, so a `repo` below the root still matches the walker |
| D-t (R3, J0) | The history span starts at the Javadoc (`Symbol.history_start_line`), so a doc-only commit and its ticket belong to the symbol. `start_line` is unchanged for display. The cache table is renamed `history_cache_v2` and the old `history_cache` is dropped on open | Keep the declaration-only span and pin it with a test; bump a version column inside `history_cache` | The content hash already treats the Javadoc as part of the symbol, and doc commits often carry the "why". The key (fqn + hash + file sha) does not change when the span does, so old rows would be silently wrong: dropping the table is the ground-rule answer, and a new name makes the one-off `DROP TABLE IF EXISTS` idempotent |
| D-u (R4) | `git log` runs with `--no-color --no-ext-diff --no-show-signature`, `-L` with `-s`; history-cache writes are batched in one cache-connection transaction per file | Enforce a minimum git version for `-s` with `-L`; one cache transaction per run | The flags make output independent of the developer's git config and stop git from producing hunks nobody reads; keeping the hunk-tolerant parser avoids a version check. Per file (not per run) bounds what a crash loses to one file's results while still removing the per-symbol fsync |
| D-v (R6) | `Config.ticket_regex` is a compiled `Regex` (custom `deserialize_with`); `--path` prunes directories off the prefix path inside the walker's `filter_entry` instead of starting the walk at `repo/prefix` | Keep a `String` plus a cached `Option<Regex>`; start `WalkBuilder` at `repo/prefix` | One compile and one error path, reported by the TOML parser with line/column. The default pattern uses `Regex::new(DEFAULT_TICKET_REGEX).expect(..)`: a constant, covered by the config defaults test, unlike the data-dependent `expect` removed from `ticket_span`. Starting the walk at the prefix with `parents(false)` would silently skip the repo-root `.gitignore`; pruning in `filter_entry` keeps the walk equivalent while still never visiting the rest of the repo |
| D-w (3.1, J1) | Jira auth: `ANNATAR_JIRA_EMAIL` set (non-empty) → basic `email:token` (Cloud API token); otherwise the token is sent as a bearer PAT (Server/DC). A missing or empty `ANNATAR_JIRA_TOKEN` fails `JiraClient::new`. PM default, adopted audit recommendation; product owner may override. | Cloud only; a separate `auth` config key | Both schemes are a few lines and the existing optional `email` already tells them apart; resolves open #1 |
| D-x (3.1, J2) | `reqwest` 0.13 with `default-features = false`, features `json` + `rustls` (0.13 renamed the old `rustls-tls` feature; it uses aws-lc-rs and the platform certificate verifier), and `html2text` (`plain_no_decorate`, no table borders, no link footnotes) for `renderedFields` HTML. PM default, adopted audit recommendation; product owner may override. | `ureq`/blocking; hand-written HTML stripper; ADF parser via REST v3 | Async fits the tokio runtime and 3.2's bounded concurrency; `html2text` keeps lists, tables and code blocks readable. Accepts the TLS compile cost decision 13 avoided for libSQL. A lighter alternative exists: reqwest `rustls-no-provider` plus `rustls` with the `ring` provider avoids the `aws-lc-sys` build; not adopted for now. `default-features = false` also drops reqwest's `system-proxy` (OS proxy settings); `HTTPS_PROXY`/`HTTP_PROXY` from the environment are still honoured. Plain output avoids `**`/URL noise in LLM prompts |
| D-y (3.1, J9) | `parent_key` = `fields.parent.key`; only when absent, the optional `jira.epic_link_field` custom field (a string key, or an object with `key`). Unset = ignored. PM default, adopted audit recommendation; product owner may override. | Discover the epic-link field via `/rest/api/2/field`; parent only | Cloud covers it via `parent`; Server/DC opts in without code changes |
| D-z (3.1, J3) | **Deviation:** fixtures under `tests/fixtures/jira/` are synthetic (REST v2 `renderedFields` shape, `jira.example.com`), not real captures; the real-token run is the `#[ignore]` `jira::tests::fetches_real_ticket` (base URL from `ANNATAR_JIRA_URL` or `annatar.toml`, key from `ANNATAR_JIRA_TEST_KEY`; prints only auth mode, key, type, parent and lengths). Also: REST v2 (works on Cloud and Server/DC), `Ticket.key` is the key Jira returns — a moved issue returns its *new* key, so 3.2 must key the cache by the *requested* key (and `fetches_real_ticket` only logs a mismatch), description `None` when null or blank after conversion, keys outside `[A-Za-z0-9_-]` are rejected before any request | Block 3.1 on a token | No token is available to the agent; parsing is pure, so swapping in real captures only changes test data. Tracked as open #20 |

## Open questions

| # | Question | Raised at | Status |
| --- | --- | --- | --- |
| 1 | Jira auth scheme: Cloud uses email + API token (basic auth), Server/DC often uses a personal access token (bearer). Assume Cloud for now? | 0.1 | resolved (3.1, D-w) — email set → basic, else bearer PAT; PM default, product owner may override |
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
| 14 | Phase 2.1 assumes `repo` is a git work tree and the file paths it stores match git's root. A dirty or renamed tree may not match `HEAD` line numbers. | 1.5 review | resolved by R2 — dirty files get history but never touch the cache; one warning per run (decision D-r) |
| 15 | `show::render` recurses with plain function calls, so a pathologically deep symbol nesting could overflow the stack. Guard the depth, or move rendering to an explicit stack? | H8/13 | open, defer — POC-scale nesting is shallow; revisit if a real repo triggers it. Before 6.1, also replace the load-everything `render` (all `symbols`, `symbol_commits`, `symbol_tickets` rows per call) with a subtree query (audit 2, finding 10) |
| 16 | The scratch-git test helpers are now duplicated across the `history`, `indexer`, `show` and `benchmark` test modules (four copies of `git`/`git_ok`/`init_repo`/`commit`). Extract a shared `#[cfg(test)]` support module. | 2.2 review | resolved by R5 — `src/test_support.rs`, shared with `tests/benchmark.rs` via `#[path]` (decision D-s) |
| 17 | The history cache makes *repeat* runs fast (1.2 s), but the first cold run over a huge real repo is still minutes (per-symbol `git log -L`). Add bounded parallelism (results through one writer) if cold-run time starts to hurt, or rely on the project's central-build model. | 2.3 | open, defer — the plan offered parallelism *or* the cache; the cache was chosen (daily iteration is the stated goal). Revisit when a real repo is indexed end to end (R8). **R8 measured it:** cold 14.97 s for 960 symbols on `argus` (release), which does not hurt, so parallelism stays deferred — but note the cold run sits at 44 % CPU, waiting on sequential `git log -L` subprocesses, so bounded parallelism (not faster Rust) is the only lever that would move it if a larger repo does start to hurt. The proposed cheaper lever — replacing a top-level type's `-L` with `git log -- <file>` (audit 2, finding 8) — is **withdrawn**: measured over 30 types it matches in only 9 cases and drops ~47 % of the commits (history simplification hides TREESAME-through-merge commits that `-L` reports), so it would lose history rather than save time |
| 18 | Phase 2.3 measured a synthetic repo by product-owner decision; no real GRLD repo has been indexed end to end (multi-module `grld-core` would also hit the test-pruning gap of open #13). | 2.3 | **resolved** (R8) — `argus` indexed end to end: 217 files, 960 symbols, cold 14.97 s / warm 1.92 s (release), 92.5 % ticket coverage. Single-module, so #13 is still untested by a real repo |
| 19 | A `--path` run rebuilds `index.db` wholesale, so it leaves an index holding only that prefix, and a missing prefix empties it (`show` then fails repo-wide). Should `--path` merge into the existing index, refuse to replace it, or is narrowing intended and merely undocumented? | R8 | open — found by the R8 run; decide with R9/J4 (the staged pipeline owns the build until commit), or document the flag as destructive |
| 20 | The 3.1 Jira fixtures are synthetic (no token available to the agent). Replace them with scrubbed real `renderedFields` captures (story, bug, sub-task, epic child, empty description) and confirm the auth mode and parent/epic field shape (J3, audit §3.1). | 3.1 | open — **product owner** |
