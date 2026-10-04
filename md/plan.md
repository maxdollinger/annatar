# Annatar — POC Plan

> The proof-of-concept is a subset of the target described in
> [`project.md`](./project.md). Anything there that this plan doesn't cover is
> in **Later** at the end.

Scope: **Java / Spring** only, **Jira** only (keys like `GRLD-123`), **Ollama** for LLM and embeddings, **libSQL** for storage, written in **Rust**.

## What the POC has to prove

1. **Quality:** what/why records generated from signature, Javadoc and ticket summaries are correct and useful.
2. **Retrieval:** searching what/why by meaning finds the right class or method for a plain-language question.
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

**4.3 Method and constructor what/why**
*Goal:* every method and constructor has a one-line `what` and a `why`, stored by symbol in `index.db`.
Input, built by a pure, tested function: signature, Javadoc, parent class name and role, and ticket summaries (the first ticket plus the most recent few, capped). If all its tickets are unavailable, use commit subjects instead. A method whose chosen available ticket has no summary this run (failed, or skipped after the breaker tripped; an invalid one falls back to the Jira title since the review fixes) is skipped this run rather than generated without it, so the cache never holds a what/why built from incomplete input; a `NULL` purpose is valid input and means the ticket gives no "why". Try it on one package with `--path` first and note time per symbol.
*Done when:* `show` prints what/why for methods.
*Note:* a stage after summaries (also on a non-git repository, code only) writes `what` / `why` columns on the index `symbols` row; besides signature, Javadoc and parent it sends a capped excerpt of the member's source (`describe.body_chars`, default 1500: on `argus` it made `what` concrete and stopped it guessing from ticket text); the first ticket is the oldest `first_date`, the recent ones the newest `last_date` (`describe.recent_tickets`, default 3), dates compared in UTC, ties by key; commit subjects (`describe.commit_subjects`, default 5, merges left out) only when no ticket is available; `why` is empty (`NULL`) when the history gives no reason (`state.md` D-at–D-aw). *Review fixes (D-az–D-bb):* only the *chosen* tickets can make a member incomplete — a ticket not fetched yet competes in the selection and blocks only when it would be chosen; permanent fetch failures are cached unavailable and, without Jira configured, uncached tickets count as unavailable; a ticket whose summary was *invalid* stands in with its Jira title (a summary missing because the breaker tripped or `--no-llm` still holds the member back); the source excerpt replaces the signature line.

**4.4 Type what/why, bottom-up**
*Goal:* every type has a `what` and `why` built from its own context and its members' `what` lines.
Process the deepest nested types first, so an outer class can include its nested types' `what` lines. Cap the input for large classes (e.g. public members first, then a fixed maximum). A type's history span is its whole body, so its tickets are effectively the file's tickets: cap those too. Because the input includes members' output, a changed method automatically misses the cache for its class.
*Done when:* `show` prints what/why for types, including a large class and a nested class.
*Note:* a stage after describe writes the same `what` / `why` columns for types. The prompt carries kind, fqn, Spring role, the enclosing type, Javadoc, the declaration with its members and nested types cut out (annotations, header, fields, enum constants; capped by `describe.body_chars`), the `what` lines of up to `describe.type_members` (default 30) members and nested types — public first, then the rest, listed in source order, the remainder counted — and the type's own first plus `describe.type_recent_tickets` (default 3) most recent tickets, chosen and blocked as for members. A *listed* child without a `what` this run holds the type back (and so its outer type); a child whose reply was invalid is listed as "(not described)", mirroring the title fallback (`state.md` D-be–D-bg).

**4.5 Quality review and golden set**
*Goal:* evidence that the records are trustworthy, and a fixed benchmark for retrieval.
Review 20–30 symbols you know well against the code and tickets; adjust prompts. Write 15–20 plain-language questions, each with the fqn it should find, as a test fixture.
*Done when:* you'd trust the records you reviewed; the golden set exists. **This answers question 1.**

## Phase 5 — Search and agent trial

**5.1 Embeddings**
*Goal:* every symbol with what/why has a vector in a searchable index.
Embed `fqn + what + why` through the embedding cache. Take the dimension from the first embedding response, not from config. Store it in an `F32_BLOB(<dim>)` column with a `libsql_vector_idx` index.
*Done when:* every symbol with what/why has a vector.

**5.2 `annatar search "<query>"`**
*Goal:* a plain-language question returns the top-k symbols with fqn, what, why and file:line.
Use `vector_top_k`, with optional kind and role filters. The CLI is the agents' interface too (D-ax): results and errors on stdout/stderr are stable, compact lines an agent can read without a parser, logs go to stderr only (`state.md` open #29), and `show` gives the detail view (what, why, tickets, parent, children, file:line).
*Done when:* a query for a known responsibility lists the right class.

**5.3 Evaluate**
*Goal:* a measured retrieval quality.
Run the golden set; report how often the expected fqn is in the top 1 and top 5.
*Done when:* numbers are in the PR. **This answers question 2.**

**5.4 Agent trial**
*Goal:* a measured with/without comparison of agent efficiency and correctness.
The agent uses Annatar through the CLI (`annatar search`, `annatar show`) with its normal shell tool; there is no MCP server (`state.md` D-ax). Pick 3–5 real tasks where you know the relevant code. Decide up front how to measure tokens and tool calls (e.g. the agent's session cost or usage report). Run each task with and without Annatar.
*Done when:* results are written down. **This answers question 3.**

---

## Later (after the POC proves itself)

- **Usages from SCIP:** scip-java setup, definition mapping, reference edges, interface and override edges (needed for Spring injection), Lombok-generated members, callers in prompts, `annatar trace`.
- **Spring entry points:** HTTP handlers with full paths, listeners, `@Scheduled`; an entry-point command.
- **Incremental indexing:** stable IDs, upserts, incremental history.
- **Package summaries** and `annatar module`.
- **Central build and local use:** CI publishing the index per commit, local branch overlay.
- **Multi-module handling** (Maven/Gradle module per file), test-code indexing.
- **Analysis:** hotspots from churn and bug tickets, temporal coupling.
- **Spring configuration:** `application.yml`, `@Value`, `@ConfigurationProperties`.
- **Human search page** for non-technical roles.
- **MCP server** (`serve`, `search_intent`/`get_symbol` over stdio): dropped from the POC by product-owner decision (D-ax); agents use the CLI. Revisit only if a CLI proves insufficient.
- **Cache growth:** prune `history_cache_v2` rows (and later cache tables) for deleted or renamed symbols; today they stay forever.
- **CI pipeline** with fmt, clippy and tests (add whenever it starts to hurt not having it).