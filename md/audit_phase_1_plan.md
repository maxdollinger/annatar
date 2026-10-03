# Annatar — Phase 1 Audit Remediation Plan

Plan to address [`audit_phase_1.md`](./audit_phase_1.md) (18 findings against
commit `9e61f74`). This is **hardening, not scope change**: no new plan phases,
no new features. It sits between the end of Phase 1 and the start of Phase 2.

- **Source audit:** [`audit_phase_1.md`](./audit_phase_1.md)
- **Authoritative scope:** [`plan.md`](./plan.md) — unchanged by this work.
- **Status/living doc:** [`state.md`](./state.md) — updated after each item.

## How to read this

Each work item (`H1`–`H9`) is one small PR, following the project workflow in
`AGENTS.md`: implement exactly one item, ship tests, run `cargo fmt`,
`cargo clippy -- -D warnings`, `cargo test`, update `state.md`, commit.

Order below is the recommended execution order (correctness first, then the
2.3 run-time levers, then design/hygiene). The only hard dependency is H1 →
H6 (both edit `symbols.rs`; H1 changes the parse API first).

## 1. Scope and principles

- Fix all 18 findings; none are out of scope.
- Prefer the smallest change that makes the contract honest. Where the audit
  offers "fix code or fix doc", choose per the decision table in §5.
- Keep public API changes contained; when a signature must change (H1), change
  it once and update all call sites and tests in the same PR.
- Do not refactor unrelated code. H6 is the one deliberate refactor; it is
  behaviour-preserving and its proof is the existing `symbols` test suite.
- Anything that cannot be automated as a unit test (parse-skip statistics,
  read-only side effects) gets a temp-directory test, matching existing style.

## 2. Work items at a glance

| Item | Findings | Theme | Touches | Priority |
| --- | --- | --- | --- | --- |
| H1 | 5, 6 | Parse contract + parser reuse (unblocks 2.3) | `symbols.rs`, `indexer.rs` | P0 |
| H2 | 2 | Bounds-safe `content_hash` | `indexer.rs` | P0 |
| H3 | 3 | Enforce declared foreign key | `store.rs`, `schema.rs` | P0 |
| H4 | 1, 8, 11 | Side-effect-free read path, honest contract, drop dead API | `store.rs`, `main.rs`, `show.rs` | P1 |
| H5 | 4 | Surface decode failures | `show.rs`, `symbols.rs` | P1 |
| H6 | 9, 10, 12 | Parser de-duplication, single annotation walk, dead arms | `symbols.rs` | P1 |
| H7 | 7 | Batch symbol writes in a transaction | `indexer.rs` (optionally `store.rs`) | P1 (2.3 lever) |
| H9 | 17 | Narrow walker pruning of `build`/`generated` | `walk.rs` | P2 |
| H8 | 13, 14, 15, 16, 18 | Doc drift, test smells, minor cleanups | `state.md`, `config.rs`, `store.rs`, `indexer.rs` | P2 |

P0 = correctness, do first. P1 = robustness/performance/design. P2 = hygiene.

---

## 3. Work items

### H1 — Distinguish parse failure from an empty file; reuse one parser

**Findings:** 5 (`parse_file` conflates parse errors with empty files),
6 (new `Parser` + grammar load per file).
**Files:** `src/symbols.rs`, `src/indexer.rs`, `tests/index_show.rs`.
**Resolves:** open question #11; updates decision 42.

**Why now:** This is the explicit prerequisite the audit calls out "before 2.3
reports run quality". Doing it first also means the `symbols` API changes once,
before H6 refactors the same file.

**Change:**

1. Introduce a reusable parser handle instead of constructing one per file:
   ```rust
   pub struct JavaParser { parser: Parser }
   impl JavaParser {
       pub fn new() -> Result<Self>;
       pub fn parse(&mut self, path: &Path, source: &str) -> Result<ParsedFile>;
   }
   pub struct ParsedFile { pub symbols: Vec<Symbol>, pub parse_error: bool }
   ```
   `build_index` creates one `JavaParser` for the whole run and passes it down.
   A struct (not a free `&mut Parser`) keeps grammar-load fallibility in `new`
   and gives 2.3 a per-thread parser when parallelism lands.
