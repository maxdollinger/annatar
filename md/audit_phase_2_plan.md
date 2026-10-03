# Annatar — Phase 2 Audit Remediation Plan

Plan to address [`audit_phase_2.md`](./audit_phase_2.md) (37 findings against
commit `4ff0c8b`). Like the Phase 1 remediation, this is **hardening, not scope
change**. It sits between the end of Phase 2 and the start of Phase 3, plus a
set of decisions Phase 3 needs before it can start.

- **Source audit:** [`audit_phase_2.md`](./audit_phase_2.md)
- **Authoritative scope:** [`plan.md`](./plan.md). Only R0 amends it, to
  record the Phase 2 deviations.
- **Status/living doc:** [`state.md`](./state.md), updated after each item.

## How to read this

Each work item (`R0`–`R8`) is one small PR, following `AGENTS.md`: implement
exactly one item, ship tests, run `cargo fmt`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`, update `state.md`,
then commit. §5 lists the Phase 3 decisions (`J1`–`J8`). They need a
product-owner answer, not code.

## 1. Verification of the audit

I checked every finding against the code at `4ff0c8b` and the uncommitted
`state.md` diff. **All 37 hold.** Notes where the finding needs nuance:

| # | Verdict | Note |
| --- | --- | --- |
| 1 | Confirmed | `indexer.rs:165-181`: a `None` from `file_last_commit` falls into the per-symbol `history_for_span` path, so every symbol spawns git, fails and warns. The `Err` arm of `file_last_commit` (`:140-147`) also degrades to `None`, so a failing file gets the same N+1 treatment. |
| 2 | Confirmed | `cache.get(...)?` (`:182`) and `cache.put(...)?` (`:197-199`) propagate, and `get` hard-fails on JSON decode (`history.rs:160`). |
| 3 | Confirmed | `-L` without a revision resolves against `HEAD`. Spans and `content_hash` come from the working copy. A dirty file's history is cached under `(dirty hash, HEAD sha)`. Open #14 is still undecided. |
| 4 | Confirmed | No-cache symbols are counted in neither bucket. |
| 5 | Plausible, unmeasured | `-L` lists a merge only when the merge itself changes the range (e.g. a conflict resolution). Squash merges are fine, because the key is in the squash subject. Real impact depends on the target repo's merge style, so **measure first** (R8). |
| 6 | Confirmed | `git log -L … -s --no-color --no-ext-diff` works on this machine's git 2.54. Older gits may ignore `-s` with `-L`. That's harmless, because the parser already drops hunks. |
| 7–10 | Confirmed | |
| 11–16 | Confirmed | `build_index` is `indexer.rs:68-240`. The regex is compiled at `config.rs:111` and again at `main.rs:68`. |
| 17 | Confirmed | Four corruptions in the uncommitted `state.md` diff, plus a removed blank line. The D-p alternatives cell now holds a draft question. |
| 18–23 | Confirmed | `project.md:59` is the broken `&#91;embedded content…\]` export. |
| 24–27 | Confirmed | For 27: the span starts at `declaration.start_position()` (`symbols.rs:299`). A Javadoc is a preceding sibling comment, so it falls outside `-L`, but `content_hash` includes it. |
| 28–37 | Confirmed as gaps | |

**Not re-verified:** `cargo` isn't installed in this container either, so fmt,
clippy and tests weren't run. Run all three before starting R1.

## 2. Work items at a glance

| Item | Findings | Theme | Touches | Priority |
| --- | --- | --- | --- | --- |
| R0 | 17, 18, 19 (partly), 20 | Fix `state.md` and record the Phase 2 deviations | `state.md`, `plan.md` | P0, before any commit |
| R1 | 1, 2, 4, 11, 13, 26 | Extract `symbol_history`: skip untracked files once, degrade on cache faults, honest counters | `indexer.rs`, `history.rs`, `main.rs` | P0 |
| R2 | 3 | Dirty-tree detection: never cache dirty files | `history.rs`, `indexer.rs` | P0 |
| R3 | 27 | Javadoc-inclusive history span (needs decision J0) | `symbols.rs`, `indexer.rs`, `schema.rs` | P0 |
| R4 | 6, 7, 15 | Deterministic, cheaper git output; batched cache writes | `history.rs`, `indexer.rs` | P1 |
| R5 | 24, 25 | Shared test helpers; exact benchmark assertions | `src/test_support.rs`, `tests/common/`, all test modules | P1 |
| R6 | 9, 12, 14 | Small cleanups: `TicketSpan`, regex once, `--path` walk root | `indexer.rs`, `config.rs`, `main.rs`, `walk.rs` | P2 |
| R7 | 21, 22, 23 | Doc drift: `project.md`, `AGENTS.md` clippy gate, README usage | docs only | P2 |
| R8 | 5, 19, 37 | Real-repo run: cold time, symbol count, ticket coverage | `state.md` (numbers) | P0 for Phase 3, **needs you** |
| R9 | 31 | Staged pipeline: one orchestrator owns the build until commit | `indexer.rs`, `main.rs` | Before 3.2 |

