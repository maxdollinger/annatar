# Annatar

> [!WARNING]
> **Vibe-coded proof of concept.** Expect rough edges and breaking changes. Not ready for real use.

Annatar maps a codebase by **intent**: what each class, method and function does, why it exists, and where it is used. The "why" comes from the tickets and commits that shaped the code.

## POC scope

For this proof of concept, Annatar targets:

- **Language:** Java
- **Ticket system:** Jira
- **LLM inference and embeddings:** Ollama, for local inference

## Usage

Annatar needs `git` on `PATH`, and the configured `repo` must be a git work tree for history and ticket keys. Otherwise only the structure is indexed, with one warning.

Configure `annatar.toml` (the default file; `--config <FILE>` points elsewhere). Relative paths resolve against the config file's directory. Secrets come only from the environment: `ANNATAR_JIRA_TOKEN` (Cloud API token or Server/DC personal access token) and `ANNATAR_JIRA_EMAIL` (basic auth when set, bearer otherwise). Jira Cloud requires `ANNATAR_JIRA_EMAIL`: it rejects an API token sent as bearer. How to point `[jira] base_url` at Cloud depends on the token:

- **Classic API token:** `ANNATAR_JIRA_EMAIL` plus the site URL, e.g. `base_url = "https://your-site.atlassian.net"`.
- **Scoped API token:** `ANNATAR_JIRA_EMAIL` plus the Atlassian API gateway, `base_url = "https://api.atlassian.com/ex/jira/<cloudId>"` (scoped tokens are not accepted on the site URL), and the scope `read:jira-work`. Without `read:jira-user` the credential check below is inconclusive and `index` goes on to fetch; a token that lacks `read:jira-work` fails the run on its first fetch.

```sh
cargo build --release                     # binary in target/release/annatar
annatar index                             # rebuild .annatar/index.db
annatar index --path src/main/java/com/acme   # only part of the repo
annatar index --offline                   # no Jira requests; cached tickets only
annatar index --no-llm                    # no chat or embedding calls; cached summaries, descriptions and embeddings only
annatar show com.acme.user.UserRepository     # a symbol, its children (with descriptions), commits and tickets (with summaries)
annatar search "who deletes expired tokens"   # the 10 symbols whose descriptions match best
annatar search -k 5 --kind method --role repository "delete expired tokens"
annatar --path src/main/java/com/acme/token search "delete expired tokens"   # only symbols in files under the prefix
annatar eval golden.toml                  # retrieval quality: top-1 / top-5 / MRR of a golden set
```

Results go to stdout, logs (`-v`, `RUST_LOG`) to stderr, without colours unless stderr is a terminal. A failing command prints one line, `error: …`, on stderr and exits with 1 (2 for a usage error such as an unknown `--kind`); warnings logged on the way (e.g. a retried request) may come before it.

`search` embeds the query with `ollama.embedding_model` and prints the most similar symbols, most similar first, two lines each:

```text
1. 0.770 com.acme.token.TokenCleanup#deleteExpiredTokens() [method] src/main/java/com/acme/token/TokenCleanup.java:32-37
   Schedules the deletion of expired user tokens every 5 minutes.
2. 0.758 com.acme.token.TokenCleanup [class] role=service src/main/java/com/acme/token/TokenCleanup.java:18-48
   Deletes expired user tokens from the database periodically.
```

