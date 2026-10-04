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
annatar index --no-llm                    # no chat-model calls; cached summaries and what/why only
annatar show com.acme.user.UserRepository     # a symbol, its children (with what/why), commits and tickets (with summaries)
```

`index` fetches every ticket key found in the history from Jira once and caches it in `cache.db` (403/404 and other permanent failures — another 4xx except 408/425/429, an issue that does not parse — are cached as unavailable; 5xx, timeouts, 429 and non-JSON answers are retried next run), so later runs make no Jira requests for known keys; the `tickets:` line of its output counts cached, fetched, unavailable and failed keys and the Jira requests made. Without a `[jira]` section or without `ANNATAR_JIRA_TOKEN` it warns once and uses cached tickets only; `--offline` does the same on purpose. Before fetching, `index` checks the credentials once (`/rest/api/2/myself`, retried on 429 like fetches); rejected credentials fail the run and nothing is cached. If Jira is unreachable or still rate limiting after retries, `index` stops fetching for that run with one warning, counts the remaining keys as not fetched and retries them next run. `show` lists each ticket with its type and summary, or `(unavailable)`.

`[ollama]` configures the LLM client (Ollama's OpenAI-compatible `/v1` API: chat model, embedding model, `reasoning_effort`, `temperature`; see `annatar.toml`). `index` asks the chat model for a short English summary and purpose of every available ticket (tickets are often German) and stores them in the index; `show` prints them under each ticket. Replies are cached in `cache.db`, so a rerun makes no chat calls; the `summaries:` line of the output counts summarised, invalid, failed and skipped tickets and the chat calls made. Without `[ollama]` it warns once and tickets get no summaries; `--no-llm` makes no chat calls and uses only the summaries already cached. The purpose is left out when the ticket gives no reason. Before building, `index` checks once that the Ollama server has the chat model: a missing model fails the run (`--no-llm` skips the check). A reply that names a ticket key (the `ticket_regex`) is invalid, here and in the what/why below. A reply that is still invalid after one retry is skipped for that ticket (retried next run); if the chat model fails (unreachable, HTTP error, timeout) or five replies in a row are invalid, `index` makes no further chat calls that run, warns once and keeps using cached summaries. After the ticket summaries, `index` asks the chat model for a one-line `what` and a `why` of every method and constructor and stores them in the index; `show` prints them under each member's signature. The prompt carries the member's Javadoc, its source (capped, `describe.body_chars`; the signature instead when it is `0`), the enclosing type and its Spring role, and the summaries of its first ticket plus its most recent few (`describe.recent_tickets`); a ticket whose summary was invalid stands in with its Jira title; unavailable tickets are left out (without Jira configured, so are tickets not in the cache), and when no ticket is available the most recent commit subjects stand in (`describe.commit_subjects`, merge commits left out). A member whose chosen ticket has not been fetched yet (`--offline`, a failed fetch) or has no summary because the breaker tripped or `--no-llm` is left without a what/why and asked again on a later run, so nothing built from incomplete input is cached; one warning lists the blocking ticket keys. The `why` is left out when the history gives no reason. The `describe:` line of the output counts described (and how many came from the cache), invalid, failed, incomplete and skipped members and the chat calls made; the breaker and `--no-llm` work as for the summaries. Then every type gets a `what` and `why` the same way, nested types first so their outer type can use them: the prompt carries the type's Javadoc, Spring role, enclosing type, its declaration with members and nested types cut out together with the comments directly above them (annotations, header, fields, enum constants; enum constant bodies and initializer blocks shortened to `{ … }`; capped by `describe.body_chars`), the `what` of up to `describe.type_members` members and nested types (public ones first, the rest counted) and its first plus most recent tickets (`describe.type_recent_tickets`). A type whose listed member or nested type has no `what` this run waits for a later run, like a member waiting for a ticket; one whose reply was invalid is listed as not described. `show` prints what/why for types too, and the `types:` line counts them like the `describe:` line. The live checks run with `cargo test llm -- --ignored` (`ANNATAR_OLLAMA_URL`, `ANNATAR_OLLAMA_CHAT_MODEL`, `ANNATAR_OLLAMA_EMBEDDING_MODEL`, `ANNATAR_OLLAMA_REASONING_EFFORT` override `annatar.toml`).

A golden set (questions with the fqns that answer them, the benchmark retrieval is measured against) is a TOML file of `[[question]]` tables (`text`, `expect` = the fqn the question is about first, then acceptable alternates, optional `kind` and `note`; see `src/golden.rs` and `tests/fixtures/golden/sample.toml`). A real set names a private repository's symbols and stays out of git (`.annatar-local/` is ignored). Check it against a full index (a run without `--path`) with `ANNATAR_GOLDEN_SET=<set.toml> ANNATAR_GOLDEN_DATA_DIR=<data dir holding index.db> cargo test real_golden_set -- --ignored`: it fails on any expected fqn the index does not hold and on a first fqn of another kind.

`--path <PREFIX>` is for fast iteration on part of the repo, not for refreshing a slice: a `--path` run still replaces the whole `index.db`, which then holds only that prefix (empty if the prefix matches nothing). The run logs a warning saying so; run `annatar index` without `--path` to get the full index back.

## Goals

- **Faster onboarding** for new engineers
- **Better decisions** by showing the original requirement and what depends on it
- **Self-service answers** for non-technical roles like product, support and QA
- **Fewer tokens** for coding agents, which find relevant code directly
- **Better agentic coding**, with changes that respect intent and don't break callers

## Where it fits

Long-lived codebases where the reasoning lives in old tickets and people's heads, teams that onboard often, and teams using coding agents on real repositories.
