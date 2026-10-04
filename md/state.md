# Annatar — State

Living document. Updated after every step. Tracks where the project stands,
the decisions taken, and the tradeoffs behind them.

- **Plan:** [`plan.md`](./plan.md)
- **Overview:** [`project.md`](./project.md)

## Current state

| | |
| --- | --- |
| Phase | 4 — LLM summaries, in progress (Phase 3 Jira complete, including the product owner's real-Jira checks; Phase 2 and its audit remediation R0–R9 complete) |
| Step | 4.3 Method and constructor what/why — **done**, review fixes done (real run on `argus` after the fixes: 81 tickets, 729/729 members re-described in 1362 s, rerun 0 chat calls and 0 Jira requests); next 4.4 Type what/why, bottom-up |
| Last updated | 2026-10-04 |
| Toolchain | rustc 1.99.0, edition 2024 |

### Done

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

- **Agent:** 4.4 Type what/why, bottom-up (deepest nested types first, members'
  `what` lines as input, capped for large classes; the type's tickets are the
  file's, cap them too; reuse `describe`'s history selection and the
  `[describe]` caps and the D-az/D-ba rules (chosen tickets only, title
  fallback), call `reset_invalid_streak` first; decide whether test classes (open #13) are worth
  describing).
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
| 4.1 LLM client | done | `llm` module: `LlmClient::complete::<T>` (schemars 1 schema as `json_schema` response format, serde validation, one retry with the error, `InvalidOutput`, no caching of failures) and `embed` (per-text cache, misses only, batches of 64); `LlmBackend` seam + `OllamaBackend` + fake; `LlmStats`; `llm_cache` + `embedding_cache` (D-ag); `ollama.reasoning_effort` default `none` (D-af); live: struct cold 1.2–2.5 s / cached < 1 ms, bge-m3 1024 dims; 155 tests (4 ignored) |

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
| D-av (4.3) | `Description { what, why }`: `what` one verb-first line (prompt ≤ 150, valid ≤ 200), `why` from the change history only (prompt ≤ 200, valid ≤ 300), empty (`NULL`) when the history gives none; ticket purpose sent as `Reason:`, a `NULL` purpose omitted; context fenced `<<<CONTEXT` / `CONTEXT>>>`; meta phrases ("this method", "the ticket", "javadoc", "not stated", …) and a restated `why` are invalid (retry); whitespace collapsed before storing | `why` from code too; reject newlines | Same anti-invention approach as D-as. Real run: 0 invalid replies, 0 retries; only 2 empty whys (most members have a ticket); accessor whys tend to be generic (open #30). **Amended (4.3 review, D-bb):** the meta check is narrowed (openers for `what`, source/missing-information phrases for `why`) |
| D-aw (4.3) | Describe stage runs after summaries on every run, including a non-git repository (code-only prompts); per-stage `summary_llm` / `describe_llm` stats, the `summaries:` line shows its stage's calls and a `describe:` line shows `described (… from cache)`, invalid, failed, incomplete, skipped and the stage's calls; `[describe]` settings travel on `Summarizer` (`with_describe`) so `build_index`'s signature is unchanged; sequential calls (D-ap) | Run only on git work trees; a run-wide chat count only; a new `build_index` parameter | A structure-only index still benefits from `what`. With two LLM stages a run total no longer tells which stage called the model. 1.86 s per call is dominated by the model, so concurrency would only help with `OLLAMA_NUM_PARALLEL` > 1 |
| D-ax (PO, after 4.3) | **No MCP server: Annatar stays CLI-only.** Phase 6 is removed from `plan.md`; 6.2's agent trial moves to **5.4** and uses `annatar search` / `annatar show` through the agent's shell tool; POC question 3 now reads "a coding agent using the `annatar` CLI". The `serve` stub subcommand is removed; `project.md`'s interface table lists CLI commands (`search`, `show`, `module`, `trace`) instead of MCP tools; MCP is listed under **Later** only as "revisit if a CLI proves insufficient". | `rmcp` server over stdio (old 6.1) | Product owner: coding agents drive CLIs well, so a second interface adds code and test surface without answering a POC question. Consequence: the CLI output is the agents' interface, so it must be compact, stable and free of log noise (open #29 now gates 5.2; plan 5.2 says so). |
| D-ay (4.3 review, bug fix of a default) | Default `ticket_regex` is `\bGRLD-\d+` (was `\bGRLD-\d+\b`), in `config.rs`, `annatar.toml` and plan 0.1 | Keep `\b` and document it; `\bGRLD-\d+(?:\b|_)` with the `_` stripped | `\b` does not match between a digit and `_` (both word characters), so branch-style subjects (`GRLD-98595_Fix_login`, 59 commit subjects in `argus`) yielded no key at all. Without the trailing `\b` the greedy `\d+` still takes every digit, so keys stay exact (`GRLD-123` never cut from `GRLD-1234`), and the leading `\b` still rejects `XGRLD-1`; Jira's `is_plain_key` check is unchanged. Ticket keys are derived from the cached commits on every run, so no cache is invalidated; the new keys are fetched and summarised once (`argus`: 197 distinct keys in all commit texts vs 194 before) and the members they touch get new prompts. A custom `ticket_regex` is the user's own |
| D-az (4.3 review, finding 1) | Only the tickets the selection *chooses* can make a member incomplete: a ticket not in the index (`TicketState::Unknown`) competes like an available one and blocks only if it would be the first or one of the recent ones. Permanent fetch failures (`FetchError::is_permanent`: 403/404, another 4xx except 408/425/429, an unparseable issue, an invalid key) are cached unavailable (amends D-ac); a non-JSON 2xx is the new transient `NotJson`. `TicketFetch::from_config` returns `JiraMode` (`Fetch`, `Offline` = `--offline` with Jira configured, `Disabled` = no `[jira]` or no token, also under `--offline`); `build_index` takes `&JiraMode`; with `Disabled` the describe stage counts uncached tickets as unavailable, so members fall back to commit subjects. `Incomplete` lists the blocking keys (not fetched / no summary); the stage warning lists them, sorted, at most 10 (`+N more`); the summaries warning lists the invalid keys the same way | Keep "any unknown ticket blocks"; treat every unknown ticket as unavailable; cache parse errors only after N runs | An unknown ticket that would not be chosen cannot change the choice whichever way it resolves (removing a non-chosen candidate leaves the oldest and the newest N as they are), so waiting for it only cost coverage; one that would be chosen still waits, keeping D-au's "never cache from incomplete input". A permanent failure never fetched before blocked its members forever. Without Jira configured nothing will ever be fetched, so waiting is pointless; `--offline` is a temporary state of a configured Jira, so its unknown tickets keep blocking. Caching an unparseable issue risks keeping a ticket that a later Jira change would make parseable (drop `ticket_cache` to refresh, as for 403/404); a proxy page is excluded by the JSON check |
| D-ba (4.3 review, finding 2) | A chosen available ticket whose 4.2 summary was **invalid this run** (`InvalidOutput` after the retry, returned by `index_summaries`) enters the prompt with its Jira title in place of the summary and no `Reason:` line (`describe::Summary::Title`). A summary missing for any other reason (circuit breaker, `--no-llm`, backend failure) still makes the member incomplete | Keep skipping the member; leave the ticket out of the prompt; use the title for every missing summary | A ticket the model cannot summarise (every run, deterministic at temperature 0) otherwise blocks every member that chooses it and, in 4.4, their types. The title is deterministic input, so the prompt is a stable cache key; once a later run summarises the ticket validly, the prompt changes and the member is re-described. A breaker trip is transient and says nothing about the ticket, so caching a title-based record then would keep a weaker what/why that a normal run replaces anyway — it still waits. Under `--no-llm` an invalid summary is a cache miss (skipped, not invalid), so such members wait there even if a normal run described them from the title (accepted) |
| D-bb (4.3 review, finding 3) | Meta-text checks narrowed. `what`: only the openers "This method / This constructor / This function". `why` and ticket `purpose`: the shared `summaries::REASON_META_PHRASES` ("the ticket does/states/says/…", "according to the ticket", "this ticket", "the commit", "commit message", "the javadoc", "change history", "not stated/specified/mentioned/provided/given in", "no reason (is) given/stated", "no specific reason", "does not state/specify") plus, for `why`, "this method/constructor/function"; a whole reply that only says "Not specified." / "Unknown" / "None" / "N/A" is invalid. Ticket `summary`: only the openers "This ticket / The ticket" (amends D-as, D-av) | Keep the broad substring list; an LLM judge | The broad list rejected ordinary domain text ("Uses the default page size when the size is not specified.", "Falls back to the default locale if one is not provided.", "Returns the ticket price …", "Prevents sessions being revoked for no reason."), costing a retry and, twice in a row, the record. The narrowed phrases still catch every meta reply seen in the 4.2/4.3 probes. Loosening a check never invalidates cached rows (they are re-validated on a hit and still pass) |
| D-bc (4.3 review, findings 5, 7, 10) | The source excerpt replaces the `Signature:` line (the signature is sent only when `body_chars = 0` or the source is empty; an abstract/interface method's excerpt is its declaration, sent once); merge commits (`Merge ` prefix) are left out of the commit-subject fallback before dedup and cap; `LlmClient::reset_invalid_streak` runs at the start of each LLM stage, so the 5-in-a-row breaker counts per stage (amends D-an) | Keep both lines; keep merges; one streak per run | The excerpt starts with the declaration, so the signature line only repeated it (and twice for abstract methods). Merge subjects ("Merge branch 'x' into master") say nothing about why the code exists. Four invalid ticket summaries followed by one invalid description are not evidence of a broken model in the describe stage. The prompt change misses the describe cache for every member once (re-describe on `argus` in the real run below) |
| D-bd (before 4.4, open #13) | The walker treats a file as test code when a `src` component is directly followed by `test` anywhere before the first `java` component (`backend/src/test/java/…`, `a/b/src/test/…`); `src/test` inside a source root (`src/main/java/com/acme/src/test/…`) stays a package, `src/testng` and `test/src` stay production | Repo-root `src/test` only (1.1); any `test` directory; Maven/Gradle module detection | Plan 1.1 already says skip `src/test/`; `argus` keeps its tests in `backend/src/test`, so 55 test files (and their members) were indexed, described (≈ 20 % of the chat calls) and would pollute search. Same "not under a source root" rule as the build-dir pruning (decision 25). Drops their history/describe rows from the next index; their cache rows stay unused |

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
| 24 | `reasoning_effort: "none"` (the default, D-af) is only tested against thinking models; every chat model on the host thinks. Does a non-thinking model accept or reject it on `/v1`? Workaround if it errors: `reasoning_effort = ""` (not sent). | 4.1 review | open — check when a non-thinking model is tried (4.5) |
| 25 | `OllamaBackend` retries a transient failure once (D-ak), but a down server still costs every call that attempt. The looping stage (4.2+) needs a circuit breaker like the Jira one: stop after N consecutive backend failures and leave the rest for the next run. | 4.1 review | **resolved (4.2, D-an)** — the summaries stage trips on the first backend failure, then serves cached summaries only; 4.3/4.4 reuse the pattern |
| 26 | A small Ollama `num_ctx` silently truncates a long prompt (the model sees only part of it). The client does not send `num_ctx`; detection is the debug-logged `prompt_tokens` (close to the context size = suspect). Check in 4.3 with the longest real prompts; send `options.num_ctx` (config option) if needed. | 4.1 review | **resolved (4.3)** — longest real prompt on `argus` 1012 tokens (ticket summaries ≤ 1050), far below any default context; no option needed. Recheck in 4.4 (type prompts with member lists are longer) |
| 27 | The agent container cannot reach the Jira gateway `api.atlassian.com` (proxy allowlist: `CONNECT tunnel failed, response 403`; only `blueocean.jira.com` is allowed, where the scoped token gets 404), so 4.2's real run used 4 seeded fixture tickets (D-aq). Run `annatar index` twice on `argus` with the token from the host (or add `api.atlassian.com` to the allowlist): how many of the 79 tickets are summarised / invalid, time per ticket, and does run 2 print `0 chat calls`? | 4.2 | **resolved (4.2 review)** — gateway opened; run 1: 79 fetched, 79 summarised, 0 invalid/failed, 79 chat calls, ~2.4 s/ticket incl. fetching, 194 s; run 2: 2 s, 0 Jira requests, **0 chat calls** |
| 28 | When a ticket states no reason (e.g. only an "Out of scope" list), the `purpose` restates the summary ("To enable SMS sending ..."). Allow an explicit "unknown" purpose, or let 4.3 fall back to commit subjects then? | 4.2 | **mostly resolved (4.2 review, D-as)** — empty purpose (`NULL`) when no reason: 30/79 on `argus`, no invented "parent task" or meta reasons. Still open for 4.5: a few title-only tickets get a synonym paraphrase of the summary (GRLD-20961, GRLD-35843, GRLD-88420, GRLD-90364); 4.3 treats a `NULL` purpose as "no why" (commit subjects may help) |
| 29 | `init_logging` uses `tracing_subscriber::fmt()` defaults, which write logs to **stdout**, mixed with the `index` result lines (seen in the 4.2 review real run with `-v`). Send logs to stderr? | 4.2 review | open — small fix, before 5.2 (the CLI is the agents' interface, D-ax); was: before the MCP server (6.1) at the latest, where stdout is the protocol channel |
| 30 | Method `why`s are often generic or borrowed: accessors inherit the first ticket's theme (54 of 729 say "proof of concept for extending JWT functionality"), a recent ticket can lend an unrelated reason (`DatabaseCacheUserTokenStrategy#removeToken` → JWT-ID storage / Redis), and some whys are derived from the code despite the prompt. Fewer tickets for trivial members (e.g. only the first), a "why only if specific to this member" instruction, or empty whys for accessors? | 4.3 | open — review in 4.5 with the golden set |
| 31 | A ticket whose summary stays invalid is asked again (two chat calls) on every run, so "a rerun makes 0 chat calls" fails while one exists, and under `--no-llm` its members wait although a normal run describes them from the Jira title (D-ba). Remember invalid prompts (a negative cache entry keyed like `llm_cache`) so the title fallback also applies cache-only? | 4.3 review | open — none on `argus` (0 invalid of 81); revisit if real runs show one |