2. `ParsedFile.parse_error` is `true` for `root.has_error()` and for
   `parser.parse` returning `None`; `symbols` is empty in both cases. A clean
   file with zero symbols (`package-info.java`) is `parse_error: false`.
3. Replace `skipped` with explicit buckets, but keep `files` meaning "files
   that produced at least one symbol" (its current meaning, `src/indexer.rs:28`)
   so this stays a correctness fix and not a reporting change:
   ```rust
   pub struct IndexStats {
       pub files: usize,        // produced >= 1 symbol (unchanged meaning)
       pub symbols: usize,      // rows inserted
       pub empty: usize,        // parsed cleanly, zero symbols
       pub parse_errors: usize, // tree-sitter errors / no tree
       pub unreadable: usize,   // fs::read_to_string failed
   }
   ```
   The three failure buckets are disjoint and, together with `files`, sum to
   the walked file count. Update the `main` summary line and the log fields.
4. `parse_error` stays a `tracing::warn!` with the path; `empty` stays debug.

**Tests:** update the `symbols` unit tests to the new API. Add an indexer test
where a good file, a `package-info.java` and a broken file yield
`files=1, empty=1, parse_errors=1, unreadable=0`. Update the integration test
`tests/index_show.rs:20`, which asserts `stats.skipped == 0`, to assert the
three new buckets are zero. Keep the existing broken-file and duplicate-fqn
tests green.

**Acceptance:** a broken file and a symbol-less file are distinguishable in
stats and logs; parser is constructed once per run.

---

### H2 — Bounds-safe `content_hash`

**Finding:** 2 (byte-range slice can panic).
**File:** `src/indexer.rs` (`content_hash`, ~line 150).

**Why now:** An indexer must never panic on a source file. This is the only
unchecked slice in the run path.

**Change:**

- Replace `&source.as_bytes()[start..end]` with `source.as_bytes().get(start..end)`.
- On `None`, return `Err(anyhow!(...))` naming the symbol fqn and span; thread
  the `Result` through `write_symbol` / `build_index` (both already return
  `Result`).
- Do not silently clamp: a mismatch is a parser bug, so fail the run with
  context rather than hash the wrong bytes.

**Tests:** add a unit test in `indexer`'s existing `mod tests` (which already
does `use super::*`, so no visibility change is needed) that builds a `Symbol`
with an out-of-range `end_byte` and asserts `content_hash` returns an error.
Existing hashing tests stay green.

**Acceptance:** out-of-range spans yield a contextual error, never a panic.

---

### H3 — Make the declared foreign key real

**Finding:** 3 (`parent_id ... REFERENCES symbols(id)` never enforced; no
`PRAGMA foreign_keys = ON` anywhere).
**Files:** `src/schema.rs`, `src/store.rs`.

**Why now:** The schema advertises integrity it does not check. Pre-order
insertion (parents first) already satisfies the constraint, so enabling it is
safe.

**Change (recommended: enable, not drop):**

- Run `PRAGMA foreign_keys = ON` on every connection that writes or reads the
  index: `Store::begin_index` (build connection) and the read connection
  (which H4 replaces with `IndexReader`; carry the pragma across). libSQL/SQLite
  foreign keys are per-connection, so set it at open, not in DDL.
- The cache side does not need it yet; add it when a cache table gains a
  reference.

**Tradeoff:** `parent_id` is always taken from the in-memory map of rows just
inserted (`src/indexer.rs:108`), so in normal operation the constraint can never
trip — enabling it is belt-and-braces that turns a future linking bug into a
loud failure instead of an orphan. The one-line alternative is to drop the
`REFERENCES` clause and document that integrity is by construction. Prefer
enabling, because a schema that advertises a constraint should enforce it; if
the extra per-insert check is unwanted, dropping is an acceptable close.

