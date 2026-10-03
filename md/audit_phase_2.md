# Annatar — Phase 2 Code Audit

External review of the codebase at the end of Phase 2 (history and ticket
keys), before Phase 3 (Jira) starts. Reviewed against commit `4ff0c8b` plus the
uncommitted edit to `md/state.md`, on 2026-10-03.

Scope: code smells and robustness issues in the Phase 2 code (`history`,
`indexer`, `schema`, `show`, `tests/benchmark.rs`), doc drift between
`plan.md` / `state.md` / `project.md` and the code, and what is missing
before Phase 3. Like the Phase 1 audit, these are findings, not scope
changes.

**Not re-verified:** the audit sandbox blocks `~/.cargo`, so `cargo fmt`,
`cargo clippy` and `cargo test` could not be run. The "69 lib + 1 integration
tests, green" in `state.md` is taken on trust. Re-run all three before acting
on this list. `git` 2.55.0 was available and was used to check finding 6.

No memory-safety issues and no secret leakage found. Production code has one
`expect` (finding 12) and no `unwrap`. The order is: correctness first, then
performance, design, doc drift, tests, and the Phase 3 gaps.

## Correctness / robustness

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 1 | An untracked file runs `git log -L` once per symbol, and each call is guaranteed to fail | `src/indexer.rs:137-151`, `165-181` | When `file_last_commit` returns `None` (no commit touched the file), the code still calls `history_for_span` for every symbol. Each call fails and logs a warning, so a new 40-method class costs 41 git spawns and 41 warnings. `None` already means "no history"; skip history for the whole file and warn once. This also contradicts D-l, which promises "one warning naming the file". |
| 2 | Cache failures abort the run, but git failures only warn | `src/indexer.rs:182`, `197-199`; `src/history.rs:157-161` | `cache.get(...)?` and `cache.put(...)?` propagate errors, so one corrupt `commits` JSON row or a locked `cache.db` sinks the whole index. A cache is disposable by definition (ground rules). Treat a decode or read failure as a miss with a warning, and a failed write as a warning. This is the same degrade-don't-abort policy D-l chose for git. |
| 3 | A dirty working tree mis-attributes history, and the cache now stores the wrong result | `src/indexer.rs:154`, `166`, `190`; open question #14 | Spans and `content_hash` come from the working copy, but `-L` resolves the span against `HEAD` and the cache key uses `HEAD`'s file sha. An uncommitted edit that shifts lines produces history for the wrong lines. That history is then cached under (dirty hash, HEAD sha) and reused on the next dirty run. It heals on commit, but the developer loop (`--path` iteration on a dirty checkout) is exactly where this happens. Open question #14 is still open with no decision. At least detect dirty files (`git status --porcelain`, once per run) and either warn or skip caching for them. |
| 4 | `history_misses` is documented as "cache miss or no cache", but the no-cache path counts nothing | `src/indexer.rs:41-42`, `165-181` | Symbols in an untracked or errored file land in neither `history_hits` nor `history_misses`, so `hits + misses != symbols` on a git repo, and the benchmark table and the `index` summary under-report git work. Either count them or fix the doc and add a bucket (e.g. `history_skipped`). |
| 5 | Ticket keys that appear only in merge commits are never seen | `src/history.rs:221-229` | `git log -L` does not list merge commits by default. In a Jira/Bitbucket flow, the key often lives only in the merge message or branch name (`Merge pull request #12 from feature/GRLD-123-...`), and the individual commits carry none. Phase 4's "why" depends on this coverage. Measure the ticket hit rate on a real repo (`project.md` lists this as an open risk) before Phase 4 quality work. If it is low, add a first-parent/merge-message pass. |