Deferred with a note only (§6): 8, 10, 16.

---

## 3. Work items

### R0 — Fix `state.md`; record the Phase 2 deviations

**Findings:** 17, 18, 20 (wording part), 19 (structure only; numbers in R8).

1. Revert the four corruptions from the uncommitted diff: the
   `` `parsing annotations for `<fqn>`` `` code span, `+ tree-sitter-java`,
   `(ISO `%aI`)`, and the D-p alternatives cell. Restore the original
   alternatives text, or replace it with a deliberate rewrite. Don't leave the
   draft question in.
2. Current state table: `Phase 2 — complete with deviations (see open #18)`.
3. `plan.md` 2.2/2.3: add a one-line *Deviation* note each. 2.2 was verified on
   a scratch repo. 2.3 measured a synthetic repo (D-p). Both are closed by R8.
   I recommend this over rewriting the "Done when" criteria, which should keep
   their original intent.
4. D-l wording is fixed by the R1 code change ("one warning naming the file").
   R0 leaves it alone, and R1 edits it if needed.

*Done when:* `git diff md/state.md` shows no formatter damage and the
deviations are visible in both docs.

### R1 — `symbol_history` helper: one failure path, honest counters

**Findings:** 1, 2, 4, 11, 13, 26. **Resolves:** D-l drift (20).

**Change:**

1. `IndexStats` gets `#[derive(Default)]` and a new `history_skipped` field
   ("symbols in a git repo with no history: untracked or failed file"). The
   `history_misses` doc becomes "cache miss → `git log -L`". Invariant on a
   git repo: `hits + misses + skipped == symbols`. `main` prints the new
   counter.
2. Per file: if `file_last_commit` is `Ok(None)` or `Err`, warn **once** with
   the path, count every symbol in the file as `skipped`, and make no `-L`
   calls. This makes D-l's "one warning naming the file" true.
3. Extract
   `async fn symbol_history(cache, repo, file, symbol, hash, last_commit) -> HistoryOutcome`,
   where `HistoryOutcome` is `Hit(Vec<Commit>) | Miss(Vec<Commit>) | Failed`.
   Cache policy is "degrade, don't abort", matching D-l:
   - `get` error (read or JSON decode): `warn!` with the fqn, treat as a miss.
   - `put` error: `warn!`, keep the commits.
   - `-L` error: `warn!` per symbol, `Failed` (counted as skipped). This should
     now be rare, because untracked files never reach it.
   `build_index` keeps one call site and loses two levels of nesting.
4. Tests (finding 26):
   - A scratch repo with one committed file and one untracked file: the
     counters add up, there are no `symbol_commits` rows for the untracked
     file, and the run succeeds.
   - A corrupt `history_cache.commits` value (`UPDATE … SET commits='{'`): the
     run succeeds, the symbol counts as a miss, the row is rewritten with valid
     JSON, and history is attached.

**Decision to record:** D-q: cache faults degrade to a miss.

### R2 — Dirty working tree: warn and don't cache

**Finding:** 3. **Resolves:** open #14.