**Tests:** because the insert uses `INSERT OR IGNORE` (decision 38), an FK
violation is *ignored*, not an error. Add a store/indexer test that inserts a
row with a dangling `parent_id`, then asserts the row count did **not** grow
(OR IGNORE + FK). The existing parent-link test in `indexer` continues to pass.

**Acceptance:** the constraint is enforced (dangling parents are rejected or
ignored, never stored); no order-dependent insert breaks.

---

### H4 — Read path has no side effects; honest open; drop dead API

**Findings:** 1 (`open_index` documented read-only but opens read-write),
8 (`show` creates `data_dir` + `cache.db` as a side effect), 11 (unused
`Store::data_dir`, `Store::cache_path`).
**Files:** `src/store.rs`, `src/main.rs`, `src/show.rs`, `tests/index_show.rs`.

**Why now:** This is the same read path (`show` today, MCP `get_symbol` in
6.1), so fix it once.

**Change (recommended design):**

- Add a side-effect-free reader that opens only `index.db`, read-only:
  ```rust
  pub struct IndexReader { conn: Connection, _db: Database }
  impl IndexReader {
      pub fn connection(&self) -> &Connection;
      pub async fn open(data_dir: &Path) -> Result<Self>;
  }
  ```
  Built with `Builder::new_local(path).flags(OpenFlags::SQLITE_OPEN_READ_ONLY)`
  (`libsql::OpenFlags`, confirmed available in 0.9.30). Keep the `exists()`
  guard so a missing index still yields the friendly "run `annatar index`"
  error. Anchoring `_db` mirrors `Store`'s `_cache_db` (decision 18); it is
  defensive — the current bare-`Connection` return already works — but keeps
  the handle owned alongside its connection.
- **Remove `Store::open_index` entirely** and use `IndexReader::open`. Leaving
  both would give two ways to open the same file with different flags. Call
  sites to move: `src/main.rs:77`, `src/show.rs:168,196`,
  `src/indexer.rs:263,366,385,424`, `src/store.rs:238,247` (store test),
  `tests/index_show.rs:23`.
- `main`'s `Show` arm uses `IndexReader::open(&config.data_dir)`, never
  `Store::open` — so `show` creates neither the data directory nor `cache.db`.
- `show::render` keeps taking `&Connection`; unit tests build with `Store` then
  read via `IndexReader::open`.
- Remove `Store::data_dir` and `Store::cache_path` (no call sites). Keep
  `Store::index_path` (used by store tests and by `begin_index`).

**Tests:** a temp-dir test that `IndexReader::open` on a directory with no
committed index errors and leaves no `index.db`/`cache.db`; a test that after
an index build, a read works via the reader and the read path does not create
`cache.db` (delete `cache.db` after building, then read, then assert it is
still absent). Update `tests/index_show.rs` to read through `IndexReader::open`.

**Acceptance:** a read command creates nothing; no read-write `open_index`
remains; the read path is genuinely read-only.

---

### H5 — Surface decode failures instead of swallowing them

**Finding:** 4 (`show.rs` `parse_annotations` uses `unwrap_or_default`; 
`symbols.rs` `text()` uses `unwrap_or_default`).
**Files:** `src/show.rs`, `src/symbols.rs`.

**Why now:** Corrupt annotations currently render as "none" and a bad text
extraction as "", with no signal. Silent corruption is worse than a loud
failure.

**Change:**

- `show::parse_annotations` → `Result<Vec<String>>`; on JSON error, propagate
  with the symbol's fqn attached (the caller `write_symbol` already has the
  `StoredSymbol`, so pass its fqn in). `render` then returns `Err`, so
  corruption is visible rather than masked.
- `symbols::text()` stays infallible but logs `tracing::warn!` with node kind
  and byte range on `utf8_text` failure, then returns empty. Making it
  fallible would ripple through every parser helper for an impossible case
  (tree-sitter spans never split a UTF-8 char); the warn is the proportionate
  signal the audit allows.