rank, cosine similarity (1 = identical), fqn, `[kind]`, `role=<role>` when the symbol has a Spring role, `file:start-end`; the fqn is the third space-separated field, and the location is everything after the `[kind]` and `role=` tokens (a path may contain spaces); then the description on one line, indented by three spaces. `-k/--limit` sets the number of results (default 10, at most 100). `--kind` (`class`, `interface`, `enum`, `record`, `annotation`, `method`, `constructor`) and `--role` (`controller`, `service`, `repository`, `component`, `configuration`, `entity`; a method or constructor matches by its enclosing type's own role, so members of a nested type match by the nested type's role) filter the results and may be repeated. The global `--path <PREFIX>` applies to `search` too: only symbols in files at or under the prefix (whole path components; relative to the repository or absolute inside it; `.` or the repository itself means no filter; a prefix outside the repository, also by `..`, is an error). A filter that matches nothing prints nothing on stdout and a note on stderr. `search` needs the `[ollama]` section and a reachable server; it reads only `index.db` (the query's vector is not cached) and refuses an index without vectors, or one embedded with another `embedding_model` or dimension. Use `show <fqn>` on a result for the detail view (description, parent, children, file and lines, commits, tickets).

`index` fetches every ticket key found in the history from Jira once and caches it in `cache.db` (403/404 and other permanent failures — another 4xx except 408/425/429, an issue that does not parse — are cached as unavailable; 5xx, timeouts, 429 and non-JSON answers are retried next run), so later runs make no Jira requests for known keys; the `tickets:` line of its output counts cached, fetched, unavailable and failed keys and the Jira requests made. Without a `[jira]` section or without `ANNATAR_JIRA_TOKEN` it warns once and uses cached tickets only; `--offline` does the same on purpose. Before fetching, `index` checks the credentials once (`/rest/api/2/myself`, retried on 429 like fetches); rejected credentials fail the run and nothing is cached. If Jira is unreachable or still rate limiting after retries, `index` stops fetching for that run with one warning, counts the remaining keys as not fetched and retries them next run. `show` lists each ticket with its type and summary, or `(unavailable)`.

`[ollama]` configures the LLM client (Ollama's OpenAI-compatible `/v1` API: chat model, embedding model, `reasoning_effort`, `temperature`, `max_tokens` (a safety cap against runaway replies, not a length limit); see `annatar.toml`). `index` asks the chat model for a short English summary and purpose of every available ticket (tickets are often German) and stores them in the index; `show` prints them under each ticket. Replies are cached in `cache.db`, so a rerun makes no chat calls; the `summaries:` line of the output counts summarised, invalid, failed and skipped tickets and the chat calls made (with the largest prompt and completion token counts of one reply, to spot a prompt near the model's context size). Without `[ollama]` it warns once and tickets get no summaries; `--no-llm` makes no chat calls and uses only the summaries already cached. The purpose is left out when the ticket gives no reason. Before building, `index` checks once that the Ollama server has the chat model and the embedding model: a missing model fails the run (`--no-llm` skips the check). A reply that names a ticket key (the `ticket_regex`) is invalid, here and in the descriptions below. A reply that is still invalid after one retry is skipped for that ticket (retried next run); if the chat model fails (unreachable, HTTP error, timeout) or five replies in a row are invalid, `index` makes no further chat calls that run, warns once and keeps using cached summaries. After the ticket summaries, `index` asks the chat model for a `description` of every method and constructor — what it does and, where its code, tickets or commits give one, the reason it was created or changed, as short as possible without losing information (no length limit) — and stores it in the index; `show` prints it under each member's signature. The prompt carries the member's Javadoc, its source (capped, `describe.body_chars`; the signature instead when it is `0`), the enclosing type and its Spring role, the summaries of its first ticket plus its most recent few (`describe.recent_tickets`) and always its most recent commit subjects (`describe.commit_subjects`, merge commits left out); a ticket whose summary was invalid stands in with its Jira title; unavailable tickets are left out (without Jira configured, so are tickets not in the cache). A member whose chosen ticket has not been fetched yet (`--offline`, a failed fetch) or has no summary because the breaker tripped or `--no-llm` is left without a description and asked again on a later run, so nothing built from incomplete input is cached; one warning lists the blocking ticket keys. The `describe:` line of the output counts described (and how many came from the cache), invalid, failed, incomplete and skipped members and the chat calls made; the breaker and `--no-llm` work as for the summaries. Then every type gets a description the same way, nested types first so their outer type can use them: the prompt carries the type's Javadoc, Spring role, enclosing type, its declaration with members and nested types cut out together with the comments directly above them (annotations, header, fields, enum constants; enum constant bodies and initializer blocks shortened to `{ … }`; capped by `describe.body_chars`), the descriptions of up to `describe.type_members` members and nested types (public ones first, the rest counted), its first plus most recent tickets (`describe.type_recent_tickets`) and its most recent commit subjects. A type whose listed member or nested type has no description this run waits for a later run, like a member waiting for a ticket; one whose reply was invalid is listed as not described. `show` prints descriptions for types too, and the `types:` line counts them like the `describe:` line. Last, `index` embeds every symbol that has a description — its fqn and description, one per line (`embedding.parent_description = true` adds a method's or constructor's enclosing type's description) — with the embedding model and stores the vectors in the index (`symbol_vectors`, an `F32_BLOB` column of the dimension the model returns, with a cosine `libsql_vector_idx` index for `vector_top_k`) and records the embedding model, the dimension and `embedding.parent_description` in the `index_meta` table. Vectors are cached in `cache.db` by model and text, so a rerun makes no embedding calls; the `embeddings:` line counts embedded (and how many came from the cache), failed and skipped symbols, the embedding calls and texts sent and the dimension. `--no-llm` and a tripped breaker use cached embeddings only; a failed embedding request warns once, keeps the cached ones and leaves the rest for the next run; symbols left without a vector because their embedding is not cached get one warning. Cached vectors of another length than the model now returns (the model behind the name changed) are re-embedded. An index without any vector has no `symbol_vectors` table (inspect it with libSQL: the stock `sqlite3` shell sees no rows in it). The live checks run with `cargo test llm -- --ignored` (`ANNATAR_OLLAMA_URL`, `ANNATAR_OLLAMA_CHAT_MODEL`, `ANNATAR_OLLAMA_EMBEDDING_MODEL`, `ANNATAR_OLLAMA_REASONING_EFFORT` override `annatar.toml`).