**Recommendation:** run one `git status --porcelain -z --untracked-files=no`
per run and build a `HashSet<PathBuf>` of modified or staged paths. For a
dirty file, still compute `-L` history (approximate, which is good enough for
the `--path` loop), but **never read or write the cache** for it. Log one
summary `warn!` per run ("N dirty files: history may be mis-attributed and is
not cached"), plus one `debug!` per file.

- Alternatives: skip history for dirty files, which is too aggressive for the
  dev loop; map working-copy lines onto `HEAD` via `git diff`, which is a lot of
  machinery for a POC.
- Paths from porcelain are relative to the repo **root**, so check that
  `repo` *is* the root (`rev-parse --show-toplevel`), or prefix-adjust. The
  same caveat already applies to `-L` paths, so note it in the decision.
- **Test:** commit a file, edit it in place so the lines shift, and index. No
  `history_cache` row is written, and the warning fires. Then commit and index
  again: the row is now written.

**Decision to record:** D-r, closing open #14.

### R3 — Javadoc-inclusive history span

**Finding:** 27. **Needs:** J0 (below). The recommended answer is to include
the Javadoc.

1. `Symbol` gains `history_start_line`: the Javadoc comment's start row when
   `javadoc_of` found one, else `start_line`. `start_line` itself is unchanged,
   so `show` and the file:line output stay the same.
2. `symbol_history` passes `history_start_line..=end_line` to `-L`.
3. **Cache invalidation:** the cache key (fqn + hash + sha) doesn't change, but
   the cached *value* would. Per the ground rules, drop the table: bump to
   `DROP TABLE IF EXISTS history_cache` once, or rename it to
   `history_cache_v2` in `CACHE_TABLES`. Renaming is simpler and leaves no
   one-shot code behind.
4. **Test:** commit a method, then commit only a Javadoc change with
   `GRLD-9 document X`. `GRLD-9` is in the method's `symbol_tickets`.

### R4 — Deterministic and cheaper git; batched cache writes

**Findings:** 6, 7, 15.

1. Every production `git log` call (`file_last_commit`, `history_for_span`)
   adds `--no-color --no-ext-diff --no-show-signature`, and `-L` adds `-s`.
   Keep `parse_records` tolerant of hunks: older gits ignore `-s` with `-L`,
   and the minimum version isn't worth enforcing.
2. Wrap each file's cache `put`s in one transaction on the **cache**
   connection, separate from the index transaction as D-o requires. Committing
   per file bounds the loss from a crash to one file.
3. Re-run `tests/benchmark.rs` and record the cold-run delta (expected:
   noticeably fewer bytes read and one fsync per file instead of per symbol).
4. **Test:** a scratch repo with `color.ui=always` and `log.showSignature=true`
   set in its local config still yields the same commits.

### R5 — Shared test helpers; exact benchmark assertions

**Findings:** 24, 25. **Resolves:** open #16 / D-n.

1. `src/test_support.rs` (`#[cfg(test)] pub(crate) mod`) holds
   `git`/`git_ok`/`init_repo`/`commit`/`write_file`. `history`, `indexer` and
   `show` test modules use it.
2. `tests/common/mod.rs` holds the same helpers for `tests/benchmark.rs` (and
   `tests/index_show.rs` if useful). Two copies (lib and integration) is the
   minimum without adding a feature flag. Record that as the decision.
3. Benchmark: replace `warm_time < cold_time` with the count assertions
   `warm.history_hits == SYMBOLS`,
   `changed.history_misses == FILES / CHANGED_DIVISOR * (METHODS + 1)`. Keep
   timings printed, not asserted. Add a note next to the recorded numbers in
   `state.md` that they are from a debug build.

R1–R3 add git tests, so doing R5 **first** saves rewriting them. Either order
works. I recommend R5 right after R0 (see §7).

### R6 — Small cleanups

**Findings:** 9, 12, 14.

- `TicketSpan { key, first_date, last_date }`; `ticket_span` returns
  `Vec<TicketSpan>` with a `HashMap<String, usize>` index, which removes the
  production `expect`.
- `Config` keeps the compiled `Regex` (`#[serde(skip)]`, filled in
  `validate`), exposed via `ticket_regex()`. `main` drops its second compile.
- `walk::java_files` starts the walker at `repo.join(prefix)` when a prefix is
  given and still strips paths against `repo`. Test: same output as today with
  and without a prefix. A missing prefix directory is an error naming the path.

### R7 — Doc drift

**Findings:** 21, 22, 23.

- `project.md`: storage becomes "two libSQL files (`index.db` rebuilt,
  `cache.db` persistent) with libSQL's native vector index";
  `get_symbol(fqn)`; `search_intent(query, kind?, role?, path?)` (the target
  may keep `path?` as a superset); fix or remove the line-59 artifact.
- `AGENTS.md` step 4: `cargo clippy --all-targets -- -D warnings`. Run it
  once and fix anything it surfaces in test code in the same PR.
- `README.md`: a "Usage" section with `annatar index [--path <prefix>]`,
  `annatar show <fqn>`, the `annatar.toml` location, and "requires `git` on
  `PATH`; `repo` must be a work tree, otherwise only structure is indexed".