**Tests:** a `show` test that writes a `symbols` row with invalid
`annotations` JSON (via `IndexBuild`) and asserts `render` returns an error
naming the fqn. Existing `show`/`symbols` tests stay green.

**Acceptance:** corrupt JSON fails loudly; text-extraction failure is logged.

---

### H6 — De-duplicate `Symbol` construction and walk annotations once

**Findings:** 9 (duplicated 15+ field construction), 10 (annotation nodes
walked by `annotations_of`, again by `signature_of`, and re-parsed by
`role_of`), 12 (dead match arms).
**File:** `src/symbols.rs`.

**Why now:** After H1/H5 have settled the parser API, this is the single
internal refactor. Pure behaviour, no public API change; the existing 17
`symbols` tests are the regression net.

**Change:**

- Compute `let annotation_nodes = annotation_nodes(declaration)` once per
  declaration and derive the annotation strings, the signature and the role
  from that one list. `signature_of` takes the node list instead of re-walking;
  `role_of` takes the already-extracted names.
- Extract one constructor/factory, e.g.
  `fn build_symbol(kind, name, fqn, parent, declaration, source, annotations)`
  used by both the type branch and `member_symbol`, so the field list lives
  once.
- Delete the dead arms: `body_node`'s `"record_body"` and
  `is_type_container`'s `"program"`. Verified before removal:
  `tree-sitter-java` 0.23.5 `node-types.json` has no `record_body` and
  `record_declaration` declares `field('body', $.class_body)` (grammar.js:887),
  so `body_node`'s `child_by_field_name("body")` already resolves records;
  `"program"` is never a child inspected by `is_type_container`. If `"program"`
  is nevertheless needed by `collect_symbols`'s root entry, document that the
  recursion is entered from the root directly instead of via
  `is_type_container`.

**Tests:** the existing `parses_each_type_kind`,
`signature_is_the_declaration_without_its_body`,
`annotations_are_captured_as_written_including_multiline` and role tests prove
behaviour is unchanged. Add one record *with a body* (compact constructor or
members) and assert its signature stops at the header, locking the
`record_body` removal against a future grammar bump.

**Acceptance:** one construction site; no repeated annotation traversal; no
dead match arms; all symbols tests green.

---

### H7 — Batch symbol writes in a transaction

**Finding:** 7 (one awaited `execute` per symbol, no transaction).
**File:** `src/indexer.rs` (optionally expose a transaction guard on
`IndexBuild`).

**Why now:** The audit calls this "the first lever for 2.3". Do it before 2.3
starts so the 2.3 measurement is against a batched baseline, not the
un-batched worst case.

**Change:**

- In `build_index`, begin a transaction on the build connection after
  `begin_index` (`Connection::transaction()` exists in libsql 0.9.30 and
  `Transaction` derefs to `Connection`). The begin is synchronous — do not
  `.await` it; only `execute`/`commit` are async.
- Write all symbols through the transaction; commit it before
  `build.commit()`. An error drops the transaction (rollback) before the temp
  file is discarded, so nothing partial is ever renamed.
- Keep `last_insert_rowid` usage — it is transaction-local and correct.
- Optionally add `IndexBuild::transaction()` so the transaction lifetime is
  explicit and the commit ordering is enforced by the type, rather than a
  comment. If added, H4's reader is unaffected.

**Tests:** existing indexer tests prove the committed path. Do **not** attempt
to force a mid-index parser/constraint failure — the parser cannot fail a
single symbol and `INSERT OR IGNORE` masks constraint errors, so such a test
would be contrived. Instead add a store-level rollback test: begin a
transaction on a build connection, insert a row, drop the transaction without
committing, and assert the row is not visible.

**Acceptance:** one commit per build; dropping the transaction before commit
leaves nothing in the temp index.

---

### H9 — Narrow walker pruning of `build`/`generated`

**Finding:** 17 (`is_skipped_dir` prunes any directory named `build`/
`generated` anywhere, e.g. a legitimate `com.acme.build` package).
**File:** `src/walk.rs`.
**Updates:** decision 25.

