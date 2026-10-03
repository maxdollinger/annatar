# Annatar — POC Plan

Scope: **Java / Spring** only, **Jira** only (keys like `GRLD-123`), **Ollama** for LLM and embeddings, **libSQL** for storage, written in **Rust**.

## What the POC has to prove

1. **Quality:** what/why records generated from signature, Javadoc and ticket summaries are correct and useful.
2. **Retrieval:** searching what/why by meaning finds the right class or method for a plain-language question.
3. **Agent value:** a coding agent using the MCP server finds relevant code faster and with fewer tokens.

Everything that doesn't help answer these three questions is in **Later**, at the end.

## Ground rules

- One step = one small PR. Don't start the next step in the same change.
- Every step ships with tests. Unit tests use fixtures and never need network, Jira or Ollama. Tests that do are marked `#[ignore]` and run manually.
- **Rebuild, don't update.** Each `annatar index` run rebuilds symbols and history from scratch. No incremental updates, no stable IDs.
- **Cache only what's expensive,** in cache tables that survive rebuilds, keyed by content:
  - tickets by ticket key (fetched once, never refreshed: done tickets don't change)
  - LLM outputs by a hash of model + prompt text + input
  - embeddings by a hash of model + text
  
  A changed prompt, model or input simply misses the cache. No versioning or invalidation logic is needed.
- `--path <prefix>` limits a run to part of the repo, for fast iteration on prompts.

---

## Phase 0 — Skeleton

**0.1 CLI, config, logging**
Binary `annatar` with `clap` subcommands `index`, `show`, `search`, `serve` (stubs), `--path` and `-v`. Load `annatar.toml`: repo path, database path, ticket key regex (default `\bGRLD-\d+\b`), Jira base URL and auth, Ollama URL, chat model, embedding model and dimension. Secrets from environment variables. Logging with `tracing`.
*Done when:* `--help` works; config tests cover defaults and a missing required value.

**0.2 Database**
Open a local libSQL file. Create the rebuild tables (dropped and recreated on every index run) and the cache tables (kept).
*Done when:* a test shows rebuild tables are emptied and cache tables survive a second run.

## Phase 1 — Symbols

**1.1 File walker**
List `*.java` files with the `ignore` crate, respecting `.gitignore`, skipping `build/`, `target/`, generated sources and `src/test/`.
*Done when:* a temp-directory test returns exactly the expected files.

**1.2 Parse types**
With `tree-sitter` + `tree-sitter-java`, extract the package and all classes, interfaces, enums and records, including nested ones, with spans and a parent link. Log and skip files with parse errors.
*Done when:* fixture tests cover each kind, nesting and a broken file.

**1.3 Parse methods and constructors**
Extract methods and constructors with a qualified name like `com.acme.user.UserRepository#findById(Long)` (parameter types as written, so overloads are distinct). Ignore anonymous and local classes.
*Done when:* fixture tests cover overloads and constructors.

**1.4 Signature, Javadoc, annotations, Spring role**
Capture signature (without body), Javadoc and annotations. Derive a `role` per type from annotations: `controller`, `service`, `repository` (including Spring Data interfaces extending e.g. `JpaRepository`), `component`, `configuration`, `entity`.
*Done when:* fixture tests cover Javadoc present and absent, and each role.

**1.5 Write symbols and `show`**
`symbols` table: id, parent_id, kind, role, fqn, file, start/end line, signature, javadoc, annotations. `annatar show <fqn>` prints a symbol and its children.
*Done when:* a real repo indexes and `show` works on it.

## Phase 2 — History and ticket keys

**2.1 History for a span**
Run `git log -L <start>,<end>:<file>` via `std::process::Command` with a custom `--format` and record delimiter; return sha, date, subject and body. Note in the PR how it behaves on renamed files.
*Done when:* a test on a scratch git repo returns the expected commits.

**2.2 Ticket keys**
Extract keys with the configured regex, deduplicate, and store `symbol_tickets(symbol_id, ticket_key, first_seen_date)`. `show` lists them.
*Done when:* tests cover multiple, duplicate and missing keys.

**2.3 Measure**
Time a full run on the target repo. If it's too slow to iterate, add bounded parallelism (results written by one writer) or a history cache keyed by fqn + symbol content hash + the file's last commit sha.
*Done when:* run time is noted in the PR and acceptable for daily iteration.

## Phase 3 — Jira

**3.1 Fetch and parse**
`fetch(key)` with `reqwest` and the target's auth. Request `expand=renderedFields` and strip the HTML to plain text; this avoids writing an Atlassian Document Format parser. Keep key, issue type, summary, description, parent/epic key.
*Done when:* parsing is tested against saved JSON fixtures; an `#[ignore]` test fetches a real ticket.

**3.2 Ticket cache**
Store tickets in a cache table. Fetch only missing keys. Record 403/404 as unavailable; back off on 429.
*Done when:* a second run makes no Jira calls.

## Phase 4 — LLM summaries

**4.1 LLM client**
`complete<T: DeserializeOwned + JsonSchema>(prompt)` and `embed(texts)` against Ollama's OpenAI-compatible `/v1` endpoint, using the `schemars` schema as structured-output format. Retry once on invalid output. Results go through the LLM cache.
*Done when:* an `#[ignore]` test returns a valid struct; a repeated call hits the cache.

**4.2 Ticket summaries**
Per ticket: a one- to two-sentence summary and the purpose (why).
*Done when:* all cached tickets have a summary; a rerun makes no LLM calls.

**4.3 Method and constructor what/why**
Input: signature, Javadoc, class name and role, ticket summaries (first ticket plus the most recent few, capped). Output: `what` (one high-level line) and `why`. Build the input in a pure, tested function. Try it on one package with `--path` first and note time per symbol.
*Done when:* `show` prints what/why for methods.

**4.4 Class what/why**
Input: class signature, Javadoc, role, its tickets and its members' `what` lines. Because the input includes the members' output, a changed method automatically changes the class input and misses the cache.
*Done when:* `show` prints what/why for classes.

**4.5 Quality review and golden set**
Review 20–30 symbols you know well against the code and tickets; adjust prompts. Write 15–20 plain-language questions, each with the symbol it should find, as a test fixture.
*Done when:* you'd trust the records you reviewed; the golden set exists. **This is the answer to question 1.**

## Phase 5 — Search

**5.1 Embeddings**
Add an `F32_BLOB(<dim>)` column with a `libsql_vector_idx` index. Embed `fqn + what + why` per symbol through the embedding cache.

**5.2 `annatar search "<query>"`**
Top-k via `vector_top_k`, optional kind and role filters, printing what, why and file:line.

**5.3 Evaluate**
Run the golden set; report how often the expected symbol is in the top 1 and top 5.
*Done when:* numbers are in the PR. **This is the answer to question 2.**

## Phase 6 — MCP

**6.1 Server**
`annatar serve` with `rmcp` over stdio: `search_intent(query, kind?, role?)` and `get_symbol(id)` (what, why, tickets, parent, children).

**6.2 Agent trial**
Pick 3–5 real tasks where you know the relevant code. Run each with and without Annatar and compare tool calls, tokens and whether the agent found the right code.
*Done when:* results are written down. **This is the answer to question 3.**

---

## Later (after the POC proves itself)

- **Usages from SCIP:** scip-java setup, definition mapping, reference edges, interface and override edges (needed for Spring injection), Lombok-generated members, callers in prompts, `trace_usage`.
- **Spring entry points:** HTTP handlers with full paths, listeners, `@Scheduled`; `find_entry_point`.
- **Incremental indexing:** stable IDs, upserts, incremental history, last indexed HEAD.
- **Package summaries** and `get_module`.
- **Central build and local serve:** CI publishing the index per commit, local branch overlay.
- **Multi-module handling** (Maven/Gradle module per file), test-code indexing.
- **Analysis:** hotspots from churn and bug tickets, temporal coupling.
- **Spring configuration:** `application.yml`, `@Value`, `@ConfigurationProperties`.
- **Human search page** for non-technical roles.
- **CI pipeline** with fmt, clippy and tests (add whenever it starts to hurt not having it).