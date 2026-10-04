# Annatar — POC Plan

> The proof-of-concept is a subset of the target described in
> [`project.md`](./project.md). Anything there that this plan doesn't cover is
> in **Later** at the end.

Scope: **Java / Spring** only, **Jira** only (keys like `GRLD-123`), **Ollama** for LLM and embeddings, **libSQL** for storage, written in **Rust**.

## What the POC has to prove

1. **Quality:** symbol descriptions generated from the code, its tickets and its commit history are correct and useful (until 4.5: separate what/why records; the product owner merged them into one description in 4.6).
2. **Retrieval:** searching the descriptions by meaning finds the right class or method for a plain-language question.
3. **Agent value:** a coding agent using the `annatar` CLI finds relevant code faster and with fewer tokens.

Everything that doesn't help answer these three questions is in **Later**, at the end.

## Ground rules

- One step = one small PR. Don't start the next step in the same change.
- Every step ships with tests. Unit tests use fixtures and never need network, Jira or Ollama. Tests that do are marked `#[ignore]` and run manually.
- **Two database files.**
  - `index.db` is rebuilt from scratch on every run. It's written to a temporary file and atomically renamed into place, so a concurrent `search` or `show` never sees a half-built index.
  - `cache.db` persists and holds only expensive results, keyed by content: tickets by ticket key (fetched once, never refreshed, since done tickets don't change), LLM outputs by a hash of model + prompt text + input (plus the response schema, reasoning effort and temperature, so a changed struct or setting misses), and embeddings by a hash of model + text. A changed prompt, model or input simply misses the cache, so no versioning or invalidation logic is needed. If a cache table's schema changes, drop that table.
- **Symbols are identified by fully qualified name (fqn)** everywhere outside a single run: in caches, the golden set and the CLI (`show`, `search`). Row IDs change on every rebuild.
- `--path <prefix>` limits a run to part of the repo, for fast iteration on prompts. The run still replaces the whole `index.db`, which then holds only that prefix (one warning says so; `state.md` D-ab).

---

## Phase 0 — Skeleton

**0.1 CLI, config, logging**
*Goal:* a binary that loads a validated configuration and exposes the subcommands later steps fill in.
Binary `annatar` with `clap` subcommands `index`, `show`, `search` (stubs; a `serve` stub was dropped when MCP left the POC, `state.md` D-ax), `--path` and `-v`. Load `annatar.toml`: repo path, data directory, ticket key regex (default `\bGRLD-\d+`; no trailing `\b`, so `GRLD-123_Fix` matches, `state.md` D-ay), Jira base URL and auth, Ollama URL, chat model, embedding model. Secrets from environment variables. Logging with `tracing`.
*Done when:* `--help` works; config tests cover defaults and a missing required value.

**0.2 Database files**
*Goal:* a `Store` that opens `cache.db` and builds `index.db` in a temp file that is atomically renamed when the run finishes.
Each later phase adds its own tables. Index tables are created on the temp file; cache tables use `CREATE TABLE IF NOT EXISTS`.
*Done when:* a test shows a finished run replaces `index.db`, an aborted run leaves the old one intact, and `cache.db` survives both.

## Phase 1 — Symbols

**1.1 File walker**
*Goal:* the list of production Java files to index, relative to the repo root.
Use the `ignore` crate, respecting `.gitignore`, skipping `build/`, `target/`, `generated/`/`generated-sources/` (when not under a source root) and test source sets: the `<set>` of the first `src/<set>/java` (else `src/<set>`) is `test`, starts with `test` as a word (`testFixtures`) or ends with `Test` (`integrationTest`, `androidTest`), in the repository or a module (`backend/src/test/`); `src/test` deeper inside a source root is a package (`state.md` D-bd). Apply `--path`.
*Done when:* a temp-directory test returns exactly the expected files.

**1.2 Parse types**
*Goal:* every type in a file as a record with package, name, kind, line span, byte span and parent type.
Use `tree-sitter` + `tree-sitter-java` for classes, interfaces, enums and records, including nested ones. Log and skip files with parse errors.
*Done when:* fixture tests cover each kind, nesting and a broken file.

**1.3 Parse methods and constructors**
*Goal:* every method and constructor as a record with a unique fqn, spans and parent type.
fqn format: `com.acme.user.UserRepository#findById(Long)`, with parameter types as written so overloads are distinct. Ignore anonymous and local classes.
*Done when:* fixture tests show overloads get distinct fqns, and constructors are included.

**1.4 Signature, Javadoc, annotations, Spring role**
*Goal:* each symbol record carries the text the LLM will later read (signature without body, Javadoc, annotations) plus a Spring role for types.
Roles from annotations: `controller`, `service`, `repository` (including Spring Data interfaces extending e.g. `JpaRepository`), `component`, `configuration`, `entity`.
*Done when:* fixture tests cover Javadoc present and absent, and each role.

**1.5 Write symbols and `show`**
*Goal:* all symbols of a real repo in `index.db`, inspectable by fqn.
`symbols` table: id, parent_id, kind, role, fqn (unique), file, start/end line, signature, javadoc, annotations, content hash (blake3 over the symbol's Javadoc + source). `annatar show <fqn>` prints a symbol and its children.
*Done when:* a real repo indexes and `show` works on it.

## Phase 2 — History and ticket keys

**2.1 History for a span**
*Goal:* for any file and line range, the list of commits that changed it, with sha, date, subject and body.
Run `git log -L <start>,<end>:<file>` via `std::process::Command` with a custom `--format` and record delimiter. Note in the PR how it behaves on renamed files.
*Done when:* a test on a scratch git repo returns the expected commits.

**2.2 Store history and ticket keys**
*Goal:* for every symbol (types and members), its commits and ticket keys with the dates needed to pick "first" and "most recent".
Tables: `symbol_commits(symbol_id, sha, date, subject)` and `symbol_tickets(symbol_id, ticket_key, first_date, last_date)`. Keys come from the configured regex on subject and body. Commit subjects are kept as a fallback for when a ticket is unavailable. `show` lists both.
*Done when:* tests cover multiple, duplicate and missing keys; `show` on a real symbol lists its tickets.
*Deviation:* verified on a scratch git repo, not a real one; **closed** by the real-repo run (audit item R8): `argus`, 960 symbols, 92.5 % with ≥1 ticket, history spot-checked against `git log -L`.

**2.3 Measure**
*Goal:* a full run on the target repo is fast enough to iterate on daily.
If it isn't, add bounded parallelism (results written by one writer) or a history cache in `cache.db` keyed by fqn + content hash + the file's last commit sha.
*Done when:* run time is noted in the PR and acceptable.
*Deviation:* measured on a synthetic benchmark repo by product-owner choice (`state.md` D-p); **closed** by audit item R8 — `argus`, cold 14.97 s / warm 1.92 s for 960 symbols (release build; the cold run is git-subprocess-bound, so debug measured the same).

## Phase 3 — Jira

**3.1 Fetch and parse**
*Goal:* for any ticket key, a `Ticket` with key, issue type, summary, plain-text description and parent/epic key.
Use `reqwest` with the target's auth. Request `expand=renderedFields` and strip the HTML to text, which avoids writing an Atlassian Document Format parser.
*Done when:* parsing is tested against saved JSON fixtures; an `#[ignore]` test fetches a real ticket.
*Deviation:* the fixtures in `tests/fixtures/jira/` started synthetic (no token); **closed** — they are now scrubbed real captures from the target Cloud Jira and `jira::tests::fetches_real_ticket` passes against it (audit J3, `state.md` D-z, open #20).

**3.2 Ticket cache**
*Goal:* every distinct key in `symbol_tickets` is in `cache.db`, either with content or marked unavailable.
Fetch only missing keys. Record 403/404 as unavailable (since the 4.3 review also other permanent failures, `state.md` D-az); back off on 429.
Runs as a stage of the index build (after history, on the build's transaction, before the single commit; audit R9), so it reads the keys from the in-progress `symbol_tickets`.
*Done when:* a second run makes no Jira calls.
*Deviation:* proven against a fake `TicketSource` end to end through `build_index`; the real-repo second run with a token was the product owner's check (audit §3.1 step 3, `state.md` open #21) — **closed**: on `argus` the first run fetched all 79 keys, the second made 0 Jira requests. Without Jira (or with `--offline`) the stage makes no requests but still copies already-cached tickets into the index `tickets` table, which `show` reads (D-ac, D-ad).

## Phase 4 — LLM summaries

**4.1 LLM client**
*Goal:* a typed, cached call that turns a prompt into a validated Rust struct.
`complete<T: DeserializeOwned + JsonSchema>(prompt)` and `embed(texts)` against Ollama's OpenAI-compatible `/v1` endpoint, with the `schemars` schema as structured-output format. Retry once on invalid output. Results go through the LLM cache.
*Done when:* an `#[ignore]` test returns a valid struct; a repeated call hits the cache.
*Note:* "validated" is serde plus an optional caller check (`complete_with`, e.g. blank or overlong fields) that also triggers the retry; the client takes its own `cache.db` connection (`Store::connect_cache`) so concurrent stages can share it (`state.md` D-ai, D-aj).

**4.2 Ticket summaries**
*Goal:* every available ticket has a one- to two-sentence summary and a purpose (why).
*Done when:* all available tickets have a summary; a rerun makes no LLM calls.
*Note:* a stage after tickets writes English `llm_summary` / `llm_purpose` to the index `tickets` table; the prompt depends on the ticket alone (no parent), and `purpose` is empty (`NULL`) when the ticket gives no reason; sequential calls; the chat model is checked once before the build (missing → the run fails); the circuit breaker (first model failure or 5 invalid replies in a row) lives on the shared LLM client, so later stages honour it; `--no-llm` for cache-only runs, `temperature` 0 by default (`state.md` D-al–D-ap, D-ar, D-as). Verified on the full `argus` run: 79/79 tickets summarised, a rerun makes 0 chat calls (D-aq, open #27 closed).

**4.3 Method and constructor what/why** *(superseded by 4.6: the `what`/`why` pair became one `description`; selection, hold-back and caching below still apply)*
*Goal:* every method and constructor has a one-line `what` and a `why`, stored by symbol in `index.db`.
Input, built by a pure, tested function: signature, Javadoc, parent class name and role, and ticket summaries (the first ticket plus the most recent few, capped). If all its tickets are unavailable, use commit subjects instead. A method whose chosen available ticket has no summary this run (failed, or skipped after the breaker tripped; an invalid one falls back to the Jira title since the review fixes) is skipped this run rather than generated without it, so the cache never holds a what/why built from incomplete input; a `NULL` purpose is valid input and means the ticket gives no "why". Try it on one package with `--path` first and note time per symbol.
*Done when:* `show` prints what/why for methods.
*Note:* a stage after summaries (also on a non-git repository, code only) writes `what` / `why` columns on the index `symbols` row; besides signature, Javadoc and parent it sends a capped excerpt of the member's source (`describe.body_chars`, default 1500: on `argus` it made `what` concrete and stopped it guessing from ticket text); the first ticket is the oldest `first_date`, the recent ones the newest `last_date` (`describe.recent_tickets`, default 3), dates compared in UTC, ties by key; commit subjects (`describe.commit_subjects`, default 5, merges left out) only when no ticket is available; `why` is empty (`NULL`) when the history gives no reason (`state.md` D-at–D-aw). *Review fixes (D-az–D-bb):* only the *chosen* tickets can make a member incomplete — a ticket not fetched yet competes in the selection and blocks only when it would be chosen; permanent fetch failures are cached unavailable and, without Jira configured, uncached tickets count as unavailable; a ticket whose summary was *invalid* stands in with its Jira title (a summary missing because the breaker tripped or `--no-llm` still holds the member back); the source excerpt replaces the signature line.

**4.4 Type what/why, bottom-up** *(superseded by 4.6 like 4.3: types now get one `description`, built from their members' descriptions; bottom-up order and hold-back unchanged)*
*Goal:* every type has a `what` and `why` built from its own context and its members' `what` lines.
Process the deepest nested types first, so an outer class can include its nested types' `what` lines. Cap the input for large classes (e.g. public members first, then a fixed maximum). A type's history span is its whole body, so its tickets are effectively the file's tickets: cap those too. Because the input includes members' output, a changed method misses the cache for its class when its `what` text changes and it is one of the listed members (within the cap); a change that leaves its `what` as it was, or a member left out by the cap, keeps the class's cache entry.
*Done when:* `show` prints what/why for types, including a large class and a nested class.
*Note:* a stage after describe writes the same `what` / `why` columns for types. The prompt carries kind, fqn, Spring role, the enclosing type, Javadoc, the declaration with its members and nested types cut out (annotations, header, fields, enum constants; capped by `describe.body_chars`), the `what` lines of up to `describe.type_members` (default 30) members and nested types — public first, then the rest, listed in source order, the remainder counted — and the type's own first plus `describe.type_recent_tickets` (default 3) most recent tickets, chosen and blocked as for members. A *listed* child without a `what` this run holds the type back (and so its outer type); a child whose reply was invalid is listed as "(not described)", mirroring the title fallback (`state.md` D-be–D-bg). *Review fixes (D-be amended, D-bh, D-bi):* the outline also drops the comments directly above a cut member and a `//` comment after it on its last line, and collapses enum constant bodies and initializer blocks to `{ … }`; a record's compact constructor is listed as `compact constructor Point`; the prompt asks the model not to read a meaning into names alone; a reply naming a ticket key is invalid for ticket summaries, members and types.

**4.5 Quality review and golden set**
*Goal:* evidence that the records are trustworthy, and a fixed benchmark for retrieval.
Review 20–30 symbols you know well against the code and tickets; adjust prompts. Write 15–20 plain-language questions, each with the fqn it should find, as a test fixture.
*Done when:* you'd trust the records you reviewed; the golden set exists. **This answers question 1.**
*Note:* the review is in [`quality_review_4_5.md`](./quality_review_4_5.md) (34 symbols, 13 tickets, before/after; verdicts pending the product owner's confirmation). It led to stricter `why` prompts: a reason only when the history explains this member or type itself, none from a project name alone, always empty for accessors, `equals`/`hashCode`/`toString` and field-storing constructors (prompt-only, ≈ 85 % followed), nested types not inheriting their file's reason; ticket purposes empty for title-only tickets; type `what`s do not expand abbreviations (`state.md` D-bj, D-bk). The golden set format is TOML (`[[question]] text, expect = [fqn, alternates…], kind?, note?`), loaded and validated by `golden::GoldenSet`; `golden::check_index` reports expected fqns an index lacks. Real golden sets name employer packages, so they stay out of git (`.annatar-local/golden-argus.toml`, 24 questions for `argus`; also because a real set is only usable against the private index); the repository holds a synthetic fixture for the sample project. Questions are written from code and tickets, never by paraphrasing generated what/why (D-bl). *After 4.6:* the review judged the what/why records; question 1 is re-checked for the single description by 4.6's spot check, and the golden set (fqns only) is unaffected.

**4.6 Symbol descriptions (product-owner redesign, 2026-10-04)**
*Goal:* every method, constructor and type has one general `description` instead of the separate `what` and `why`.
Product owner: "Move from the pure what / why to a simple general symbol description with the code itself the ticket and the commit history as context. There should be no length limit only the instruction to describe it as short as possible." One response field `description` (members `Description`, types `TypeDescription`, `deny_unknown_fields`) and one `description` column on `symbols` in place of `what`/`why`. The prompt asks for a description of what the symbol does, as short as possible, and the reason it exists when its tickets or commits explain it for this symbol (same anti-borrowing and anti-invention guidance as 4.5). Context: the code (members: the capped source excerpt, `describe.body_chars`; types: the outline plus their members' and nested types' descriptions, bottom-up), the chosen tickets with their 4.2 summary and purpose (title fallback as in 4.3), and **always** the newest `describe.commit_subjects` distinct commit subjects (merges left out), no longer only when no ticket is available. No length limit in prompt or validation; `ollama.max_tokens` (default 1024) is only a safety cap against runaway replies, and a reply cut off at it is invalid. Validation: non-blank, no control characters, no ticket key, no "This method/This class…" opener, no clearly meta phrase about sources or missing information ("according to the ticket", "the Javadoc says", "not stated in the commits", a bare "Unknown"); whitespace collapsed before storing (`state.md` D-bm–D-bp).
*Product-owner follow-up (2026-10-04):* after "as short as possible" made the model drop reasons from the symbol's own tickets, the product owner said: "no reasons should not be droped. as short as possible without loosing information." The prompt now asks for a description "as short as possible without losing information" and to keep any reason the code, Javadoc, tickets or commits give for this symbol being created or changed; only borrowed reasons (a file-wide or initiative theme that does not explain the symbol) and name-only guesses are left out. Accessors and field-storing constructors get a reason only when the history explains that very member or its field (D-bm amended).
*Done when:* `show` prints `description` for every described symbol; a rerun makes 0 chat calls; a spot check of the 4.5 sample against the code is in `state.md`.

## Phase 5 — Search and agent trial

**5.1 Embeddings**
*Goal:* every symbol with a description has a vector in a searchable index.
Embed `fqn + description` through the embedding cache (was `fqn + what + why` before 4.6). Take the dimension from the first embedding response, not from config. Store it in an `F32_BLOB(<dim>)` column with a `libsql_vector_idx` index.
*Done when:* every symbol with a description has a vector.
*Built (5.1, `state.md` D-br–D-bv):* table `symbol_vectors (symbol_id, embedding F32_BLOB(<dim>))` with a cosine `libsql_vector_idx` (`compress_neighbors=float8`, `max_neighbors=32`; review fixes), created by the embeddings stage once the dimension is known, so an index without vectors has no such table (5.2 must say so); text `fqn\ndescription`, `[embedding] parent_description` (default off) appends a member's type description for 5.3 to compare; the run checks the embedding model like the chat model; `--no-llm` uses cached embeddings only; `index_meta` records the embedding model, dimension and `parent_description` so 5.2 can refuse a mismatched query model; `vector_top_k` returns at most ~200 rows, so a filtered query cannot over-fetch the whole table through it.

**5.2 `annatar search "<query>"`**
*Goal:* a plain-language question returns the top-k symbols with fqn, description and file:line.
Use `vector_top_k`, with optional kind and role filters. The CLI is the agents' interface too (D-ax): results and errors on stdout/stderr are stable, compact lines an agent can read without a parser, logs go to stderr only (`state.md` open #29), and `show` gives the detail view (description, tickets, commits, parent, children, file:line).
*Done when:* a query for a known responsibility lists the right class.
*Built (5.2, `state.md` D-by–D-cd):* `annatar search [-k N] [--kind K]… [--role R]… "<query>"`; the query is embedded through `LlmClient::embed` with an in-memory cache (search writes no file); an index without vectors, without `index_meta`, or embedded with another model or dimension is refused with one error line. Unfiltered queries use `vector_top_k`; filtered ones an exact `vector_distance_cos` scan of the matching rows (open #36); a member matches `--role` by its enclosing type's role. Output: `rank. score fqn [kind] role=… file:start-end` plus the description on an indented line (README; since 5.5 the `--symbols` output). Logs go to stderr, every error is one `error: …` line (open #29); `show` also names a symbol's parent.

**5.3 Evaluate**
*Goal:* a measured retrieval quality.
Run the golden set; report how often the expected fqn is in the top 1 and top 5.
The set is `.annatar-local/golden-argus.toml` (outside git, D-bl), loaded with `golden::GoldenSet::load`; run `golden::check_index` first so a renamed symbol fails loudly instead of counting as a miss. Score the first `expect` entry and, separately, "any of `expect`" (the alternates); report types and methods apart (`kind`).
*Done when:* numbers are in the PR. **This answers question 2.**
*Built (5.3, `state.md` D-cf–D-ch):* `annatar eval [-k N] <golden.toml>` checks the set against the index, runs every question through the unfiltered `search` path and prints per question the rank of the first and of the best `expect` fqn, then top-1, top-5 and MRR for all questions, types and members (README). `argus` (24 questions): an acceptable symbol in the top 5 for 71 %, first for 42 %; the intended one first for 17 % (n = 24, one repo, one author; 95 % CI roughly ±18 pp, e.g. 71 % ≈ 51–86 %). `[embedding] parent_description = true` is slightly better on the totals but within noise and hurts types, so the default stays off; `eval -k` is at least 5. Verdict: a good "where to look", not a one-shot answer; 5.4 tests whether that is enough for an agent.

**5.4 Agent trial**
*Goal:* a measured with/without comparison of agent efficiency and correctness.
The agent uses Annatar through the CLI (`annatar search`, `annatar show`) with its normal shell tool; there is no MCP server (`state.md` D-ax). Pick 3–5 real tasks where you know the relevant code. Decide up front how to measure tokens and tool calls (e.g. the agent's session cost or usage report). Run each task with and without Annatar.
*Done when:* results are written down. **This answers question 3.**

*Done (5.4, `state.md` D-cj–D-cl):* Claude Code headless on `claude-sonnet-5`, 4 read-only `argus` tasks (a cap found from user-facing words, two "which code and why" questions whose reasons live in Jira, one message-flow trace) × with/without `annatar` × 5 runs; metrics from the CLI's stream (tokens incl. cache, cost, tool calls by tool, time), correctness 0/1/2 against an answer key written before the runs and graded blind; harness `scripts/agent_trial/run.py`, tasks and raw results private. With `annatar`: −47 % tokens, −40 % tool calls, −32 % cost, −39 % time on average and lower on every task (significant per task on T1–T3, uncorrected; T4 a trend); correctness net equal, 35 vs 35 of 40 (after a strict regrade in the 5.4 review), with opposite effects — better where the reason lives only in tickets (5/5 vs 0/5), worse on call-chain completeness: on the flow trace 4 of 5 runs skipped the dispatcher in front of the consumer that search returned (open #42). Verdict for question 3: yes on this repository; n is small (5 runs × 4 tasks, one ≈ 7k-line repo, one model; open #41).

**5.5 File-level search results (product-owner redesign, 2026-10-04)**
*Goal:* `annatar search` identifies the file and lists its symbols, so an agent knows what to read.
Product owner: "ok so for a search return not the exact symbol but return the file and the class method symbols so for java where only on class per file is allowed return the class + description and then the methods with line numbers but without a description so a search basically tries to identify the file and then gives the symbols so an agent has a much better chance of identifying what he needs to read." Background: 5.3 found the right file in the top 5 for 75 % of questions but the exact symbol first for only 17 % (7 of 20 misses a class tying with its own member), and in 5.4 12 of 20 *with* runs called `show` after `search`.
Ranking stays symbol-vector based (the same `search::search` and filters): the 100 nearest symbols are grouped by file, files ranked by their best symbol, top `-k` files (default 5, at most 20). Per file: `rank. score path`; each top-level type as `kind fqn [role] :start-end` with its description; then every member and nested type in source order as `#name(params)` / `kind Name` with `:start-end`, no description, nested ones indented; symbols among the hits end with `*score`; more than 40 member lines are cut to the hits plus the first lines and `… N more` per top-level type. `--kind`/`--role`/`--path` decide which symbols can match (file rank and marks); the file is still shown whole. `--symbols` keeps the 5.2 output unchanged. `annatar eval` adds file-level top-1/top-5/MRR through the same grouping (`state.md` D-cm–D-cr).
*Done when:* the file output is the default, `--symbols` is unchanged, `eval` reports file-level scores on `argus` with the symbol scores unchanged from 5.3.
*Built (5.5):* `argus` (24 questions, 10 files): symbol scores unchanged (primary 4 / 14 of 24 top-1 / top-5, any 10 / 17); the primary's file first for 11 and in the top 5 for 17 (MRR 0.582), a file of any expected fqn first for 12 and in the top 5 for 20 (MRR 0.641). Output ≈ 20–30 % smaller than the old 10 symbols, query-dependent (6 own queries: ≈ 3.1k chars, ≈ 770 tokens, vs ≈ 4.4k), ≈ 50 ms per query. *Trial re-run (5.5, `state.md` D-cs, D-ct):* the 5.4 *with* arm repeated on the file output (20 runs, same tasks, model and key; the appended prompt describes the new output). Against no `annatar` (5.4) still −42 % tokens and tool calls, −26 % cost, significant on T1–T3; against the symbol output equal on T1–T3, while on the flow trace agents called `show` on whole classes (3.4 vs 0.8 per run) for ≈ 25 % more tokens (a trend). Correctness 37 vs 35 (symbols) vs 35 (without) of 40: the dispatcher missed in 2 of 5 flow traces instead of 4 of 5 — not significant, open #42 stays. The file view did not reduce `show` calls; it stays the default.

## Phase 6 — Usages (after the POC, planned and done 2026-10-04)

*Why:* open #42 — in the agent trial a search hit stood in for its caller (T4: the queue dispatcher in front of the consumer was missed in 4 of 5 runs with the symbol output, 2 of 5 with the file output). `project.md` promises `used_by`/`uses` in `show` and `annatar trace`. This phase builds them from the tree-sitter trees, without a build and without the LLM (`usages_state.md` D-cu); progress, decisions and questions of this phase are in [`usages_state.md`](./usages_state.md); SCIP stays in **Later** as a precision upgrade that fills the same `edges` table.

**Design:** [`usages.md`](./usages.md) — what counts as a usage (edge kinds `extends`, `implements`, `overrides`, `instantiate`, `call`, `reference`; imports, Javadoc links, recursion and external symbols are not usages; a type's `used by` rolls up its members), how a name resolves (Java's lookup order to an fqn or no edge, members through the receiver's static type, implicit Lombok/record/enum members, overloads), what stays invisible, entry points, modules, the `edges` table and the output (`usages_state.md` D-cu–D-cx).

**6.1 Parser facts for resolution**
*Goal:* everything the resolver needs from one file, on the parsed file, nothing stored yet.
Per file: package, single-type, wildcard and static imports. Per type: superclass and interfaces as written (generic arguments kept apart), fields (name, declared type, static, Lombok accessor annotations on field and type), Lombok type annotations. Per method: the declared return type as written, parameter names and types, varargs. Types inside anonymous or local classes stay ignored as symbols (1.3).
*Done when:* fixture tests cover each import form, generic super types, fields with and without Lombok, a varargs method, and that the `Symbol` records and fqns are unchanged (no reindex differences).

**6.2 Type resolution and the `edges` table**
*Goal:* every type-to-type usage in `index.db`.
`edges(id, src_id, dst_id, kind, line, ambiguous)` in `INDEX_TABLES`, `UNIQUE(src_id, dst_id, kind, line)`, indexes on `dst_id` and `src_id`. A new structure-side pass after all symbols are written (it needs the whole symbol table; it walks the syntax trees the structure stage parsed, kept in memory for it — `usages_state.md` D-db); kinds `extends`, `implements`, `reference`, `instantiate` (type side). No cache table, no LLM, no prompt change: a warm cache stays warm. `IndexStats` and the `index` summary gain edges by kind and unresolved type names (top unresolved names at `-v`). `--path`: only edges between symbols under the prefix; the existing warning says usages from outside it are missing.
*Done when:* fixture tests cover each `reference` construct, nested types, a same-package type, the same simple name in two packages chosen by import / same package / nested type (as `argus`'s four `MessageBearer`s), shadowing of a wildcard import, an unresolved external type (counted, no edge), a duplicate fqn, and the `--path` cut; the benchmark reports the pass's time.

**6.3 Member resolution**
*Goal:* `call` and `instantiate` edges to methods and constructors.
Receiver typing, chains, implicit members (Lombok, records, enums), method references (`this::dispatch`, `T::m`, `T::new`), static imports, `super.m()`, overload choice with `ambiguous` as described above; needs 6.1's return and field types. The `index` summary counts call sites resolved to a member / implicit / ambiguous / unresolved (top unresolved calls at `-v`).
*Done when:* fixture tests cover each case, including a chain through a getter, a chain cut by an external type, an untyped lambda parameter (no edge), overloads by count and by argument type, and an ambiguous pair; a test on `argus`-shaped fixtures resolves `Dispatcher#dispatch → Config#getAddEventConsumer() → AddEventConsumer#consume(..)`.

**6.4 Overrides and measured quality**
*Goal:* `overrides` edges and numbers for how complete and how precise the edges are.
`overrides`: a method to the method with the same name and parameter count (then resolved parameter types, `usages_state.md` D-dn) in a superclass or interface in the index, through generic interfaces (`MessageConsumer<T>#consume(T)`). The `index` summary reports resolved / unresolved / ambiguous call sites (since 6.3). A usage golden set outside git (`.annatar-local/usages-argus.toml`, like `golden-argus.toml`, D-bl): about 15 `argus` symbols (types, public and private methods, an interface method, a Lombok getter, a consumer), plus a reproducible random sample scored on its own, with their true direct users, taken from an IDE's *Find usages* on main sources (done by hand in 6.4: no IDE in this environment, `usages_state.md` D-dm); checked against the index first so a renamed symbol fails loudly (as `golden::check_index`); scored by `annatar eval-usages`.
*Done when:* precision and recall of direct users per symbol and overall, and the share of resolved call sites, are in the PR; every miss is classified (lambda, external chain, generics, other).

**6.5 `show`: `used by` and `uses`**
*Goal:* the direct usages of a symbol next to its description.
`used by`: the using symbols grouped by file, `fqn :line[, line…]` (call-site lines), edge kind when it is not `call`, `ambiguous` marked, callers of an overridden method labelled `via <fqn of I#m>`; a type rolls up its members' and nested types' incoming edges, without its own. `uses`: the symbols it uses, grouped the same way. Entry-point annotations are named. Each list is capped (`… N more`); `reference.md` documents it.
*Done when:* tests cover a type roll-up, `via`, the cap and a symbol without users; on `argus`, `show` of a T4 consumer names the dispatcher config and, through `via` or `call`, the dispatcher.

**6.6 `annatar trace <fqn> [--depth N]`**
*Goal:* transitive callers, so an agent sees the chain and what a change can break.
A depth-first walk over one query per symbol (`usage_query::callers`, the recursive CTEs of 6.5; `usages_state.md` D-dx) along incoming `call`, `instantiate` and `reference` edges, through `overrides` (`via`), default depth 6 (at most 10; D-ea), each symbol once, at its shallowest level (a repeat prints `(see above)` / `(see below)`, D-eb), cycles safe; leaves are entry points (`[entry: @Scheduled]`, unless code calls them too, D-ec) or symbols without callers (`[no callers in main sources]`), and a type below the root (`[type, not followed]`; one whose initializer is the caller followed through its constructions, `[initializer]`, D-dy); output as an indented tree with file:line (D-dz), capped per symbol (`-k`, default 10). A type traces its members' callers (rolled up as in `show`). Usage errors as `search` (exit 2).
*Done when:* tests cover depth, a cycle, a diamond, `via`, the entry-point and no-caller leaves; on `argus`, tracing a T4 consumer reaches the dispatcher's `@Scheduled` method.

**6.7 Agent trial (T4) on usages**
*Goal:* a measured answer to #42.
Re-run the *with* arm on T4 (5 runs, the D-cs protocol and grading) with `--with-prompt usages` in `scripts/agent_trial/run.py`: the file-output prompt plus one paragraph on `used by` and `trace`; update the agent snippet in the README in the same change. Success: the dispatcher in 5 of 5 answers, no cost increase over the 5.5 runs. Optional: T1–T3 once each to check nothing regressed.
*Done when:* results are in `usages_state.md`; #42 (`state.md`) closed or narrowed.
*Done (6.7, `usages_state.md` D-ed–D-eg):* `run.py --with-prompt usages` (the `files` prompt plus one descriptive bullet on `used by` / `uses` and `trace`), the *with* arm on T4 × 5 plus T1–T3 once, graded blind. The dispatcher named in 5 of 5 T4 answers (5.5: 3/5; symbols 1/5, p = 0.048), full marks 5/5 (2/5; 4/5 under a strict reading); no measurable cost increase over 5.5 (mean +7 %, median −8 %, p = 1.0; cache creation +21 % from the larger `show` output; tokens −10 %); T1–T3 unchanged in correctness. 3 of 5 agents found the dispatcher in the consumer's `used by`; none called `trace` (open #53). #42 closed; README agent snippet names `used by` and `trace`.

---

## Later (after the POC proves itself)

- **Usages from SCIP** (precision upgrade of Phase 6, same `edges` table): scip-java inside the real build, so generics, lambda parameters, chains through library types and modules resolve as the compiler does; needs the repository to build on the indexer (`usages_state.md` D-cu).
- **Callers in description prompts:** a member's callers (and a type's users) in its prompt. Every prompt changes, so the whole LLM cache misses (≈ 20 min of chat calls on `argus`); kept in Later after 6.7 (`usages_state.md` #47: the trial measured navigation, not descriptions).
- **Spring entry points:** HTTP handlers with full paths, listeners, `@Scheduled`; an entry-point command.
- **Incremental indexing:** stable IDs, upserts, incremental history.
- **Package summaries** and `annatar module`.
- **Central build and local use:** CI publishing the index per commit, local branch overlay.
- **Multi-module handling** (Maven/Gradle module per file, name resolution limited to the modules a file can see; a prerequisite for usages on a multi-module repository, `usages_state.md` D-cx), test-code indexing (first as sources of usages only, so `used by` and `trace` show test callers apart from main code: rows marked as test, never described, embedded or searched; `usages_state.md` #45, D-dv).
- **Analysis:** hotspots from churn and bug tickets, temporal coupling.
- **Spring configuration:** `application.yml`, `@Value`, `@ConfigurationProperties`.
- **Human search page** for non-technical roles.
- **MCP server** (`serve`, `search_intent`/`get_symbol` over stdio): dropped from the POC by product-owner decision (D-ax); agents use the CLI. Revisit only if a CLI proves insufficient.
- **Cache growth:** prune `history_cache_v2` rows (and later cache tables) for deleted or renamed symbols; today they stay forever.
- **CI pipeline** with fmt, clippy and tests (add whenever it starts to hurt not having it).