**Why now:** It is a real false negative on real repos and cheap to fix; H1/H7
make the file set more important to 2.3.

**Change:** prune a directory whose name is in `SKIP_DIRS` only when it is
**not under a source root**. Concretely, skip a candidate only if none of its
ancestor components is named `java`, so `src/main/java/com/acme/build/`
survives while a module-root `build/` is pruned. This keeps `java_files`'s
signature (`src/walk.rs:29`) unchanged and touches one function.

**Scope note:** do not add a `[index] skip_dirs` config escape hatch here. That
changes `Config`, `java_files`' signature, `build_index` and the committed
`annatar.toml`, ballooning a P2 cleanup into a cross-module change. If a repo
needs a different skip set, raise it as a separate config item after 2.x.

**Tests:** a walker test that `src/main/java/com/acme/build/Keep.java` is
returned while `build/Out.java` is pruned. Update the existing
`skips_build_output` fixture only if its layout relies on the old behaviour.

**Acceptance:** a package directory named `build` under a source root is
indexed; real build output is still excluded.

---

### H8 — Doc drift, test smells and minor cleanups

**Findings:** 13 (decision 41 claims an explicit stack), 14 (`minimal_without`
can pass vacuously), 15 (test helper builds SQL with `format!`), 16
(`relative_path` hand-rolls path joining), 18 (`Config::load` uses
`mem::take`).
**Files:** `md/state.md`, `src/config.rs`, `src/store.rs`, `src/indexer.rs`.

**Why last:** Hygiene only; no behaviour dependencies.

**Change:**

- **13:** fix decision 41 in `state.md` to describe the actual implementation
  ("loads the whole table, builds a child map, recurses") and add the
  deep-nesting overflow risk as a new open question. Do not rewrite `show`
  to an explicit stack in this PR; the doc fix is what the audit permits and
  reworking render for a pathological input is not warranted at POC scale.
- **14:** `minimal_without` asserts each pattern was present and removed
  (`assert!(text.contains(line))` before replace, and/or assert the result no
  longer contains it), so an edited `MINIMAL` cannot silently make a test
  vacuous.
- **15:** rewrite the `store` test helper's two `format!` inserts using
  `params!` (the `CREATE TABLE` still needs an identifier, but the value goes
  through params).
- **16:** replace `relative_path`'s component loop with
  `path.to_string_lossy().replace('\\', "/")`.
- **18:** in `Config::load`, stop `std::mem::take`-ing `repo`/`data_dir`;
  resolve via a clone or into the resolved value directly.

**Tests:** existing config/store/indexer tests cover 14–18; the walker/indexer
path tests prove `relative_path` still yields `/`-separated repo-relative
paths. No new tests needed beyond the assertions in 14.

**Acceptance:** docs match code; tests cannot pass vacuously; minor cleanups
in place.

---

## 4. Traceability

| Finding | Work item | Finding | Work item |
| --- | --- | --- | --- |
| 1 | H4 | 10 | H6 |
| 2 | H2 | 11 | H4 |
| 3 | H3 | 12 | H6 |
| 4 | H5 | 13 | H8 |
| 5 | H1 | 14 | H8 |
| 6 | H1 | 15 | H8 |
| 7 | H7 | 16 | H8 |
| 8 | H4 | 17 | H9 |
| 9 | H6 | 18 | H8 |

## 5. Decisions to record in `state.md`

Add these to *Decisions and tradeoffs* as they are implemented, and update the
existing entries noted:

