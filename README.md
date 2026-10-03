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

Configure `annatar.toml` (the default file; `--config <FILE>` points elsewhere). Relative paths resolve against the config file's directory. Secrets come only from the environment: `ANNATAR_JIRA_TOKEN` (Cloud API token or Server/DC personal access token) and, for Cloud only, `ANNATAR_JIRA_EMAIL` (basic auth when set, bearer otherwise).

```sh
cargo build --release                     # binary in target/release/annatar
annatar index                             # rebuild .annatar/index.db
annatar index --path src/main/java/com/acme   # only part of the repo
annatar index --offline                   # no Jira requests; cached tickets only
annatar show com.acme.user.UserRepository     # a symbol, its children, commits and tickets
```

`index` fetches every ticket key found in the history from Jira once and caches it in `cache.db` (403/404 are cached as unavailable), so later runs make no Jira requests for known keys; the `tickets:` line of its output counts cached, fetched, unavailable and failed keys and the Jira requests made. Without a `[jira]` section or without `ANNATAR_JIRA_TOKEN` it warns once and uses cached tickets only; `--offline` does the same on purpose. Rejected credentials fail the run. If Jira is unreachable or still rate limiting after retries, `index` stops fetching for that run with one warning, counts the remaining keys as not fetched and retries them next run. `show` lists each ticket with its type and summary, or `(unavailable)`.

`--path <PREFIX>` is for fast iteration on part of the repo, not for refreshing a slice: a `--path` run still replaces the whole `index.db`, which then holds only that prefix (empty if the prefix matches nothing). The run logs a warning saying so; run `annatar index` without `--path` to get the full index back.

## Goals

- **Faster onboarding** for new engineers
- **Better decisions** by showing the original requirement and what depends on it
- **Self-service answers** for non-technical roles like product, support and QA
- **Fewer tokens** for coding agents, which find relevant code directly
- **Better agentic coding**, with changes that respect intent and don't break callers

## Where it fits

Long-lived codebases where the reasoning lives in old tickets and people's heads, teams that onboard often, and teams using coding agents on real repositories.
