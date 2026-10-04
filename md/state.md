# Annatar — State

Living document. Updated after every step. Tracks where the project stands,
the decisions taken, and the tradeoffs behind them.

- **Plan:** [`plan.md`](./plan.md)
- **Overview:** [`project.md`](./project.md)

## Current state

| | |
| --- | --- |
| Phase | 5 — Search and agent trial — **extended by 5.5** (product-owner redesign, 2026-10-04: `search` returns files with their symbols). Questions 1–3 were answered by 4.5/4.6, 5.3 (D-ch) and 5.4 (D-cl); 5.5 changes the search output the trial measured, so the trial's *with* arm is re-run on it next. (Phase 4 LLM summaries complete pending the product owner's confirmation of the 4.5 review and the 4.6 descriptions; Phase 3 Jira complete, including the product owner's real-Jira checks; Phase 2 and its audit remediation R0–R9 complete) |
| Step | 5.5 File-level search results — **done** (`annatar search` prints the top 5 files of the 100 nearest symbols, each with its top-level type and description and every member and nested type with lines, hits marked `*score`; `--symbols` keeps the 5.2 output; `eval` adds file-level scores: `argus` primary file top-1 / top-5 11 / 17 of 24, any 12 / 20, symbol scores unchanged; output ≈ 29 % smaller than the old 10-symbol default; D-cm–D-cr); 5.4 review fixes — **done** (significance recomputed with ties: tokens and tool calls significant on T1–T3, T4 a trend; strict regrade → correctness 35 vs 35 of 40; harness hardened and unit-tested); 5.4 Agent trial — **done** (4 `argus` tasks × 2 arms × 5 runs, Claude Code headless on `claude-sonnet-5`; with `annatar`: −47 % total tokens, −40 % tool calls, −32 % cost, −39 % wall time on average, lower on every task (significant per task on T1–T3, uncorrected); correctness 35 vs 35 of 40 points, net equal with opposite effects: better where the reason lives only in tickets (T2 reason 5/5 vs 0/5), worse on call-chain completeness (T4: 4 of 5 *with* runs skipped the message dispatcher, 0 of 5 without); question 3 answered: yes on this repository, with caveats (D-cl)); 5.3 Evaluate and its review fixes — done |
| Last updated | 2026-10-04 |
| Toolchain | rustc 1.99.0, edition 2024 |

### Done

- **5.5 File-level search results (product-owner redesign).** One commit
  (`feat(search)`), no reindex, no prompt or cache change. *Why:* the
  product owner asked for search to identify the file and list its
  symbols (quote in `plan.md` 5.5); 5.3 had the right file in the top 5
  for 75 % but the exact symbol first for 17 %, and 7 of its 20 misses
  were a class tying with its own member. *Ranking (D-cm):* unchanged
  symbol search (`search::search`, same filters) for the 100 nearest
  symbols, `search::group_by_file` keeps files in the order of their best
  hit, top `-k` files (default 5, at most 20; `-k 21` without `--symbols`
  is a usage error, exit 2). *Output (D-cn–D-cp):* per file
  `rank. score path`, each top-level type `kind fqn [role] :start-end` with
  its description, then every member and nested type of the file in
  source order as the fqn part after its parent (`#find(Long)`,
  `#<init>(Repo)`, `enum Mode [role]`) with `:start-end`, no description,
  two spaces more per nesting level; hits end with `*score` (the hits
  ranked above the next file's best hit); over 40 member lines the hits
  and the first lines are kept, then `… N more`. Filters decide hits and
  file rank, the file is shown whole (D-cq). `--symbols` is the 5.2 output
  and default (10, at most 100), byte for byte. *Eval (D-cr):* also runs
  each question through `search::search_files` with `-k` files (the query
  is embedded once, the embedder's in-memory cache serves the second
  search) and scores the rank of the primary's file and the best rank of
  any expected fqn's file; per-question `file <p> <a>` columns and three
  `files …` summary lines. *Tests (311, +5):* grouping, formatting (nested
  types two deep, constructors, two top-level types, roles, descriptions
  collapsed, hit marks), the cap (hits inside nested types kept with their
  parents, `… N more`, cap 0), file search over a fake embedder with
  `--role` and `--kind` filters (whole file shown), eval file ranks; a
  CLI test pins both outputs on the binary and the `-k 21` usage error
  (hermetic, localhost embedding server). *`argus` (no reindex; 24
  questions, k = 10):*

  | ranking | primary top-1 | primary top-5 | primary MRR | any top-1 | any top-5 | any MRR |
  | --- | --- | --- | --- | --- | --- | --- |
  | symbols, all 24 (unchanged from 5.3) | 4 | 14 | 0.358 | 10 | 17 | 0.536 |
  | files, all 24 | 11 (46 %) | 17 (71 %) | 0.582 | 12 (50 %) | 20 (83 %) | 0.641 |
  | files, types (10) | 5 | 6 | 0.578 | 6 | 9 | 0.720 |
  | files, members (14) | 6 | 11 | 0.585 | 6 | 11 | 0.585 |

  The 5.3 ad hoc file numbers (file of the first / of the top-5 symbol
  hits: 12 / 18) are the "any" column's top-1 and a lower bound of its
  top-5 (5 distinct files reach further than 5 symbols: 20). The primary's
  file misses the top 5 for 7 questions (vocabulary, DTOs and the
  truncated member of open #39, as in 5.3). *Size and latency* (6 own
  queries, not golden questions; chars of stdout, tokens ≈ chars / 4):
  file output (5 files) 2350–4042 chars, mean ≈ 3100 (≈ 770 tokens),
  25–49 lines; `--symbols` default (10) 4053–4720, mean ≈ 4350 (≈ 1090
  tokens); `--symbols -k 5` mean ≈ 2140 (≈ 535 tokens). So the new default
  is ≈ 29 % smaller than the old default and ≈ 45 % larger than 5 symbols,
  while it lists every member of 5 files with lines. ≈ 50 ms per query
  (median of 18 runs; `--symbols` ≈ 48 ms): `vector_top_k` for 100 rows
  plus one `symbols` query for the 5 files. On `argus` the largest file
  has 17 symbols, so the 40-line cap never fires.

- **5.4 review fixes.** One commit (`fix(trial)`), no Rust change, no
  agent re-runs. *Significance (finding 1):* recomputed as an exact
  two-sided Mann-Whitney test with mid-ranks over all 252 splits of 5 vs
  5: tokens p = 0.008 / 0.03 / 0.008 / 0.06, tool calls p = 0.008 / 0.008
  / 0.008 / 0.08 (T1–T4); "tool calls p ≤ 0.03 on all four" was wrong for
  T4. Uncorrected for the 8 tests (the smallest attainable p, 0.008, is
  above any Bonferroni threshold); D-cl now says "significant on T1–T3".
  *Correctness (findings 2, 3):* T4-with-1 was graded 2 although its
  fresh-read MUST fact was as vague as in an answer that lost the point;
  regraded strictly to 1 (`grades.json` notes it, the lenient original is
  kept in `grades-original.json`, both private), so correctness is 35 vs
  35 of 40, not 36 vs 35. The verdict states the opposite effects: better
  where the reason lives only in tickets (T2 5/5 vs 0/5, Fisher p ≈
  0.008), worse on call-chain completeness (T4 dispatcher missed 4/5 vs
  0/5, p ≈ 0.05; full marks 0/5 vs 5/5), net equal; the T4 regression is a
  finding in D-cl and #42. *Explanation of the gains (finding 4):* the
  "largest where words differ and on why-questions" claim is now a
  hypothesis (T2 is a why-question too at −48 %, T4 −29 %). *Harness
  (findings 5, 8, 9):* `run` catches `TimeoutExpired`, writes
  `<run>.meta.json` (exit code, timeout, wall time) and skips the model
  check for a run without a result; `summary` warns about timeouts,
  non-zero exits, missing result events and non-`success` subtypes; the
  `annatar` call parser lexes with `shlex`, splits on newlines and shell
  operators, recurses into `$(…)` and backticks, skips `VAR=x` prefixes
  and global options before the subcommand (`annatar -k 5 search` →
  `search`); `run` exits when `annatar` is on `PATH` in the *without* arm
  or is not the `ANNATAR_TRIAL_BIN` one in the *with* arm. Re-parsing the
  40 raw streams gives a `runs.csv` identical to the published one (no
  count changed). *Tests (finding 6):* `scripts/agent_trial/test_run.py`
  (stdlib `unittest`, 10 tests, a 3-run fixture with made-up content in
  `scripts/agent_trial/fixtures/`), run with `python3 -m unittest
  discover -s scripts/agent_trial`; not part of `cargo test` (D-ck
  amended). *Docs (findings 7, 10–12):* the turns column is dropped
  (`num_turns` = tool calls + 1 in all 40 runs); D-cj cites the pilot for
  the 5 runs; open #40 updated; D-cj notes the imperfect blinding (one
  answer revealed its arm through "[index]'s description").