## Performance

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 6 | `git log -L` prints full diff hunks that are parsed and thrown away | `src/history.rs:222-229`, `243-265` | Verified on git 2.55: `git log -L <range> -s --format=…` is accepted and returns only the records (41 → 4 lines on a small range in this repo). Large classes with long history produce megabytes of patch text per call on the cold run. Add `-s` (check the minimum git version), and re-measure the cold run. |
| 7 | Every cache miss is its own autocommitted write to `cache.db` | `src/indexer.rs:197-199`, `src/history.rs:165-186` | A cold run does one `fsync` per symbol on top of the `git` spawn. Batch the puts in a cache transaction per file (still separate from the index transaction, as D-o requires). Cheap, and it helps the one-time cold run that open #17 worries about. |
| 8 | A type's span covers its whole body, so the type's `-L` is the most expensive call and returns almost every commit in the file | `src/indexer.rs:166`, `190` | Pure cost here, but it also means class-level `symbol_tickets` are effectively file-level. Note it for 4.4 (cap the input) and for open #17. File history (`git log -- <file>`) is a much cheaper approximation for top-level types. |
| 9 | `--path` filters after a full walk | `src/walk.rs:41-79` | The walk still visits the whole repo, then drops files outside the prefix. On a large multi-module repo, the iteration loop the flag exists for still pays the full walk. Start the walk at `repo.join(prefix)` (the walk itself is unchanged). Low priority. |
| 10 | `show` loads every symbol, commit and ticket row to render one fqn | `src/show.rs:65-87`, `105-146` | Decision 41 accepted this for `symbols`. `symbol_commits` is now an order of magnitude larger (one row per symbol × commit), and `get_symbol` in 6.1 will run this per request. Fine for the CLI now. Revisit before 6.1 with a subtree query. |

## Design / duplication

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 11 | `build_index` is ~170 lines with five levels of nesting and two copies of the `history_for_span` + warn block | `src/indexer.rs:68-240` (copies at `166-180` and `190-216`) | Phases 3–5 will add stages to this function. Extract a `symbol_history(...) -> Option<Vec<Commit>>` helper that does cache get → git → cache put and returns the hit/miss outcome. That also fixes findings 1, 2 and 4 in one place. Do it before 3.2 so the Jira stage is not added to an already-tangled loop. |
| 12 | `ticket_span` returns `(String, String, String)` and uses `expect` | `src/indexer.rs:340-368` | Three positional strings are easy to swap (first/last date). A `TicketSpan { key, first_date, last_date }` kept in a `Vec`, with an index map, removes both the tuple and the production `expect`. |
| 13 | `IndexStats` is built field by field with zeros | `src/indexer.rs:89-99`, `657-668` | Every new counter (3.x adds tickets fetched/unavailable, 4.x adds LLM hits) touches both literals. `#[derive(Default)]`. |
| 14 | The ticket regex is compiled twice | `src/config.rs:110-114`, `src/main.rs:68-69` | `validate` compiles it and drops it, then `main` compiles it again with a second error path. Keep the compiled `Regex` (or a `ticket_regex()` accessor) on `Config`. |
| 15 | Production `git` calls inherit the user's git config | `src/history.rs:64-110`, `222-229` | Settings like `color.ui=always`, `diff.external` and `log.showSignature` can add text to stdout. The record parser tolerates it, but it is fragile. With `-s` (finding 6) plus `--no-color` and `--no-ext-diff`, the output no longer depends on the machine. |
| 16 | `history_cache` rows are never removed | `src/schema.rs:63-68` | Deleted or renamed symbols keep their rows forever. Harmless at POC scale. Note it with the other cache-growth items for the Later list. |