### R8 — Index a real repo once (**needs you**)

**Findings:** 5, 19, 37. **Resolves:** open #18, and informs #13 and #17.

I can't do this without the target checkout. Run on `grld-spring-auth`
(single-module, avoids #13) or `grld-core`, after R1–R4 so the numbers reflect
the fixed code:

```sh
cargo build --release
time ./target/release/annatar index        # cold
time ./target/release/annatar index        # warm
```

Record in `state.md` 2.3 Done: files, **symbols**, cold and warm time,
ms/symbol, and **ticket coverage**: the share of symbols with ≥1 ticket, from
`SELECT COUNT(DISTINCT symbol_id) FROM symbol_tickets` vs `symbols`. For
finding 5, also sample `git log --merges --format=%s | grep -cE 'GRLD-[0-9]+'`
against non-merge commits. If keys mostly live in merges, add a follow-up item
"first-parent/merge-message pass" before Phase 4.

---

## 4. Traceability

| Finding | Item | Finding | Item | Finding | Item |
| --- | --- | --- | --- | --- | --- |
| 1 | R1 | 14 | R6 | 27 | R3 |
| 2 | R1 | 15 | R4 | 28 | J1 |
| 3 | R2 | 16 | §6 | 29 | J2 |
| 4 | R1 | 17 | R0 | 30 | J3 |
| 5 | R8 | 18 | R0 | 31 | J4 → R9 |
| 6 | R4 | 19 | R0 + R8 | 32 | J5 |
| 7 | R4 | 20 | R0 / R1 | 33 | J6 |
| 8 | §6 | 21 | R7 | 34 | J7 |
| 9 | R6 | 22 | R7 | 35 | J8 |
| 10 | §6 | 23 | R7 | 36 | J9 |
| 11 | R1 | 24 | R5 | 37 | R8 |
| 12 | R6 | 25 | R5 | | |
| 13 | R1 | 26 | R1 | | |

## 5. Decisions needed (product owner)

Each has my recommendation. Confirm or override, and the answer goes into
`state.md` → Decisions.

| # | Question | Recommendation | Why |
| --- | --- | --- | --- |
| J0 | Should a Javadoc-only commit be attributed to its symbol? (27) | **Yes**: extend the `-L` span to the Javadoc (R3). | Javadoc edits often carry the "why" ticket, and `content_hash` already treats the Javadoc as part of the symbol. |
| J1 | Jira auth (28, open #1) | `email` set → Basic (Cloud, email:token); else Bearer PAT (Server/DC). | Both are a few lines. `JiraConfig` already has optional `email` + `token`. |
| J2 | HTTP stack (29) | `reqwest` (async, `default-features = false`, `json` + `rustls-tls`), `html2text` for HTML → text. | Accepts the rustls/hyper compile cost that decision 13 avoided for libSQL. That's unavoidable for HTTPS. `html2text` handles lists and tables, which a hand stripper wouldn't. |
| J3 | Fixtures and end-to-end (30) | **Owner: product owner, with a real token.** Capture one `renderedFields` response per case (story, bug, sub-task with parent, epic child, empty description), scrubbed, and run the `#[ignore]` end-to-end test (§3.1 below). | Needs real Jira access. Without it, 3.1 tests guess at the shape. The token stays in `ANNATAR_JIRA_TOKEN` (and `ANNATAR_JIRA_EMAIL` for Cloud), never in a file or commit. |
| J4 | Pipeline shape (31) | One orchestrator owns `IndexBuild`. Stages structure → history → tickets → summaries take `&Transaction`, and commit + rename happen once at the end (R9, before 3.2). | Keeps the temp-file + atomic-rename ground rule. A long write transaction on a private temp file blocks nobody. |
| J5 | `index` without Jira configured (32) | Skip the ticket stage with one warning, plus an `--offline` flag to skip it even when configured. | Keeps the `--path` loop offline and makes decision 21 explicit. |
| J6 | Unavailable / errors (33) | 401 → fail the run, never cached. 403/404 → cached unavailable with `fetched_at`, no expiry in the POC (dropping the table refreshes). 429 → honour `Retry-After`, max 3 retries. 30 s timeout. Concurrency cap 4 (config). | Bad credentials must never poison the cache. `fetched_at` keeps a future expiry policy possible without a schema change. |
| J7 | Test seam (34) | `TicketSource` trait. The fake counts calls; the real one wraps `reqwest`. | Portable (no port binding) and directly proves "a second run makes no Jira calls". |
| J8 | Ticket data for readers (35) | During the build, copy the needed fields (key, type, summary, parent, unavailable) from `cache.db` into an index table `tickets`. Add both tables in 3.2. | `show` and MCP stay read-only on `index.db` (D-d), and the index stays portable. |
| J9 | Parent/epic (36) | POC: `fields.parent` only, plus optional config `jira.epic_link_field` (unset = ignored). | Cloud covers it. A Server/DC instance can opt in without code changes. |

### §3.1 Jira end-to-end test (manual, product owner)

The agent writes the code and the fixture-based unit tests. The real-token
run is manual and done by the product owner:

1. **Agent (step 3.1):** add an `#[ignore]` test, e.g.
   `jira::tests::fetches_real_ticket`, that reads the base URL from
   `annatar.toml` (or `ANNATAR_JIRA_URL`) and the ticket key from
   `ANNATAR_JIRA_TEST_KEY`. It asserts that the key, issue type and a non-empty
   summary are parsed, and that the description converts to plain text. It
   prints nothing secret.
2. **Product owner:** run it with a token:
   ```sh
   export ANNATAR_JIRA_TOKEN=…         # PAT or API token
   export ANNATAR_JIRA_EMAIL=…         # Cloud only (basic auth, J1)
   export ANNATAR_JIRA_TEST_KEY=GRLD-123
   cargo test -- --ignored fetches_real_ticket --nocapture
   ```
   Repeat for one key per fixture case (story, bug, sub-task, epic child,
   empty description). Save the raw responses, scrub them, and commit them as
   fixtures (J3).
3. **Product owner, step 3.2:** after R8, run `annatar index` twice on the
   real repo with the token set. The second run must make no Jira calls (check
   the logged fetch count). Record the ticket counts (fetched, unavailable) in
   `state.md`.
4. Report back to the agent: auth mode used (Cloud basic or Server bearer),
   any 401/403/429 seen, and the parent/epic field shape (J9). The agent
   records the decisions.

Never paste the token into chat, `annatar.toml` or a fixture. Scrub
`self`/avatar URLs and personal names from saved JSON.

## 6. Explicitly deferred

- **8**: a type's `-L` is close to file history. Note it in 4.4 (cap the input)
  and in open #17 as a cold-run lever ("use `git log -- <file>` for top-level
  types").
- **10**: `show` loads all rows. Add to open #15/decision 41: "replace with a
  subtree query before 6.1".
- **16**: `history_cache` rows are never removed. Add to the Later list with
  other cache growth.

## 7. Suggested execution order

1. **R0** (docs, unblocks committing anything)
2. **R5** (helpers first, so R1–R3 tests use them)
3. **R1** → **R2** → **R3** (correctness; R3 after J0)
4. **R4** (re-measure)
5. **R8** (real repo, needs you), in parallel with answering **J1–J9**
6. **R6**, **R7** (ride-along hygiene; can go any time)
7. Start **3.1** (needs J1, J2, J9). Product owner runs the token
   end-to-end test and supplies the fixtures (J3, §3.1).
8. **R9** (needs J4), then **3.2** (needs J5–J8). Product owner runs the
   real-repo "second run makes no Jira calls" check (§3.1 step 3).

## 8. Definition of done

- All findings are mapped (§4) and closed or explicitly deferred (§6).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test`
  are green.
- On a git repo, `hits + misses + skipped == symbols`, asserted in a test.
- `state.md`: open #14, #16 and #18 resolved; D-q/D-r (and R3/R5 decisions)
  recorded; Phase 2 real-repo numbers present.

## 9. Execution log

| Item | Commit | Notes |
| --- | --- | --- |
| R0 | docs: add phase 2 audit and remediation plan | Formatter damage discarded (`git checkout md/state.md`; the only content in that diff was the damage plus the D-p draft question); Phase 2 marked complete with deviations; `plan.md` 2.2/2.3 deviation notes; open #18 points at R8 |
| R5 | test: share scratch-git helpers, exact benchmark counts | `#[path]` include instead of a second copy in `tests/common/` (D-s); benchmark re-run on linux/debug: cold 3.41 s, warm 0.39 s, changed 0.72 s |
| R1 | fix(indexer): skip untracked files once, degrade cache faults | `HistoryOutcome` enum instead of updating stats inside the helper; corrupt-row test proves the row is rewritten (third run hits) |