- **5.4 Agent trial.** One commit (`docs(trial)`): the harness
  `scripts/agent_trial/run.py` and these notes; no Rust change, no
  reindex. *Protocol (D-cj, fixed before the main runs):* Claude Code
  2.1.289 headless (`claude -p … --output-format stream-json`) on
  `claude-sonnet-5`, effort medium, cwd `/Repos/argus` (a read-only mount;
  `git status` checked after every run), tools Bash/Read/Grep/Glob only,
  no MCP, no settings files, no skills, no session persistence,
  auto-memory off (its directory checked empty after every run), ≤ 60
  turns, ≤ $3 per run. Both arms get the same prompt (a preamble: "new to
  this repository … do not modify files … answer with classes, methods,
  file paths"; then the task). The *with* arm also gets a neutral appended
  system prompt (what `annatar search` / `annatar show` print, "the best
  match is not always first … check the code"; no "always use it") and an
  `annatar` wrapper on `PATH` (`--config` of the real index, 5.3 default,
  unchanged); the *without* arm gets neither. Four read-only tasks on
  `argus`, written from the code (not from descriptions, not checked
  against search, not from the golden set) with an answer key before any
  run: **T1** oldest sessions of users on many devices stop working →
  per-user token cap (`DatabaseCacheUserTokenStrategy`, 500, oldest by
  creation deleted on each login; question words ≠ code words); **T2**
  users thrown out after login when the contract cannot be processed →
  which code, which HTTP statuses, why (`SubscriptionDataService` →
  `AuthDataService#getAuthData` → `removeAllTokens`; the reason, a
  dashboard/login redirect loop, is only in the Jira ticket); **T3** which
  users skip the "is this JWT still an active session" check, where, why
  (`UserTokenService#isTokenValid` admin bypass, used by the
  prevalidation endpoint; reason in tickets, partly inferable from code);
  **T4** trace what happens when the subscription service announces a
  booked feature until other services see it (queue dispatcher → feature
  consumer → contract cache refresh → DynamoDB update event). 5 runs per
  task and arm (40), rep-major, arm order alternating. Metrics from the
  stream: total tokens = input + cache read + cache creation + output
  (each also kept), `total_cost_usd`, tool calls by tool (Bash commands
  parsed for `annatar search` / `show`), turns, `duration_ms`.
  Correctness 0/1/2 per answer key (2 = every must-fact, 1 = right place
  but a must-fact missing or wrong, 0 = wrong place), graded by a separate
  agent from the 40 final answers shuffled, the word `annatar` redacted
  (one answer named it), without access to the run mapping; spot-checked
  (T4 misses confirmed). Pilot (T1 once per arm) excluded.
  *Results (mean ± sd over 5 runs; tokens in thousands):*

  | task | arm | total tokens | cost $ | tool calls | `annatar` search / show | wall s | score (0–2) |
  | --- | --- | --- | --- | --- | --- | --- | --- |
  | T1 session cap | with | 53 ± 2 | 0.051 | 4.0 | 2.0 / 0 | 11 | 2.0 |
  | | without | 140 ± 68 | 0.080 | 8.2 | – | 23 | 2.0 |
  | T2 contract error | with | 112 ± 13 | 0.089 | 6.0 | 2.6 / 1.6 | 19 | 2.0 (reason 5/5) |
  | | without | 216 ± 99 | 0.132 | 11.6 | – | 35 | 1.0 (reason 0/5) |
  | T3 admin bypass | with | 79 ± 16 | 0.058 | 5.0 | 1.8 / 1.0 | 14 | 2.0 (reason 5/5) |
  | | without | 217 ± 49 | 0.125 | 12.8 | – | 33 | 2.0 (reason 5/5, inferred) |
  | T4 feature flow | with | 220 ± 44 | 0.161 | 17.0 | 3.0 / 0.8 | 37 | 1.0 (dispatcher missed 4/5, fresh read vague 1/5) |
  | | without | 310 ± 58 | 0.190 | 20.6 | – | 42 | 2.0 |
  | **all** | with | 116 ± 69 | 0.090 | 8.0 | 2.35 / 0.85 | 20 | 1.75 (35/40) |
  | | without | 221 ± 90 | 0.132 | 13.3 | – | 33 | 1.75 (35/40) |

  (Table and the two sentences on significance and grading corrected in
  the 5.4 review fixes: the turns column is gone because `num_turns` was
  tool calls + 1 in all 40 runs; T4 *with* 1.2 → 1.0 after a strict
  regrade; p-values recomputed with ties.)

  With `annatar` vs without, per task: total tokens −62 / −48 / −63 /
  −29 %, cost −36 / −32 / −53 / −15 %, tool calls −51 / −48 / −61 /
  −17 %, wall time −52 / −47 / −58 / −12 %; overall −47 % tokens, −32 %
  cost, −40 % tool calls, −39 % wall time. The token ranges
  of the two arms do not overlap for T1 and T3 and overlap for T2 and T4
  (exact two-sided Mann-Whitney test on 5 vs 5 with mid-ranks for ties,
  all 252 splits, T1 / T2 / T3 / T4: tokens p = 0.008 / 0.03 / 0.008 /
  0.06, tool calls p = 0.008 / 0.008 / 0.008 / 0.08; 8 tests, no
  multiple-comparison correction — 0.008 is the smallest p that 5 vs 5
  can give, so no corrected threshold could be met; significant on T1–T3
  only, T4 a trend). Token split
  (means, with vs without): cache read 102k vs 202k, cache creation 12.6k
  vs 15.5k, output 1.9k vs 2.9k, uncached input 11 vs 21; tool calls by
  tool: Bash 4.3 (with the `annatar` calls) vs 4.6, Read 3.4 vs 5.8, Grep
  0.2 vs 2.9, Glob 0.2 vs 0.15. Every *with* run used `annatar` (1–4
  searches, often two in one Bash call; `show` in 12 of 20, always on T2
  and T3, where the ticket reasons are). Correctness: T1 and T3 full
  marks in both arms (the *without* arm inferred T3's reason from the
  code); T2's reason (redirect loop) came only with `annatar` — the
  *without* arm found the ticket key in `git log` but guessed a generic
  "fail safe"; on T4 four *with* answers started at the feature consumer
  that search returned and left out the queue dispatcher that delivers the
  message (two also named a wrong reader), while all five *without*
  answers traced it from the queue (Fisher exact p ≈ 0.05); the fifth
  *with* answer named the dispatcher but left the fresh read by other
  services (`AuthDataService#getAuthData` / `AuthDataCacheRest`) vague
  and is regraded 2 → 1 (review fixes), so no *with* answer got full
  marks on T4 (0/5 vs 5/5, p ≈ 0.008), the mirror of T2 (reason 5/5 vs
  0/5, p ≈ 0.008). *Spend:* $4.43 for the 40 runs ($1.80
  with, $2.63 without), $0.52 pilot (of which $0.29 an accidental Opus
  pair, D-cj), ≈ $0.07 setup checks: ≈ $5.0 in total; the grader ran in
  this session. Raw streams, `runs.csv`, the blind answers, the mapping
  and the grades are in `.annatar-local/agent-trial/` (private).
  *Verdict:* D-cl. New open #41 (trial size), #42 (search shortcuts a
  call chain).

- **5.3 review fixes.** One commit. *Hermetic CLI tests (finding 1):*
  the `tests/cli.rs` eval test passed only because the caller's
  `NO_PROXY` covered `127.0.0.1`; with `NO_PROXY` unset, the request to the
  localhost fake embedding server went to the proxy (403). The `annatar()`
  helper now removes `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` (both cases)
  and sets `NO_PROXY`/`no_proxy` to `127.0.0.1,localhost`, so every CLI
  test is hermetic; verified with `env -u NO_PROXY -u no_proxy cargo test
  --test cli` (fails before, passes after). The production Ollama client
  is left as is (D-ci). *`eval -k` (finding 2, D-cf amended):* 5–100, since
  the summary counts top-5; `eval -k 4` is a usage error (exit 2), tested.
  *Docs (findings 3–5):* D-cg now states both sides (variant better on 5
  of 6 totals, worse on primary top-5, types regress; kept off because the
  gain is within noise at n = 24); D-ch, the Done figures and the plan.md
  5.3 note carry the sample-size caveat (n = 24, one repo, one author,
  95 % CI ≈ ±18 pp); open #39 quotes the dedented lengths the cap applies
  to (≈ 1964 / 1751, not 2046 / 1825 raw). *Tests (306, 5 ignored; +1).*
  No reindex.

- **5.3 Evaluate.** One commit. *Command (D-cf):* `annatar eval
  [-k N] <golden.toml>` (new `eval` module): loads the set with
  `GoldenSet::load`, runs `golden::check_index` first (any problem is the
  one-line error, nothing is searched), then every question through
  `search::search` without a filter — the code path of `annatar search
  "<question>"`, so `vector_top_k` — and scores the rank of the first
  `expect` fqn (primary) and the best rank of any `expect` fqn. Output: a
  settings line (`eval: embedding_model=… dim=… parent_description=… k=…`),
  one line per question `n. <primary rank> <any rank> [types|members]
  <primary fqn> top=<first hit>` (`-` = not in the top k; `top=` only when
  the primary is not first), then `all`, `types` and `members` lines with
  top-1, top-5 and MRR for primary and any; types vs members by the
  primary fqn (`#`). No `--path` (error). README documents it. Pure
  `score`/`summarize`/`format_report` plus `evaluate` over a fake embedder
  and a hand-built index unit-tested; `tests/cli.rs` runs the binary end to
  end against a one-file localhost embedding server (synthetic golden
  fixture, ranks predicted from the vectors) and checks the missing-fqn
  and `--path` errors. *Tests (305, 5 ignored; +9).*
  *`argus` (24 questions: 10 types, 14 members incl. 1 constructor; 0
  `check_index` problems; ≈ 0.6 s for all 24 queries):*

  | index | primary top-1 | primary top-5 | primary MRR@10 | any top-1 | any top-5 | any MRR@10 |
  | --- | --- | --- | --- | --- | --- | --- |
  | default (`parent_description = false`), all 24 | 4 (17 %) | 14 (58 %) | 0.358 | 10 (42 %) | 17 (71 %) | 0.536 |
  | — types (10) | 1 | 4 | 0.246 | 5 | 7 | 0.573 |
  | — members (14) | 3 | 10 | 0.439 | 5 | 10 | 0.510 |
  | `parent_description = true`, all 24 | 7 (29 %) | 13 (54 %) | 0.409 | 11 (46 %) | 17 (71 %) | 0.576 |
  | — types (10) | 1 | 4 | 0.233 | 3 | 7 | 0.468 |
  | — members (14) | 6 | 9 | 0.534 | 8 | 10 | 0.653 |

  File level (an `expect` symbol's file is the file of the first hit / of
  one of the top 5, ad hoc, not part of `eval`): default 12 / 18 of 24,
  variant 14 / 19. With `-k 100` the default primary misses beyond 10 sit
  at 11, 16, 16, 28, 45, 63 and two beyond 100. *Variant run:* a scratch copy
  of `cache.db` and a scratch config with `[embedding] parent_description
  = true` (data in the session scratchpad, `data-real` untouched): 0 chat
  calls, 8 embedding calls, 479 member texts sent (types reused from the
  cache), 8.0 s. Per question it moves the primary up for 6 and down for
  9: members gain (their class's words reach them), types lose (their
  members now carry the type's description and crowd it out: the primary
  type falls 16 → 35, 28 → 55, 8 → 23). Within the top 10 the primary moves up for 5 and down for
  4; the variant is better on 5 of 6 totals but within noise at n = 24,
  and types regress. Default kept (D-cg, amended in the review fixes).
  *Misses (primary not 1st, default: 20 of 24), by cause:* (1) **own
  parent, member or nested type first** (7: a class above its scheduled
  or private method, a method above its class 3×, a nested exception above
  the method that throws it, an outer class above its nested enum): both
  descriptions say the same, the score gap is 0.01–0.03; "any of" turns 5
  of them into a top-1 hit. (2) **data carriers restating the behaviour**
  (3: a request TO and two Cronus response DTOs outrank the service or
  cache they feed). (3) **vocabulary gap** (5: the question's domain words
  — login cookie, second factor, "contract" meaning a Java interface, a
  cache-change notice for a DynamoDB event, session check for token
  validity — do not occur in the description; the interface question
  matches the business `CachedContract` instead). (4) **description lacks
  the asked behaviour** (3): `AuthDataService#getAuthData(UUID)` (45th)
  and `RoleRelationsUpdatedMessageConsumer#perform(…)` (7th) are the only
  2 of 479 members whose source exceeds `describe.body_chars` (1500; ≈ 1964
  and 1751 chars dedented, corrected in the review fixes), and both asked behaviours (the logout in the error
  branch, the role-cache refresh) sit after the cut (open #39); the
  repository interface's description does not mention its native
  row-limited deletes. (5) **wrong reason** (1): `OAuth2RequestCacheService`'s
  object-size reason (open #30) sends the question to its controller
  (alternate, 1st) and the primary to 28th. (6) **lookalike in the
  other direction** (1: the endpoint that publishes a user-deleted event
  outranks the consumer that handles one, 0.008 apart). Shared initiative reasons
  ("without requiring user re-login", "independently of the CORE")
  make sibling consumers and dispatchers near-identical (the second-factor
  dispatcher loses to the identification one). No golden entry looks
  wrong (checked against the source where the description and the
  question disagreed); the nested-type role rule (D-cb) costs nothing here
  because `eval` uses no filter. *Verdict on question 2 (D-ch):* search by
  meaning finds the right neighbourhood — an acceptable symbol in the top
  5 for 71 % (n = 24, 95 % CI ≈ 51–86 %) of deliberately identifier-free questions, the right file for
  75 % — but names the exact intended symbol first for only 17 % (42 %
  counting alternates); it is a good "where to look" for an agent that
  reads the top hits and their `show` detail, not a one-shot answer. 5.4
  measures whether that is enough. plan.md 5.3 notes what was built.

- **5.2 review fixes.** One commit. *`--path` (findings 1, 2, D-cb
  amended, D-ce):* `walk::relative_prefix` now normalises lexically — `.`
  dropped, `..` removes the component before it, a `..` that climbs above
  the repository returns `None` (like an absolute path outside it); the
  former `search_path` in `main.rs` is `search::path_filter`, which turns
  the empty relative prefix (`.`, `./`, the repository's absolute path,
  with or without a trailing `/`) into *no* path filter (it matched
  nothing before) and any escape (`..`, `../x`, `src/../../x`) into the
  existing "`--path … is outside the repository …`" error. `index --path
  backend/../backend` now indexes `backend` instead of nothing; `index
  --path ../x` still yields an empty index with the existing warning
  (open #38). *Docs (findings 3, 4, 6, 7):* README: warnings (such as
  the retry of an unreachable Ollama) may precede the single `error:` line
  (D-cd amended, the warning is kept); the search paragraph and examples
  document `--path`; members of a nested type match `--role` by the nested
  type's role (D-cb amended, Next notes it for 5.3); the fqn is the third
  space-separated field (fqns hold no whitespace), the location is
  everything after the `[kind]`/`role=` tokens (D-cc amended, the `cut -f`
  hint for later fields is gone). *Tests (296, 5 ignored; +6):*
  `relative_prefix` dots and escapes, `java_files` with `..`,
  `path_filter` (repo root, trailing `/`, escapes), `vector_top_k` vs the
  exact scan (same hits and scores for 3 queries over all 5 rows),
  `tests/cli.rs`: a search against an existing index (`index --no-llm`,
  `cache.db` removed, 3-dim vectors and `index_meta` added through libSQL)
  leaves the data dir byte-for-byte unchanged with only `index.db` in it —
  hermetic, so it asserts on the model-mismatch error after the index is
  opened (no embedding backend in the binary test), with and without
  `--path .`; `--path ..`/`../repo`/`src/../../x` give one `error:` line.
  *Real check (`argus`, release, no reindex):* `--path .`, `./`,
  `/Repos/argus`, `/Repos/argus/` return the unfiltered top results;
  `--path ../x` → one error line, exit 1; `…/configuration` and
  `backend/../backend/…/configuration` rank alike; `data-real` mtimes and
  sizes unchanged.

- **5.2 `annatar search`.** One commit. New `search` module:
  `search::search` checks the index first (`symbol_vectors` missing → "the
  index has no vectors; run `annatar index` with an [ollama] section …";
  `index_meta` rows missing → "rebuild it"; `embedding_model` differs from
  `ollama.embedding_model` → refused before any request), then embeds the
  trimmed query through `LlmClient::embed` (`QueryEmbedder`: the client over
  an in-memory `embedding_cache`, so `search` writes no file, D-by), refuses
  a query vector whose length differs from `embedding_dim` (D-bz), and calls
  `search::nearest`: without a filter `vector_top_k(k)` joined to
  `symbols`, with a filter an exact `vector_distance_cos` scan of the
  matching rows (D-ca, resolves open #36); every hit is scored with the
  exact cosine similarity and ordered by it (ties by fqn). CLI:
  `annatar search [-k/--limit N] [--kind K]… [--role R]… "<query>"` (limit
  1–100, default 10; kinds and roles checked by clap from
  `SymbolKind::ALL` / the new `Role::ALL`; a member matches `--role` by its
  enclosing type's role, D-cb); the global `--path` limits results to files
  under the prefix (whole path components, same normalisation as the
  walker's, now `pub walk::relative_prefix`). Output (pure
  `search::format_hits`, D-cc): `rank. score fqn [kind] role=… file:start-end`
  and the description, whitespace collapsed, on the next line indented by
  three spaces; no hits → empty stdout and a note on stderr, exit 0.
  *Open #29 (D-cd):* `init_logging` writes to stderr (ANSI only when stderr
  is a terminal); `main` returns `ExitCode` and prints every error as one
  line `error: <chain joined by ": ">` on stderr, exit 1 (clap usage errors
  keep clap's own message, exit 2); the `index` stats lines and the `show`
  output stay on stdout. The `not implemented` stub is gone. *`show`:*
  already printed description, tickets (with summaries), commits, children
  and `file:start-end`; it now also names the rendered symbol's parent
  (`- parent: <fqn>`, root only). *Tests (290, 5 ignored; +14):* 9 search
  unit tests over a fixture index with 3-dim vectors and `FakeBackend`
  (ranking and exact score, kind filter, role filter incl. members, path
  filter by component, query embedded with the configured model, model
  mismatch refused without a request, dimension mismatch, no vectors / no
  metadata, empty query and failed embedding, output format); a `show`
  parent test; `tests/cli.rs` (3) runs the binary: `index -vv` stats lines
  only on stdout and DEBUG/WARN only on stderr without ANSI, `show` on
  stdout with its parent, `search` without vectors and without an index
  (one stderr line, exit 1, empty stdout, no data dir created), unknown
  `--kind`/`--role` and `-k 0`/`101` (exit 2). *Real run (`argus`, release,
  index rebuilt `--offline`: 0 chat and 0 embedding calls, unchanged 654
  vectors; queries written from reading the code, not from the golden set;
  package-relative fqns):*

  | Query | Filter | Rank 1 (score) | Right symbol |
  | --- | --- | --- | --- |
  | periodically delete expired user tokens from the database | — | `…databaseCacheStrategy.DatabaseCacheUserTokenSchedulerService#deleteExpiredTokens()` (0.770) | yes; its class 3rd, the repository's delete query 4th |
  | route database queries to a read-only replica data source | — | `configuration.DataSourceRoutingConfig#readOnlyDataSource(…)` (0.630) | yes; ranks 1–4 are the class and its 3 beans |
  | turn a Redis connection failure into an HTTP error response | — | `common.exception.ArgusExceptionHandler#handleRedisConnectionFailure(…)` (0.733) | yes; its class 2nd |
  | configure JSON serialization | — | `configuration.ArgusJacksonConfig#configureJackson()` (0.580) | yes, but ranks 2–5 are unrelated `getMessage` serialisers within 0.01; the class itself is 2nd with `--path …/configuration` |
  | answer calls to its own auth data service locally instead of over HTTP | — | `configuration.ArgusSelfCallInterceptor#getIntercepted(…)` (0.638) | yes; its class 2nd |
  | consume messages about changed role relations | `--kind class` | `…role.message.consumer.RoleRelationsUpdatedMessageConsumer` (0.704) | yes |
  | invalidate all tokens of a user | `--role controller` | `…token.boundary.TokenValidationRest#deleteAllFromList(…)` (0.674) | yes (3 delete-all endpoints of the same controller in ranks 1–3) |
  | cache of OAuth2 requests in Redis | `--kind class --kind interface` | `oAuth2.oAuth2Request.OAuth2RequestCache` (0.763) | yes; its service 2nd |
  | delete expired token entries | `--role repository --kind method` | `…UserTokenEntryRepository#deleteByExpirationDateBefore(…)` (0.739) | yes |

  Latency (release, wall clock of the whole process): ≈ 41–70 ms per query
  with the model loaded: query embedding ≈ 25–45 ms, `vector_top_k` ≈ 13–15
  ms, a filtered exact scan 2–6 ms (all 654 rows ≈ 15 ms). The first query
  after Ollama unloaded `bge-m3` took 1.07 s to embed. Scores of the right
  hits are 0.58–0.77; unrelated neighbours sit at ≈ 0.50–0.58, so a score
  threshold would be fragile (none is applied). Mismatched model: one line
  naming both models, exit 1, no request; Ollama unreachable: one retry
  warning (2 s backoff) and one `error: embedding the query with
  "bge-m3:latest": … Connection refused` line. `cache.db` was not touched
  by any search (mtime unchanged).

- **5.1 review fixes.** One commit. *Index size (finding 1, D-bt
  amended):* `schema::VECTOR_MAX_NEIGHBORS = 32` goes into the vector index
  (`'max_neighbors=32'`). libSQL's default for 1024 dimensions is 96
  neighbours, ≈ 105 KB of graph per symbol with float8 (71 of 75 MB of
  `index.db`); it would reach ≈ 450 MB at 4k symbols. Measured with a
  throwaway test on the real vectors (fresh file on the same mount, one
  transaction, recall of `vector_top_k` k=10 against exact
  `vector_distance_cos`):

  | Neighbours | Set | Insert (256 MiB cache / 2 MB) | Size | Top-1 | Recall@10 |
  | --- | --- | --- | --- | --- | --- |
  | default (96) | `argus` 654, own vectors as queries | 0.95 s / 5.3 s | 74.0 MB | 654/654 | 1.0 |
  | 32 | same | 0.60 s / 1.63 s | 27.8 MB | 654/654 | 1.0 |
  | 16 | same | 0.49 s / 0.84 s | 16.8 MB | 654/654 | 1.0 |
  | default / 32 / 16 | `argus` 654, 654 mid-point queries (two vectors + noise) | as above | as above | 654/654 | 1.0 / 1.0 / 1.0 |
  | default (96) | 3924 (`argus` + 5 noisy copies), mid-point queries | 7.2 s / 22.2 s | 444 MB | 654/654 | 1.0 |
  | 32 | same | 4.2 s / 8.7 s | 167 MB | 654/654 | 1.0 |
  | 16 | same | 3.3 s / 5.3 s | 101 MB | 652/654 | 0.998 / 0.997 |

  16 is the smallest with recall@10 ≥ 0.99 on `argus`, but it is the first
  to lose recall on the 6× set, so 32 (≈ 42 KB per symbol) was chosen.
  The 256 MiB page cache still helps (32 neighbours: 1.63 → 0.60 s on
  `argus`, 8.7 → 4.2 s on the 6× set), so it stays. *Over-fetch (finding
  2):* `vector_top_k` returns at most 200–202 rows whatever `k` (libSQL's
  default search list; checked with k = 654 on every variant), so open #36
  and Next now say: a filtered query uses an exact `vector_distance_cos`
  scan (≈ 654 rows on `argus`), or over-fetches ≤ 200 and falls back to the
  exact scan when too few rows survive. *Index metadata (finding 3, D-bw):*
  new `index_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)` in
  `INDEX_TABLES`; the embeddings stage writes `embedding_model`,
  `embedding_dim` and `embedding_parent_description` (`true`/`false`) when
  the index has vectors (none without vectors), for 5.2 to refuse or warn
  on a mismatch. *Dimension change (finding 5, D-bx):* `LlmClient::embed`
  no longer fails for good when the model behind the name returns another
  length: the first live response sets the dimension, cached vectors of
  another length are re-embedded (one warning naming both lengths) and
  their cache rows replaced; cached vectors that differ among themselves
  with nothing else to send are all re-embedded (one warning).
  `cached_embeddings` (cache-only) still errors on mixed lengths, now with
  "cached embeddings differ in length (… ); did the embedding model … change?
  A run that may call the model re-embeds them". Responses of different
  lengths and empty vectors stay errors. Embedding keys are unchanged and
  the chat cache is never touched. *Warnings (finding 8):* symbols left
  without a vector because their embedding is not cached on a cache-only
  run (`--no-llm`, breaker) get one warning (`skipped=N`). *Cleanups
  (findings 6, 7):* the unreachable `Some(_) => embed_failed` arm is gone
  (an empty vector is rejected in the client, so every `Some` has the
  table's dimension); the `vector_fqns` test helper checks `sqlite_master`
  instead of hiding SQL errors with `.ok()?`. *Tests (finding 4; 276, 5
  ignored; +3):* a request failing from the 2nd batch on (71 symbols: 64
  embedded from the cache fallback and counted as cached, 7 failed; the
  next run sends only the 7); breaker tripped in the describe stage →
  cached embeddings only, a member whose description stays NULL gets no
  vector while a described sibling does, an uncached sibling is skipped
  with the warning; `index_meta` rows on vector runs, none without
  vectors, `true` with `parent_description`; dimension recovery with the
  fake `widen()` (re-embed stale, mixed cache, rewrite, `llm_cache`
  untouched); schema test checks `max_neighbors`. `with_warnings` moved to
  `llm::fake` so the llm tests can assert warnings. README, `plan.md` 5.1,
  `project.md` (`index_meta`) updated. *Real run (`argus`, release,
  `--offline`, warm cache):* 654 symbols, **654 embedded (654 from cache),
  0 embedding calls, 0 chat calls**, dim 1024; SQL (libSQL): 654 described,
  654 vectors of 4096 bytes, 0 described without a vector, `index_meta`
  `embedding_model=bge-m3:latest, embedding_dim=1024,
  embedding_parent_description=false`; **`index.db` 75.1 → 28.9 MB**,
  embeddings stage **0.53 s** (was 0.66 s), run **2.2 s** (was 2.5 s);
  three reruns identical (0 embedding, 0 chat calls).

- **5.1 Embeddings.** One commit. *Stage:* `index_embeddings` (7th stage,
  after types) reads every symbol with a non-NULL `description` (id order),
  builds its text with the pure `embeddings::embedding_text` (`fqn`, a
  newline, the description; D-br) and sends all texts to
  `LlmClient::embed` (embedding cache, misses only, batches of 64). The
  first vector's length is the dimension: `schema::create_vectors` then
  creates `symbol_vectors (symbol_id INTEGER PRIMARY KEY REFERENCES
  symbols(id), embedding F32_BLOB(<dim>) NOT NULL)` and
  `symbol_vectors_embedding` = `libsql_vector_idx(embedding,
  'metric=cosine', 'compress_neighbors=float8')` in the build transaction
  and inserts `vector32(?)` per symbol (D-bs, D-bt). *Failures (D-bu):* a
  cache-only client (`--no-llm`, or the breaker tripped earlier) uses
  `LlmClient::cached_embeddings` (new: cache only, `None` per miss) and
  counts misses as skipped; a failed `embed` warns once, falls back to the
  cache and counts the rest as failed; the run never fails on it; an index
  without vectors has no `symbol_vectors` table. *Model check:*
  `check_chat_model` → `check_models`: the embedding model is checked
  after the chat model (missing → run fails before any chat call; D-bv).
  *Config:* `[embedding] parent_description` (default `false`, the plan
  default) appends a method's/constructor's enclosing type's description;
  for 5.3 to compare (D-br). *Output:* `embeddings: N symbols, N embedded
  (N from cache), N failed, N skipped; N embedding calls, N texts sent,
  dim N` and `embed_*` fields in the info log; `IndexStats.embed_*`,
  `embedding_dim`, `embed_llm`. `--no-llm` help, README, `annatar.toml`,
  `plan.md` 5.1 and `project.md` (`symbol_vectors`) updated. `show` does
  not print vectors (not needed; 5.2 search is the reader). *Tests (273,
  5 ignored; +9):* libSQL `F32_BLOB(3)` + `libsql_vector_idx` +
  `vector32` + `vector_top_k` + dimension mismatch rejected (schema);
  `cached_embeddings` never calls the backend; embedding-model check;
  `[embedding]` config; `embedding_text` default/parent/blank parent;
  indexer: vectors for every described symbol, dim from the response,
  `vector_top_k` on the committed index, rerun 0 embedding calls;
  cache-only run + parent option; failed request keeps the run and leaves
  no table; missing embedding model fails before any chat. *Real run
  (`argus`, release, `--offline`, warm chat cache):* first run 654
  symbols, **654 embedded, 11 embedding calls, 654 texts, dim 1024**, 0
  chat calls; SQL: 654 symbols, 654 with a description, **0 described
  without a vector**, 654 blobs of 4096 bytes. Cold embedding (scratch
  copy, `embedding_cache` emptied): stage 8.6 s, run **10.1 s**; rerun
  **0 embedding calls** (654 from cache), stage 0.66 s, run **2.5 s**
  (4.6: 1.6 s); `--no-llm` 2.2 s, all 654 from cache. *Index size and
  speed (D-bt):* the first version (default index options, SQLite's 2 MB
  page cache) took 13.3 s for a fully cached stage and made `index.db`
  144 MB: DiskANN rewrites multi-KiB nodes on each insert and the open
  transaction spilled to the temp file on the container-mounted disk.
  `PRAGMA cache_size` 256 MiB on the build connection → 1.4 s;
  `compress_neighbors=float8` → 0.66 s and 75 MB. Recall of `vector_top_k`
  against exact `vector_distance_cos` (each of the 654 vectors as query,
  throwaway test): top-1 654/654 and recall@10 1.0 for default, float8,
  `max_neighbors=32`, float8 + `max_neighbors=16` (18 MB, 0.47 s);
  `float1bit` 0.9998.

- **PO decision D-bq: member reasons are optional.** A method without its
  own reason is good enough when its class has one. All 654 `argus`
  symbols have a description (checked on `data-real`); the reason counts
  in the 4.6 entries count only descriptions that also say why. No code
  rule for member reasons; Next and open #34 updated.

- **4.6 review fixes and PO feedback (keep reasons).** One commit. *PO
  feedback (verbatim), after seeing that "as short as possible" dropped
  `UserTokenService#removeAllTokens(List<UUID>)`'s emergency-logout reason
  from its own ticket:* "no reasons should not be droped. as short as
  possible without loosing information." *Prompt (D-bm amended):* members
  and types: "describe the {kind} as short as possible without losing
  information"; "When the code, Javadoc, tickets or commits below give a
  reason why this {kind} was created or changed (…), keep that reason:
  never drop it to make the description shorter. Leave out only reasons
  that are not about this {kind}: …" (the borrowed-reason caveats follow,
  now framed as what is *not* a reason); "Where a collective term carries
  the same information, use it instead of listing items one by one (…), but
  keep the details that set this {kind} apart, such as conditions, limits,
  roles and targets" replaces "in plain sentences" + "the domain objects it
  works on" (finding 3); the accessor/constructor clause became "rarely
  have a reason of their own: give them one only when the history explains
  this very {kind} or its field" (kept, because for them a reason is almost
  always the file's theme, i.e. borrowed); "Do not read a meaning or a
  reason into names alone"; no length limit is mentioned (none exists); the
  schema field docs say the same. A second wording ("a reason is its own
  when it explains what this {kind} itself does, such as its condition,
  limit, target or effect") was tried on `auth/authentication/token`
  (3 min 4 s): it moved borrowed reasons around (`isExpired` and
  `countByExpirationDateBefore` lost theirs, a field-storing constructor
  gained one) without a net gain, so it was dropped. *Validation (finding
  1, D-bo amended):* `summaries::source_meta_phrase` is gone; descriptions
  use `describe::description_meta_phrase`, a list of clearly meta phrases
  built from sources × verbs ("according to the ticket", "the ticket says",
  "the Javadoc does not specify", "the commits don't explain", "not stated
  in the commit history", "no reason is given", …) plus the bare
  "Not specified"/"Unknown"/"None" replies; "does not specify", "the change
  history", "the commit message", "the Javadoc settings", "this ticket",
  "the commit does not succeed", "the commit state" are domain text. The
  4.2 purpose list (`REASON_META_PHRASES`) is unchanged: a purpose is a
  reason, not a behaviour, and the summary field only rejects openers (0
  invalid summaries in every real run). *Safety cap (finding 7, D-bo):*
  `ollama.max_tokens` (default 1024, `0` = not sent) goes into every chat
  request; a reply with `finish_reason=length` is now invalid even when it
  parses (retried once with "the reply was cut off before it finished",
  never cached). Probed on Ollama 0.35.1: `max_tokens: 5` → `length` after
  5 completion tokens; with `reasoning_effort: "none"` no reasoning is
  produced, with `"low"` the reasoning counts against the cap (documented).
  Not part of the cache key (judgement call, D-bo): only complete replies
  are cached and a cap does not change them, so keying it would only
  re-ask the 69 cached ticket summaries. *Token peaks (finding 8, D-bn
  corrected):* `LlmStats` has `peak_prompt_tokens` / `peak_completion_tokens`
  (reset per stage, `LlmClient::reset_peaks`), printed as "max N prompt /
  M completion tokens" on the `summaries:`/`describe:`/`types:` lines and
  in the info log; D-bn's "no truncation warning" was wrong (annatar has no
  input-truncation check) and now reads "longest prompt 1425 tokens". *Merge
  filter (finding 6, D-bn):* subjects starting with "Merge " **or "Merged "**
  (Bitbucket "Merged in …", "Merged master into …") are left out; the
  parent count (`%P`) was not used: `git log -L` on `argus` shows only 5
  merge commits among 272 distinct commits, all "Merge …", and `%P` would
  need a new history cache table, a `symbol_commits` column and a cold
  history run. *Tests (findings 4, 5):* `selection_does_not_depend_on_input_order`
  reverses the commits too (ties and a UTC offset); the `build_index` test
  `commit_subjects_without_an_available_ticket` checks the type prompt as
  well; the eight sentences from the review (and three more) must pass the
  description validator for members and types; new llm tests for
  `max_tokens` (sent, `0` not sent, not in the key) and a parseable
  `length` reply being invalid, plus the per-stage peaks. *NITs:* "what/why"
  wording in `golden.rs` and the golden fixture, the llm test struct uses
  neutral `text`/`note` (`ClassSummary.description`), long doc lines
  rewrapped. 264 tests pass (5 ignored). *Real runs (`argus`, release,
  `data-real`):* `--path auth/authentication/token`: 3 min 10 s, 113 calls,
  0 retries; full run **19 min 21 s** (4.6: 15 min 29 s), 654 chat calls
  (479 members, 175 types, 69 summaries from cache), **0 invalid / failed
  / incomplete, 0 retries, no warnings**; peak prompt 1209 (members) /
  1425 (types) tokens, peak completion 71 / 84 tokens (the 1024 cap is
  12× above); rerun **0 chat calls**, 1.6 s. *Reasons:* members with a
  reason clause (regex as in 4.6: "to enable/support/prevent …", "so that",
  "ensuring", …) 34 → 62 of 479 (broader regex incl. "-ing" forms 64 →
  108), types 47 → 74 of 175; in `auth/authentication/token`, hand-counted,
  members with a reason 11 → 19 (≈ 15 own: `removeAllTokens(List<UUID>)`
  "… to support emergency bulk logout", `isTokenValid` "… administrators
  who are not registered in Argus", `preValidateJwt` "preventing excessive
  log entries …"; 4 borrowed: `UserToken#getExpirationDate`/`#isExpired`,
  `UserTokenEntryRepository#deleteByUserIdAndTokenId`/`#countByExpirationDateBefore`
  get the memory/expiry theme). *Lengths (chars):* members min 25, p10 51,
  **median 126**, p90 224, **max 334** (mean 132; 4.6: 105 / 193 / 489);
  types p10 97, **median 165**, p90 259, max 362 (4.6: 147 / 206 / 279);
  members 476 × 1 sentence, 3 × 2; types all 1. The longest member,
  `AuthDataService#getAuthData(UUID)`, went 489 → 303 chars ("…
  organization-specific details (including contract status, feature sets,
  and roles) …") but lost "empty Optional when the user is not found or an
  HTTP client error occurs" (information lost). No description contains
  "ticket", "commit", "javadoc", "GRLD-", "this method/class", "likely",
  "unclear", "not specified", "mobile" or "proof of concept". *Accessor and
  constructor rule (finding 2):* trivial accessors (one statement) with a
  reason clause 11 → 22 of 176, field-storing constructors 5 → 12 of 44
  (≈ 85 % followed, as in 4.5); borrowed examples:
  `IdentificationDataEventDispatcher#<init>` (the propagation feature),
  `PactumConnector#<init>`, `OAuth2RequestCache#<init>`, the four
  `Entry` flag accessors ("to ensure purchase actions in ReadOnly mode are
  correctly propagated"), `IdentificationDataEventDispatcherConfig`
  getters; some are the symbol's own (`UserDataCreatedMessage#<init>`
  "enabling the publication of user data creation events",
  `SubscriptionDataService#getGrldAdminOrganizationSubscription`). The
  empty class `IdentificationDataRemoveMessage` reads "Represents a message
  payload for removing identification data to propagate identification
  method changes and update user-specific caches across lexoffice web
  servers." — the reason is the creating ticket's, which created this class
  (own, acceptable). *Spot check (15 symbols, incl. the 4.6 ones):*
  correct 6 (`removeAllTokens(List<UUID>)` reason regained,
  `SubscriptionDataService#getSubscriptionFromPactum` "… to ensure data
  accuracy and prevent processing failures for cancelled contracts"
  regained, `ArgusErrorController#handleError`, `ArgusJacksonConfig`,
  `AuthDataCacheRest#delete`, `DatabaseCacheUserTokenStrategy#removeToken`);
  vague 4 (`AbstractCache#supply` initiative theme as before,
  `UserDataMessagePublisher` "to support user action messaging",
  `AuthDataService#getAuthData` lost the empty-Optional case,
  `RoleRelationsUpdatedMessageConsumer#perform` no longer names the cache
  update); borrowed 4 (`TokenType` "for JWT functionality extensions" — the
  initiative's name; `Privilege` and `Privilege.User` "supporting Spring
  Boot 3 and Jakarta security annotation compatibility" — an upgrade;
  `AuthDataCacheRest` the person-list reason as in 4.6; `Privilege.User`
  is otherwise better: "static marker"); wrong 1
  (`OAuth2RequestCacheService` "to enable estimation of storage usage by
  determining object size" — the 4.5 wrong why is back). *Golden set:* the
  ignored real check passes (24 questions, 56 fqns, 0 problems).

- **4.6 Symbol descriptions (product-owner redesign).** One commit. *PO
  request (verbatim):* "Move from the pure what / why to a simple general
  symbol description with the code itself the ticket and the commit history
  as context. There should be no length limit only the instruction to
  describe it as short as possible." Recorded as a new step after 4.5 (plan
  4.6); 4.3/4.4 are marked superseded, their selection, hold-back and
  caching rules unchanged (D-bm). *Code:* `describe::Description` and
  `TypeDescription` now have one field `description` (`deny_unknown_fields`,
  own schemas, so every member and type missed the LLM cache once);
  `symbols.what`/`why` → `symbols.description`; `History` is a struct
  (`tickets` first-then-recent, `commits`) and the newest
  `describe.commit_subjects` (5) distinct non-merge subjects are **always**
  sent, not only without an available ticket; prompt sections `Tickets:` /
  `Commits:` (each `(none)` when empty); no raw Jira title next to the 4.2
  summary (D-bn). Prompts: "describe the {kind} as short as possible, in
  plain sentences", what it does concretely, and a reason only when the
  tickets or commits say why this symbol was created or changed — none when
  it fits every member, names only a project/initiative/upgrade, or for
  accessors/`equals`/field-storing constructors (D-bj guidance kept); types
  sum up their members' descriptions (listed in full, "(with their
  descriptions)"); no length limit anywhere. Validation (D-bo): non-blank,
  control characters, ticket key (D-bh), "This method/…/This class…"
  openers, source/missing-information phrases (`summaries::source_meta_phrase`:
  D-bb's list without "the commit", plus "commit history") and bare "Not
  specified"/"Unknown"; the length caps, the why-restates-what check and
  the why-only "this method"/"this class is" checks are gone. `show` prints
  `- description:` for the symbol and every child (full text, D-bp); stats
  lines and counters unchanged; `ChildWhat` → `ChildDescription`. Docs:
  plan (question 1, 4.3–4.6, 5.1 embeds `fqn + description`, 5.2 output),
  project.md, README, `annatar.toml`, note in `quality_review_4_5.md`.
  Tests: prompts carry code + tickets + commits (unit and `build_index`),
  no length cap, one-field schemas, whitespace collapsing, "after the
  commit" is domain text, type prompts list member descriptions, `show`
  output, rerun 0 chat calls; 262 tests pass (5 ignored).
  *Prompt iteration (`--path`):* `messaging/consume/pactum` (47 symbols,
  1 min 14 s, 1.57 s/symbol): descriptions short and correct but almost no
  reasons; `auth/authentication/token` (113 symbols, 3 min 2 s): the first
  wording ("add why only when … explain it for this method itself" plus the
  caveats) dropped even own-ticket reasons; reworded to "when the tickets or
  commits say why this {kind} was created or changed, add that reason
  briefly … leave it out when …" (3 min 8 s): a few reasons came back
  (`TokenValidationRest#deleteAll`, `UserTokenService#removeAllTokens(UUID)`:
  logout / password reset / email change), most members still describe
  behaviour only. *Full run (`argus`, release, `data-real`):* run 1:
  654 symbols, 69 tickets (0 Jira requests, 69 summaries from cache),
  479 members (386 chat calls, 93 hits from the `--path` run), 175 types
  (155 calls, 20 hits), **0 invalid/failed/incomplete, 0 retries**, no
  warnings, **15 min 29 s** (≈ 1.72 s per chat call; 4.5's what/why run
  took 18 min 28 s); run 2: **0 chat calls**, 1.8 s. *Lengths (chars):*
  members min 26, p10 50, **median 105**, p90 193, max 489 (mean 118);
  types min 36, p10 86, **median 147**, p90 206, max 279; sentences:
  members 471 × 1, 7 × 2, 1 × 3, types 175 × 1 (old what+why together:
  median 180, max 308). A rough reason-clause count ("to enable/support/
  prevent …", "so that") finds 33 members and 46 types. No description
  contains "ticket", "commit", "javadoc", "GRLD-", "this method/class",
  "likely", "unclear", "not specified" or "mobile". *Spot check (20 symbols
  of the 4.5 sample, against the source):* 15 correct, 4 vague
  (`AbstractCache#supply` appends the caching initiative's theme;
  `TokenType` "Defines token types for JWT authentication" no longer names
  the constants; `Privilege.User` "Represents a user within the privilege
  domain" (was the more precise "marker type"); `AuthDataCacheRest` appends
  a person-list reason borrowed from a recent ticket), 1 vague with an
  invented domain (`Privilege` "… user representations for the
  authentication domain"), **0 wrong** (the three wrong 4.5 whys are gone:
  `AuthDataCacheRest#delete` now names its `@RolesAllowed` role,
  `RoleRelationsUpdatedMessageConsumer#perform` now names the cache update,
  `OAuth2RequestCacheService` has no object-size reason). Reasons kept where
  supported (`ArgusErrorController#handleError` "… prevent framework
  identification", `ArgusJacksonConfig` "for Spring Boot 4"); own-ticket
  reasons lost (`UserTokenService#removeAllTokens(List<UUID>)` emergency
  logout, `SubscriptionDataService#getSubscriptionFromPactum` data accuracy,
  `UserDataMessagePublisher`'s create/update/delete topic reason is now
  part of its behaviour). Examples: `ArgusErrorController#handleError()` →
  "Returns null for any HTTP request to the /error endpoint to suppress the
  default Spring Boot error page and prevent framework identification.";
  `TokenValidationRest#deleteAllFromList(…)` → "Deletes all user tokens for
  a specified list of user IDs, enforcing a maximum count limit to prevent
  excessive invalidation requests."; the longest member,
  `AuthDataService#getAuthData(UUID)` (489 chars, 3 sentences), lists every
  field it aggregates. *Golden set:* the ignored real check passes on the
  new `data-real` index (all 56 fqns present, 0 problems).

- **4.5 review fixes (findings 1–13).** One commit, no prompt change (no
  re-describe). *Review honesty (`quality_review_4_5.md`, D-bj, D-bk):* new
  headline section leading with strictly correct / wrong / vague / empty
  whys (correct 32 % → 35 %, wrong 12 % → 9 %, vague 56 % → 21 %, empty
  0 → 35 %): the 32 % → 71 % "acceptable" gain is mostly vague turned empty.
  Tuning vs held-out split: the 15 symbols under the two `--path` prefixes
  went 33 % → 80 % acceptable, the 19 held-out ones 32 % → 63 %. #6's
  before-verdict W → V (before-wrong 5 → 4, misleading 6 → 5). Index-wide
  section: 240 member + 35 type whys became empty (none gained); 195 of the
  240 are accessors/constructors/`equals`…; of the 45 others ≈ 5–8 lost a
  reasonably supported reason (`DynamoDBEventService#publishAsync`,
  `DynamoDBEventTableService#init`/`#tableExists`,
  `UserTokenService#removeToken`, `OAuth2RequestCacheService#find`,
  `CronusConnector#createSystemJwt`). Rule compliance: 13/120 trivial
  accessors and 12/44 field-storing constructors still have a why (≈ 85 %
  followed; plus 12 constant-returning `getCachePrefix`/`getCacheTTLSeconds`
  overrides); no reviewed verdict changes (none of the 12 correct whys is on
  such a symbol), the iteration note's kept flag-accessor whys are now
  counted as non-compliance, not correct. #3's cause: D-bk emptied
  GRLD-40363's purpose (description only a chat link); 10 purposes became
  empty in total. The "specific enough" instruction had little effect on the
  per-initiative theme (one why verbatim on 33 symbols, 54 with variants).
  Suspected prompt edge cases recorded (open #35). *Confidentiality (D-bl
  amended, open #34):* package-relative fqns in md docs stay; the real golden
  set stays outside git and its single copy is flagged for the product
  owner; no full package prefix in the current tree, one left in history
  (`abc05c7`, a Jira fixture scrubbed in `9b44532`; rewrite is the product
  owner's call, open #34).
  *Golden set (`.annatar-local/golden-argus.toml`, not in git):* Q2 +
  `TokenStoreStrategy#removeAllTokensForUserIdExceptOfGivenJwtIds(UUID,List<UUID>)`,
  Q5 + `DatabaseCacheUserTokenSchedulerService#deleteExpiredTokensInternal()`,
  Q18 + `RoleGreenlandAdmin#getPrivileges(AuthProperties)`; Q17 reworded
  without the ticket's example phrase and with the per-organisation
  `PersonOrganizationRelationCache` as primary (the deprecated per-user cache
  is the alternate); Q12 noted as easy; 4 new questions whose primaries are
  an interface (`TokenStoreStrategy`), a repository interface
  (`UserTokenEntryRepository`), a nested enum
  (`ContractMessage.Contract.LifecycleStatus`) and a non-trivial constructor
  (`DynamoDBEvent#<init>(String,String,List<String>)`); `argus` has no
  records. Now 24 questions (13 method, 7 class, 2 interface, 1 enum,
  1 constructor primaries), 56 fqns, 4 marked hard, 1 easy; the ignored
  real check passes on `data-real` (0 problems). *Code (`golden`):* fqns
  may not contain whitespace (the index strips it, `symbols::normalize`);
  `GoldenSet`/`Question` are no longer `Deserialize` and have private fields
  with accessors (private raw structs are validated into them, so
  `primary()` cannot panic and `kind()` is a `SymbolKind`); kinds come from
  the new `SymbolKind::ALL` / `SymbolKind::parse` (a test with an exhaustive
  `match` keeps `ALL` complete); "an interface"/"an enum" in messages; the
  ignored test requires `ANNATAR_GOLDEN_SET` (no argus default path); README
  documents the golden set and both env vars. 262 tests pass (5 ignored).

- **4.5 Quality review and golden set.** Two commits. *Review
  ([`quality_review_4_5.md`](./quality_review_4_5.md)):* 34 symbols (20
  members: services, controllers, a repository query, strategy code,
  accessors, constructors, a scheduled method, a config bean, 1–11 tickets or
  commit subjects only; 14 types: enum, nested class/enum, empty marker,
  exception, entity, Lombok DTO, configuration, annotation, controller,
  service, event) and 13 ticket summaries, judged against source and ticket
  text as correct / empty / vague / wrong / invented, before and after.
  Before: `what` 32/34 correct (1 vague, 1 invented: `TokenType`
  "mobile"); `why` 11 correct, 19 vague (project themes on accessors, file
  themes on nested types), 4 wrong (borrowed from an unrelated ticket;
  corrected from 18/5 in the review fixes); 4 of
  13 purposes restated a title. *Prompt changes (`feat(describe)`, D-bj,
  D-bk):* member and type `why` only when the history explains this symbol
  itself (whole-file changes and project-only histories give no reason;
  always empty for getters, setters, `equals`, `hashCode`, `toString` and
  field-storing constructors; nested types do not inherit their file's
  reason); type `what` does not expand abbreviations or name parts; ticket
  purpose empty for title-only / link-only tickets unless the title names a
  goal beyond the change. Iterated with `--path` on
  `auth/authentication/token` (93 members, 20 types; first wording emptied
  only 13/93 member whys, final 49/93 + 7/20 types; 4 min 38 s / 3 min
  33 s) and `messaging/consume/pactum` (33 members, 14 types incl. three
  nesting levels; 1 min 41 s). *Full run (`argus`, release, `data-real`):*
  run 1: 69 tickets (0 Jira requests), 40 summary calls (29 hits from the
  `--path` runs), 479 members described (353 calls, 126 hits), 175 types
  (141 calls, 34 hits), 0 invalid/failed/incomplete, 0 retries, 18 min
  28 s; run 2: **0 chat calls**, 1.6 s. Index-wide: member whys empty
  242/479 (197 of them accessors/constructors/`equals`…), type whys 35/175,
  purposes 35/69 (30 of the 31 tickets without description); `what` median
  96, max 159 chars; no meta text, no "mobile". After: `what` 33/34
  correct; `why` 12 correct (35 %, was 32 %), 12 empty, 7 vague, 3 wrong
  (9 %, was 12 % after the review-fix correction); acceptable (C + E) 71 %
  overall, but 80 % on the 15 tuning-prefix symbols and **63 % on the 19
  held out** (see the review-fixes entry above); purposes acceptable 12/13
  (was 8/13). No
  reviewed correct why became empty; one became vague and one vague one
  wrong (a recent ticket lending its reason). Question 1: `what` is
  trustworthy; `why` is trustworthy when present on a member with its own
  ticket, otherwise context. *Golden set (`test(golden)`, D-bl):* new
  `golden` module: `GoldenSet` / `Question` (TOML `[[question]] text,
  expect = [fqn, alternates…], kind?, note?`, `deny_unknown_fields`),
  `GoldenSet::parse` / `load` validate (non-empty set, non-blank texts
  distinct ignoring case and whitespace, non-empty `expect`, fqn shape for
  types / members / constructors with parameter types as written, no
  repeated fqn, known `kind` that fits the first fqn); `check_index(conn,
  set)` lists `IndexProblem::Missing` / `WrongKind`. Synthetic fixture
  `tests/fixtures/golden/sample.toml` (5 questions on the sample project);
  integration test `tests/golden.rs` indexes the sample project and finds
  every expected fqn; `#[ignore]` `golden::tests::real_golden_set_matches_the_real_index`
  reads `ANNATAR_GOLDEN_SET` (default `.annatar-local/golden-argus.toml`)
  and `ANNATAR_GOLDEN_DATA_DIR`. The real `argus` set (20 questions, 13
  method / 7 type primaries, 44 fqns incl. alternates, 3 marked hard — this
  entry first said 4) is in
  `.annatar-local/` (now in `.gitignore`); the ignored test passed on the
  `data-real` index before and after the prompt change (0 problems).
  Questions were written from code and tickets; content-word overlap with the
  expected symbol's new what/why is 0–3 words (median 1). 7 new `golden`
  unit tests + 1 integration test, prompt tests extended; 261 tests pass (5
  ignored).

- **4.4 review fixes (findings 1–14).** Two commits. *Walker (`fix(walk)`,
  D-bd amended, plan 1.1):* `is_test` takes the source set from the first
  `src/<set>/java` triple (else the first `src/<set>` pair), so a module
  directory named `java` no longer hides `java/src/test/java`; Gradle-style
  sets count as tests (`test`, `testFixtures`/`test-fixtures`,
  `integrationTest`, `androidTest`, `…_test`), `testing`/`testng`/`contest`
  and `src/test` inside `src/main/java/…` stay production; tests for all
  listed paths and `--path` interplay. *Describe (`fix(describe)`):*
  (1) the old "unfetched ticket" type test passed through its incomplete
  member; the new `only_the_types_own_chosen_ticket_holds_it_back` has a
  member described from GRLD-1 while the type chose the unfetched GRLD-2,
  asserts `types_incomplete`, 0 type calls and the type-stage warning
  (`incomplete=1 waiting_for_members=0 blocking_tickets="GRLD-2"`, captured
  with a test-only `fmt` subscriber), then a run with GRLD-2 describes it.
  (4) any LLM field naming a ticket key (configured `ticket_regex`) is
  invalid — summaries, purposes, member and type what/why (D-bh;
  `DescribeInputs` carries the regex). (5) `describe::cut_range`: cuts start
  at attached comments / line start and swallow a trailing `//` comment;
  `Symbol::blocks` (enum constant bodies, initializers) collapse to `{ … }`
  (D-be amended). (6) type why check: openers and "this <kind>
  exists/is/was" only (D-bi). (7) type prompt: no meaning read into names
  alone (D-bi, open #32). (8) plan 4.4 / D-bg cache wording. (9) open #33
  (breaker vs a deterministic failure; Ollama probed: oversized prompts are
  truncated and answered 200, no 400/413). (10) open #31 extended. (11)
  compact constructors listed as `compact constructor Point`. (12) ranking
  rule recorded in D-bf. (13) heading "Methods, constructors and nested
  types" / "…: (none)". (14) `build_index` tests: interface with implicitly
  public members + compact constructor + cap, invalid nested type listed
  bare, a failed type call tripping the breaker (`types_failed` 1,
  `types_incomplete` 2 for the outer chain, `types_skipped` 1, warning
  `waiting_for_members=2`; next run catches up). 254 tests pass (4 ignored).
  *Real runs (`argus`, release, `data-real`):* run 1: 162 files, 654
  symbols, 69 tickets (0 Jira requests), summaries 69 from cache; members
  477 from cache + 2 re-asked whose cached `why` named GRLD-24681 (4 chat
  calls, 2 retries: the model repeated the key once, the retry fixed it);
  **types 175/175 described, 175 chat calls, 0 retries, 0
  invalid/failed/incomplete**; whole run 6 min 48 s. Run 2: **0 chat
  calls** (69 + 479 + 175 cache hits), 1.6 s. Content: type `what` median
  111, max 151 chars; 0 `NULL` whys; no "GRLD-", "ticket", "commit",
  "javadoc", "likely", "unclear", "this class" anywhere. `DynamoDBEvent`
  unchanged and correct; `ContractMessage` → "Extends MessageBearer to
  encapsulate a contract payload containing customer organization ID and
  lifecycle status."; `AuthDataEvent` → "Defines the structure for
  authentication data events and converts them into web response formats";
  `AuthDataEvent.WebListResponse` why no longer names GRLD-24681;
  `Privilege.User` → "Defines a static marker type for user privileges …"
  (was an invented "user entity"); `TokenType` still "mobile-specific"
  (open #32).

- **4.4 Type what/why, bottom-up.** Two commits. *Walker (separate commit
  `fix(walk)`, D-bd):* a `src` directory directly followed by `test` before
  the first `java` component is test code, so `argus`'s `backend/src/test`
  (55 files) is no longer indexed or described (open #13 closed). *Types:*
  new sixth `build_index` stage `index_type_descriptions` after describe;
  `index_descriptions` now returns what each member got this run
  (`describe::ChildWhat`: `Described(what)` / `Invalid` / `Missing`). Types
  are taken bottom-up (deepest nesting first, then walk/source order), each
  result feeding its outer type. New in `describe`: `TypeDescription` (own
  response schema, same fields), `TypeContext`, pure `outline(source, span,
  cuts)` (the type's declaration with members and nested types cut out from
  their Javadoc line: annotations, header, fields, enum constants, comments;
  blank lines dropped, dedented), `select_children` (public first, then the
  rest, each in source order, capped at `describe.type_members`, listed in
  source order, the rest counted as "(N more not listed)"), `is_public`
  (explicit `public`, or not `private` inside an interface/annotation),
  `type_prompt` (type kind + fqn + Spring role, `Nested in:`, Javadoc,
  declaration capped at `body_chars` — `Signature:` only without it —
  children as `- method find(Long): what`, `- constructor Orders(Repo): …`,
  `- class Line: …`, invalid ones `(not described)`, then the history),
  `validate_type_description` ("This class / interface / enum / record /
  annotation / type" openers and mentions, shared reason meta checks). The
  type's history is chosen by `select_history` with
  `describe.type_recent_tickets` (default 3), with the same blocking and
  title-fallback rules (D-az, D-ba). A listed child without a `what` this run
  holds the type back (`types_incomplete`, which in turn holds back its outer
  type); an invalid child is listed bare (D-bg). New config
  `[describe] type_members` (30) and `type_recent_tickets` (3); `IndexStats`
  `type_symbols`, `types_described`, `types_described_cached`,
  `types_invalid`, `types_failed`, `types_incomplete`, `types_skipped`,
  `type_llm`; a new `types:` output line. `show` already printed what/why for
  every symbol, so types and the children listing show them unchanged.
  Member prompts and the member validation messages are unchanged (the
  Javadoc and history blocks moved into shared helpers; the member cache
  still hits: 479/479 on `argus`). 13 new tests (7 `describe`: child order,
  cap and omitted count, holding back vs bare invalid, `is_public`, outline
  cuts and clamping, exact type prompt with nested/role/children/history,
  prompt without outline + cap + fencing, type validation; 5 indexer through
  `build_index`: bottom-up order with nested `what`s in the outer prompt,
  exact outer prompt, rerun 0 chat calls and `show` for the outer, nested
  and doubly nested type; cap with public first; invalid member listed bare
  then, under `--no-llm`, the missing member holding back two levels; an
  unfetched chosen ticket holds the type back; no summarizer → skipped; the
  `--path` test now also proves type prompts are run-independent; 1 walker
  test, 1 config extension); 244 tests pass (4 ignored).
  *Real runs (`argus`, release, `data-real`, tickets/summaries/members
  cached):* `--path …/messaging/consume/pactum` (9 files, 14 types incl.
  `ContractMessage.Contract.Customer`, three levels): 14 described, 0
  invalid, 33.3 s (**2.38 s per type**). Full run: 162 files (was 217), 654
  symbols, 69 tickets (0 Jira requests), 479 members from the cache, **175
  types, 175 described** (14 from the `--path` run's cache), 0
  invalid/failed/incomplete/skipped, 161 chat calls, 0 retries, stage 372 s
  (2.31 s per call), whole run 6 min 13 s. Run 2: **0 chat calls** (69 +
  479 + 175 cache hits), 2.7 s. Content: `what` median 110, max 156 chars;
  0 `NULL` whys; no "ticket", "commit", "javadoc", "likely", "unclear",
  "this class"; openers "Stores" 52, "Represents" 23, "Consumes" 12,
  "Defines" 11. Large class `auth.event.dynamoDB.DynamoDBEvent` (16
  members): "Stores authentication event data with DynamoDB annotations for
  partitioning, indexing, and TTL management." — correct (partition/sort
  keys, two indexes, TTL). Nested `messaging.consume.pactum.ContractMessage`
  → `.Contract` ("Stores customer organization ID and lifecycle status")
  → `.Contract.Customer` ("Stores the organizationId") and
  `.Contract.LifecycleStatus` (lists the seven states) — all correct, and
  the outer `what` sums up its nested types. Spot checks against the code:
  `UserTokenService` (adds/removes/validates via a strategy, publishes
  invalidation events, admin exception) correct; `TokenValidationRest`
  (save/retrieve/invalidate, bulk delete with a count limit) correct;
  `AuthDataService` (aggregates cached roles, security and organization data
  into `AuthData`, cache invalidation events) correct;
  `SubscriptionDataService.UnprocessableContractException` correct `what`,
  borrowed `why` (redirect loop ticket of the outer file); `Privilege.User`
  is an empty class but got "Represents a user entity … for access
  control" (invented; open #32), `Privilege.GreenlandAdmin` correct.
  Weak spots: nested types share the file's tickets, so their whys repeat
  the outer type's (all four `ContractMessage` types say "Core-Split"; 6+5
  types share the same "parallel caching structure" why) (open #30).

- **4.3 review fixes: chosen tickets only, permanent fetch failures, title
  fallback, narrower meta check.** Two commits. *Regex (low, separate
  commit `fix(history)`):* the default `ticket_regex` `\bGRLD-\d+\b` found
  no key in branch-style subjects (`GRLD-98595_Fix_…`, 59 subjects in
  `argus`) because `\b` does not match between a digit and `_`; now
  `\bGRLD-\d+` (greedy digits keep keys exact), `annatar.toml`, plan 0.1,
  new history test (`_`, `-`, `(…)`, `GRLD-123` vs `GRLD-1234`, `XGRLD-1`)
  and a config test that the shipped `annatar.toml` uses the default (D-ay).
  *Blocking tickets (medium):* `select_history` lets tickets not fetched yet
  compete like available ones; only a *chosen* unfetched or unsummarised
  ticket makes the member incomplete, `Incomplete` lists the blocking keys
  and the stage warning names them (sorted, ≤ 10, `+N more`). Permanent
  fetch failures (`FetchError::is_permanent`: 403/404, other 4xx except
  408/425/429, unparseable issue JSON, invalid key) are cached unavailable
  (`put_unavailable(key, Option<u16>)`); a non-JSON 2xx is the new transient
  `FetchError::NotJson`. `TicketFetch::from_config` returns `JiraMode`
  (`Fetch` / `Offline` / `Disabled`) and `build_index` takes `&JiraMode`;
  without `[jira]` or a token the describe stage counts uncached tickets as
  unavailable (commit-subject fallback), `--offline` keeps them pending
  (D-az, D-ac amended). *Invalid summaries (medium):* `index_summaries`
  returns the keys whose summary was invalid (also listed in its warning);
  such a chosen ticket enters the member prompt with its Jira title
  (`describe::Summary::Title`, no `Reason:`); a summary missing because the
  breaker tripped or `--no-llm` still holds the member back (D-ba). *Meta
  check (medium):* `what` rejects only the openers "This method /
  constructor / function"; `why` and the 4.2 `purpose` share the narrowed
  `REASON_META_PHRASES` (plus "this method…" for `why`) and reject bare "Not
  specified." / "Unknown" replies; the 4.2 `summary` rejects only "This
  ticket / The ticket" openers; the reviewer's examples ("…when the size is
  not specified.", "…if one is not provided.", "Returns the ticket price…",
  "…revoked for no reason.") pass (D-bb). *Low/nits:* merge subjects
  dropped from the commit fallback; `LlmClient::reset_invalid_streak` at
  the start of each LLM stage; the source excerpt replaces the `Signature:`
  line (kept for `body_chars = 0` or an empty source; an abstract method's
  declaration is sent once) (D-bc); `utc_seconds` no longer slices bytes
  (a multibyte zone returns `None`); the "Days from civil" comment folded
  into the doc comment; indexer module doc rewrapped; schema comment
  rewrapped. D-au notes that a 4.2 summary prompt change re-describes every
  member (~20 min on `argus`). 11 new tests (2 in the regex commit; 1 `llm`
  streak reset; 1 `summaries` domain text; 3 `describe`: title fallback,
  abstract method declared once, dedent with tabs — plus the chosen-only
  test replacing the old incomplete test, merges, multibyte zone, more
  validation cases; 4 indexer: permanent failures cached and never asked
  again while 500/408 are retried, summaries breaker then describe serves
  the cached member / holds back the one choosing the unsummarised ticket /
  skips the changed one with 0 describe calls, invalid streak does not
  carry into describe, `Disabled` Jira → commit subjects; the end-to-end
  prompt is asserted exactly, proving the dedent; the invalid-summary test
  now proves the title fallback); 231 tests pass (4 ignored).
  *Real run (`argus`, release, `data-real`, tickets/summaries/describe
  cached from 4.3):* run 1: 81 keys (2 new from the regex: GRLD-87062,
  GRLD-98595), 79 cached, 2 fetched, 0 unavailable/failed, 3 Jira requests;
  81 summarised (2 chat calls, 79 hits, 0 retries); describe 729 members,
  **729 described** (0 from cache — the prompt changed for every member),
  0 invalid/failed/incomplete/skipped, 729 chat calls, 0 retries, stage
  1362 s (1.87 s per call), whole run 22 min 55 s. Run 2: **0 Jira
  requests, 0 chat calls** (81 + 729 cache hits), 2.0 s. Content: `what`
  median 100, max 152 chars; 2 `NULL` whys; no "ticket", "commit",
  "javadoc", "likely", "unclear", "not stated", "this method" in any
  what/why; 386/729 `what`s identical to before; members linked to an
  available ticket 897 (was 888). Spot check: members of the newly found
  GRLD-98595 (`UserOrganizationService#getOrganizationUserIds`) now carry
  its "AuthData Contract cache was not renewed" why.

- **PO decision: CLI only, no MCP (D-ax).** Phase 6 removed from
  `plan.md`; the agent trial is now step 5.4 and runs through `annatar
  search` / `annatar show`; question 3, the ground rules, 5.2 (agent-ready
  output, logs on stderr), Later, `project.md` (interface table of CLI
  commands) and `AGENTS.md` no longer mention MCP. The `serve` stub
  subcommand is gone from `src/main.rs`. Open #5, #6 and #29 updated.

- **4.3 Method and constructor what/why.** New `describe` module:
  `Description { what, why }` (`deny_unknown_fields`); pure
  `select_history(tickets, commits, &DescribeConfig)` — the first ticket
  (oldest `first_date`) plus the `recent_tickets` (default 3) others with the
  newest `last_date`, dates compared in UTC (small `%aI` parser), ties by key;
  unavailable tickets left out; no available ticket → up to `commit_subjects`
  (default 5) distinct commit subjects, newest first (ties by subject); neither
  → code only; a chosen ticket without a summary or any never-fetched ticket
  → `Incomplete` (D-au); pure `member_prompt(&Member, &History, body_chars)`:
  kind, enclosing type (kind, fqn, Spring role), signature, Javadoc (capped
  2000), the member's source dedented and capped at `body_chars` (default
  1500, D-at), then the history (`Created for KEY (Type): summary` / `Changed
  for …`, `Reason:` = ticket purpose when not `NULL`), fenced between
  `<<<CONTEXT` / `CONTEXT>>>`; asks for a one-line verb-first `what` (≤ 150)
  and a `why` from the history only, empty when it gives none (D-av);
  `validate_description`: blank `what`, > 200 / > 300 chars, control chars,
  meta phrases ("this method", "the ticket", "javadoc", "not stated", …), a
  `why` that restates `what` (reuses the 4.2 word check). New fifth
  `build_index` stage `index_descriptions` after summaries (also on a non-git
  repository, code only): members in walk/source order, sequential
  `complete_with` (D-ap), writes the new index columns `symbols.what` /
  `symbols.why` (empty `why` → `NULL`); breaker, `--no-llm` and no `[ollama]`
  as in 4.2 (shared client). `IndexedFile` keeps the file source for the
  excerpt. New `[describe]` config section (`recent_tickets`,
  `commit_subjects`, `body_chars`; `deny_unknown_fields`, every key optional),
  carried on `Summarizer` (`with_describe`). `IndexStats`: `describe_members`,
  `described`, `described_cached`, `describe_invalid`, `describe_failed`,
  `describe_incomplete`, `describe_skipped` (sum = members) and per-stage
  `summary_llm` / `describe_llm` next to the run total `llm`; the `summaries:`
  line now prints its own stage's chat counts, a new `describe:` line follows
  (D-aw). `show` prints `- what:` / `- why:` after the signature of every
  symbol that has them (children included). Test fake: `FakeBackend::always`
  (standing reply per response type). 22 new tests (13 `describe`: first +
  recent selection and cap, UTC/tie order, input-order independence,
  unavailable omitted, commit fallback (dedup/cap/blank), incomplete, prompt
  layout with and without source/Javadoc/history, fencing, Javadoc cap,
  validation, restated why + whitespace, schema; 8 indexer through
  `build_index`: what/why from ticket summaries + rerun 0 chat calls + `show`,
  unsummarised ticket waits (no call) then described with `NULL` why,
  unfetched ticket incomplete, commit fallback prompt, non-git code only,
  `--path` run hits the full run's cache, no summarizer / `--no-llm` skipped,
  invalid + failed + breaker; 1 config); 220 tests pass (4 ignored).
  *Real runs (`argus`, release, `data-real`, tickets and summaries cached):*
  `--path …/auth/authentication/token` (20 files, 93 members): without source
  (`body_chars = 0`) 93 described, 0 invalid, 190.7 s (2.05 s/member, first
  call 6.5 s warm-up), prompts 365–600 tokens; with source (1500) 173.6 s
  (1.87 s/member), prompts ≤ 813 tokens. Comparison on the 41 service,
  REST and strategy members: without the source `what` is a paraphrase of
  name + signature and sometimes guesses from the ticket text
  (`UserTokenService#getValidTokens(UUID)` "… with a read fallback
  strategy", taken from a ticket; `DatabaseCacheUserTokenStrategy#<init>`
  claims both parameters are injected, only the repository is kept); with it
  `what` names the actual effect, checked against the code:
  `UserTokenService#removeAllTokens(List<UUID>)` removes the tokens and
  publishes an async `TokenInvalidationEvent`; `TokenValidationRest#save`
  adds the token, pre-warms the auth data, returns the web response;
  `TokenValidationRest#deleteAllFromList` enforces a maximum count;
  `UserTokenService#isTokenValid` skips the check for Greenland admins → the
  source stays on by default (D-at). Full run: 729 members, **729
  described**, 0 invalid/failed/incomplete/skipped, 636 chat calls (93 from
  the `--path` run's cache), 0 retries, stage 1186 s (**1.86 s per call**),
  whole run 19 min 48 s; longest prompt 1012 tokens (open #26 closed). Run 2:
  2.0 s, 729 from cache, **0 chat calls**. Content: `what` median 101 chars,
  max 151; no meta text ("ticket", "commit", "javadoc", "likely",
  "unclear"); only 2 `NULL` whys; 53 members have no available ticket (commit
  subjects). Spot checks outside the package all correct on `what`
  (`UserDataEventRest#sendDeleteEvent` publishes a `UserDataDeletedMessage`
  asynchronously and answers 202 with the data; `AuthDataEvent#toWebResponse`
  maps the metadata and the first payload element;
  `ArgusMultiOrgaUrlExtractorConfigService#getPassThroughEqualsUrls` returns
  an empty array, `NULL` why). Weak spots (open #30): `why` is generic for
  accessors (54 members say "proof of concept for extending JWT
  functionality", the first ticket of `UserToken`/`UserTokenBearer`), and a
  recent ticket can lend a member an unrelated why
  (`DatabaseCacheUserTokenStrategy#removeToken(UUID,String)` → JWT-ID
  storage / Redis optimisation); `AuthDataEvent#toWebResponse` got a
  plausible but code-derived "standardized response format" why. Test
  classes under `backend/src/test` are indexed and described (open #13).

- **4.2 review fixes: model check, shared breaker, run-independent prompt,
  purpose rework.** *Real run (product owner opened `api.atlassian.com`;
  orchestrator, release build of 6e8713b, `argus`, fresh data dir):* run 1
  194 s wall, 217 files, 960 symbols, 2920 commits, 1821 ticket rows; 79
  keys, 79 fetched, 0 unavailable, 80 Jira requests; 79 summarised, 0
  invalid/failed, 79 chat calls, 0 retries (~2.4 s/ticket incl. fetching).
  Run 2: 2 s, 79 cached, 0 Jira requests, 79 summaries from the cache, **0
  chat calls**, 79 hits. Closes open #27 and replaces the seeded-fixture
  deviation (D-aq). *Missing model (medium):* Ollama answers an unknown chat
  model with HTTP 404, which tripped the breaker and the run exited 0 with no
  summaries, forever. New `LlmBackend::has_model` (`OllamaBackend`: native
  `POST /api/show`, 404 → missing; accepts names with `/`, unlike
  `/v1/models/{id}`) and `LlmClient::check_chat_model`, run once by
  `build_index` before the index build starts: a missing model fails the run
  (`the chat model "x" does not exist on the Ollama server; pull it …`), an
  unreachable server or other error trips the breaker and the run goes on
  cache-only; skipped when cache-only (`--no-llm`) (D-ar). *Breaker on the
  client (medium):* the tripped state was a local of the stage, so 4.3/4.4
  would have called a dead model again. `LlmClient` now holds `cache_only`
  (`AtomicBool`): `set_cache_only` (`--no-llm`, `Summarizer::new`), tripped
  on a chat backend failure or after `MAX_CONSECUTIVE_INVALID` (5)
  `InvalidOutput`s in a row (a valid reply resets the count), one warning;
  afterwards every `complete_with`, in any stage, answers from the cache or
  fails with the new `LlmUnavailable` (stage: `summaries_skipped`).
  `Summarizer` lost its `cache_only` field; the stage no longer calls
  `cached_with` (D-an amended). The stage logs one summary warning when
  tickets were invalid or failed. *Run-independent prompt (medium, nit 9):*
  `ticket_prompt(&Ticket)` no longer carries the parent key or title (the
  title came from this run's tickets only, so `--path` runs and failed parent
  fetches changed the prompt → cache misses / skips under `--no-llm`); the
  prompt depends on the ticket alone (D-al amended). *Purpose (medium, open
  #28):* the prompt forbids mentioning the ticket or missing information,
  says the purpose must give a reason and not restate the summary, and asks
  for an **empty purpose** when no reason is stated or clearly implied;
  `validate_summary` accepts an empty purpose, rejects meta phrases ("the
  ticket", "not stated", "does not specify", …) and a purpose whose content
  words (> 3 letters, 5-letter stems) all appear in the summary; the stage
  stores an empty purpose as `NULL`, `show` omits the `purpose:` line (D-as).
  *Whitespace (low):* fields with control characters other than whitespace
  are invalid (retry); whitespace runs (newlines) are collapsed before
  storing (`TicketSummary::summary_text` / `purpose_text`). *Fence (low):*
  the ticket (key, type, title, description) sits between `<<<TICKET` and
  `TICKET>>>`, stated to be content, not instructions; markers inside ticket
  text are defused. *Temperature (nit):* `-0.0` is normalised to `0.0` (client
  and cache key). *Empty content (suspected):* a reply with `content: null` /
  no content is now an empty reply → invalid output, retried, instead of a
  backend failure tripping the breaker. 17 new tests (8 `llm`: breaker for
  later completions/other types with cached ones still served, backend
  failure on the retry after an invalid reply, empty reply retried, 5
  consecutive invalid trip + reset, cache-only never calls nor checks,
  model check missing/failing, `/api/show` over a local HTTP server, `-0.0`
  key; 5 `summaries`: fenced prompt, run independence, marker defusing,
  empty purpose, whitespace/control chars, meta text, restated purpose; 4
  indexer: missing model fails the run before the index is written and
  `--no-llm` skips the check, unreachable server → cache-only, consecutive
  invalid stop the chat calls, empty purpose `NULL` + `show`), several
  adapted; 198 tests pass (4 ignored); live `cargo test llm -- --ignored`
  green. Wrong model live: `chat_model = "nope:latest"` → exit 1 with that
  message; with `--no-llm` exit 0. *Prompt probe (14 + 11 real tickets, then
  all 79):* before, the no-reason tickets got invented or meta purposes
  (GRLD-109586/96679/97031 "implement or update the Argus component as part
  of the parent task", GRLD-21224 "improve the code quality or
  functionality", GRLD-24231 a guessed user-information reason, GRLD-36499
  "likely to support … auditing", GRLD-24229/31155/24010 restating the
  summary). After: all of these have an empty purpose; tickets with a stated
  reason keep a concrete one (GRLD-50018 out-of-support Spring Boot versions,
  GRLD-37944 emergency logout of all users, GRLD-68743 memory issues around
  4am, GRLD-82584 excessive log entries, GRLD-72066 roles sharing privilege
  strings). *Real run with the new prompt (release, `data-real`, cached
  tickets):* run 1: 79 summarised, 0 invalid/failed/skipped, 79 chat calls,
  **0 retries**, stage 148.3 s (1.88 s/ticket), whole run ≈ 150 s; 30 empty
  purposes (27 of them tickets without a description), no meta text in any
  field (searched for "ticket", "not stated", "unclear", "likely", "parent
  task"). Run 2: 2.1 s, **0 chat calls**, 79 hits. Remaining weak spot: a
  few title-only tickets still get a purpose that paraphrases the summary
  with synonyms the word check cannot see (GRLD-20961 "secure … using a
  System Token", GRLD-35843, GRLD-88420, GRLD-90364 "resolve" vs "fixes")
  (open #28).

- **4.2 Ticket summaries.** New `summaries` module: `TicketSummary {
  summary, purpose }` (`deny_unknown_fields`), the pure `ticket_prompt(&Ticket,
  parent_title)` (key, type, parent key plus the parent's title when the parent
  is an indexed ticket, title, description capped at 4000 chars with a
  `[description truncated]` marker; asks for English, ≤ 300 / ≤ 200 chars, no
  invented details) and `validate_summary` (blank or > 400 / > 300 chars is
  invalid → the client's retry). `Summarizer` = `LlmClient` + `cache_only`,
  built by `from_config` (no `[ollama]` → one warning, `None`). New fourth
  stage `index_summaries` after tickets (git work trees only): reads the
  available tickets from the in-progress index `tickets` table, sequentially
  calls `complete_with`, writes the new index columns `llm_summary` /
  `llm_purpose` (D-al). Invalid after retry → warning, `summaries_invalid`,
  next ticket; first backend failure → `summaries_failed`, circuit breaker:
  no further chat call, remaining tickets still get cached summaries, rest
  `summaries_skipped` (D-an). New `annatar index --no-llm` (cache only, D-ao).
  `LlmClient::cached_with` (cache lookup that never calls the backend),
  `LlmStats::since`; new `ollama.temperature` (default `0.0`, validated
  0–2, sent and part of the `llm_cache` key, D-am). `IndexStats` gains
  `summary_tickets`, `summaries`, `summaries_invalid`, `summaries_failed`,
  `summaries_skipped` (sum = `summary_tickets`) and `llm: LlmStats` (this run
  only), logged and printed as a `summaries:` line. `show` prints `summary:`
  / `purpose:` under each ticket that has them. `build_index` takes
  `Option<&Summarizer>`. 14 new tests (7 `summaries`: prompt fields/parent/no
  description/cap/stability, validation, schema, `from_config`; 5 indexer
  through `build_index` with the fake LLM and fake Jira: summaries written,
  unavailable skipped, rerun 0 chat calls + `show` output; no summarizer;
  invalid output skipped then retried; breaker after one failed call keeps
  cached summaries; `--no-llm` cache only; 1 `llm` `cached_with`; 1 config
  temperature); 181 tests pass (4 ignored). *Concurrency (open #23):* 4
  summary-sized requests to the host Ollama, sequential 7.56 s vs parallel
  7.03 s → it serves one at a time, calls stay sequential (D-ap); the same
  request at `temperature: 0` gave identical text 4/4 (open #22).
  *Real run (`argus`, release, fresh data dir, deviation D-aq):* the Jira
  gateway `api.atlassian.com` is not on the agent container's proxy allowlist
  (`CONNECT tunnel failed, response 403`; the run warned once at the auth
  check and went on with 79 keys `not fetched`, 0 summaries, exit 0), so the
  ticket cache was seeded with the 6 scrubbed real fixture tickets through
  `parse_issue` + `TicketCache` and the runs used `--offline`. 4 of them are
  referenced by `argus` symbols (GRLD-21115, GRLD-24229, GRLD-72066,
  GRLD-82584). Run 1: 4 tickets, 4 summarised, 0 invalid/failed/skipped,
  4 chat calls, 0 retries; stage 15.2 s (7.2 s for the first call incl. model
  warm-up, then 1.5 / 3.8 / 2.7 s; 223–1050 prompt and 44–71 completion
  tokens), whole run 17.2 s. Run 2: **0 chat calls**, 4 cache hits, whole run
  2.07 s. Spot check against the ticket text: all 4 correct, English, no
  invented facts; e.g. GRLD-82584 → summary "Fixes a bug where Argus Export
  validation only checked file completeness (header and checksum) but failed
  to verify that individual rows matched current valid database entries.",
  purpose "To prevent the nightly export from generating invalid log entries
  by ensuring every row matches current database records, not just file
  integrity."; GRLD-21115 → "Create the new 'Argus' project, including the
  CachingService and future AuthorizationService, ensuring it is runnable in
  the Dev environment." Weak spot: when a ticket gives no reason (GRLD-24229,
  "Out of scope" list only) the purpose restates the summary (open #28).

- **4.1 review fixes: concurrency, validation hook, truncation, cache keys.**
  *Concurrency (high):* `LlmClient` cloned `Store::cache()`, and libsql
  connection clones share one SQLite connection, so `store_embeddings`'
  explicit transaction collided with other tasks' (16 tasks × 200 texts
  cached only 1136/3200) and its writes could join the history stage's
  per-file transactions. New `Store::connect_cache()` opens a separate
  connection (30 s busy timeout, `CACHE_BUSY_TIMEOUT`); the client takes that
  one, serialises its writes behind a `tokio::sync::Mutex` and writes single
  statements only (one multi-row `INSERT` per embedding batch), D-ai.
  *Validation (medium):* `complete_with(prompt, validate)` runs a caller check
  on the parsed reply inside the retry loop (its message goes back to the
  model; a cached row that fails it is a miss); `complete` = `complete_with`
  that accepts everything. Convention: response types carry
  `#[serde(deny_unknown_fields)]`, which makes schemars send
  `additionalProperties: false` and rejects extra fields, D-aj.
  *Truncation (medium):* the backend now returns `ChatReply` (content,
  `finish_reason`, `prompt_tokens`, `completion_tokens`); every reply is
  debug-logged with the token counts and elapsed ms, a `length` reply warns,
  and `InvalidOutput` carries `finish_reason` and says "cut off at the token
  limit". *Keys (low):* the `llm_cache` key hashes a canonical schema
  (recursively sorted keys, top-level `$schema` dropped), independent of
  `serde_json`'s `preserve_order` (D-ag). *Dims (low):* a cached row whose
  `dim` disagrees with its blob is a miss; vectors of different lengths in one
  `embed` call (within a batch or vs cached) are an error and the batch is not
  cached. *Transient errors:* `OllamaBackend` retries a 503 or a failed
  connection once after 2 s; 4xx, other 5xx and timeouts are not retried
  (D-ak). README lists `ANNATAR_OLLAMA_REASONING_EFFORT`. Open #23 updated,
  #24–#26 added. 12 new tests (11 `llm` incl. a 16-task concurrent test with
  a warm rerun making 0 backend calls and a rollback on the shared connection
  that keeps the client's writes; local HTTP server for 503/404/refused;
  1 `store`); 167 tests pass (4 ignored). *Live*
  (`cargo test llm -- --ignored`, `qwen3.8:latest` `none`, `bge-m3:latest`):
  struct cold 0.9 s warm model (7.1 s incl. model load), cached < 1 ms;
  3 × 1024-dim vectors cold 1.1 s, cached < 1 ms.

- **4.1 LLM client.** New `llm` module. `LlmClient::complete::<T:
  DeserializeOwned + JsonSchema>(prompt)` sends the `schemars` 1 schema of `T`
  as `response_format` `{type: json_schema, json_schema: {name, strict, schema}}`
  to Ollama's `/v1/chat/completions` and deserializes the reply into `T`
  (validation = serde). An invalid reply is retried once with the bad reply
  and the serde error appended to the conversation; a second invalid reply is
  a typed `InvalidOutput` error (downcastable) and nothing is cached; backend
  (HTTP/transport) errors are returned, not retried. `LlmClient::embed(texts)`
  (`/v1/embeddings`) looks each distinct text up in the cache, sends only the
  misses in batches of `EMBED_BATCH` (64), never sends an empty batch, and
  returns one vector per input in order. Seam `LlmBackend` (object-safe,
  boxed `Send` futures) with `OllamaBackend` (reqwest, 600 s request / 10 s
  connect timeout, Ollama's `error.message` in HTTP errors) and a test
  `fake::FakeBackend` (scripted replies, records requests and batches).
  `LlmStats` (`chat_calls`, `chat_hits`, `chat_retries`, `embed_calls`,
  `embed_texts`, `embed_hits`) via `LlmClient::stats()` so 4.2+ can show a
  rerun made no LLM calls. New cache tables `llm_cache` (key = blake3 of
  chat model + reasoning effort + schema JSON + prompt; reply text) and
  `embedding_cache` (key = blake3 of embedding model + text; `dim` +
  little-endian f32 blob), D-ag. New config option `ollama.reasoning_effort`
  (default `none`, validated at load), D-af; `schemars` 1.2 added. Not wired
  into `index` yet (4.2). *Live probe (Ollama 0.35.1 on the host,
  `qwen3.8:latest` = 27B Q4 thinking model, `bge-m3:latest` 1024 dims):*
  `/v1` returns a thinking model's reasoning in `message.reasoning`, so
  `content` is clean JSON; `"think": false` is ignored on `/v1`,
  `reasoning_effort: "none"` turns thinking off (82 → 31 completion tokens,
  ~8.0 s → ~1.5 s on a toy prompt); unknown effort values are accepted
  silently; a schemars-shaped schema (`$schema`, `title`, nullable types) is
  honoured; an unknown model is `404 {"error": {"message": ...}}`; an empty
  embeddings `input` is a 400. *Live tests* (`cargo test llm -- --ignored`):
  `completes_a_real_struct_and_caches_it` returned a valid `ClassSummary`
  (role `repository`) cold in 1.2–2.5 s (`none`; 3.1 s with thinking left to
  the model), repeat from cache in < 1 ms with 0 chat calls;
  `embeds_real_texts_and_caches_them` 3 vectors × 1024 dims cold 85–120 ms,
  cached < 1 ms. 19 new tests (18 `llm`, 1 config) + 2 ignored; 155 tests
  pass (4 ignored).

- **Phase 3 PO checks review fixes: finish the fixture scrub, cover tables.**
  `rich_description.json`: employer package paths and internal repo/module
  names replaced with neutral ones (`com/acme/...`, `com.acme.starter`,
  `service-starter`, `platform-core`, `platform-ejb`, `PlatformSystems`).
  PM decision (reword rather than keep): description text that contained
  internal security details was reworded into neutral, non-security text of
  similar length with identical markup (quotes, `…`, image tag, blank-line
  runs, panels, code blocks, nested lists) in both `fields.description` and
  `renderedFields.description` — the bug's token pre-validation write-up
  (and its summary, now `Argus Export Validierung Problem`) and the rich
  description's privilege-collision example (now a message-key example).
  Fixtures otherwise stay real-shaped captures. The bug's attachment id is
  `1`. `parse_issue` treats a summary that is empty after trimming as
  missing (`<key>: no summary`, D-ae). New `html_table_keeps_cells_without_borders`
  (header + 2 rows: cells present, no border characters) covers D-x's table
  claim again. `Ticket.issue_type` doc example and the plan 3.2 deviation
  wording updated. 136 tests pass (2 ignored).

- **Phase 3 product-owner checks: real Jira fixtures and real-repo run
  (opens #20, #21).** A scoped API token now works, so J3 is closed.
  *Fixtures:* the synthetic files in `tests/fixtures/jira/` are replaced by
  scrubbed real REST v2 captures (`expand=renderedFields`) from the target
  Cloud Jira, picked from the keys `argus` references: `story.json`
  (GRLD-24229, Story, epic parent, list, summary with a trailing space),
  `bug.json` (GRLD-82584, Bug, no parent, image + blank-line runs),
  `subtask.json` (GRLD-100, `Task` with `subtask: true`, Story parent,
  empty description), `epic_child.json` (GRLD-21115, `Verbesserung`, Epic
  parent plus the legacy epic-link field, trailing space),
  `empty_description.json` (GRLD-1500, Story, no parent; `fields.description`
  `null`, `renderedFields.description` `""`) and the new
  `rich_description.json` (GRLD-72066, `Verbesserung`: headings, panels,
  `<pre>` and highlighted code, nested lists, links). Scrub: the gateway
  and site hosts → `https://jira.example.com`, the Bitbucket org →
  `https://git.example.com/acme`; kept only `expand`, `id`, `self`, `key`,
  `fields.{issuetype, summary, description, parent (key, summary, status,
  issuetype), customfield_10930, status, created, updated}` and the matching
  `renderedFields` entries (no people, accountIds, avatars, comments,
  worklog or other custom fields). Description text kept as captured
  (later reworded where it contained internal security details; see the
  review-fixes entry above).
  *Findings:* sub-tasks on this instance are typed `Task` (`subtask: true`,
  `hierarchyLevel: -1`), not `Sub-task`; epic membership shows up as
  `fields.parent` with an `Epic`-typed parent (`hierarchyLevel: 1`) **and**
  as the legacy Epic Link `customfield_10930` (a plain key string) holding the
  same key, so `parent` alone covers it and `jira.epic_link_field` is not
  needed here (D-y); summaries can carry trailing whitespace (GRLD-21115,
  GRLD-24229), so `parse_issue` now trims the summary (D-ae); Cloud smart
  links use the URL as link text, so such URLs stay in the text as
  `[url]`, and headings keep `##` markers (D-x). Parse tests now assert the
  exact real key, type, summary, parent and description text; new tests
  `parses_a_rich_description`, `epic_child_takes_the_epic_from_its_parent`,
  `summary_is_trimmed`; the epic-link tests use `customfield_10930` with
  `parent` removed. The `#[ignore]` `fetches_real_ticket` accepts a
  `scope` 401 from the auth check (as the ticket stage does) and passed on
  GRLD-100, GRLD-1500 (product owner) and GRLD-24229, GRLD-21115,
  GRLD-72066, GRLD-100 (agent; basic auth via the gateway).
  *Real run (product owner):* `annatar index` on `/Repos/argus`, release
  build, fresh data dir. Auth: Cloud, scoped API token (`read:jira-work`
  only) through the `api.atlassian.com/ex/jira/<cloudId>` gateway, basic
  `email:token`; the `/myself` preflight got the 401 scope mismatch →
  inconclusive, fetches went ahead. Earlier probes: bearer on the site URL →
  403 Connect token error; basic on the site URL with the scoped token → 401.
  No 403/429 on issues, no circuit-breaker warning. Run 1 (cold): 19.5 s;
  217 files, 960 symbols, 2920 commits, 1821 ticket rows; tickets: 79 keys,
  0 cached, 79 fetched, 0 unavailable, 0 failed, 0 not fetched; 80 Jira
  requests (79 + 1 preflight). Run 2 (warm): 1.9 s; 79 cached, 0 fetched,
  **0 Jira requests** (3.2 "done when" met on real data). Content: 43/79
  tickets have a description (avg 672 chars, max 3593), 48/79 a parent key;
  types Verbesserung 57, Bug 11, Task 4, Story 4, Todo 3; 888/960 symbols
  link to at least one available ticket. 135 tests pass (2 ignored).

- **3.2 fix: scoped-token auth handling (preflight review).** Live facts
  for the Cloud target (cloudId `a91bc83a-f441-4972-9af4-e290d68c06e3`):
  scoped API tokens work only through the gateway
  `https://api.atlassian.com/ex/jira/<cloudId>` with basic `email:token`, and
  a token lacking a scope gets `401 {"code":401,"message":"Unauthorized;
  scope does not match"}`; `/myself` needs `read:jira-user`, issues need
  `read:jira-work`. `FetchError::Unauthorized` gains `scope: bool`, set for a
  401 whose body contains `scope does not match` (fetch and auth check); its
  message says the token lacks the scope Jira needs and that scoped tokens
  need `read:jira-work` (via the gateway). The ticket stage treats such a
  `/myself` 401 as **inconclusive**: debug log, fetches go ahead (a fetch 401
  is fatal and never cached anyway). A 429 on the preflight is retried with
  the fetch back-off: the loop moved out of `fetch_with_retry` into a generic
  `with_retry`, shared with the new `check_auth_with_retry`; every preflight
  request counts in `jira_requests`. Non-2xx bodies are read in chunks up to
  4 KiB (`MAX_ERROR_BODY`). The Cloud bearer hint and README now name both
  Cloud setups (classic token + site URL; scoped token + gateway +
  `read:jira-work`). Fake source: auth answers are a script (last repeats),
  `Answer::ScopeMismatch`. New tests: scope 401 mapping on `status_error` and
  `auth_check_error` (403 never sets it), scope message, 401 + Connect body →
  `cloud: true`, auth check retries a 429 (direct and through the stage, no
  real sleeps), a scope-limited preflight still fetches, a fetch scope 401
  fails the run and caches nothing; Cloud hint names the gateway. `send()`'s
  body plumbing stays untested by design (no HTTP mock, D-ad; noted on the
  test module). 132 tests pass (2 ignored).

- **3.2 fix: Jira auth preflight (J6 real-Jira finding).** The real-Jira
  check showed the target (`https://blueocean.jira.com`) is **Jira Cloud**:
  a Cloud API token sent as bearer (no `ANNATAR_JIRA_EMAIL`) gets
  `HTTP 403 {"error": "Failed to parse Connect Session Auth Token"}` on
  every request, without `X-Authentication-Denied-Reason`, so every key
  would have been cached unavailable forever. `TicketSource` gains
  `check_auth()`; `JiraClient` implements it as `GET /rest/api/2/myself`
  (2xx ok; **any** 401/403 → `Unauthorized`; other statuses and transport
  errors as for fetches). The ticket stage calls it once before the first
  fetch, only when keys are missing (a fully cached run still makes 0 Jira
  requests); it counts in `jira_requests`. `Unauthorized` fails the run with
  nothing cached and no ticket fetched; any other failure skips all fetches
  like the circuit breaker (one warning, every missing key `not fetched`,
  run succeeds). Defence in depth: an issue fetch's 401/403 whose body
  contains `Connect Session Auth Token` is `Unauthorized` too, not
  `Unavailable` (error bodies are now read). `Unauthorized { basic, cloud }`:
  in bearer mode with the Cloud body the message says Jira Cloud needs
  `ANNATAR_JIRA_EMAIL` for basic auth (the token is never printed).
  `fetches_real_ticket` runs the auth check first. Fake source gets a
  scripted auth answer and its own counter. New tests: rejected auth check
  fails the run with zero fetches, nothing cached and the previous index
  kept; a transport-failed auth check skips fetching without failing; the
  second-run test asserts no auth check on a fully cached run; Cloud body →
  `Unauthorized` (plain 403 stays `Unavailable` for fetches); auth-check
  status mapping; Cloud hint message; `myself` URL / `Accept` /
  `Authorization` (bearer and basic, no network). Existing `jira_requests`
  assertions +1 for the check. 126 tests pass (2 ignored).

- **3.2 review fixes.** A circuit breaker stops the ticket stage from
  stalling on an unresponsive Jira (D-ac): the first transport failure, or a
  429 still failing after its retries, stops new fetches, aborts the in-flight
  ones (`JoinSet::abort_all`), counts every key left as `tickets_not_fetched`
  and logs one warning; the run still succeeds. `JiraClient` gains a 10 s
  `connect_timeout` (request timeout stays 30 s). Requests are counted through
  a shared counter as each one is sent, so aborted fetches still show in
  `jira_requests`; a key that is not a plain Jira key is rejected before the
  source is called and no longer counts as a request (still a warning and
  `tickets_failed`). `TicketFetch` / `Fetched` fields are `pub(crate)`, so
  `TicketFetch::new` is the only concurrency clamp (the stage's `.max(1)` is
  gone). Per-task 429 waits stay as they are (the breaker bounds them to one
  key's back-off) and so does the early config validation in `main.rs`. New
  tests: transport failure trips the breaker (queued keys never called, nothing
  cached), the breaker aborts a hanging in-flight fetch, an exhausted 429
  trips it, an aborted run (401 on GRLD-2) keeps GRLD-1 in the cache, never
  calls GRLD-3 and leaves the previous index untouched, an invalid key is not
  sent or counted; the offline test also asserts that a cached key no symbol
  references gets no index `tickets` row. 120 tests pass (2 ignored).

- **3.2 Ticket cache.** A third index stage `index_tickets(&Transaction,
  cache, Option<&TicketFetch>, &mut stats)` runs after history (git work trees
  only) on the build transaction, before the single commit. It reads the
  distinct keys from the in-progress `symbol_tickets`, looks each up in the new
  `cache.db` table `ticket_cache` (keyed by the **requested** key, D-z; content
  fields incl. description, `unavailable` + HTTP `status`, `fetched_at` UTC),
  fetches only the missing ones, and copies every cached entry (key,
  unavailable, issue type, summary, description, parent key) into the new
  index table `tickets`. Fetching goes through a `jira::TicketSource` trait
  (boxed `Send` future, object-safe; `JiraClient` implements it), wrapped in
  `tickets::TicketFetch` (source + policy). Up to `jira.concurrency` (default
  4) fetches run as tokio tasks; the stage alone writes the results, as they
  arrive. Error policy (J6, D-ac): 401 / CAPTCHA-403 (`Unauthorized`) fails
  the run, nothing cached for it; 403/404 cached unavailable, no expiry; 429
  retried up to 3 times after `Retry-After` (fallback 5 s doubling, each wait
  capped at 60 s), then skipped like a transport/5xx/parse error: one warning,
  counted as failed, never cached, retried next run. `index` without
  `[jira]` or without `ANNATAR_JIRA_TOKEN` warns once and skips fetching; new
  `annatar index --offline` skips it even when configured (info log). Both
  still copy already-cached tickets into the index (J5, D-ac). `show` lists
  each ticket with `[Type] summary` or `(unavailable)` from the index
  `tickets` table only. `IndexStats` gains `ticket_keys`, `ticket_hits`,
  `tickets_fetched`, `tickets_unavailable`, `tickets_failed`,
  `tickets_not_fetched`, `jira_requests` (the five buckets sum to
  `ticket_keys`), logged and printed by `index`, so "a second run makes no
  Jira calls" is visible as `0 Jira requests`. 3.1 carry-over: an empty
  `fields.parent.key` now falls back to the epic-link field. tokio gains the
  `time` feature. New tests (fake `TicketSource` counting calls, scripted
  outcomes, end-to-end through `build_index` on scratch git repos): second
  run makes no Jira calls (+ `show` output, R1 invariant), failed fetches are
  not cached and retried, bad credentials fail the run and keep the previous
  index, offline uses the cache, moved issue cached under the requested key,
  concurrency peaks at the cap; plus cache round-trip, 429 retry cap, backoff,
  `from_config` skip paths and validation, empty-parent fallback. 115 tests
  pass (2 ignored). **`argus` without Jira** (release): the skip path warns
  once; cold 14.83 s / warm 1.85 s, 960 symbols, 2920 commits, 1821 ticket
  rows, 79 distinct keys (all `not fetched`, 0 Jira requests) — identical to
  R8. The real-token run (audit §3.1 step 3) is pending with the product
  owner.

- **R9 review nits.** The stage-level abort test is renamed
  `aborted_build_after_all_stages_keeps_previous_index_and_no_temp_file` (it
  drives the stages directly, not `build_index`); the `IndexedFile` doc now
  says files whose fqns were all duplicates are kept with no rows; R9 docs
  (Done, D-aa, audit plan §9) record that the uncommitted-changes warning
  now counts only indexed files. No behaviour change; 103 tests pass (2
  ignored).

- **R9 Staged pipeline.** `build_index` is now the one orchestrator: it owns
  the `IndexBuild` and a single write transaction on it, runs the stages
  `index_structure(&Transaction, repo, files, &mut stats) ->
  Vec<IndexedFile>` then (git repos only) `index_history(&Transaction,
  cache, repo, &[IndexedFile], ticket_regex, &mut stats)`, and commits the
  transaction and renames the temp file exactly once, after the last stage. No
  stage commits; any stage error returns early, dropping the transaction
  (rollback) and the build (temp file deleted), so the previous `index.db` is
  untouched. `IndexedFile`/`IndexedSymbol` carry the row id, `Symbol` and
  content hash from structure to history. The dirty-file check moved into the
  history stage and now counts only indexed files (a dirty file that fails
  to parse no longer triggers the warning). Otherwise behaviour-preserving:
  `IndexStats` and the R1 invariant are unchanged, and a live re-run on `argus` reproduced R8 exactly (960 symbols,
  2920 commits, 1821 tickets; cold 960 misses, warm 960 hits). Open #19
  decided (D-ab): a `--path` run keeps narrowing the whole index, now with one
  warning (prefix, file count, index path), documented in the README and
  `--path` help. New tests: stages run on a build and then aborted leave the
  previous index and no temp file; a `--path` run replaces the index with
  only that prefix. 103 tests pass (2 ignored).

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

- **5.5 review** of the file-level search (D-cm–D-cr).
- **Re-run the agent trial's *with* arm** on the 5.5 output (same tasks,
  protocol and harness as 5.4, D-cj; the *without* arm needs no re-run;
  the appended system prompt describing `search` must be updated to the
  file output). Measures whether the file view removes the `show` calls
  after `search` (12 of 20 *with* runs) and whether it fixes or worsens the
  T4 call-chain shortcut (open #42).
- **Product owner:** the POC is complete (questions 1–3 answered: 4.5/4.6,
  D-ch, D-cl). Decide whether to go on to **Later** (`plan.md`), and with
  which levers first: #40 (retrieval), #42 (callers / `trace` so a search
  hit does not shortcut a call chain), #41 (repeat the agent trial on a
  larger repository with tasks written by someone else; the harness is
  `scripts/agent_trial/run.py`).
- **Product owner:** 5.3 open #39 (raise `describe.body_chars` so the two
  longest members are described from their whole source; ≈ 4 chat calls).
- **Product owner:** the 4.6 trade-off is settled (D-bq: a member without
  a reason is fine when its class has one). Still open: the 4.5 review
  verdicts and golden questions (open #34), the borrowed reasons on
  `Privilege`/`TokenType` and the wrong one on `OAuth2RequestCacheService`
  (open #30), and the backup of `.annatar-local/golden-argus.toml`.
- **Product owner:** confirm or override the PM defaults for J1, J2, J4,
  J9 (D-w–D-y, D-aa), the open #19 decision (D-ab) and J5–J8 (D-ac, D-ad);
  D-w and D-y are now backed by the real Jira (see their notes).

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
| R9 Staged pipeline | done | `build_index` orchestrates `index_structure` → `index_history` on one `&Transaction`, single commit + rename at the end; stage abort keeps the previous index (test); open #19 decided: `--path` narrows by design, one warning + README/help (D-aa, D-ab); `argus` re-run identical to R8; 103 tests |
| R9 review nits | done | abort test renamed to match what it exercises; `IndexedFile` doc fixed; R9 docs note the dirty-file warning counts only indexed files; no behaviour change; 103 tests |
| 3.2 Ticket cache | done | `index_tickets` stage after history; `ticket_cache` (cache.db, requested key, `fetched_at`) + `tickets` (index.db); `TicketSource` trait + `TicketFetch` (concurrency 4, 429 ×3 retries); 401 fails, 403/404 unavailable, others skipped uncached; `--offline`; `show` prints type/summary; empty parent → epic field; fake-source tests prove a second run makes no Jira calls; `argus` offline run = R8 numbers; 115 tests. Real-token run pending (open #21) |
| 3.2 review fixes | done | circuit breaker on the first transport failure or exhausted 429 (stop, abort in-flight, rest `not fetched`, one warning); 10 s `connect_timeout`; requests counted as sent, invalid keys not counted; `TicketFetch`/`Fetched` fields `pub(crate)`, one concurrency clamp; tests for breaker, aborted-run cache, unreferenced cached key; D-ac, open #21 reworded; 120 tests |
| 3.2 fix: auth preflight | done | real-Jira J6 finding: Cloud answers a bearer token with a plain 403 (`Failed to parse Connect Session Auth Token`) on every request; `TicketSource::check_auth` (`GET /rest/api/2/myself`, any 401/403 → `Unauthorized`) once before the first fetch, only when keys are missing, counted in `jira_requests`; other check failures skip fetching like the breaker; Cloud body → `Unauthorized` on fetches too; bearer + Cloud message points to `ANNATAR_JIRA_EMAIL`; D-ac, open #21 note, README; 126 tests |
| 3.2 fix: scoped-token auth | done | `Unauthorized { scope }` from a 401 `scope does not match`; preflight scope 401 inconclusive (debug, fetch anyway), fetch 401 still fatal; preflight 429 retried via the shared `with_retry` (`check_auth_with_retry`); error bodies capped at 4 KiB; README / Cloud hint / open #21: classic token + site URL vs scoped token + `api.atlassian.com/ex/jira/<cloudId>` + `read:jira-work`; 132 tests |
| Phase 3 PO checks | done | real scrubbed fixtures (story, bug, sub-task `Task`, epic child `Verbesserung`, empty description, new rich description) replace the synthetic ones (closes J3, open #20); summary trimmed (D-ae); epic = `fields.parent` (Epic) + legacy `customfield_10930` (D-y); `fetches_real_ticket` tolerates a `scope` 401 preflight; PM real run on `argus` (release, fresh data dir): cold 19.5 s, 79 keys fetched, 80 requests; warm 1.9 s, 0 requests (closes open #21); 135 tests |
| Phase 3 PO checks review fixes | done | `rich_description.json` scrub finished (`com/acme`, neutral repo/module names); security write-ups in `bug.json` (incl. summary) and `rich_description.json` reworded to neutral text, same markup (PM decision); attachment id `1`; whitespace-only summary = missing (D-ae); `html_table_keeps_cells_without_borders` (D-x); doc/plan wording; 136 tests |
| 4.1 review fixes | done | own cache connection (`Store::connect_cache`, busy timeout) + write mutex + no explicit transactions (D-ai); `complete_with` validation hook, `deny_unknown_fields` convention (D-aj); `ChatReply` with finish reason + usage, truncation warned and named in `InvalidOutput`; canonical schema in the key (D-ag); embedding dim checks; one retry for 503 / refused connection (D-ak); README env var; open #23–#26; 167 tests |
| 4.2 Ticket summaries | done | `summaries` module (`TicketSummary`, pure `ticket_prompt`, `validate_summary`, `Summarizer`); `index_summaries` stage after tickets → index `tickets.llm_summary`/`llm_purpose`; sequential, breaker on the first backend failure (then cache only), invalid skipped; `--no-llm`; `ollama.temperature` (default 0, in the cache key); `LlmClient::cached_with`; `IndexStats` summary buckets + `llm`; `show` prints summary/purpose; live: sequential ≈ parallel on the host Ollama; `argus` (seeded with 4 real fixture tickets, Jira gateway blocked): 4 summarised in 15.2 s, rerun 0 chat calls; D-al–D-aq; 181 tests |
| 4.2 review fixes | done | chat-model check before the build (`/api/show`; missing → run fails, unreachable → cache-only) (D-ar); breaker state on `LlmClient` (`cache_only`, trips on a backend failure or 5 invalid in a row, `LlmUnavailable`), shared by later stages (D-an); prompt without parent key/title (D-al); purpose may be empty (→ `NULL`), meta text and restated purpose invalid, ticket fenced as data, whitespace collapsed, control chars invalid (D-as); `-0.0` temperature normalised; null content = invalid output; real run (79 tickets: 79 summarised, rerun 0 chat calls) closes open #27; new prompt on `argus`: 79/79, 0 retries, 148 s, 30 empty purposes, no meta text, rerun 0 chat calls; 198 tests |
| 4.3 Method and constructor what/why | done | `describe` module (`Description`, pure `select_history` + `member_prompt`, `validate_description`); `index_descriptions` stage after summaries → `symbols.what`/`why`; first + 3 most recent tickets (UTC dates), commit subjects without tickets, incomplete members skipped; source excerpt (`describe.body_chars` 1500); `[describe]` config; per-stage LLM stats, `describe:` line; `show` what/why; `argus`: 729/729 described, 1.86 s/call, 19 min 48 s, rerun 0 chat calls in 2.0 s; D-at–D-aw; 220 tests |
| 4.3 review fixes | done | default `ticket_regex` `\bGRLD-\d+` (separate commit, D-ay); only chosen tickets block, permanent fetch failures cached unavailable, `JiraMode` (Disabled → uncached tickets unavailable), blocking keys in the warning (D-az); Jira title for an invalid summary (D-ba); narrowed meta checks in describe and summaries (D-bb); excerpt replaces signature, merges dropped, invalid streak per stage (D-bc); `argus`: 81 tickets, 729/729 re-described in 1362 s, rerun 0 chat calls / 0 Jira requests in 2.0 s; 231 tests |
| 4.4 Type what/why, bottom-up | done | `fix(walk)`: nested `src/test` skipped (D-bd, #13); `index_type_descriptions` after describe, bottom-up; pure `outline` / `select_children` / `type_prompt` / `validate_type_description`; `[describe] type_members` (30), `type_recent_tickets` (3); held back on a missing listed child, invalid child listed bare (D-be–D-bg); `types:` line; 244 tests; `argus`: 175/175 types in 372 s (2.31 s/call), rerun 0 chat calls |
| 4.4 review fixes | done | `fix(walk)`: source set from the first `src/<set>/java`, Gradle test sets (D-bd amended); type test blocked only by its own ticket + warning asserted; ticket keys invalid in all LLM fields (D-bh); outline cuts attached comments / trailing `//`, collapses constant bodies and initializers (D-be); type why check narrowed, prompt against name-only guesses (D-bi); compact constructors; heading; open #31–#33; 254 tests; `argus`: 175/175 types in 6 min 48 s run, rerun 0 chat calls |
| 4.5 Quality review and golden set | done | `quality_review_4_5.md`: 34 symbols + 13 tickets before/after; acceptable whys 32 % → 71 % (mostly vague → empty; strictly correct 32 % → 35 %; held out 32 % → 63 %, corrected in the review fixes), wrong 15 % → 9 % (corrected: 12 % → 9 %), `what` 94 % → 97 %; stricter member/type why and title-only purpose prompts (D-bj, D-bk); `golden` module (TOML format, validation, `check_index`), synthetic fixture + integration test, real 20-question `argus` set in `.annatar-local/` (D-bl), 44/44 fqns present; full run 18 min 28 s, rerun 0 chat calls; 261 tests |
| 4.5 review fixes | done | review reports correct / wrong / vague / empty separately (correct 32 % → 35 %, wrong 12 % → 9 %), tuning vs held-out (33 % → 80 % vs 32 % → 63 % acceptable), index-wide losses (240 + 35 whys emptied, ≈ 5–8 supported), rule compliance ≈ 85 %, #3 cause (GRLD-40363 purpose), #6 W → V; golden set 24 questions / 56 fqns (interface, repository, enum, constructor primaries; alternates for Q2, Q5, Q18; Q17 reworded), 0 problems; `golden`: no whitespace in fqns, validated types not `Deserialize`, kinds from `SymbolKind::ALL`; README env vars; open #35; 262 tests |
| 4.6 review fixes and PO feedback | done | PO: "as short as possible without losing information", own reasons kept (borrowed ones and name guesses still excluded; accessor/constructor clause kept as "only when the history explains this very member or its field"); collective terms instead of enumerations; description-specific meta-phrase list (8 review sentences pass); `ollama.max_tokens` 1024 safety cap, `length` reply invalid, not in the cache key; per-stage peak prompt/completion tokens in the stats; "Merged " subjects dropped too; tests (commit order, type commit subjects); `argus` 19 min 21 s, 654 calls, 0 invalid/retries, rerun 0 calls; members with a reason 34 → 62, median 105 → 126 chars, max 489 → 334; spot check 6 C / 4 V / 4 B / 1 W of 15; 264 tests |
| 4.6 Symbol descriptions (PO redesign) | done | one `description` (members `Description`, types `TypeDescription`) replaces `what`/`why`, `symbols.description` column; commit subjects always in the prompt (`History { tickets, commits }`); "as short as possible", no length limit; validation: blank, control chars, ticket key, openers, source phrases (no "the commit"); `show` prints `- description:`; `argus` 15 min 29 s, 541 chat calls, 0 invalid/retries, rerun 0 calls; median 105 (members) / 147 (types) chars, 99 % one sentence; spot check 15 C / 5 V / 0 W of 20; D-bm–D-bp; 262 tests |
| 4.1 LLM client | done | `llm` module: `LlmClient::complete::<T>` (schemars 1 schema as `json_schema` response format, serde validation, one retry with the error, `InvalidOutput`, no caching of failures) and `embed` (per-text cache, misses only, batches of 64); `LlmBackend` seam + `OllamaBackend` + fake; `LlmStats`; `llm_cache` + `embedding_cache` (D-ag); `ollama.reasoning_effort` default `none` (D-af); live: struct cold 1.2–2.5 s / cached < 1 ms, bge-m3 1024 dims; 155 tests (4 ignored) |
| 5.5 File-level search results | done | product-owner redesign: `search` default = top 5 files (of the 100 nearest symbols, by best hit) with top-level type + description and every member / nested type with lines, hits `*score`, 40-line cap; `--symbols` = old output; filters choose hits only; `eval` file-level scores (`argus`: primary file 11 / 17 of 24 top-1 / top-5, any 12 / 20; symbol scores unchanged); output mean ≈ 3.1k chars vs ≈ 4.4k, ≈ 50 ms; 311 tests; no reindex (D-cm–D-cr) |
| 5.4 review fixes | done | exact Mann-Whitney with ties: tokens p 0.008/0.03/0.008/0.06, tool calls 0.008/0.008/0.008/0.08 (T1–T4, uncorrected) → significant on T1–T3; T4-with-1 regraded 2 → 1 → correctness 35 vs 35 (T2 reason better 5/5 vs 0/5, T4 dispatcher worse 4/5 vs 0/5 missed); `run.py`: timeout/exit-code meta, `summary` warnings, `shlex` call parser (env prefixes, subshells, global options), PATH assertion; `test_run.py` (10 stdlib tests, fixture); turns column dropped; published counts unchanged; D-cj/D-ck/D-cl, #40/#42 amended |
| 5.4 Agent trial | done | `scripts/agent_trial/run.py` (headless Claude Code, stream-json metrics, blind answers); `claude-sonnet-5`, 4 `argus` tasks × with/without × 5 runs; with `annatar` −47 % tokens, −40 % tool calls, −32 % cost ($0.090 vs $0.132 per run), −39 % wall time; score 36 vs 35 / 40, 35 vs 35 after the review regrade (ticket reason T2 5/5 vs 0/5; T4 dispatcher missed 4/5 with); ≈ $5.0 spent; D-cj–D-cl, open #41/#42 |
| 5.3 review fixes | done | `tests/cli.rs` helper clears proxy variables and sets `NO_PROXY=127.0.0.1,localhost` (eval test passed only through the caller's `NO_PROXY`); `eval -k` 5–100 (top-5 in the summary); D-cg states both sides (variant better on 5 of 6 totals, types regress, within noise at n = 24, default kept); sample-size caveat in D-ch / Done / plan.md; open #39 dedented lengths; 306 tests; no reindex (D-cf, D-cg, D-ch amended; D-ci) |
| 5.3 Evaluate | done | `eval` module + `annatar eval [-k N] <golden.toml>` (check_index first, unfiltered `search::search` per question, primary / any rank, top-1/top-5/MRR for all, types, members); 305 tests (+9, incl. a CLI run against a localhost embedding server); `argus` 24 questions default: primary 4/14 of 24 (top-1/top-5), any 10/17; `parent_description = true`: primary 7/13, any 11/17, types worse (0 chat calls, 8 embedding calls) → default kept; misses: parent/member ties 7, DTOs 3, vocabulary 5, truncated source / missing behaviour 3, wrong reason 1, lookalike 1; D-cf–D-ch, open #39, #40 |
| 5.2 review fixes | done | `relative_prefix` applies `..` lexically (escape → `None`); `search::path_filter`: repo root / `.` → no filter, escape → error (index: `a/../a` now works, `../x` still empty, open #38); README/D-cb/D-cc/D-cd: `--path` for search, nested-type role rule, field-3 fqn and location after the tokens, warnings may precede `error:`; tests: path normalisation, top_k vs exact scan, CLI data dir unchanged (model-mismatch path, hermetic), `--path` escapes; 296 tests; `argus` checked without reindex (D-cb, D-cc, D-cd amended; D-ce) |
| 5.2 `annatar search` | done | `search` module: meta checks (no vectors / no `index_meta` / other model → refused before any request; other dimension → refused), query embedded via `LlmClient::embed` with an in-memory cache (no file written); unfiltered `vector_top_k`, filtered exact `vector_distance_cos` scan (#36); `--kind`/`--role` (members by their type's role)/`--path`/`-k` (1–100, default 10); two-line output `rank. score fqn [kind] role=… file:lines` + description; logs on stderr, one-line `error:` and exit 1 (#29); `show` names the parent; 290 tests (+14, incl. `tests/cli.rs` on the binary); `argus`: 9/9 own queries rank the right symbol 1st, ≈ 45–70 ms per query |
| 5.1 review fixes | done | `max_neighbors=32` (16 vs 32 vs default measured on `argus` and a 6× synthetic set; 16 loses recall at 6×), 256 MiB page cache kept (still 2.7× faster); `index.db` 75 → 29 MB, cached stage 0.53 s; `index_meta` (embedding model, dim, `parent_description`); cached vectors of another length re-embedded instead of failing for good; warning for skipped embeddings; #36 / Next: `vector_top_k` caps at ~200 rows → exact scan for filters; tests: mid-way failure, tripped breaker + NULL member, meta, dimension recovery; 276 tests; rerun 0 embedding / 0 chat calls (D-bt, D-bs, D-bu amended; D-bw, D-bx) |
| 5.1 Embeddings | done | `index_embeddings` stage after types: `fqn\ndescription` (pure `embeddings::embedding_text`, `[embedding] parent_description` off by default) through the embedding cache; `symbol_vectors` `F32_BLOB(<dim>)` + cosine `libsql_vector_idx` (`compress_neighbors=float8`) created by the stage from the first vector's length; `PRAGMA cache_size` 256 MiB for the inserts; `--no-llm`/breaker → cached embeddings only, failed request → cache fallback; embedding model checked with the chat model; `embeddings:` line; 273 tests; `argus`: 654/654 vectors, dim 1024, 11 calls cold (run 10.1 s), rerun 0 calls (2.5 s), index.db 75 MB, ANN recall 1.0 (D-br–D-bv) |

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
| 24 | A working dummy `annatar.toml` is committed (data dir `.annatar`, gitignored) | No config in the repo | The binary's default config now loads out of the box; secrets still come only from the environment, so the file stays committable; not tried in 4.5 (the review kept `qwen3.8:latest`; prompt changes, not a model change, fixed the weak spots) |
| 25 | Build/generated sources detected by directory name (`build`, `target`, `generated`, `generated-sources`), pruned only when not under a source root (no ancestor named `java`) | Prune the names anywhere; parse Maven/Gradle build files; add an `index.skip_dirs` config | Language/build-system agnostic, cheap, and testable, and now does not drop a legitimate package such as `com.acme.build`; a config escape hatch is deferred (D-h) |
| 26 | `--path` is a literal path prefix (`Path::starts_with`), not a glob | Glob/pattern matching | The plan calls it a prefix; prefix semantics cover both a directory and a single file with no pattern engine |
| 27 | Walker sets `require_git(false)` and `parents(false)` | Require a git repo / honour ancestor ignores | Temp-dir tests need no `git init` and stay deterministic; `parents(false)` also stops a parent `.gitignore` outside the repo from affecting a run |
| 28 | `tree-sitter` 0.25 + `tree-sitter-java` 0.23 (ABI-matched pair) | Other version pairs | A real parse test proves the pair loads; later bumps must keep the grammar crate compatible |
| 29 | Type recursion is scoped to type-container nodes (`program`, `*_body`, `enum_body_declarations`), never executable scopes | Filter `class_declaration` by inspecting its `parent.kind()` | `tree-sitter-java` has no `local_class_declaration`; local and anonymous classes are also `class_declaration`, so only descending type containers correctly excludes them |
| 30 | `TypeDef`/`parse_types` replaced by one unified `Symbol`/`parse_file` in 1.3 | Keep types and members as separate structs | 1.4 adds text to every symbol and 1.5 persists one `symbols` table; one model avoids parallel fields and makes the parent_id join trivial **4.6 follow-up (PO: keep reasons):** with "never drop a reason", own-ticket reasons come back (members with a reason 34 → 62) but so do borrowed ones: upgrade themes (`Privilege`, `Privilege.User` "Spring Boot 3 and Jakarta …"), the initiative name (`TokenType` "JWT functionality extensions"), file-wide themes on accessors and field-storing constructors (22/176, 12/44), and `OAuth2RequestCacheService`'s wrong object-size reason. A wording that defines an own reason by what the symbol itself does was tried on one prefix without a net gain. Next lever: the D-bj code rule (no reason for detected accessors / field-storing constructors) or per-member ticket choice, if 5.3 shows borrowed reasons hurt |
| 31 | Constructor fqn uses `<init>` (name field `<init>` too) | Use the type's simple name | JVM/SCIP/JDT convention; unambiguous and stable, and later SCIP usage mapping matches without translation |
| 32 | Member fqn parameter types are whitespace-stripped source text, no parameter names (`Map<String,List<X>>`) | Keep original spacing / resolve simple names | Identity only needs distinct overloads; 1.4 stores the full signature separately, so fqn can be compact and deterministic |
| 33 | Fields, initializer blocks and annotation-type elements are not symbols | Include them | The plan scopes members to methods and constructors; annotations are covered by 1.4's annotation text, not as symbols |
| 34 | `signature` drops declaration-level annotations and collapses whitespace; parameter annotations stay in it | Keep annotations in the signature / preserve source formatting | Annotations have their own field; a one-line signature is what the 4.3 prompt wants. Parameter annotations are part of the method's shape **4.6 follow-up:** also confirm the keep-reasons trade-off (D-bm amended: 4 borrowed and 1 wrong in a 15-symbol spot check vs 6 correct) |
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
| D-w (3.1, J1) | Jira auth: `ANNATAR_JIRA_EMAIL` set (non-empty) → basic `email:token` (Cloud API token); otherwise the token is sent as a bearer PAT (Server/DC). A missing or empty `ANNATAR_JIRA_TOKEN` fails `JiraClient::new`. PM default, adopted audit recommendation; product owner may override. | Cloud only; a separate `auth` config key | Both schemes are a few lines and the existing optional `email` already tells them apart; resolves open #1. **Real Jira (Phase 3 PO checks):** the target is Cloud; a scoped API token (`read:jira-work`) works with basic `email:token` only through `https://api.atlassian.com/ex/jira/<cloudId>`; bearer on the site URL → 403 Connect token error, basic on the site URL with the scoped token → 401 |
| D-x (3.1, J2) | `reqwest` 0.13 with `default-features = false`, features `json` + `rustls` (0.13 renamed the old `rustls-tls` feature; it uses aws-lc-rs and the platform certificate verifier), and `html2text` (`plain_no_decorate`, no table borders, no link footnotes) for `renderedFields` HTML. PM default, adopted audit recommendation; product owner may override. | `ureq`/blocking; hand-written HTML stripper; ADF parser via REST v3 | Async fits the tokio runtime and 3.2's bounded concurrency; `html2text` keeps lists, tables and code blocks readable. Accepts the TLS compile cost decision 13 avoided for libSQL. A lighter alternative exists: reqwest `rustls-no-provider` plus `rustls` with the `ring` provider avoids the `aws-lc-sys` build; not adopted for now. `default-features = false` also drops reqwest's `system-proxy` (OS proxy settings); `HTTPS_PROXY`/`HTTP_PROXY` from the environment are still honoured. Plain output avoids `**`/URL noise in LLM prompts. **Real data:** Cloud smart links use the URL as their text, so those URLs remain as `[url]`; headings keep `##`/`###` markers; image tags become `[file name]`. Kept: harmless for LLM prompts |
| D-y (3.1, J9) | `parent_key` = `fields.parent.key`; only when absent (or, since 3.2, empty), the optional `jira.epic_link_field` custom field (a string key, or an object with `key`). Unset = ignored. PM default, adopted audit recommendation; product owner may override. | Discover the epic-link field via `/rest/api/2/field`; parent only | Cloud covers it via `parent`; Server/DC opts in without code changes. **Real Jira:** confirmed — an epic child has `fields.parent` with an `Epic`-typed parent (`hierarchyLevel: 1`) and the legacy Epic Link `customfield_10930` (plain key string) mirrors it; sub-tasks are typed `Task` with `subtask: true` and also use `parent`. `epic_link_field` stays unset for this instance |
| D-z (3.1, J3) | **Deviation:** fixtures under `tests/fixtures/jira/` are synthetic (REST v2 `renderedFields` shape, `jira.example.com`), not real captures; the real-token run is the `#[ignore]` `jira::tests::fetches_real_ticket` (base URL from `ANNATAR_JIRA_URL` or `annatar.toml`, key from `ANNATAR_JIRA_TEST_KEY`; prints only auth mode, key, type, parent and lengths). Also: REST v2 (works on Cloud and Server/DC), `Ticket.key` is the key Jira returns — a moved issue returns its *new* key, so 3.2 must key the cache by the *requested* key (and `fetches_real_ticket` only logs a mismatch), description `None` when null or blank after conversion, keys outside `[A-Za-z0-9_-]` are rejected before any request | Block 3.1 on a token | No token is available to the agent; parsing is pure, so swapping in real captures only changes test data. Tracked as open #20. **Closed (Phase 3 PO checks):** fixtures are now scrubbed real captures (hosts → `example.com`, employer package/repo names → neutral ones, only the parsed fields plus status/dates kept, description text reworded where it contained internal security details); open #20 resolved |
| D-aa (R9, J4) | One orchestrator (`build_index`) owns the `IndexBuild` and one write transaction; stages are separate functions taking `&libsql::Transaction` (structure → history now, tickets in 3.2, summaries later), and the transaction commit + atomic rename happen once after the last stage. Stages take `&Transaction` rather than `&Connection`: the type says "you are inside the build's transaction" so a stage cannot be called on the committed index or the cache by mistake, while `Transaction: Deref<Target = Connection>` keeps the existing helpers (`write_symbol`, `attach_history`) unchanged. Structure hands history an in-memory `Vec<IndexedFile>` (row id, `Symbol`, content hash) instead of re-reading `symbols`. PM default, adopted audit recommendation; product owner may override. | Later stages write into the committed `index.db` (breaks temp file + atomic rename); an `IndexBuild::transaction()` guard type; stages re-query `symbols` | Keeps the ground rule and D-g's single-transaction batching; one long write transaction on a private temp file blocks nobody. Structure-then-history instead of interleaved per symbol changes log order, and the uncommitted-changes warning now counts only indexed files (a dirty file that fails to parse no longer triggers it), not results (R8 numbers reproduced). Holding the `Symbol`s for the run is a few MB at most |
| D-ab (R9, open #19) | A `--path` run keeps replacing the whole `index.db` with an index of only that prefix (empty if it matches nothing). It now logs one warning (prefix, file count, index path), and the README and the `--path` help say so. No merging. PM default, adopted audit recommendation; product owner may override. | Merge the prefix into the existing index; refuse to replace a full index | `--path` exists for fast prompt iteration (plan ground rules); merging needs stable ids/upserts, which is "Incremental indexing" in Later. Making the narrowing explicit removes the surprise at no cost |
| D-ac (3.2, J5, J6) | `index` without `[jira]` or without `ANNATAR_JIRA_TOKEN` skips fetching with one warning; `--offline` skips it even when configured (info log). The stage still runs offline: cached tickets are copied into the index, uncached keys are counted `tickets_not_fetched`. Errors: `Unauthorized` (401, CAPTCHA 403) fails the run and is never cached; 403/404 cached unavailable with `status` and `fetched_at`, no expiry (drop `ticket_cache` to refresh); 429 honours `Retry-After` (seconds form) with a 5 s doubling fallback, each wait capped at 60 s, at most 3 retries, then the key is **skipped without caching** (counted failed, retried next run); transport, other statuses, invalid keys and parse errors are likewise skipped with a warning and a count, never cached (an invalid key is rejected before any request and is not counted as one). **Amended (4.3 review, D-az):** permanent failures — another 4xx except 408/425/429, an issue JSON that does not parse, an invalid key — are cached unavailable like 403/404 (`status` = the HTTP status, `NULL` for the last two); a 2xx body that is not JSON (`FetchError::NotJson`, e.g. a proxy page) stays transient. **Circuit breaker** (3.2 review): the first transport failure (connect/TLS/timeout, body read) or 429 still failing after its retries stops the stage from starting fetches, aborts the in-flight ones and counts every remaining key `tickets_not_fetched`, with one warning; the run succeeds with the tickets it has. It trips on the first occurrence, not after N in a row: such failures are systemic (Jira down, VPN off, account throttled), the keys already in flight give the same evidence, and each further attempt costs up to the 30 s request timeout or a full back-off, so a hanging Jira would otherwise cost (keys / concurrency) × 30 s on every run. The client's `connect_timeout` is 10 s, so an unreachable host trips it fast. Per-task 429 waits are kept (no shared back-off across tasks): the breaker limits them to one key's retries. Other statuses and parse errors do not trip it. **Auth preflight** (real-Jira J6 finding): Jira Cloud answers a bearer token with a plain `403 {"error": "Failed to parse Connect Session Auth Token"}` on every request and no `X-Authentication-Denied-Reason`, which the per-issue mapping would read as "unavailable" and cache for every key. So before the first fetch (only when keys are missing, so a fully cached run makes 0 requests) the stage calls `GET /rest/api/2/myself` once, counted in `jira_requests`: any 401/403 there is `Unauthorized` (fails the run, nothing cached), other failures skip all fetches like the breaker. An issue 401/403 with the Connect body is `Unauthorized` too. **Scoped tokens** (preflight review): a 401 whose body says `scope does not match` is `Unauthorized { scope: true }`; on `myself` it is inconclusive (that endpoint needs `read:jira-user`, issues need `read:jira-work`, so a token fit for fetching may fail it): debug log and the fetches go ahead, where any 401 fails the run uncached. A 429 on the check is retried with the fetch back-off (shared `with_retry`), each request counted. Error bodies are read up to 4 KiB. A per-issue 403 that is a real permission denial stays `Unavailable`; a credential that passes `myself` but is denied per issue is still indistinguishable from that, which is accepted. Concurrency `jira.concurrency` (default 4, 0 rejected) as tokio tasks; the stage is the only writer. Successful fetches are cached as they arrive, so an aborted run keeps them. PM default, adopted audit recommendation; product owner may override. | Fail the run on 429/5xx; no offline flag; skip the whole stage offline; breaker after N > 1 consecutive failures; a shared 429 back-off | A flaky or rate-limited Jira should degrade the ticket data, not block the index, and nothing transient may poison the cache; bad credentials would otherwise silently leave every ticket missing. The 60 s cap is an addition so an absurd `Retry-After` cannot stall a run. Using the cache offline keeps `--path`/offline loops informative at zero cost. **Real run (open #21):** `argus` cold run 80 requests (79 keys + 1 preflight, preflight inconclusive on the scope 401), warm run 0 requests; no 403/429/breaker |
| D-ad (3.2, J7, J8) | Test seam `jira::TicketSource` (object-safe: `fetch` returns a boxed `Send` future), implemented by `JiraClient`; tests use a scripted fake that counts calls and records peak concurrency. Cache table `ticket_cache` (cache.db) keyed by the requested key with issue type, summary, description, parent key, `unavailable`, `status`, `fetched_at`; index table `tickets` (key, unavailable, issue type, summary, description, parent key) for every key in `symbol_tickets` the cache knows. `show` reads only index.db. New `tickets` module holds the cache, fetch policy and retry; the stage lives in `indexer`. PM default, adopted audit recommendation; product owner may override. | `async fn` in trait with generics (not object-safe; would make `build_index` generic); a local HTTP mock server; `show` reading `cache.db` | Boxing one future per request is noise next to an HTTP call, and `Arc<dyn TicketSource>` lets fetches run as `'static` tasks. Keeping description in the index too lets Phase 4 build ticket summaries from index.db alone. The Jira-returned key of a moved issue is not stored (logged at debug) |
| D-ae (Phase 3 PO checks) | `parse_issue` trims surrounding whitespace from the summary; a summary that is empty after trimming is treated as missing (`<key>: no summary`, review fix) | Keep the summary verbatim; accept an empty summary | Real summaries end in a space (GRLD-21115, GRLD-24229); trimming keeps prompts, `show` output and later comparisons clean. Already-cached `ticket_cache` rows keep the untrimmed text until refetched (schema unchanged, so no drop; delete `ticket_cache` to refresh) |
| D-af (4.1) | New `ollama.reasoning_effort` (default `none`; `low`/`medium`/`high`; `""` = not sent), validated at config load and part of the LLM cache key | Leave thinking on; `think: false` (Ollama native API); no option | Live: `qwen3.8:latest` is a thinking model; on `/v1` `think` is ignored and `reasoning_effort: "none"` disables thinking, cutting completion tokens and time by ~2–5× on probes. Summaries from signature/Javadoc/tickets should not need reasoning; turn it on per run to compare in 4.5. Ollama accepts unknown values silently, hence the validation. The reasoning text (`message.reasoning`) is never used |
| D-ag (4.1) | Cache layout: `llm_cache(key, model, output, created_at)`, key = blake3 over length-prefixed `completion`, chat model, reasoning effort, canonical text of the schemars schema (object keys sorted recursively, top-level `$schema` dropped — review fix, so the key does not depend on `serde_json`'s transitively enabled `preserve_order`), full prompt; `output` is the trimmed validated reply text, re-deserialized and re-checked on a hit (an undecodable or no longer valid row is a miss and is rewritten). `embedding_cache(key, model, dim, vector)`, key = blake3 over `embedding`, model, text; vector as little-endian f32 blob; a row whose `dim` disagrees with its blob is a miss. Cache read/write faults degrade like D-q. A `schemars` bump that changes the generated schema shape (not just key order) changes every key and so invalidates `llm_cache` | Store `T` re-serialized; JSON vectors; key without the schema/effort; store the prompt | The schema in the key makes a changed struct (or field docs) miss, as the ground rules ask. Raw reply text needs no `Serialize` bound. The LE f32 blob is libSQL's `F32_BLOB` layout, so 5.1 can copy it straight into the vector column. Prompts are not stored (size); add a column if 4.5 review wants them |
| D-ah (4.1) | `LlmBackend` seam (object-safe, boxed futures, like D-ad) and `LlmClient` owning a cache `Connection` (its own one since the review fixes, D-ai). Retry once on invalid output by appending the bad reply and the serde error as a follow-up turn; transport/HTTP errors are not retried. No `temperature`/`max_tokens` sent (model defaults); 600 s request / 10 s connect timeouts as constants; `ollama.url` may end in `/v1` | Resend the identical request; retry HTTP errors; `temperature: 0` | With the grammar-constrained format invalid JSON is rare, and resending the same prompt to a deterministic sampler would likely repeat the error, so the retry tells the model what was wrong. HTTP errors (model missing, server down) don't fix themselves on an immediate retry (amended by D-ak: one delayed retry for 503 / refused connection). Leaving temperature at the model default keeps that retry meaningful; determinism comes from the cache (open #22). A 27B local model on a long prompt can take minutes, hence the long timeout |
| D-ai (4.1 review) | `LlmClient` gets its own `cache.db` connection (`Store::connect_cache`, 30 s busy timeout), serialises its writes behind a `tokio::sync::Mutex`, and writes single statements only (one multi-row `INSERT` per embedding batch of ≤ 64 rows) | Keep the shared `Store::cache()` clone and only add the mutex; a dedicated writer task | libsql `Connection` clones share one SQLite connection and its transaction state, so an explicit transaction on it collides with other tasks' and the client's writes would join (and roll back with) the history stage's per-file transactions. A separate connection isolates transaction state; the mutex plus single statements keep concurrent tasks from interleaving on it. Cost: cross-connection writes now take SQLite file locks, so a writer waits for another connection's open write transaction (bounded by the busy timeout); stages run sequentially today. `cache.db` stays in rollback-journal mode (WAL is open #5) |
| D-aj (4.1 review) | Validation hook: `complete_with(prompt, validate: Fn(&T) -> Result<(), String>)`, run after serde inside the retry loop and on cache hits; convention that response types use `#[serde(deny_unknown_fields)]` (schemars then emits `additionalProperties: false`) | A `Validate` trait every `T` implements; validation in each stage after `complete` | A closure needs no impl for types without checks and keeps the check next to the prompt in 4.2+; running it inside the loop means a blank/overlong field triggers the error-carrying retry and is never cached, and a tightened check re-asks for already-cached rows. `deny_unknown_fields` is the caller's choice per struct, documented in the module docs |
| D-ak (4.1 review) | `OllamaBackend` retries once, after 2 s, on HTTP 503 or a failed connection (`reqwest` `is_connect`); 4xx, other 5xx and timeouts are returned at once. A run-level circuit breaker is the job of the stage that loops (4.2+), not the client | No retry (D-ah); retry all 5xx and timeouts; backoff with several attempts in the client | Ollama answers 503 while overloaded/loading and a restarting server refuses connections briefly; both often clear in seconds. A 4xx (unknown model, bad request) won't, and a timeout already spent up to 600 s. One bounded retry keeps a dead server from stalling each call for long; the stage sees repeated failures and stops (open #25) |
| D-al (4.2) | Ticket summaries are a fourth `build_index` stage after tickets that reads the available tickets from the in-progress index `tickets` table and writes `llm_summary` / `llm_purpose` columns there (index only; the LLM cache is the persistence). Output language **English** (tickets are mostly German); `TicketSummary { summary, purpose }` with `deny_unknown_fields`; prompt from a pure `ticket_prompt(&Ticket)`: key, type, title, description capped at 4000 chars (real max 3593), fenced as data (D-as); validation rejects a blank summary and > 400 / > 300 chars (the prompt asks for 300 / 200, so a slightly long reply is not retried). **Amended (4.2 review):** no parent key and no parent title — the prompt depends on the ticket alone | German output; a separate cache table keyed by ticket key; parent key only; no cap; parent title from `ticket_cache` independent of the run | English matches code identifiers and later English questions/embeddings; ticket text never repeats in the prompt beyond the cap. The parent title came from this run's tickets only, so `--path` runs, a failed parent fetch or a new commit mentioning the epic changed a child's prompt (cache miss, or skipped under `--no-llm`); sourcing it from `ticket_cache` would still miss once whenever the parent is first fetched or refetched. A bare parent key tells the model nothing. Dropping both is the only fully run-independent option; epic context can come back in 4.3/4.4 from the indexed parent's own summary. Column names keep `summary` = Jira title |
| D-am (4.2, open #22) | New `ollama.temperature` (default `0.0`, validated 0–2), always sent and part of the `llm_cache` key; supersedes D-ah's "no temperature" | Model default (no field); no option | Reproducible records: dropping `llm_cache` (or a cache miss on another machine) regenerates the same text; live, 4/4 identical replies at 0. Existing 4.1 cache rows miss once (key changed); the option lets 4.5 try another value |
| D-an (4.2, open #25) | Summaries circuit breaker trips on the **first** backend failure (the client already retried a 503/refused connection once, D-ak): one warning, no further chat call this run, remaining tickets still get summaries the LLM cache holds, the rest are `summaries_skipped`. A reply still invalid after the client's retry is per-ticket: warning, `summaries_invalid`, next ticket. LLM problems never fail the run (except a missing chat model, D-ar). **Amended (4.2 review):** the breaker is state of the shared `LlmClient` (`cache_only: AtomicBool`), not a local of the stage: `--no-llm` sets it, a chat backend failure or `MAX_CONSECUTIVE_INVALID` (5) invalid completions in a row trip it (one warning); after that every `complete_with` in every stage answers from the cache or returns `LlmUnavailable`. A reply with no/`null` content is invalid output (retried), not a backend failure | N consecutive failures; classify connect/timeout errors | Same reasoning as the Jira breaker (D-ac): a down or hung server would otherwise cost every ticket the connect retry or the 600 s timeout; a one-off failure only defers the rest to the next run, which hits the cache for everything done |
| D-ao (4.2) | `annatar index --no-llm`: cache-only summaries (0 chat calls, cached ones still reach the index, misses skipped). `--offline` stays Jira-only. No `[ollama]` → one warning, every ticket `summaries_skipped` | `--offline` also disables the LLM; no flag | Ollama is local, so "offline" (no Jira) and "no LLM" are different needs; a run without the model must not drop already-paid-for summaries from the rebuilt index. Without `[ollama]` there is no model name, hence no cache key, so nothing can be reused |
| D-ap (4.2, open #23) | Chat calls in the stage are sequential; no concurrency option | Bounded concurrency (`ollama.concurrency`) | Measured on the host Ollama: 4 summary-sized requests sequential 7.56 s vs 4 in parallel 7.03 s — it serves one at a time, so concurrency only queues and adds the no-dedup issue. Revisit if `OLLAMA_NUM_PARALLEL` > 1 or for 4.3's larger volume |
| D-aq (4.2 real run) | **Superseded (4.2 review):** the product owner opened `api.atlassian.com` and the full run happened (79 tickets fetched and summarised, rerun 0 chat calls; see Done). Original deviation: the agent's real run seeded `ticket_cache` with the 6 scrubbed real fixture tickets (via `parse_issue` + `TicketCache`, scratch data dir) and ran with `--offline`, because the container's proxy allowlist blocks `api.atlassian.com` | Skip the real run | Exercises the real pipeline, model and prompts on real (scrubbed) ticket text; only 4 of the 6 keys are referenced by `argus` symbols. The full 79-ticket run is the product owner's check (open #27) |
| D-ar (4.2 review) | Before the index build, `build_index` checks once that the server has the chat model (`LlmBackend::has_model`, Ollama `POST /api/show`): missing (404) → the run fails with a "pull it or fix `ollama.chat_model`" message; unreachable/other error → the breaker trips and the run is cache-only; no check when cache-only (`--no-llm`). The check is not a chat call (`chat_calls` stays 0 on a cached rerun) | Treat a chat 404 as fatal; `GET /v1/models/{id}`; no check | Consistent with rejected Jira credentials being fatal: a typo in the model name otherwise exits 0 with no summaries on every run. Failing before the build keeps the previous index and needs no chat request; `/api/show` accepts every model name (a `/` in `hf.co/...` names breaks the `/v1/models/{id}` route) and is the same server the backend already targets |
| D-as (4.2 review, open #28) | Purpose may be **empty** (stored `NULL`, `show` omits it) when the ticket states or clearly implies no reason; the prompt forbids talking about the ticket or missing information and restating the summary; the ticket is fenced between `<<<TICKET` / `TICKET>>>` as content, not instructions (markers inside defused). Validation: blank summary, overlong fields, control characters other than whitespace, meta phrases ("the ticket", "not stated", "does not specify", …) and a purpose whose content words all appear in the summary are invalid (→ retry); whitespace runs are collapsed before storing. `purpose` stays a required `String` | `Option<String>` / an explicit "unknown" value; reject newlines (retry) instead of collapsing; fuzzy/LLM-judged restatement check | An empty purpose tells 4.3 there is no "why" instead of a made-up one; a required string keeps the strict schema simple (an optional field may be omitted under `json_schema`). Collapsing whitespace costs no call. The word check is cheap and catches literal restatements; synonym paraphrases slip through (open #28). Real run: 30/79 empty purposes (27 without description), no meta text, 0 retries. **Amended (4.3 review, D-bb):** narrowed to summary openers and source/missing-information phrases in the purpose |
| D-at (4.3) | The member's own source (declaration and body, dedented) goes into the prompt, capped by the new `describe.body_chars` (default 1500 characters, `[source truncated]` marker; `0` = signature and Javadoc only, as the plan wrote) | Signature + Javadoc only (plan text); the whole body uncapped | Measured on 93 `argus` members: without the body most members (no Javadoc) get a `what` that paraphrases the name and sometimes borrows from the ticket text (invented "read fallback strategy", wrong injected dependencies); with it the `what` names the actual effects (events published, pre-warming, limits, admin bypass), checked against the code. It costs nothing measurable (1.87 vs 2.05 s/member, prompts ≤ 1012 tokens). The body is part of the prompt, so a body change misses the cache, which is correct |
| D-au (4.3) | History selection: first ticket = oldest `first_date`, then the `describe.recent_tickets` (default 3) others with the newest `last_date`; `%aI` dates compared in UTC, ties by key; unavailable tickets omitted; commit subjects (`describe.commit_subjects`, default 5, distinct, newest first) only when no ticket is available. A chosen ticket without a summary this run **or any ticket the index does not know** (never fetched: `--offline` without cache, failed fetch) makes the member `describe_incomplete`: not asked, asked again next run. Ticket summaries never influence which tickets are chosen | Treat unfetched tickets as unavailable; require summaries of all tickets, not only the chosen ones; string date order | The plan's "never cache from incomplete input": an unfetched ticket may turn out available, so a record built without it would be cached under a prompt that later changes anyway, but meanwhile show a weaker what/why; skipping is the same rule as a missing summary. Choosing independently of summaries keeps the prompt identical once they exist. UTC order avoids DST offsets reordering same-day commits. **Amended (4.3 review, D-az, D-ba):** only *chosen* tickets can block; an unfetched ticket competes in the selection; an invalid summary falls back to the Jira title. **Cost note:** the members' prompts carry the 4.2 ticket summaries, so any change to the summary prompt (or model, effort, temperature) re-summarises every ticket and then misses the describe cache for every member that has a ticket: a full re-describe, ~20 min on `argus` (729 members at ~1.9 s) |
| D-av (4.3) | `Description { what, why }`: `what` one verb-first line (prompt ≤ 150, valid ≤ 200), `why` from the change history only (prompt ≤ 200, valid ≤ 300), empty (`NULL`) when the history gives none; ticket purpose sent as `Reason:`, a `NULL` purpose omitted; context fenced `<<<CONTEXT` / `CONTEXT>>>`; meta phrases ("this method", "the ticket", "javadoc", "not stated", …) and a restated `why` are invalid (retry); whitespace collapsed before storing | `why` from code too; reject newlines | Same anti-invention approach as D-as. Real run: 0 invalid replies, 0 retries; only 2 empty whys (most members have a ticket); accessor whys tend to be generic (open #30). **Amended (4.3 review, D-bb):** the meta check is narrowed (openers for `what`, source/missing-information phrases for `why`) **Superseded (4.6, D-bm, D-bo):** one `description` field, no length caps, no restate check. |
| D-aw (4.3) | Describe stage runs after summaries on every run, including a non-git repository (code-only prompts); per-stage `summary_llm` / `describe_llm` stats, the `summaries:` line shows its stage's calls and a `describe:` line shows `described (… from cache)`, invalid, failed, incomplete, skipped and the stage's calls; `[describe]` settings travel on `Summarizer` (`with_describe`) so `build_index`'s signature is unchanged; sequential calls (D-ap) | Run only on git work trees; a run-wide chat count only; a new `build_index` parameter | A structure-only index still benefits from `what`. With two LLM stages a run total no longer tells which stage called the model. 1.86 s per call is dominated by the model, so concurrency would only help with `OLLAMA_NUM_PARALLEL` > 1 |
| D-ax (PO, after 4.3) | **No MCP server: Annatar stays CLI-only.** Phase 6 is removed from `plan.md`; 6.2's agent trial moves to **5.4** and uses `annatar search` / `annatar show` through the agent's shell tool; POC question 3 now reads "a coding agent using the `annatar` CLI". The `serve` stub subcommand is removed; `project.md`'s interface table lists CLI commands (`search`, `show`, `module`, `trace`) instead of MCP tools; MCP is listed under **Later** only as "revisit if a CLI proves insufficient". | `rmcp` server over stdio (old 6.1) | Product owner: coding agents drive CLIs well, so a second interface adds code and test surface without answering a POC question. Consequence: the CLI output is the agents' interface, so it must be compact, stable and free of log noise (open #29 now gates 5.2; plan 5.2 says so). |
| D-ay (4.3 review, bug fix of a default) | Default `ticket_regex` is `\bGRLD-\d+` (was `\bGRLD-\d+\b`), in `config.rs`, `annatar.toml` and plan 0.1 | Keep `\b` and document it; `\bGRLD-\d+(?:\b|_)` with the `_` stripped | `\b` does not match between a digit and `_` (both word characters), so branch-style subjects (`GRLD-98595_Fix_login`, 59 commit subjects in `argus`) yielded no key at all. Without the trailing `\b` the greedy `\d+` still takes every digit, so keys stay exact (`GRLD-123` never cut from `GRLD-1234`), and the leading `\b` still rejects `XGRLD-1`; Jira's `is_plain_key` check is unchanged. Ticket keys are derived from the cached commits on every run, so no cache is invalidated; the new keys are fetched and summarised once (`argus`: 197 distinct keys in all commit texts vs 194 before) and the members they touch get new prompts. A custom `ticket_regex` is the user's own |
| D-az (4.3 review, finding 1) | Only the tickets the selection *chooses* can make a member incomplete: a ticket not in the index (`TicketState::Unknown`) competes like an available one and blocks only if it would be the first or one of the recent ones. Permanent fetch failures (`FetchError::is_permanent`: 403/404, another 4xx except 408/425/429, an unparseable issue, an invalid key) are cached unavailable (amends D-ac); a non-JSON 2xx is the new transient `NotJson`. `TicketFetch::from_config` returns `JiraMode` (`Fetch`, `Offline` = `--offline` with Jira configured, `Disabled` = no `[jira]` or no token, also under `--offline`); `build_index` takes `&JiraMode`; with `Disabled` the describe stage counts uncached tickets as unavailable, so members fall back to commit subjects. `Incomplete` lists the blocking keys (not fetched / no summary); the stage warning lists them, sorted, at most 10 (`+N more`); the summaries warning lists the invalid keys the same way | Keep "any unknown ticket blocks"; treat every unknown ticket as unavailable; cache parse errors only after N runs | An unknown ticket that would not be chosen cannot change the choice whichever way it resolves (removing a non-chosen candidate leaves the oldest and the newest N as they are), so waiting for it only cost coverage; one that would be chosen still waits, keeping D-au's "never cache from incomplete input". A permanent failure never fetched before blocked its members forever. Without Jira configured nothing will ever be fetched, so waiting is pointless; `--offline` is a temporary state of a configured Jira, so its unknown tickets keep blocking. Caching an unparseable issue risks keeping a ticket that a later Jira change would make parseable (drop `ticket_cache` to refresh, as for 403/404); a proxy page is excluded by the JSON check |
| D-ba (4.3 review, finding 2) | A chosen available ticket whose 4.2 summary was **invalid this run** (`InvalidOutput` after the retry, returned by `index_summaries`) enters the prompt with its Jira title in place of the summary and no `Reason:` line (`describe::Summary::Title`). A summary missing for any other reason (circuit breaker, `--no-llm`, backend failure) still makes the member incomplete | Keep skipping the member; leave the ticket out of the prompt; use the title for every missing summary | A ticket the model cannot summarise (every run, deterministic at temperature 0) otherwise blocks every member that chooses it and, in 4.4, their types. The title is deterministic input, so the prompt is a stable cache key; once a later run summarises the ticket validly, the prompt changes and the member is re-described. A breaker trip is transient and says nothing about the ticket, so caching a title-based record then would keep a weaker what/why that a normal run replaces anyway — it still waits. Under `--no-llm` an invalid summary is a cache miss (skipped, not invalid), so such members wait there even if a normal run described them from the title (accepted) |
| D-bb (4.3 review, finding 3) | Meta-text checks narrowed. `what`: only the openers "This method / This constructor / This function". `why` and ticket `purpose`: the shared `summaries::REASON_META_PHRASES` ("the ticket does/states/says/…", "according to the ticket", "this ticket", "the commit", "commit message", "the javadoc", "change history", "not stated/specified/mentioned/provided/given in", "no reason (is) given/stated", "no specific reason", "does not state/specify") plus, for `why`, "this method/constructor/function"; a whole reply that only says "Not specified." / "Unknown" / "None" / "N/A" is invalid. Ticket `summary`: only the openers "This ticket / The ticket" (amends D-as, D-av) | Keep the broad substring list; an LLM judge | The broad list rejected ordinary domain text ("Uses the default page size when the size is not specified.", "Falls back to the default locale if one is not provided.", "Returns the ticket price …", "Prevents sessions being revoked for no reason."), costing a retry and, twice in a row, the record. The narrowed phrases still catch every meta reply seen in the 4.2/4.3 probes. Loosening a check never invalidates cached rows (they are re-validated on a hit and still pass) |
| D-bc (4.3 review, findings 5, 7, 10) | The source excerpt replaces the `Signature:` line (the signature is sent only when `body_chars = 0` or the source is empty; an abstract/interface method's excerpt is its declaration, sent once); merge commits (`Merge ` prefix) are left out of the commit-subject fallback before dedup and cap; `LlmClient::reset_invalid_streak` runs at the start of each LLM stage, so the 5-in-a-row breaker counts per stage (amends D-an) | Keep both lines; keep merges; one streak per run | The excerpt starts with the declaration, so the signature line only repeated it (and twice for abstract methods). Merge subjects ("Merge branch 'x' into master") say nothing about why the code exists. Four invalid ticket summaries followed by one invalid description are not evidence of a broken model in the describe stage. The prompt change misses the describe cache for every member once (re-describe on `argus` in the real run below) |
| D-bd (before 4.4, open #13) | The walker treats a file as test code when a `src` component is directly followed by `test` anywhere before the first `java` component (`backend/src/test/java/…`, `a/b/src/test/…`); `src/test` inside a source root (`src/main/java/com/acme/src/test/…`) stays a package, `src/testng` and `test/src` stay production | Repo-root `src/test` only (1.1); any `test` directory; Maven/Gradle module detection | Plan 1.1 already says skip `src/test/`; `argus` keeps its tests in `backend/src/test`, so 55 test files (and their members) were indexed, described (≈ 20 % of the chat calls) and would pollute search. Same "not under a source root" rule as the build-dir pruning (decision 25). Drops their history/describe rows from the next index; their cache rows stay unused. **Amended (4.4 review):** the old rule stopped at the first component named `java`, so a module directory `java` (`java/src/test/java/A.java`) hid the test set. Now the source set is the `<set>` of the first `src/<set>/java` triple (else the first `src/<set>` pair, for `src/test/kotlin`, `src/test/resources`), and Gradle-style test sets count too: `test`, a name starting with the word `test` (`testFixtures`, `test-fixtures`; the next character is not a lowercase letter) or ending with `Test` / `-test` / `_test` (`integrationTest`, `androidTest`, `functionalTest`). `testing`, `testng`, `contest` stay production; Maven's `src/it` is not recognised. Plan 1.1 says so. No `argus` file changes (its only test tree is `backend/src/test`) |
| D-be (4.4) | Type prompt: kind, fqn, Spring role, `Nested in:` (enclosing kind + fqn, no role), Javadoc, the declaration with members and nested types cut out (from the start of their Javadoc's line; blank lines dropped; dedented; capped by the existing `body_chars`, `Signature:` only without it), the children's `what` lines, the type's history. Own response type `TypeDescription` (same fields, so its own schema in the cache key); `what` asks for one responsibility summing up the members, verb first; validation rejects "This class/interface/enum/record/annotation/type" | Fields extracted by tree-sitter into `Symbol`; signature + annotations only; reuse `Description` | The outline gives annotations (`@Service`, `@DynamoDbBean`), header with extends/implements, fields (Lombok classes have no methods), enum constants and annotation elements at no parser change; cutting from the Javadoc line keeps member docs out. Fields are worth it: on `argus` most entities/DTOs have Lombok accessors only. The enclosing role is left out (the outer's own `what` comes later, bottom-up, so it cannot be sent) **Amended (4.4 review, finding 5, 11, 13):** a cut now starts at the comments directly above the member (its Javadoc, other `/* */` and `//` comments, no blank line in between; a Javadoc after code on the same line, `int z; /** h */ void h()`, too) and at the line start when only whitespace precedes it, and ends at the line end when only whitespace or a `//` comment follows (`} // trailing` no longer leaves debris that broke the dedent); a comment after a blank line, a trailing comment on a field and a `/* */` after code stay. Enum constant bodies and static/instance initializer blocks (new `Symbol::blocks`, from tree-sitter) are collapsed to `{ … }` instead of being sent in full. The cut range is the pure `describe::cut_range`, so the indexer's `declaration_start` is gone. A record's compact constructor (signature without `(`) is listed as `compact constructor Point` instead of `constructor Point()`. The children heading is "Methods, constructors and nested types" ("… : (none)"), since fields stay in the declaration. These change the type prompts, so every type is re-described once (~6 min on `argus`); member prompts are unchanged |
| D-bf (4.4) | Children listed: public first (explicit `public`, or not `private` in an interface/annotation type), then the rest, each in source order, at most `describe.type_members` (default 30), shown in source order, the rest counted "(N more not listed)". Type history: `select_history` with `describe.type_recent_tickets` (default 3), commit fallback and blocking as for members. Both are new options (D-ab style: add an option rather than change a default) | Sort by kind; count tokens; cap the type's tickets lower (1–2) | Visibility is the cheapest proxy for "what the type offers". 30 lines × ~110 chars stays far below the context window; the largest `argus` type has 16 children, so the cap never fired there (tested with `type_members = 1`). The first ticket is the type's origin and the recent ones its current role; the file-wide ticket list is already cut to 4 **Ranking rule recorded (4.4 review, finding 12; unchanged):** `is_public` reads the declaration's own modifiers from its signature (annotations already stripped, only the part before the first `(`), not its effective visibility: `public` counts in a class, enum or record; in an interface or annotation type everything not `private` counts (abstract, `default` and `static` methods, nested types); `protected` and package-private rank with `private`; a public member of a non-public nested type still ranks public; a record's implicit accessors are not symbols. It only orders the choice under the cap, which never fired on `argus` |
| D-bg (4.4) | Bottom-up (deepest nesting first, then walk/source order). A *listed* child (member or nested type) without a `what` this run (`Missing`: incomplete input, breaker, `--no-llm` miss, failed call) holds the type back (`types_incomplete`), which then holds back its outer type; a child whose reply was *invalid* is listed as "(not described)"; a child left out by the cap never blocks | Describe with whatever is there; skip invalid children silently; hold back on invalid too | Same rule as D-au/D-ba: the cache never holds a type built from partial input, and a deterministic stand-in for an invalid (temperature 0, reproducible) child keeps the prompt a stable key instead of blocking the type forever. A later valid `what` changes the prompt and re-describes the type, as the plan wants ("a changed method misses the cache for its class"). Under `--no-llm` an invalid member is a cache miss, so its types wait (accepted, as in D-ba) **Wording (4.4 review, finding 8):** a changed method misses its class's cache only when its `what` text changes and it is listed (within `type_members`); an edit that leaves the `what` as it was, or a member beyond the cap (only counted), keeps the class's entry (plan 4.4 says so) |
| D-bh (4.4 review, finding 4) | A reply that names a ticket key — a non-empty match of the configured `ticket_regex` — in any field is invalid (retry, then skipped like other invalid replies): ticket `summary`/`purpose` (its own key too), member and type `what`/`why`. The stages get the regex from `build_index` (`DescribeInputs` bundles it with the invalid summaries and the unknown-ticket mode); the validators take it as a parameter. The type prompt also says "never mention tickets, ticket keys, …"; the member and ticket prompts are unchanged (validation alone, so their caches stay) | Generic `[A-Z][A-Z0-9]+-\d+`; allow a summary to name its own key; prompt only | A key means nothing to someone searching code and repeats what `show` lists anyway; the generic pattern also matches domain text (`UTF-8`, `SHA-256`, `ISO-8601`), the configured one is exactly what the repo calls a key. Consistent everywhere, so a summary cannot leak a key into the member prompts as text. A tightened check re-validates cached rows on a hit: on `argus` 3 cached whys (`… as part of the GRLD-24681 improvement.`) became misses and were asked again; no cached summary or purpose named a key |
| D-bi (4.4 review, findings 6, 7) | Type `why` meta check narrowed like D-bb: a `what` or `why` *opening* with "This class / interface / enum / record / annotation / type", or a `why` containing "this <kind> exists / is / was" anywhere, is invalid; "for this type of request", "this class of duplicates" pass. Members keep "this method / constructor / function" anywhere in `why`. The type prompt adds: "Say only what the declaration, Javadoc, members and history below show: do not guess a meaning, platform or use that only a name suggests." | Keep the substring list; skip types with nothing but a name; a fixed `what` for empty types | The substring check rejected ordinary domain text. The prompt sentence targets the inventions seen on `argus` (`Privilege.User` "… for access control", `TokenType` "mobile-specific"): it costs nothing extra since the outline and heading changes re-describe every type anyway. Whether it is enough is reviewed in 4.5 (open #32) **Amended (4.6, D-bo):** only the openers remain; the "this <kind> exists/is/was" check is gone. |
| D-bj (4.5, open #30, #32) | Member and type `why` prompts tightened: the reason must be what the history states or implies **for this member/type itself**; the prompt says that many changes touched whole files (features built around it, upgrades, migrations, refactorings), that a reason fitting every member of the enclosing type is not specific enough, and that a history naming only a project or initiative ("a proof of concept for X") gives no reason; member `why` is **always empty for getters, setters, `equals`, `hashCode`, `toString` and constructors that only store their arguments**; a nested type gets a reason only when the history explains it, not its enclosing type (sentence sent for nested types only). Type `what` adds "do not expand abbreviations or parts of names that nothing below explains". Validation unchanged | Fewer tickets for trivial members (first only); detect accessors in code and skip the history; a fixed `what` for name-only types; keep the 4.3/4.4 wording | Measured on 34 reviewed symbols (`quality_review_4_5.md`, figures as corrected in the 4.5 review fixes): strictly correct whys 32 % → 35 %, wrong 12 % → 9 %, vague 56 % → 21 %, empty 0 → 35 %, so the acceptable (correct or rightly empty) 32 % → 71 % is mostly vague turned empty; the wording was tuned on two `--path` prefixes, whose 15 reviewed symbols went 33 % → 80 % acceptable, the 19 held-out ones **32 % → 63 %**; `TokenType`'s invented "mobile" gone; a first, softer wording ("usually empty for boilerplate") emptied only 13/93 member whys in `--path` tests, so the rule names the accessor kinds. Empty whys are fine for search (5.1 embeds fqn + what + why). Prompt-only, so no code heuristic can misfire on a non-trivial `getX`. Costs: one full re-describe (18 min on `argus`); 240 member and 35 type whys emptied index-wide (none gained), 195 of the members accessors/constructors/`equals`…; of the other 45, ≈ 5–8 lost a reasonably supported, implied reason (goal-like ticket summaries without a purpose, e.g. "cache is updated immediately": `DynamoDBEventService#publishAsync`, `DynamoDBEventTableService#init`/`#tableExists`; also `UserTokenService#removeToken`, `OAuth2RequestCacheService#find`, `CronusConnector#createSystemJwt`). **Amended (4.5 review fixes):** the accessor/constructor rule is **prompt-only and best-effort** — 13/120 trivial accessors and 12/44 field-storing constructors still have a why (≈ 85 % followed), plus 12 constant-returning `getCachePrefix`/`getCacheTTLSeconds` overrides; a kept why on such a symbol counts as non-compliance in reviews, not as correct. The "not specific enough" sentence had little effect on per-initiative themes (one why verbatim on 33 symbols, 54 with variants; open #30). A code rule (skip the LLM why for detected trivial accessors) stays the fallback if 5.3 shows the leftovers hurt **4.6 (D-bm):** the guidance is kept in condensed form for the reason part of the single description. |
| D-bk (4.5, open #28) | Ticket `purpose` prompt: a title alone (or a description that is only a link) rarely gives a reason; then purpose is empty unless the title names a goal or effect beyond the change it asks for; fixing a named error or doing the named task is not a reason by itself. **Amended (4.5 review fixes):** 10 purposes became empty in total (25 → 35 of 69); one, GRLD-40363 (description only a chat link, so title-only; it restated "invalidate tokens when an organisation is deleted"), was the only stated reason for the origin of `TokenValidationRest#deleteAllFromList`, whose why then took the emergency-logout purpose of a later ticket (review #3, C → V). Accepted: the summary still carries the title, and the fix belongs to per-member ticket choice (open #30), not to restoring restated purposes | Leave restated purposes (members get the summary anyway); a code rule "no description → no purpose" | The 4.2 restatement check is word-based and missed synonym paraphrases of titles ("To secure the cache service …", "To resolve a ConcurrentModificationException …"). On `argus` 30 of the 31 tickets without a description now have an empty purpose (22 before), tickets with a stated reason keep it (13 reviewed: acceptable 8 → 12). A code rule would drop the reason a title can carry ("… → order of events no longer matters"). Changing the summary prompt re-summarises every ticket and so re-describes every member (D-au cost note); done together with D-bj in one full run |
| D-bl (4.5) | **Golden set:** TOML, `[[question]]` with `text`, `expect` (the fqn the question is about first, then acceptable alternates), optional `kind` (stored kind of the first fqn: `class` … `constructor`) and `note`; `golden::GoldenSet::parse` / `load` validate it (non-empty, distinct texts ignoring case/whitespace, fqn shape, no repeated fqn, `kind` known and consistent with `#` / `#<init>(`); `golden::check_index` reports expected fqns missing from an index and a primary with another kind. **Real sets stay out of git** (`.annatar-local/`, gitignored; `argus`: `.annatar-local/golden-argus.toml`, 20 questions); the repository has a synthetic fixture for the sample project, an integration test and an `#[ignore]` test that checks a real set against a real index (`ANNATAR_GOLDEN_SET`, `ANNATAR_GOLDEN_DATA_DIR`). **Writing rule:** questions come from the code and the tickets, as a developer new to the repo would ask them; they avoid identifier names and never paraphrase the generated what/why (measured overlap with the new what/why: median 1 content word). 5.3 scores the first fqn and "any of `expect`" | Commit the real set (as earlier fixtures, scrubbed); questions only, fqns in a private file; a `annatar golden check` subcommand now | Earlier fixtures were scrubbed of employer package paths (Phase 3 review fixes), and real fqns are those paths, so the real set cannot be committed; the format, loader and check are what 5.3 needs and are tested on synthetic data. Paraphrasing the generated text would measure the model's agreement with itself and inflate 5.3. A CLI command waits for 5.3's evaluation command (the CLI is the agents' interface, D-ax). Consequence: the golden set lives on the developer's machine; losing `.annatar-local/` loses it (the product owner may keep a copy). **Amended (4.5 review fixes, orchestrator's call, product owner to confirm in open #34):** package-relative fqns (root package left out) in md docs are acceptable — `state.md` uses them throughout earlier phases — so `quality_review_4_5.md` stays committed; no full package prefix appears in the current tree (`git grep HEAD`); in history one remains: commit `abc05c7` (Phase 3 Jira fixture `tests/fixtures/jira/rich_description.json`, a company library package in a ticket's code sample; scrubbed in the next commit `9b44532`). Removing it needs a history rewrite of a pushed branch, so it is left for the product owner (open #34). The real set stays out of git not only for its paths but because it is usable only against the private `argus` index and is a complete map of internal questions. It now has 24 questions / 56 fqns and exists **in one copy only**, `/workspace/.annatar-local/golden-argus.toml` in the agent container (a pre-fix copy is in the session scratchpad); the product owner should back it up or decide where it lives (open #34). Validation also rejects whitespace in fqns (the index stores parameter types without it), and the validated types are built only through `parse`/`load` |
| D-bm (PO, 4.6) | **Product-owner decision:** "Move from the pure what / why to a simple general symbol description with the code itself the ticket and the commit history as context. There should be no length limit only the instruction to describe it as short as possible." Every method, constructor and type gets one `description` (response field and `symbols` column) instead of `what` + `why`. Recorded as a new plan step 4.6; plan 4.3/4.4 keep their history and are marked superseded; their selection (D-au), determinism / `--path` independence, hold-back (D-az, D-ba, D-bf, D-bg) and caching rules stay. The prompt keeps D-bj's anti-borrowing guidance in condensed form (reason only when the tickets or commits say why this symbol was created or changed; none when it fits every member, names only a project/initiative/upgrade, or for accessors, `equals`/`hashCode`/`toString`, field-storing constructors) and D-bi/D-bj's anti-invention sentences for types | Keep `what`/`why`; `what` + optional `why` concatenated in code; a length cap | PO request. One field lets the model write a reason as part of the behaviour when one exists, and nothing when none does, without the empty-why bookkeeping. Measured on `argus`: median 105 / 147 chars (members / types), 99 % one sentence, 0 wrong in a 20-symbol spot check (4.5 after: 3 wrong whys in 34); cost: fewer reasons — own-ticket reasons are often left out (e.g. `removeAllTokens(List<UUID>)`), the first wording ("only when … explain it for this method itself") dropped almost all, the final one ("when the tickets or commits say why … add that reason briefly") brings back some. One full re-describe (15 min 29 s) **Amended (PO, 4.6 follow-up):** "no reasons should not be droped. as short as possible without loosing information." The prompt asks for a description "as short as possible without losing information" and to **keep** any reason the code, Javadoc, tickets or commits give for this symbol being created or changed ("never drop it to make the description shorter"); the borrowed-reason caveats (fits every member, names only a project/initiative/upgrade, file-wide changes) and "do not read a meaning or a reason into names alone" stay, phrased as what is not a reason. The accessor/constructor rule is kept but narrowed to its borrowed case: getters, setters, `equals`/`hashCode`/`toString` and field-storing constructors "rarely have a reason of their own: give them one only when the history explains this very {kind} or its field" — their tickets are nearly always the file's. Collective terms instead of field-by-field enumerations, keeping distinguishing details. Measured on `argus`: members with a reason clause 34 → 62 (types 47 → 74), own reasons regained (`removeAllTokens(List<UUID>)`, `getSubscriptionFromPactum`, `isTokenValid`); cost: more borrowed reasons (spot check of 15: 6 correct, 4 vague, 4 borrowed, 1 wrong — `OAuth2RequestCacheService`'s object-size reason is back), accessor/constructor rule ≈ 85 % followed (22/176 trivial accessors, 12/44 field-storing constructors carry a reason, 4.6: 11 and 5), median 105 → 126 chars, 19 min 21 s per full run |
| D-bn (4.6) | Context: members send the capped source (`body_chars`), types the outline and their listed children's **full** descriptions (no per-child cap); tickets as before (first + `recent_tickets`, 4.2 summary + `Reason:` purpose, Jira title only as the D-ba fallback — **no raw title next to the summary**); **commit subjects always** (newest `commit_subjects` = 5 distinct, merges and blanks left out), for members and types, also when tickets are available. `History` is a struct `{ tickets, commits }`; the prompt shows `Tickets:` and `Commits:` sections, each `(none)` when empty | Raw title too; commits only as a fallback (4.3); cap child descriptions in type prompts | The PO named the commit history as context. Titles are mostly German and already condensed by the summary (which saw title + description), so they would add tokens, not facts. Commit subjects often carry the ticket key; 0 replies named a key on `argus` (D-bh still rejects them). Children are listed in full because descriptions are short (p90 193 chars) and capped at 30 per type; the longest type prompt was 1425 tokens on `argus` (corrected in the 4.6 review fixes: annatar has no input-truncation check, so "no truncation warning" meant nothing; the stats now print the per-stage peak prompt and completion tokens, to compare against the model's context size). Commit subjects are per symbol, so prompts stay independent of `--path` **Amended (4.6 review fixes):** merge subjects are those starting with "Merge " or "Merged " (Bitbucket "Merged in …", "Merged master into …"). A parent count (`%P`) would be exact but needs a new history cache table, a `symbol_commits` column and a cold history run; `git log -L` on `argus` lists only 5 merge commits among 272 distinct commits, all "Merge …", so the prefix list is enough |
| D-bo (4.6) | Validation of a description: non-blank, no control characters, no ticket key (D-bh), no opener "This method/constructor/function" (members) or "This class/interface/enum/record/annotation/type" (types), no source or missing-information phrase (`summaries::source_meta_phrase` = D-bb's `REASON_META_PHRASES` **without "the commit"**, plus "commit history", plus a whole reply "Not specified"/"Unknown"/"None"/…); whitespace collapsed before storing. Dropped: length caps (`MAX_WHAT_CHARS`/`MAX_WHY_CHARS`), why-restates-what (D-av), "this method" anywhere and "this <kind> exists/is/was" (D-bi), which were `why`-specific. `finish_reason=length` detection (4.1) stays | Keep a generous cap (e.g. 1000); keep the full D-bb list | PO: no length limit. "the commit" is domain text in a description of behaviour ("publishes the event after the commit"); "this class is required" in the middle of a description is harmless once the opener is caught. Ticket summaries keep their own caps and checks (4.2 unchanged) **Amended (4.6 review fixes):** the meta check is description-specific (`describe::description_meta_phrase`): only clearly meta phrases, built from sources (ticket(s), commit(s), commit message(s), commit/change history, history, Javadoc) × "according to the …", "the … says/states/shows/mentions/explains", "the … does not / don't say/state/mention/specify/explain/give", "not stated/specified/mentioned/given/provided in the …", plus "no reason (is) given/stated/provided" and the bare stand-in replies; D-bb's list stays for the 4.2 purpose only. It rejected behaviour descriptions ("Returns null when the request does not specify a locale", "Records the change history of an order", "Closes this ticket …"). **Safety cap:** `ollama.max_tokens` (default 1024, `0` = not sent) bounds a looping reply, which would otherwise run to Ollama's limits or the 600 s timeout and trip the breaker for the whole run; it is not a length limit (the prompt mentions none; peak completion on `argus` 84 tokens, so the default is 12× above any real reply). A `length` reply is invalid even when it parses (retried once, never cached). Not in the LLM cache key: only complete replies are cached and a cap does not change a complete reply, so keying it would only re-ask cached summaries. With reasoning on, reasoning tokens count against the cap (probed), so raise it then |
| D-bp (4.6) | `show` prints `- description:` for the symbol and every child in full; no display truncation. Stats lines and counters keep their names (`describe:`/`types:`, `described`, …) | First sentence or a truncated line for children | 99 % of descriptions are one sentence and p90 is ≈ 200 chars, so truncation would save little and `show` is the detail view; storage is never truncated |
| D-bq (PO, after 4.6) | **Product-owner decision:** a member whose description carries no reason is fine as long as its enclosing type's description has one ("a method on a class that has a reason is good enough"). Every symbol keeps a code-based description (all 654 on `argus`); reasons are required only where the history explains the symbol, and the type level is where they are expected. No code rule to force or suppress member reasons (the D-bj fallback is not taken). | Push for a reason on every member; or a code rule that strips reasons from accessors/field-storing constructors | Asked after the PO read "62 of 479 members with a reason" as missing descriptions: the gap is reasons, not descriptions, and the class reason covers its members. Consequence for 5.1/5.3: a member's own embedding text lacks the class reason, so evaluate whether adding the parent type's description to a member's embedded text helps the golden set. |
| D-br (5.1) | Embedded text = `fqn`, newline, description (trimmed), built by the pure `embeddings::embedding_text`; `[embedding] parent_description = true` appends a method's or constructor's enclosing type's description on a third line (types never get one; a NULL or blank parent description is left out). Default off = the plan's `fqn + description` | Parent description by default (state.md Next / D-bq); `kind` or `signature` in the text; a `fqn: description` one-liner | AGENTS.md: a config option instead of a changed default; D-bq says members may lean on their class's reason, so 5.3 can measure both with one rerun each (embedding calls only, never chat calls — the text is not part of any chat cache key). The fqn carries the class and method names, so a query naming them matches too |
| D-bs (5.1) | `symbol_vectors` lives in `index.db` but not in `INDEX_TABLES`: `schema::vector_tables(dim)` / `create_vectors` build it, and the embeddings stage calls it once it has the first vector, in the build transaction. `symbol_id INTEGER PRIMARY KEY` = the rowid `vector_top_k` returns. No vector this run → no table; 5.2 must treat a missing table as "no embeddings". The dimension is not stored separately (it is in the column type) | Store the dimension from config; a fixed `F32_BLOB(1024)`; a `meta` table with the dimension; an `embedding` column on `symbols` added by `ALTER TABLE` | The plan says the dimension comes from the first response, so the DDL can only run then; a separate table keeps `symbols` unchanged and lets 5.2 test for vectors with one lookup. Verified on libsql 0.9.30 (`core`): `F32_BLOB(3)`, `libsql_vector_idx`, `vector32`, `vector_top_k`, and a wrong-length vector is rejected ("dimension"). **Review fixes:** the model, dimension and text settings are now recorded in `index_meta` (D-bw), so a reader no longer has to parse the column type. Inspect `symbol_vectors` with libSQL (the `annatar` binary or a libsql connection): the stock `sqlite3` shell reports `COUNT(*) = 0` on it |
| D-bt (5.1) | Index options `'metric=cosine', 'compress_neighbors=float8'` (default `max_neighbors`), and `PRAGMA cache_size = -262144` (256 MiB cap) on the build connection before the vector inserts (`schema::VECTOR_CACHE_KIB`) | Default options (float32 neighbours); `max_neighbors=16`/`32`; `float1bit`; no index (exact `vector_distance_cos` scan — fine for 654 rows, but the plan asks for `vector_top_k`) | Measured on `argus` (654 × 1024, fully cached stage): default + 2 MB cache 13.3 s and 144 MB `index.db` (spills to the temp file on the slow mount); + cache 1.4 s; + float8 0.66 s, 75 MB; float8 + 16 neighbours 0.47 s, 18 MB; recall vs exact search (each vector as query) top-1 654/654 and recall@10 1.0 for all of these, `float1bit` 0.9998. float8 keeps full-precision node vectors and the default neighbour count, so it should hold on larger repos too; `max_neighbors` can be lowered later if size matters (central index build, project.md). **Amended (review fixes): `'max_neighbors=32'` (`schema::VECTOR_MAX_NEIGHBORS`).** The default (96 for 1024 dims) costs ≈ 105 KB of graph per symbol (71 of 75 MB), i.e. ≈ 450 MB and ≈ 20 s at 4k symbols, and the graph outgrows the 256 MiB page cache at ≈ 2.4k symbols. 32: ≈ 42 KB per symbol (28.9 MB on `argus`, ≈ 170 MB at 4k), stage 0.53 s, recall@10 1.0 on `argus` (own and mid-point queries) and on a 6× noisy synthetic set (3924 vectors); 16 is smaller (16.8 MB) but already drops to 0.998 recall@10 / 652 of 654 top-1 on the 6× set, so 32 is the smallest that held everywhere. The page cache is kept: with 32 neighbours it still cuts the insert from 1.63 to 0.60 s on `argus` (8.7 → 4.2 s at 6×). `vector_top_k` returns at most 200–202 rows for any `k` (open #36) |
| D-bu (5.1) | Embedding failures never fail the run: a cache-only client (`--no-llm`, or the chat breaker tripped in an earlier stage) uses `LlmClient::cached_embeddings` only (misses → `embed_skipped`); a failed `embed` call (after the backend's one transient retry) warns once, falls back to the cache (batches stored before the failure count as cached) and the rest are `embed_failed`; a cache fault in the fallback warns and leaves all without a vector. All texts go in one `embed` call (64 per batch); the embedding failure does not trip the chat breaker (it is the last stage) | Embed regardless of `--no-llm` (it is not a chat call); fail the run; per-batch fallback | `--no-llm` means "no model calls" (help text and README now say chat or embedding); a run with an unreachable server already trips the breaker at the model check, so it goes cache-only without timeouts. Vectors are content-keyed in `embedding_cache`, so the next run embeds only what is missing. **Review fixes:** a cache-only run that leaves symbols without a vector warns once (`skipped=N`); a mid-way failure is tested (batches before it count as cached, the rest failed); a changed vector length no longer counts as a model failure (D-bx) |
| D-bv (5.1) | `LlmClient::check_models` (was `check_chat_model`) also checks the embedding model with `/api/show`, after the chat model: missing → the run fails before any chat call ("pull it or fix ollama.embedding_model"); unreachable → breaker as before (embedding check skipped) | Check only the chat model; check lazily in the stage | A wrong `embedding_model` would otherwise surface only after a 20-minute chat run, as 654 failed embeddings; consistent with D-ar |
| D-bw (5.1 review) | `index_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)` in `INDEX_TABLES` (empty unless vectors exist); the embeddings stage writes `embedding_model` (the configured name), `embedding_dim` and `embedding_parent_description` (`true`/`false`) after the vector inserts, only when it created `symbol_vectors`. 5.2 compares them with its config and query vector | Columns on a one-row meta table; store only the dimension; a meta row per run with every setting | Key/value lets later steps add settings without schema churn (the index is rebuilt each run anyway); without the model name a switch to another 1024-dim model would make search silently meaningless; no vectors → no meta, so "no vectors" stays one check (D-bs). The model *name* is recorded, not its digest: a model replaced behind the same name with the same dimension is still undetected (open #37) |
| D-bx (5.1 review) | `LlmClient::embed` recovers from a changed dimension: the first live response sets it; cached vectors of another length are re-embedded (one warning: "cached embeddings have N values but the model now returns M") and their rows replaced (same key, `INSERT OR REPLACE`); when only cached vectors are involved and they differ among themselves, all are re-embedded (one warning). `cached_embeddings` (no calls) still errors on mixed lengths, with a message naming the model and saying a normal run fixes it. Live responses of different lengths or empty vectors are errors (batch not cached) | Treat mixed lengths as a permanent error (before); warn only; key the cache by dimension too | The old behaviour failed every run for good with a misleading "model failed" warning. Re-embedding costs only embedding calls (≈ 9 s for all of `argus`), never chat calls; the key stays model + text so no cache row is orphaned. A cache-only run cannot know which length is current, so it leaves all without a vector and says why |
| D-by (5.2) | `search` embeds the query through `LlmClient::embed` over an in-memory `embedding_cache` (`search::QueryEmbedder`); query vectors are never stored in `cache.db`, and `search` opens only `index.db` (read-only `IndexReader`), so it creates and writes no file | Cache query vectors in `cache.db` like symbol vectors; a separate uncached embed method on `LlmClient` | A warm query embedding costs ≈ 25 ms on `argus` (the first after the model unloads ≈ 1 s, which a cache would not help for new queries), so a persistent cache saves little; storing every agent query would grow `cache.db` with cheap, non-content-keyed rows (ground rules: only expensive results) and make a read command write to the cache while an `index` run may hold its lock. The in-memory cache reuses the client unchanged (batching, length checks, error messages) |
| D-bz (5.2) | Before embedding, `search` requires `symbol_vectors` (else "the index has no vectors; run `annatar index` with an [ollama] section …") and the `index_meta` model and dimension (else "rebuild it"), and **refuses** an `embedding_model` other than the indexed one; after embedding it refuses a query vector of another length than `embedding_dim`. `embedding_parent_description` is not checked (it changes the symbol texts, not the query) | Warn and search anyway; check only the dimension (libSQL would reject it anyway) | Vectors of two models are not comparable even at the same dimension, so a warning would hand an agent confidently wrong results; refusing first saves a request. A model replaced behind the same name stays undetected (open #37) |
| D-ca (5.2) | Unfiltered queries use `vector_top_k('symbol_vectors_embedding', q, k)`; any filter (`--kind`, `--role`, `--path`) uses an exact `ORDER BY vector_distance_cos(...)` scan of the matching rows with `LIMIT k`. Every hit's score is the exact cosine similarity (1 − distance), ordered by it, ties by fqn. `-k` is 1–100 (default 10). Resolves open #36 | Over-fetch ≤ 200 from `vector_top_k` and fall back to the exact scan when fewer than k survive; always the exact scan | Measured on `argus` (release, 654 × 1024): `vector_top_k` 13–15 ms, the exact scan of all 654 rows ≈ 15 ms and of a filtered subset 2–6 ms, with identical top-10 on 4 queries. The exact scan is always correct and as fast at this scale; over-fetching adds a code path that only matters past ≈ 10k symbols. The plan's `vector_top_k` stays for the unfiltered case, which is what scales. The limit stays well below the ≈ 200 rows `vector_top_k` returns |
| D-cb (5.2) | `--kind` and `--role` take the stored names (clap rejects others, exit 2) and may be repeated (any of them); a method or constructor matches `--role` by its enclosing type's role (`symbols.role` is set on types only), a type by its own. The global `--path` limits `search` to files at or under the prefix by whole path components (`src/we` does not match `src/web/…`), normalised like `index --path` (`walk::relative_prefix`, now `pub`); a prefix outside the repository is an error | Role filter on types only; a single value per filter; ignore `--path` in `search` | "the controller endpoint that …" is the natural agent question, and its answer is a method; repeated values cover "any type" (`--kind class --kind interface …`). `--path` is global, so silently ignoring it would mislead; `project.md` lists it for `search` **Amended (review fixes):** the role is the *enclosing type's own* role, so members of a nested type inside a role-tagged type match by the nested type's role (usually none) — kept for simplicity and documented, 5.3 notes whether it costs a golden hit. `--path` that is the repository itself (`.`, `./`, its absolute path) is no filter, and `..` is applied lexically, an escape being the same error as an absolute path outside (D-ce) |
| D-cc (5.2) | Search output: per hit `{rank}. {score:.3} {fqn} [{kind}] role={role} {file}:{start}-{end}` (role only when the symbol has one) and the description on the next line, indented three spaces, whitespace collapsed; nothing else on stdout. No hits → empty stdout, `no symbol matches the filter` on stderr, exit 0. Documented in README | One line per hit with the description inline; TSV/JSON; kind/role omitted | Two lines keep the location line short and greppable (`grep '^[0-9]'`, `cut -d' ' -f3` for the fqn) while the description stays readable; `[kind] role=` is the same notation `show` uses, so an agent learns one format; the score tells how close the runner-up is **Amended (review fixes):** README says the fqn is field 3 (fqns hold no whitespace) and the location is everything after the `[kind]`/`role=` tokens, since a path may contain spaces — no `cut` hint for later fields |
| D-cd (5.2) | Logs go to stderr (`with_writer(stderr)`, ANSI only when stderr is a terminal); `main` returns `ExitCode` and prints any error as one line `error: {err:#}` (newlines replaced) on stderr with exit 1; clap's usage errors keep exit 2. `show` adds `- parent: <fqn>` for the rendered symbol (children already sit under it). Resolves open #29 | Keep anyhow's default `Error: …` + `Caused by:` block; JSON errors | The CLI is the agents' interface (D-ax): stdout carries only results (`index` stats lines, `show`, `search`), stderr logs and one error line an agent can read without a parser; the `show` detail view the plan asks for (description, tickets, commits, parent, children, file:line) was missing only the parent **Amended (review fixes):** the error itself is one line, but `WARN` logs on the way stay and may precede it (e.g. the retry warning when Ollama is unreachable, which tells the 2 s were a retry); README says so |
| D-ce (5.2 review) | `walk::relative_prefix` normalises `--path` lexically for `index` and `search`: `.` dropped, `..` pops a component, climbing above the repository → `None` (outside). `search::path_filter` maps the empty prefix (the repository) to no filter and `None` to an error; `index` keeps mapping `None` to an empty walk | Normalise only in search (leave `index` as is); canonicalise through the filesystem | One shared rule keeps `index --path X` and `search --path X` meaning the same files; the change for `index` is a fix (`backend/../backend` indexed nothing). Lexical, because the prefix may not exist and symlinks are not followed by the walker anyway. Whether `index` should also *error* on an outside prefix instead of emptying the index is open #38 |
| D-cf (5.3) | `annatar eval [-k N] <golden.toml>` in a new `eval` module: `golden::check_index` first (problems → one error, nothing searched), then each question through `search::search` with no filter (the `annatar search` path, `vector_top_k`), `-k` default 10 like `search`; scores the rank of the first `expect` fqn and the best rank of any `expect` fqn; summary top-1, top-5 and MRR (a miss counts 0, so MRR is MRR@k; the settings line prints k) for all, types and members, grouped by the primary fqn's shape (`#` = member); per-question line with the first hit when the primary is not first; output on stdout as stable lines; `--path` is an error | `annatar golden check` + a separate scorer; a test-only scorer (`cargo test -- --ignored`); group by the stored `kind`; a per-kind split (class/interface/enum/…/method/constructor); honour `--path` as a filter; JSON output | D-bl deferred the CLI command to 5.3; a command is reusable after any change to descriptions or embeddings and an agent or developer can run it like `search`. Reusing `search::search` unchanged guarantees the benchmark measures what users get. With 24 questions a per-kind split would leave cells of 1–3 questions, so two groups (as the plan's "types and methods apart"); the primary's fqn decides the group so a question without `kind` still counts. `--path` would switch to the exact scan and score a subset, so it is refused rather than silently ignored (D-cb's reasoning) **Amended (5.3 review fixes):** `eval -k` takes 5–100 (`eval::MIN_LIMIT`, clap error and exit 2 below 5), because the summary counts top-5 and a smaller k would print a top-5 that is really a top-k |
| D-cg (5.3) | Keep `[embedding] parent_description = false` as the default | Switch it on (state.md Next / D-br asked 5.3 to decide) | Measured on `argus` (24 questions, D-ch table in Done). **Amended (5.3 review fixes, orchestrator's call):** overall the variant is better on 5 of 6 metrics — primary top-1 4 → 7, primary MRR 0.358 → 0.409, any top-1 10 → 11, any MRR 0.536 → 0.576, right file first / in the top 5 12/18 → 14/19 — and worse only on primary top-5 (14 → 13; any top-5 17 → 17). Members gain (primary top-1 3 → 6, any top-1 5 → 8); types regress (any top-1 5 → 3, any MRR 0.573 → 0.468: members that carry their class's description crowd out the class itself). Per question the primary moves up for 6 and down for 9 overall, but within the top 10 for 5 up and 4 down. The default stays off not because the variant is worse — on the totals it is slightly better — but because the difference is within noise at n = 24 (one question is ≈ 4 pp; 95 % CI ≈ ±18 pp, D-ch) and the regression on types is systematic, not noise-sized in its mechanism; AGENTS.md: keep the default unless a change is clearly better. The variant stays available via config for repos or agents that ask mostly for methods; 5.4 uses the default. Cost of the switch is only embedding calls (8 calls, 8 s, 0 chat calls on `argus`) |
| D-ch (5.3) | **Question 2 verdict:** search by meaning finds the right neighbourhood but rarely the exact symbol first: on 24 identifier-free questions an acceptable symbol is in the top 5 for 17 (71 %) and the right file for 18 (75 %); the intended symbol is first for 4 (17 %), any acceptable one for 10 (42 %). These are n = 24 questions on one repository by one author: one question is ≈ 4 pp and a 95 % confidence interval is roughly ±18 pp (71 % ≈ 51–86 %, 17 % ≈ 7–36 %), so they show the order of magnitude, not a precise rate (5.3 review fixes). Good enough as "where to look" for an agent that reads the top 5 and `show`s them; not a one-shot answer. The golden set is not edited and no prompt is tuned to improve the numbers | Edit questions or add alternates that the misses suggest; tune prompts; count the 5.2 own queries (9/9 first) | Editing the set after seeing the ranks would measure the set, not the index; prompt changes would invalidate the LLM cache (≈ 20 min of chat calls) and are not this step's scope. The 5.2 queries were written by the agent with the descriptions in view, so they overstate quality; the golden set's writing rule (D-bl) is what makes this the honest number. Misses are mostly structural (a class and its main method tie, DTOs restate the behaviour, plain domain words absent from descriptions), plus two members described from a truncated source (open #39) and one wrong reason (open #30); the levers are listed in open #40 |
| D-ci (5.3 review fixes) | CLI tests clear the proxy variables and set `NO_PROXY=127.0.0.1,localhost` in the `annatar()` helper; the production Ollama client keeps honouring the environment's proxy settings, loopback included | Bypass the proxy for loopback URLs in `OllamaBackend` | The defect was test hermeticity, not product behaviour: a real Ollama on localhost works whenever `NO_PROXY` is set as usual, and the container reaches the host's Ollama through `host.docker.internal`, which is already in `NO_PROXY`. Overriding the user's proxy configuration in the client would be a surprising product change outside this fix's scope |
| D-cj (5.4) | Agent trial protocol: Claude Code 2.1.289 headless on `claude-sonnet-5` (effort medium) in both arms; tools Bash/Read/Grep/Glob; no MCP, settings files, skills, session persistence or auto-memory (checked); the repository is a read-only mount and `git status` is compared after every run; 4 tasks with an answer key written before any run, 5 runs per task and arm, arm order alternating; the *with* arm differs only by a neutral appended system prompt and the `annatar` wrapper on `PATH`; metrics from the stream (`usage`, `total_cost_usd`, tool_use blocks, `num_turns`, `duration_ms`); correctness 0/1/2 graded by a separate agent from shuffled answers with `annatar` redacted | `claude-sonnet-5-5` as the orchestrator proposed; Opus; 2 runs per arm; a placebo prompt for the *without* arm; grading by the implementer | `claude-sonnet-5-5` does not exist: the CLI silently ran `claude-opus-5-5` (seen in `modelUsage` of the first pilot), so the harness now aborts when `modelUsage` names another model; Sonnet keeps the 40 runs at ≈ $4.4. Five runs because the Sonnet pilot pair cost ≈ $0.23 (≈ $0.11 per run, so 40 runs ≈ $4.4) and its token gap on T1 (55k with vs 113k without) was not much larger than the gap in the Opus pair (59k vs 68k), so one pair per arm could not separate the effect from run-to-run noise. The main runs confirmed the noise (T1 *without* 82k–221k tokens), but that was not known when n was fixed (corrected in the 5.4 review fixes; the earlier text cited it as the reason). The appended prompt describes output and limits ("check the code") but does not push the tool, so its use is the agent's choice; a placebo prompt would add tokens without information. A separate grader keeps the arms blind (one answer named the tool, redacted); the blinding is imperfect: the redacted answer still reveals its arm ("[index]'s description text is stale", T1-with-1), and the *with* answers' ticket reasons on T2 make the arm guessable — acceptable because T1 is full marks in both arms and the T2 reason is graded against the key's fact, not by preference (5.4 review); `--permission-mode dontAsk` was overridden by the session's bypass mode in the child, so read-only rests on the mount and the tool set, verified per run |
| D-ck (5.4) | Commit the harness as `scripts/agent_trial/run.py` (Python stdlib; `run`, `summary`, `blind`), generic: the task texts, answer key, raw streams, blind answers, mapping and grades stay in `.annatar-local/agent-trial/` (they name employer code); no Rust change. **Amended (5.4 review fixes):** the pure functions (`annatar_calls`, `parse_stream`, `rows`, the `PATH` check) have stdlib `unittest` tests in `scripts/agent_trial/test_run.py` with a made-up fixture in `scripts/agent_trial/fixtures/`, run with `python3 -m unittest discover -s scripts/agent_trial` (also in the script's docstring); they are not part of `cargo test` | Keep the script in the scratchpad; a Rust benchmark; no tests (the original choice); wiring the Python tests into `cargo test` | A measurement tool, not product code: committed so the trial can be repeated on another repository (open #41) with a private task file. It was exercised end to end by the 40 runs and the pilot, but the review found parser gaps (`VAR=x annatar`, `$(annatar …)`, `annatar -k 5 search`) that end-to-end runs would not reveal, so the parsing and aggregation are unit-tested; a Rust test calling Python would add an interpreter dependency to `cargo test` for a script outside the crate |
| D-cl (5.4) | **Question 3 verdict:** yes on this repository for cost — with `annatar` the agent spent about half the tokens (116k vs 221k, −47 %), 40 % fewer tool calls, 32 % less money and 39 % less time, lower on every task and significant per task on T1–T3 (tokens and tool calls, exact rank test, uncorrected; T4 a trend, tokens p = 0.06, tool calls p = 0.08). Correctness is **net equal (35 vs 35 of 40) with opposite effects**: better where the reason lives only in tickets (T2: the redirect-loop reason 5/5 with vs 0/5 without, p ≈ 0.008), worse on call-chain completeness (T4: 4 of 5 *with* answers started at the consumer search returned and missed the queue dispatcher that all 5 *without* answers traced, p ≈ 0.05; no *with* answer got full marks on T4, 0/5 vs 5/5) — the T4 regression is a finding, not a caveat: a search hit can replace tracing from the entry point (open #42). Hypothesis, not shown by 4 tasks: the token gain depends on how far grep is from the answer — T1 (question words ≠ code words) −62 %, T3 (why) −63 %, but T2 (why) −48 % and T4 (a flow whose words partly match the code) −29 %. Caveats: n = 5 runs × 4 tasks, one small repository (162 main files, ≈ 7k lines, where grep is cheap — a larger one may widen the gap, unmeasured), one model, tasks and key by the author who built the index, an LLM grader with imperfect blinding (D-cj), one answer regraded after unblinding (strict, the review's call); total tokens are ≈ 90 % cache reads, so cost (−32 %) is the fairer money measure. **Amended (5.4 review fixes):** was "as correct (36 vs 35)", "tool calls significant on all four (p ≤ 0.03)" and "the gain is largest where …" as a finding | Call it unproven until a larger trial | The direction of the cost metrics is consistent on every task, and three of four tasks are significant on their own (0.008 is the smallest p 5 vs 5 allows, so no multiple-comparison correction could pass any of them — the evidence is the consistency, not one p-value); correctness is mixed in a way that says what to fix rather than whether to use it; the size of the effect and its transfer to larger repositories and other people's tasks are open #41; the T4 failure mode is open #42 |
| D-cm (5.5) | `annatar search` prints files by default: `search::search` (unchanged, same filters) for the `MAX_LIMIT` = 100 nearest symbols, `group_by_file` keeps files in the order of their best hit (ties as the hits: by fqn), the top `-k` files are shown (default 5, at most 20, `MAX_FILES`; above 20 without `--symbols` is a clap usage error, exit 2). `--symbols` keeps the 5.2 output, default (10) and range (1–100) unchanged | A new command (`search-files`); rank files by the sum or mean of their hits; fetch only 10 × k symbols; keep symbols the default with a `--files` flag | The product owner asked for files as *the* search result. Max of the hits keeps the ranking identical to the symbol ranking at file granularity (a file's rank is where its first symbol appears), so 5.3's file numbers stay comparable and a file with many mediocre accessors (DTOs) is not promoted, which a sum would do. 100 symbols cover 33–54 files on `argus` (6 queries), so 20 files never run short; one `vector_top_k` of 100 costs no measurable time (≈ 50 ms per query either way) |
| D-cn (5.5) | File output: `{rank}. {best score:.3} {path}`; each top-level type (no parent in the file) in source order as `   {kind} {fqn} [{role}] :{start}-{end}` and its description (whitespace collapsed) on the next line, indented five spaces; every member and nested type in source order (depth first), indented two more spaces per level, as the fqn after its parent's (`#find(Long)`, `#<init>(Repo)`; a nested type `{kind} {Name} [{role}]`) and ` :{start}-{end}`, without a description; a hit ends with ` *{score:.3}` | A leading `*` marker column; `[kind] role=` as in `--symbols`; full fqns for members; descriptions of nested types | As the spec asked: the type's description says what the file is about, the member lines are the reading map with lines for `Read`. The trailing `*score` works at every depth without a marker column, greps as `\*[0-9]`, and gives the agent the score gap. Relative names keep lines short (an `argus` package is ≈ 35 chars); the full fqn is one concatenation away for `show`. `[role]` after the fqn is the spec's form; `--symbols` keeps `role=` unchanged |
| D-co (5.5) | A symbol is marked as a hit when it ranks above the best hit of the first file not shown (`group_by_file` stops there): the symbol hits a `--symbols` search would need to fill k files. With fewer than k files in the 100 hits (a narrow `--path`), all of them are marked | Mark all 100 hits of a file; mark hits within a score gap of the file's best; mark the symbol top 10 | Scores are compressed (on `argus` the 40th hit is still ≈ 0.10 below the 1st), so marking all 100 would mark almost every member of the top file; a fixed gap is arbitrary per model. The prefix rule is self-consistent (every file has at least one mark, its best) and needs no constant |
| D-cp (5.5) | At most `MEMBER_LINES` = 40 member and nested-type lines per file: first every hit with its enclosing nested types, then the remaining lines in source order until 40, then `     … N more`; top-level type lines and descriptions are not counted. A constant, not a config option | No cap; a cap per top-level type; a config option or flag | Keeps one huge file from flooding the output while never hiding a hit; on `argus` the largest file has 17 symbols, so no real data decides the number yet — a constant is enough until a larger repository needs tuning (then a config option, per AGENTS.md) |
| D-cq (5.5) | `--kind`, `--role` and `--path` filter which symbols can be hits (and so which files rank and which lines are marked); each shown file is printed whole | Print only the matching symbols of a file | The point of the file view is the reading map of the file; a member filter that hid the class line and its siblings would defeat it. Documented in README and `--help` |
| D-cr (5.5) | `annatar eval` also scores files: per question `search::search_files` with `-k` files (the same grouping as `search`), the rank of the primary fqn's file and the best rank of any expected fqn's file (files from `symbols.file`); per-question line `n. <p> <a> file <p> <a> [group] …`, then the three symbol summary lines unchanged and three `files <group> …` lines. The query is embedded once: the second search hits the embedder's in-memory cache (the unit and CLI tests count one embedding request per question) | A separate `eval --files` mode; files ranked from the symbol top k; refactor `search` into embed + nearest | The symbol scores must stay comparable with 5.3 (they are identical on `argus`), so the symbol path is untouched and the file path is exactly `annatar search`'s ranking. `-k` files keeps one knob; top-5 is the default output's reach. The in-memory cache already makes the second embedding free, so no refactor of `search` was needed |

## Open questions

| # | Question | Raised at | Status |
| --- | --- | --- | --- |
| 1 | Jira auth scheme: Cloud uses email + API token (basic auth), Server/DC often uses a personal access token (bearer). Assume Cloud for now? | 0.1 | resolved (3.1, D-w) — email set → basic, else bearer PAT; PM default, product owner may override |
| 2 | Config discovery: search parent directories, or add `ANNATAR_CONFIG`? Currently only cwd/`--config`. | 0.1 | open, defer |
| 3 | Should `jira` / `ollama` sections become optional per command (e.g. `show` may not need Jira)? | 0.1 | resolved — both sections are now `Option`; commands require them when wired |
| 4 | `--path` is currently a `String`; make it a `PathBuf` once the walker lands (1.1)? | 0.1 | resolved — now `PathBuf` |
| 5 | Should `cache.db` use WAL for concurrent reader access (MCP server)? The index stays rollback-journal so rename is single-file. | 0.2 | open, defer; with no MCP server (D-ax) the only concurrent reader is a CLI `search`/`show` during an `index` run, which reads `index.db`, not `cache.db` |
| 6 | `Store` exposes no read connection to the committed `index.db` yet. Needed by `show` (1.5) and the MCP server reopening on replace (6.1). | 0.2 | resolved — `IndexReader::open` returns a read-only reader; the reopen-on-replace part lapsed with the MCP server (D-ax): each CLI call opens the current file |
| 7 | Is `data_dir` resolved relative to the cwd or to the config file's directory? | 0.2 | resolved — relative to the config file's directory (also applies to `repo`) |
| 8 | Index schema: a central schema module, or per-phase DDL run against `begin_index`? | 0.2 | resolved — one `schema` module (decision 23) |
| 9 | Walker uses `parents(false)`: if `repo` ever points at a subdirectory of a larger checkout, `.ignore`/`.gitignore` above it are not read. Acceptable? | 1.1 | open, defer — fine while `repo` is a repo root |
| 10 | `--path` is a literal prefix; should it accept globs (e.g. `src/main/java/com/acme/**`)? | 1.1 | open, defer — literal prefix matches the plan wording |
| 11 | A valid but symbol-less file (e.g. `package-info.java`) is counted `skipped` like a parse error. Distinguish parse-error vs empty? | 1.5 | resolved by H1 — `ParsedFile.parse_error` separates them; `IndexStats` reports `empty` / `parse_errors` / `unreadable` |
| 12 | `symbols` has no uniqueness constraint tying fqn to a file; two source roots defining the same fqn keep the first and drop the second. | 1.5 | open, defer — duplicate fqn is a compile error for a real repo |
| 13 | Test-code detection only matches a repo-root `src/test`; multi-module repos put tests at `<module>/src/test`. | 1.5 review | resolved (before 4.4, D-bd) — any `src/test` pair before the source root (`java`) is test code; on `argus` 55 files under `backend/src/test` are no longer indexed (217 → 162 files) |
| 14 | Phase 2.1 assumes `repo` is a git work tree and the file paths it stores match git's root. A dirty or renamed tree may not match `HEAD` line numbers. | 1.5 review | resolved by R2 — dirty files get history but never touch the cache; one warning per run (decision D-r) |
| 15 | `show::render` recurses with plain function calls, so a pathologically deep symbol nesting could overflow the stack. Guard the depth, or move rendering to an explicit stack? | H8/13 | open, defer — POC-scale nesting is shallow; revisit if a real repo triggers it. Before 6.1, also replace the load-everything `render` (all `symbols`, `symbol_commits`, `symbol_tickets` rows per call) with a subtree query (audit 2, finding 10) |
| 16 | The scratch-git test helpers are now duplicated across the `history`, `indexer`, `show` and `benchmark` test modules (four copies of `git`/`git_ok`/`init_repo`/`commit`). Extract a shared `#[cfg(test)]` support module. | 2.2 review | resolved by R5 — `src/test_support.rs`, shared with `tests/benchmark.rs` via `#[path]` (decision D-s) |
| 17 | The history cache makes *repeat* runs fast (1.2 s), but the first cold run over a huge real repo is still minutes (per-symbol `git log -L`). Add bounded parallelism (results through one writer) if cold-run time starts to hurt, or rely on the project's central-build model. | 2.3 | open, defer — the plan offered parallelism *or* the cache; the cache was chosen (daily iteration is the stated goal). Revisit when a real repo is indexed end to end (R8). **R8 measured it:** cold 14.97 s for 960 symbols on `argus` (release), which does not hurt, so parallelism stays deferred — but note the cold run sits at 44 % CPU, waiting on sequential `git log -L` subprocesses, so bounded parallelism (not faster Rust) is the only lever that would move it if a larger repo does start to hurt. The proposed cheaper lever — replacing a top-level type's `-L` with `git log -- <file>` (audit 2, finding 8) — is **withdrawn**: measured over 30 types it matches in only 9 cases and drops ~47 % of the commits (history simplification hides TREESAME-through-merge commits that `-L` reports), so it would lose history rather than save time |
| 18 | Phase 2.3 measured a synthetic repo by product-owner decision; no real GRLD repo has been indexed end to end (multi-module `grld-core` would also hit the test-pruning gap of open #13). | 2.3 | **resolved** (R8) — `argus` indexed end to end: 217 files, 960 symbols, cold 14.97 s / warm 1.92 s (release), 92.5 % ticket coverage. Single-module, so #13 is still untested by a real repo |
| 19 | A `--path` run rebuilds `index.db` wholesale, so it leaves an index holding only that prefix, and a missing prefix empties it (`show` then fails repo-wide). Should `--path` merge into the existing index, refuse to replace it, or is narrowing intended and merely undocumented? | R8 | **resolved (R9, D-ab)** — narrowing is intended: `--path` is for fast prompt iteration; the run warns once that it replaces `index.db` with a partial index, and README + `--path` help document it. No merging (would need incremental indexing, Later) |
| 20 | The 3.1 Jira fixtures are synthetic (no token available to the agent). Replace them with scrubbed real `renderedFields` captures (story, bug, sub-task, epic child, empty description) and confirm the auth mode and parent/epic field shape (J3, audit §3.1). | 3.1 | **resolved** (Phase 3 PO checks) — scrubbed real captures for story, bug, sub-task (`Task`, `subtask: true`), epic child, empty description, plus a rich description; auth = Cloud basic `email:token` via the gateway (D-w); epic = `fields.parent` + legacy `customfield_10930` (D-y) |
| 21 | 3.2 is proven only against a fake `TicketSource`; no real Jira was reachable. Run `annatar index` twice on the real repo with a token: the second run should make 0 Jira requests unless the first run reported failed keys, and any requests must be for exactly those keys. How many keys come back fetched / unavailable / failed / not fetched (any 401/403/429, did the circuit breaker trip)? **Finding (2026-10-03):** the target `https://blueocean.jira.com` is Jira **Cloud** (`serverInfo` `deploymentType: Cloud`); a bearer token gets `403 Failed to parse Connect Session Auth Token` on every request, so `ANNATAR_JIRA_EMAIL` must be set (basic auth). Without it the run now fails at the auth preflight instead of caching every key unavailable. Classic API token: email + site URL as `base_url`. Scoped API token: email + `base_url = "https://api.atlassian.com/ex/jira/a91bc83a-f441-4972-9af4-e290d68c06e3"` (the site URL rejects it) + scope `read:jira-work` (a token without `read:jira-user` passes the preflight as inconclusive; a missing `read:jira-work` fails the run at the first fetch with a scope message). | 3.2 | **resolved** (Phase 3 PO checks) — `argus`, release, fresh data dir: run 1 fetched all 79 keys (0 unavailable/failed/not fetched) with 80 requests (79 + preflight, scope 401 inconclusive); run 2 0 requests; no 403/429, breaker not tripped |
| 22 | Completions run at the model's default temperature (D-ah), so dropping `llm_cache` regenerates different text. Send `temperature: 0` (and keep the error-carrying retry) once 4.5 needs reproducible records? | 4.1 | **resolved (4.2, D-am)** — `ollama.temperature`, default `0.0`, sent and in the cache key; live 4/4 identical replies |
| 23 | `LlmClient` is safe to share across tasks, but nothing calls it concurrently yet. Does the host Ollama (`OLLAMA_NUM_PARALLEL`) gain from parallel requests for 4.2/4.3, or should calls stay sequential? Measure per-ticket / per-symbol time first (the debug log now has per-reply token counts and elapsed ms). If calls go concurrent: two tasks with the same prompt both miss and both call the model (no in-flight dedup); deduplicate prompts in the stage if that matters. | 4.1 | **resolved for 4.2 (D-ap)** — host Ollama serves one request at a time (4 requests: 7.56 s sequential vs 7.03 s parallel), so the stage is sequential; re-measure if the host's `OLLAMA_NUM_PARALLEL` changes |
| 24 | `reasoning_effort: "none"` (the default, D-af) is only tested against thinking models; every chat model on the host thinks. Does a non-thinking model accept or reject it on `/v1`? Workaround if it errors: `reasoning_effort = ""` (not sent). | 4.1 review | open — check when a non-thinking model is tried (4.5); not tried in 4.5 (the review kept `qwen3.8:latest`; prompt changes, not a model change, fixed the weak spots) |
| 25 | `OllamaBackend` retries a transient failure once (D-ak), but a down server still costs every call that attempt. The looping stage (4.2+) needs a circuit breaker like the Jira one: stop after N consecutive backend failures and leave the rest for the next run. | 4.1 review | **resolved (4.2, D-an)** — the summaries stage trips on the first backend failure, then serves cached summaries only; 4.3/4.4 reuse the pattern |
| 26 | A small Ollama `num_ctx` silently truncates a long prompt (the model sees only part of it). The client does not send `num_ctx`; detection is the debug-logged `prompt_tokens` (close to the context size = suspect). Check in 4.3 with the longest real prompts; send `options.num_ctx` (config option) if needed. | 4.1 review | **resolved (4.3)** — longest real prompt on `argus` 1012 tokens (ticket summaries ≤ 1050), far below any default context; no option needed. Recheck in 4.4 (type prompts with member lists are longer) |
| 27 | The agent container cannot reach the Jira gateway `api.atlassian.com` (proxy allowlist: `CONNECT tunnel failed, response 403`; only `blueocean.jira.com` is allowed, where the scoped token gets 404), so 4.2's real run used 4 seeded fixture tickets (D-aq). Run `annatar index` twice on `argus` with the token from the host (or add `api.atlassian.com` to the allowlist): how many of the 79 tickets are summarised / invalid, time per ticket, and does run 2 print `0 chat calls`? | 4.2 | **resolved (4.2 review)** — gateway opened; run 1: 79 fetched, 79 summarised, 0 invalid/failed, 79 chat calls, ~2.4 s/ticket incl. fetching, 194 s; run 2: 2 s, 0 Jira requests, **0 chat calls** |
| 28 | When a ticket states no reason (e.g. only an "Out of scope" list), the `purpose` restates the summary ("To enable SMS sending ..."). Allow an explicit "unknown" purpose, or let 4.3 fall back to commit subjects then? | 4.2 | **resolved (4.5, D-bk)** — title-only tickets no longer get a paraphrased purpose: GRLD-20961, GRLD-35843, GRLD-88420, GRLD-90364 are empty, 30/31 tickets without a description have no purpose; tickets with a stated reason keep it. Remaining weakness tracked in #30: a purpose that promotes one bullet of a list to the whole ticket's reason (GRLD-27116) |
| 29 | `init_logging` uses `tracing_subscriber::fmt()` defaults, which write logs to **stdout**, mixed with the `index` result lines (seen in the 4.2 review real run with `-v`). Send logs to stderr? | 4.2 review | **resolved (5.2, D-cd)** — logs on stderr (no ANSI off a terminal), results on stdout, every error one `error: …` line on stderr with exit 1; `tests/cli.rs` checks it on the binary |
| 30 | Method `why`s are often generic or borrowed: accessors inherit the first ticket's theme (54 of 729 say "proof of concept for extending JWT functionality"), a recent ticket can lend an unrelated reason (`DatabaseCacheUserTokenStrategy#removeToken` → JWT-ID storage / Redis), and some whys are derived from the code despite the prompt. Fewer tickets for trivial members (e.g. only the first), a "why only if specific to this member" instruction, or empty whys for accessors? | 4.3 | **mostly resolved (4.5, D-bj)** — whys only when the history explains the symbol itself, always empty for accessors/`equals`/field-storing constructors, nested types no longer inherit their file's reason (the `ContractMessage` types are empty, the proof-of-concept accessors too): acceptable whys 32 % → 71 % on 34 reviewed symbols (mostly vague → empty; strictly correct 32 % → 35 %, wrong 12 % → 9 % as corrected in the 4.5 review fixes; held out 32 % → 63 %). **Still open:** a recent ticket can lend a multi-ticket member its reason (`AuthDataCacheRest#delete`, `RoleRelationsUpdatedMessageConsumer#perform`); a purpose that promotes one bullet (GRLD-27116 → `OAuth2RequestCacheService`); classes built by one initiative repeat its purpose verbatim (33 symbols); commit-subject whys restate the subject. **4.5 review fixes:** the "a reason that fits every member is not specific enough" instruction had little effect on per-initiative themes — one why ("… parallel caching structure") verbatim on 33 symbols, 54 with variants, including constant-returning `getCachePrefix`/`getCacheTTLSeconds` overrides; the accessor rule is ≈ 85 % followed (13/120 trivial accessors, 12/44 field-storing constructors keep a why); D-bk's emptied GRLD-40363 purpose let a later ticket's reason replace a member's origin (`TokenValidationRest#deleteAllFromList`). Strictly correct whys only 32 % → 35 %; held-out acceptable 32 % → 63 %. Next levers if 5.3 shows they hurt: per-member ticket choice (only tickets whose commits changed the member's body, not just its span) or a ticket purpose per bullet **4.6 (D-bm):** with one description, borrowed reasons largely disappear (0 wrong in the 20-symbol spot check; `AuthDataCacheRest#delete`, `RoleRelationsUpdatedMessageConsumer#perform`, `OAuth2RequestCacheService` no longer carry a borrowed reason), but per-initiative themes still leak into some descriptions (`AbstractCache#supply`, `AuthDataCacheRest`) and own-ticket reasons are often left out; the commit-subject restating issue is moot (subjects are context now, not a fallback reason). **5.3:** measured on the golden set: the wrong object-size reason on `OAuth2RequestCacheService` (back since the 4.6 review fixes) puts it 28th for its question while its controller, whose reason is right, is 1st; per-initiative reasons repeated across sibling consumers ("without requiring user re-login") make them near-identical to search, so the wrong sibling can win. One of 24 questions lost to a wrong reason, one blurred by a shared one: real but not the largest cause (D-ch) |
| 31 | A ticket whose summary stays invalid is asked again (two chat calls) on every run, so "a rerun makes 0 chat calls" fails while one exists, and under `--no-llm` its members wait although a normal run describes them from the Jira title (D-ba). Remember invalid prompts (a negative cache entry keyed like `llm_cache`) so the title fallback also applies cache-only? | 4.3 review | open — none on `argus` (0 invalid of 81); revisit if real runs show one; **extended (4.4 review):** the same holds for members and types — an invalid reply is not cached, so it is asked again (two chat calls) on every run, and under `--no-llm` an invalid member is a miss that holds back its types (D-bg). The negative cache would cover all three stages; **4.5:** still none — 0 invalid replies in the three `--path` runs and the full run with the new prompts (69 tickets, 479 members, 175 types) **4.6:** unchanged and still none — 0 invalid replies on the full `argus` run with the description prompts (479 members, 175 types). |
| 32 | A type with no members, fields or Javadoc (an empty namespace class such as `Privilege.User`) gets an invented `what` ("Represents a user entity … for access control"). Tell the model to say only what the declaration shows, or skip such types / give them a fixed `what`? | 4.4 | open — review in 4.5; **extended (4.4 review):** also enums and classes whose only content is names: `TokenType` (constants only, no Javadoc, no tickets) was described with an invented "mobile-specific" meaning. The type prompt now says not to read a meaning, platform or use into names alone (D-bi); on the real run after the change `Privilege.User` became "Defines a static marker type for user privileges within the Privilege hierarchy." (no invented entity), but `TokenType` (`JWT`, `JWT_MO`) still says "distinguishing between standard and mobile-specific variants" — the instruction alone does not stop reading `MO` as mobile. **Resolved (4.5, D-bj):** with "do not expand abbreviations or parts of names" `TokenType` is "… distinguishing between standard JWT and JWT_MO variants" (no invented meaning, empty why) and no `what` in the index says "mobile"; `Privilege.User` stays a correct marker-type description |
| 33 | The breaker (D-an) trips on the first backend failure. If one member's or type's request failed deterministically (same prompt, same failure every run), every run would trip at that symbol and everything after it would be cache-only for good, with no warning beyond the usual trip. Probed live (Ollama 0.35.1): an over-long prompt (≈ 300 000 words) is not rejected with 400/413 but silently truncated to the context (`prompt_tokens` 65 538) and answered 200 after 3 min 44 s, so the "context length" case does not exist there, and the prompts are capped (`body_chars`, `type_members`) far below it. Count a per-request 4xx (not 408/429) as invalid for that symbol instead of tripping, if a real run ever shows one? | 4.4 review | open — no deterministic failure seen on `argus`; revisit if a run trips at the same symbol twice |
| 34 | The 4.5 quality review (`quality_review_4_5.md`) was done by the agent standing in for the product owner. Confirm or override the verdicts (especially the wrong/vague ones: `TokenValidationRest#deleteAllFromList`, `AuthDataCacheRest#delete`, `RoleRelationsUpdatedMessageConsumer#perform`, `OAuth2RequestCacheService`), whether empty whys on accessors, constructors and nested types are acceptable, GRLD-27116's purpose, and spot-check empty member whys in a package you know. Also review the 20 golden questions (`.annatar-local/golden-argus.toml`) | 4.5 | open — product owner. **Extended (4.5 review fixes):** also check the ≈ 5–8 lost reasons named in the review and D-bj, and the 24 golden questions (4 added: interface, repository, nested enum, constructor primaries). **Confidentiality (D-bl amended):** confirm that package-relative fqns in committed md docs are acceptable, and decide where the real golden set lives — its only copy is `/workspace/.annatar-local/golden-argus.toml` in the agent container (gitignored); please back it up. Also decide whether to rewrite history for the one company package prefix left in commit `abc05c7` (`tests/fixtures/jira/rich_description.json`, scrubbed since `9b44532`; the branch is pushed) **4.6:** the 4.5 verdicts judge the old what/why records (kept as the historical record); the product owner should also confirm the 4.6 descriptions and their trade-off (fewer reasons, D-bm). |
| 35 | Suspected gaps in the accessor rule and the type prompt (D-bj), not seen on `argus`: record accessors (`name()`, no `get` prefix; `argus` has no records), accessors with non-`get` names (e.g. German `holeX`/`liefereX`) — the rule names accessors by kind and the model may key on the prefix — and the type prompt's "this {kind}" wording echoed into a why (the validator rejects only "this <kind> exists/is/was" openers). Check on a repo with records or such names before changing the prompt (a change costs a full re-describe) | 4.5 review | open — no prompt change **4.6:** the accessor clause is kept in the member prompt ("no reason for getters, setters, …"); an echoed "this {kind}" is now only rejected as an opener (D-bo), so the suspected echo cannot cost a retry; the record/German-accessor question still applies to whether such members get a borrowed reason. |
| 36 | Kind and role filters for 5.2 search: `vector_top_k` has no `WHERE` push-down, so a filter after it can return fewer than k rows. Over-fetch and filter, or scan with exact `vector_distance_cos` when a filter is set? **Review fixes:** over-fetching is capped — `vector_top_k` returns at most 200–202 rows for any `k` (libSQL's default search list, measured with k = 654), so "all 654" is not possible. Options: exact `ORDER BY vector_distance_cos(...)` over the filtered rows (cheap at this scale), or over-fetch ≤ 200 and fall back to the exact scan when fewer than k rows survive | 5.1 | **resolved (5.2, D-ca)** — exact `vector_distance_cos` scan whenever a filter is set (`argus`: 2–6 ms filtered, ≈ 15 ms for all 654 rows vs 13–15 ms for `vector_top_k`, same top-10); `vector_top_k` for unfiltered queries; `-k` ≤ 100. Revisit (over-fetch, then fall back) past ≈ 10k symbols |
| 37 | `index_meta` records the embedding model's name (D-bw), not its digest: a model re-pulled or replaced behind the same name with the same dimension leaves the cache and index looking valid while query vectors come from a different model. Record the `/api/show` digest (and key the embedding cache by it)? | 5.1 review | open, defer — needs an extra request per run and changes the embedding cache key (one re-embed, no chat calls); revisit if models are swapped in practice |
| 38 | `index --path` outside the repository (absolute, or `../x` since D-ce) still walks nothing and replaces `index.db` with an empty one (one warning, D-ab), while `search --path` errors. Make `index` refuse an outside prefix too? | 5.2 review | open — a behaviour change to `index` beyond the review's scope; small (`java_files` would return an error instead of an empty list), decide with open #19 |
| 39 | `describe.body_chars` (1500) cuts the source of the 2 longest of 479 `argus` members (`AuthDataService#getAuthData(UUID)` ≈ 1964 chars, `RoleRelationsUpdatedMessageConsumer#perform(…)` ≈ 1751, measured on the dedented source the cap applies to; 2046 / 1825 raw, corrected in the 5.3 review fixes), and in both the behaviour a golden question asks about (the logout in the error branch, the role-cache refresh) is after the cut, so their descriptions omit it and they rank 45th and 7th. Raise the default (e.g. 2500) or make the cut keep the tail? | 5.3 | open — product owner: changing it re-describes only those 2 members and their 2 enclosing types (≈ 4 chat calls; prompts of the other members are unchanged), but it changes a default and a prompt input, which this step must not do (D-ch); the prompt tokens grow only for long members |
| 40 | Retrieval levers after the POC (5.3 misses, D-ch): group a type with its members in `search` output (a class and its main method tie within 0.01–0.03 in 7 of 20 misses); down-weight or tag data carriers (request TOs, response DTOs) that restate the behaviour they carry (3); a lexical or synonym channel for domain words absent from descriptions (5: "second factor" vs security method, "login cookie" vs JWT); fewer shared initiative reasons on sibling classes (open #30). Pursue any? | 5.3 | open — after the POC. The 5.4 trial (D-cl) adds evidence for grouping a type with its members and for callers: its one correctness loss (T4) came from a search hit standing in for its caller (#42); no task missed for vocabulary, so the lexical channel has no new evidence. Decide with #42 **5.5:** the first lever is built as the file view (the product owner's redesign): ties between a class and its member no longer matter at file level (primary file first for 11 of 24 vs the symbol 4); DTOs, vocabulary and the shared initiative reasons remain |
| 41 | The agent trial (D-cl) is small: 4 tasks × 5 runs per arm on one ≈ 7k-line repository, one model (`claude-sonnet-5`), tasks and answer key by the author who built the index, graded by an LLM. Repeat on a larger repository (where grep is costlier) with tasks written by someone else (product owner, team) and a second model before generalising the −47 % tokens / equal correctness? `scripts/agent_trial/run.py` takes any private task file | 5.4 | open — product owner (after the POC) |
| 42 | Search can shortcut a call chain: on the flow-trace task (T4) 4 of 5 agents with `annatar` started at the consumer search returned and left out the queue dispatcher that calls it (all 5 without it traced from the queue; Fisher p ≈ 0.05), and none of the 5 got full marks on T4 against 5 of 5 without (p ≈ 0.008) — the one task where `annatar` made answers worse, which cancels the T2 gain in the total (35 vs 35, D-cl). Levers: callers in `show` or the `trace` command of `project.md`; a hint in the agent prompt to check who calls a hit; group a type with its members (#40) | 5.4 | open — after the POC (with #40) **5.5:** the file view lists every member of the hit's file, not its callers in other files, so it may not fix T4; the trial re-run (Next) measures it |
| 43 | The file view ranks a file by its single best symbol (D-cm). A file whose many members all match moderately ranks below a file with one strong lookalike (a DTO or a sibling consumer); the reverse risk of a sum is promoting DTOs full of accessors. Try the mean of a file's top 2–3 hits, or the type's own score plus its best member, once the trial re-run or a larger golden set shows file misses that ranking would fix? | 5.5 | open — on `argus` the primary's file misses the top 5 for 7 of 24, mostly vocabulary (5.3 causes), not ranking |