## Doc drift

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 17 | The uncommitted `state.md` edit corrupts the markdown | `md/state.md:138`, `225`, `352`, `357` | It looks like a formatter pass: `` `parsing annotations for`<fqn>`` `` no longer renders as one code span, `+ tree-sitter-java` became `- tree-sitter-java` (now a list item that reads as "minus"), `(ISO`%aI`)` lost its space, and D-p's alternatives column now holds a draft question ("…or some syntheticone?"). Revert or fix before committing. |
| 18 | Phase 2 is marked complete, but two "Done when" criteria were not met as written | `md/plan.md:78`, `83`; `md/state.md:13-14`, `55`, `380` | 2.2 requires "`show` on a **real** symbol lists its tickets", but it was verified on a scratch repo. 2.3 requires "a full run on **the target repo**", but a synthetic repo was measured (D-p). The product owner's call is recorded, but the plan still says otherwise. Either amend `plan.md` 2.2/2.3, or mark Phase 2 "complete with deviations" in `state.md` and link open #18. |
| 19 | The real-repo recon numbers are incomplete | `md/state.md:20-23` | D-p says recon numbers "are recorded in the Done entry as context". Only ~89 ms/symbol and the 0.02 s structure run are there, with no symbol count. You can't extrapolate the real cold-run time (open #17's whole question) without it. Add symbols × ms ≈ cold time. |
| 20 | D-l's wording does not match the code | `md/state.md:353` | It says a failed history lookup gives "one warning naming the file". The code gives one warning per symbol (finding 1). Fix the code or the wording. |
| 21 | `project.md` disagrees with the ground rules and the plan | `md/project.md:59`, `69-70`, `80` | Storage is "a single SQLite database with sqlite-vec", but the plan uses two libSQL files with libSQL's native vector index. `get_symbol(id)` contradicts "symbols are identified by fqn" (the plan's 6.1 uses `get_symbol(fqn)`). `search_intent(query, kind?, path?)` differs from the plan's `kind?, role?`. Line 59 is a broken export artifact (`&#91;embedded content: …\]`). `project.md` is the target, but it should not contradict a ground rule. |
| 22 | AGENTS.md's clippy command skips tests and the benchmark | `AGENTS.md` (Workflow step 4) | `cargo clippy -- -D warnings` does not lint `#[cfg(test)]` modules or `tests/benchmark.rs`. The Phase 1 audit used `--all-targets`. Make that the documented gate. |
| 23 | The README doesn't say how to run anything or that git is required | `README.md` | History now needs `git` on `PATH` and `repo` to be a work tree. Without it, the run silently degrades (D-l). Two lines of usage (`annatar index`, `annatar show <fqn>`) and the git requirement would do. |

## Test smells