A golden set (questions with the fqns that answer them, the benchmark retrieval is measured against) is a TOML file of `[[question]]` tables (`text`, `expect` = the fqn the question is about first, then acceptable alternates, optional `kind` and `note`; see `src/golden.rs` and `tests/fixtures/golden/sample.toml`). A real set names a private repository's symbols and stays out of git (`.annatar-local/` is ignored). Check it against a full index (a run without `--path`) with `ANNATAR_GOLDEN_SET=<set.toml> ANNATAR_GOLDEN_DATA_DIR=<data dir holding index.db> cargo test real_golden_set -- --ignored`: it fails on any expected fqn the index does not hold and on a first fqn of another kind.

`annatar eval <golden.toml>` measures retrieval against a golden set: it runs that check first (any problem is the error, nothing is searched), then searches every question exactly like `annatar search "<question>"` (no filter, `-k` hits, default 10, at least 5 because the summary counts top-5) and prints one line per question and three summary lines:

```text
eval: embedding_model=bge-m3:latest dim=1024 parent_description=false k=10
1. 1 1 [members] com.acme.token.TokenCleanup#deleteExpiredTokens()
2. 3 1 [types] com.acme.token.TokenCleanup top=com.acme.token.TokenCleanup#deleteExpiredTokens()
3. - - [types] com.acme.user.User top=com.acme.user.UserService
all 3: primary top-1 1 top-5 2 mrr 0.444; any top-1 2 top-5 2 mrr 0.667
types 2: primary top-1 0 top-5 1 mrr 0.167; any top-1 1 top-5 1 mrr 0.500
members 1: primary top-1 1 top-5 1 mrr 1.000; any top-1 1 top-5 1 mrr 1.000
```

the index's embedding settings and `k`; per question its number, the rank of the first expected fqn (primary), the best rank of any expected fqn (`-` = not in the top `k`), `types` or `members` (by the primary fqn), the primary fqn and, when it is not first, the first hit; then for all questions, types and members the number of questions, top-1 and top-5 counts and the mean reciprocal rank (a miss counts 0, so MRR depends on `k`), for the primary and for any expected fqn. It needs the `[ollama]` section like `search` and takes no `--path`.

`--path <PREFIX>` is for fast iteration on part of the repo, not for refreshing a slice: a `--path` run still replaces the whole `index.db`, which then holds only that prefix (empty if the prefix matches nothing). The run logs a warning saying so; run `annatar index` without `--path` to get the full index back.

## Goals

- **Faster onboarding** for new engineers
- **Better decisions** by showing the original requirement and what depends on it
- **Self-service answers** for non-technical roles like product, support and QA
- **Fewer tokens** for coding agents, which find relevant code directly
- **Better agentic coding**, with changes that respect intent and don't break callers

## Where it fits

Long-lived codebases where the reasoning lives in old tickets and people's heads, teams that onboard often, and teams using coding agents on real repositories.
