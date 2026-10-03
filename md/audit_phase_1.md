# Annatar — Phase 1 Code Audit

External review of the Phase 1 codebase (skeleton + symbols). Findings are
coding smells and robustness issues, not scope changes. Verified against
commit `9e61f74` on 2026-10-03: `cargo test` green (41 tests),
`cargo clippy --all-targets -- -D warnings` clean.

No memory-safety issues and no secret leakage found. The list is ordered by
priority: correctness first, then performance, design, doc drift and tests.

## Correctness / robustness

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 1 | `open_index` is documented as "read-only" but opens read-write | `src/store.rs:80`, `src/store.rs:92` | Misleading contract; only the `exists()` guard prevents writes/creation. Make the comment honest or open read-only. |
| 2 | `content_hash` slices source by byte range without bounds checking | `src/indexer.rs:150` | Any offset mismatch panics the whole run. An indexer must never panic on a source file; clamp or use `get(range)` and return an error. |
| 3 | `parent_id ... REFERENCES symbols(id)` is declared but never enforced | `src/schema.rs:21` | Nothing sets `PRAGMA foreign_keys = ON` (confirmed: no PRAGMA anywhere). The constraint is decorative; readers may assume integrity that isn't checked. Enable it or drop it. |
| 4 | JSON/text decode errors are swallowed | `src/show.rs:130-132`, `src/symbols.rs:616-620` | `unwrap_or_default()` turns corrupt annotations into "none" and a text failure into an empty string, with no signal. Propagate with context or at least `tracing::warn`. |
| 5 | `parse_file` conflates parse errors with empty files | `src/symbols.rs:131-134`, `169-176` | Both return `Ok(vec![])`, so the indexer cannot tell a broken file from `package-info.java` (open question #11). Root cause of the `skipped` ambiguity; resolve before 2.3 reports run quality. |

## Performance

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 6 | A new `Parser` and grammar load per file | `src/symbols.rs:121-124` | `Parser::new` + `set_language` runs for every file; measurable on a large repo, and 2.3 is explicitly about run time. Reuse one parser across files. |
| 7 | One awaited `execute` per symbol with no transaction | `src/indexer.rs:113-133` | Thousands of round-trips into the temp DB. Wrap each build (or file) in a transaction; the first lever for 2.3. |
| 8 | `show` creates `data_dir` and `cache.db` as a side effect | `src/main.rs:75-79`, `src/store.rs:44-54` | A read-only command should not create the cache database or data directory just to read the index. |

## Design / duplication

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 9 | Duplicated `Symbol` construction | `src/symbols.rs:172-186`, `src/symbols.rs:241-255` | The type and member branches repeat 15+ fields. Extract one constructor/factory keyed on the node. |
| 10 | Annotation nodes walked repeatedly per symbol | `src/symbols.rs:280-278`, `314-343`, `390-424` | `annotations_of` is called, then `signature_of` walks the same nodes again, then `role_of` re-parses names. Compute the nodes once and thread them through. |
| 11 | Unused public API | `src/store.rs:68`, `src/store.rs:76` | `Store::data_dir` and `Store::cache_path` have no call sites. Drop them or use them. |
| 12 | Dead match arms | `src/symbols.rs:307`, `src/symbols.rs:583` | `body_node`'s `"record_body"` never matches (records use `class_body` per grammar 0.23); `is_type_container`'s `"program"` is unreachable from `type_body`. Harmless but misleading. |

## Doc drift

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 13 | Decision 41 says `show::render` "recurses with an explicit stack" | `md/state.md:158` vs `src/show.rs:87`, `125` | It is plain function recursion, not an explicit stack, and can overflow on pathologically deep nesting. Fix the code to use a stack or fix the doc. |

## Test smells

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 14 | `minimal_without` removes config lines via `str::replace` | `src/config.rs:162-168` | If `MINIMAL` is edited and a pattern stops matching, the test passes vacuously. Assert the removal happened. |
| 15 | Test helper builds SQL with `format!` | `src/store.rs:165`, `170` | Fine in a test, but it encodes a habit that is dangerous if copied. Parameterize instead. |

## Minor

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 16 | `relative_path` hand-rolls path joining | `src/indexer.rs:156-161` | `to_string_lossy().replace('\\', "/")` is clearer and less code. |
| 17 | `is_skipped_dir` prunes any directory named `build`/`generated` anywhere | `src/walk.rs:91-105` | Also prunes a legitimate `com.acme.build` package. Documented as decision 25, but over-broad for real repos. |
| 18 | `Config::load` uses `std::mem::take` on `repo`/`data_dir` | `src/config.rs:91-92` | A `clone` or consuming builder reads more clearly than an in-place `take`. |