| # | Finding | Location | Why it matters |
| --- | --- | --- | --- |
| 24 | The scratch-git helpers are copied four times | `src/history.rs:272-320`, `src/indexer.rs:753-795`, `src/show.rs:332-367`, `tests/benchmark.rs:42-84` | Open #16 / D-n, still deferred. Phase 3 will add HTTP fixtures and likely another helper set. Extract before that. The `tests/` copy needs a `tests/common/mod.rs` (or a `test-support` feature), because `#[cfg(test)]` items in the lib are not visible to integration tests. |
| 25 | The benchmark asserts on wall time and only loosely on counts | `tests/benchmark.rs:203-217` | `warm_time < cold_time` is a timing assertion, so it can flake. `changed.history_misses < warm.history_hits` would pass with almost any value. The exact expectation is `FILES / CHANGED_DIVISOR * (METHODS + 1)` misses. It also runs in a debug build, so its numbers overstate release run time. Say so next to the recorded numbers. |
| 26 | No test covers the untracked-file path in a git repo, or a corrupt cache row | `src/indexer.rs` tests | These are exactly findings 1, 2 and 4. Add a test with one committed and one untracked file (assert the warnings and counters), and one with a corrupt `history_cache.commits` value. |
| 27 | No test covers a symbol whose Javadoc changed in a separate commit | `src/indexer.rs` tests | The `-L` span starts at the declaration, not the Javadoc, so a doc-only commit (`GRLD-9 document X`) is not attributed to the symbol. Meanwhile `content_hash` does include the Javadoc. Decide whether that is intended (it probably isn't for "why"), then pin it with a test either way. |

## Missing before Phase 3 (Jira)

Phase 3.1 (fetch and parse against fixtures) can start with almost nothing
else in place. Phase 3.2 and Phase 4 depend on decisions that the plan
assumes but nobody has made yet.

| # | Gap | Needed by | Recommendation |
| --- | --- | --- | --- |
| 28 | **Jira auth scheme is undecided** (open #1: Cloud basic email+token vs Server/DC bearer PAT) | 3.1 | Decide before writing the client. `JiraConfig` already has optional `email` + `token`. Support "email present → basic, else bearer", or pick one and record it. |
| 29 | **HTTP stack not chosen**: no `reqwest`, no TLS backend, no HTML-to-text | 3.1 | Decision 13 deliberately avoided `rustls`/`hyper` for libSQL. `reqwest` with `rustls-tls` brings them back, so record the trade-off. For HTML → text, pick a crate (e.g. `html2text`) or a minimal stripper, with a decision row. |
| 30 | **No fixtures yet**: saved `renderedFields` JSON for story, bug, sub-task (with parent), epic child, and empty description | 3.1 | These need one real API response to copy from, so you need Jira access before the step starts. Scrub ticket content before committing the fixtures. |
| 31 | **Pipeline shape**: `build_index` commits `index.db` before returning | 3.2, 4.3 | 3.2 reads the distinct keys in `symbol_tickets`, and 4.3 writes what/why "into `index.db`". Either the build stays open across stages (split `build_index` into structure → history → tickets → summaries → commit, owned by one orchestrator), or later stages write to the committed index, which breaks the temp-file + atomic-rename rule. Decide this, and do finding 11, before 3.2. |
| 32 | **When `index` talks to Jira** | 3.2 | Decision 21 says commands require `[jira]` once wired. Decide whether `index` without `[jira]` or the env secrets is an error or skips the ticket stage with a warning. Keep the offline `--path` loop working (a `--no-jira`/`--offline` flag or "skip if unconfigured"). |
| 33 | **"Unavailable" semantics** | 3.2 | The plan caches 403/404 as unavailable and never refreshes. Also decide: 401 (bad credentials) must **fail the run**, never be cached as unavailable. Decide whether unavailable rows ever expire (a permission fix would otherwise never be picked up; dropping the table is the manual escape). Handle 429 with `Retry-After`, add a request timeout and a concurrency cap. |
| 34 | **Test seam for "a second run makes no Jira calls"** | 3.2 | Unit tests must not hit the network. Put fetching behind a small `TicketSource` trait (a fake counts calls), or use a local mock server. The audit sandbox refuses local port binding, so the trait is the more portable choice. |
| 35 | **Where ticket content lives for readers** | 3.2, 4.x, 6.1 | `show` and the MCP server read only `index.db` (D-d keeps the read path side-effect free, and `project.md` wants the index to be a portable artifact). Copy the needed ticket fields into an index table during the build, rather than having readers open `cache.db`. Add both tables to `schema.rs` in 3.2. |
| 36 | **Parent/epic field mapping** | 3.1 | Cloud exposes `fields.parent` for all hierarchy levels. Server/DC and older Cloud projects use the "Epic Link" custom field (`customfield_100xx`, which varies per instance). Decide whether the field id is config or out of scope for the POC. |
| 37 | **No real repo indexed end to end** (open #18) | 3.2, 4.5 | Phase 3 needs real ticket keys and Phase 4's quality review needs real tickets. Run `annatar index` on the target GRLD repo now. That also produces the missing cold-run number (finding 19), the ticket-coverage number (finding 5), and forces a decision on multi-module test pruning (open #13) if the target is `grld-core`. |

## Recommended order

1. Fix the `state.md` corruption (17) and decide how to record the Phase 2
   deviations (18, 19).
2. A small hygiene step before 3.1: extract `symbol_history` (11), which
   closes 1, 2 and 4, plus `-s`/`--no-color` (6, 15), the shared test helpers
   (24) and tests 26/27.
3. Index the real target repo once (37) and record coverage and cold-run time.
4. Decide 28, 29 and 36, then start 3.1.
5. Decide 31–35 before 3.2. 31 is the only one that reshapes existing code.

Everything else (7–10, 12–14, 16, 21–23, 25) is low-risk cleanup that can ride
along with whichever step next touches the file.