| # | Decision | Alternatives | Rationale |
| --- | --- | --- | --- |
| D-a (H1) | New `JavaParser`/`ParsedFile` API; `IndexStats`' `skipped` is replaced by `empty`/`parse_errors`/`unreadable`, `files` keeps its meaning | Keep `Ok(vec![])` for both | Distinguishes a broken file from a symbol-less one; resolves open #11; parser reused once per run |
| D-b (H2) | Out-of-range spans are a hard error with fqn/span context | Clamp the range | A mismatch is a parser bug; clamping would hash the wrong bytes silently |
| D-c (H3) | Enable `PRAGMA foreign_keys = ON` per connection | Drop the `REFERENCES` clause | Keeps the schema's advertised integrity real; pre-order insert already satisfies it |
| D-d (H4) | `IndexReader::open(data_dir)` returns a read-only reader; `Store::open_index` is removed; `show` no longer opens `Store` | Keep opening read-write and fix only the doc; or keep both open paths | The read path must not create `cache.db`/the data dir; a single read entry point becomes the contract 6.1 reuses |
| D-e (H5) | Corrupt annotations error; `text()` logs and returns empty | Make every text helper fallible | Proportionate: JSON corruption is real and cheap to propagate; UTF-8 failure is impossible for valid tree-sitter spans |
| D-f (H6) | One `build_symbol` factory; annotations computed once per declaration | Keep parallel branches | Removes duplication and the repeated node walks without changing output |
| D-g (H7) | Wrap the whole build in one transaction | Per-file transactions | Fewest round trips; the audit's first 2.3 lever; rollback keeps the old index intact |
| D-h (H9) | Prune skip-dirs only outside source roots (no ancestor named `java`) | Prune by name anywhere (current); add `index.skip_dirs` config | Fixes the false negative with a one-function change; a config escape hatch is deferred to avoid cross-module scope creep |
| D-i (H8) | Fix decision 41's wording; track deep-nesting overflow as an open question | Rewrite `show` iteratively now | Doc/behaviour drift is the actual finding; iterative rendering is unwarranted at POC scale |
| D-j (H1, H9) | Update decision 25 (walker pruning) and decision 42 (skip semantics) | — | Both describe behaviour changed by H9 and H1 respectively |

**Open questions to resolve/add:** #11 resolved by H1; #14 (Phase 2.1 git
assumptions) unaffected; add "should `show` guard against pathological nesting
depth, or move to an explicit stack?" (from H8/13).

## 6. Explicitly deferred

- **Overflow-safe `show` rendering (13):** only the doc is corrected; the
  overflow risk is tracked as an open question, not reworked now.
- **Multi-module test roots (open #13):** unchanged, remains a Later item.
- **Cache-side foreign keys:** not needed until a cache table has a reference.
- **`[index] skip_dirs` config (H9):** deliberately not added; the default is
  narrowed instead. Revisit only if a real repo needs a custom skip set.

## 7. Definition of done

For each item and at the end of the series:

- `cargo fmt` clean, `cargo clippy --all-targets -- -D warnings` clean,
  `cargo test` green (baseline 41 tests; new tests added per item).
- Every new behaviour has a test that would fail without the change.
- No public API left unused; no doc claim that the code does not honour.
- `state.md` updated: current state row, `Done`, `Next`, step log, decisions
  (per §5), open questions (#11 resolved, new nesting question added).
- Before H1 starts, point `state.md` `Next` (currently Phase 2.1) at this
  remediation plan so the audit work, not Phase 2, is the active queue.
- No `plan.md` change expected; confirm during the H1 drift check that "log and
  skip parse errors" still describes the refined behaviour.
- One commit per item, Conventional Commits matching history, e.g.
  `fix(symbols): separate parse failures from empty files`,
  `fix(indexer): bounds-check content hash`, `perf(index): batch symbol writes`.

## 8. Suggested execution order

1. **H1** parse contract + parser reuse — unblocks 2.3, settles the parse API.
2. **H2** bounds-safe `content_hash` — removes the only run-path panic.
3. **H3** enforce the foreign key.
4. **H4** side-effect-free read path + drop dead `Store` API.
5. **H5** surface decode failures.
6. **H6** parser de-duplication + single annotation walk.
7. **H7** transaction-batched writes — the 2.3 baseline.
8. **H9** narrow walker pruning.
9. **H8** doc drift, test smells, minor cleanups.

H7 may alternatively be the opening work of Phase 2.3 if the team prefers to
hold all run-time work together; either is fine, but it must land before the
2.3 measurement.
