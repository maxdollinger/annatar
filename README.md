# Annatar

> [!WARNING]
> **Vibe-coded proof of concept.** Expect rough edges and breaking changes. Not ready for real use.

Annatar maps a codebase by **intent**: what each class, method and function does, why it exists, and where it is used. The "why" comes from the tickets and commits that shaped the code.

## POC scope

For this proof of concept, Annatar targets:

- **Language:** Java
- **Ticket system:** Jira
- **LLM inference and embeddings:** Ollama, for local inference

## Install

On Linux or macOS (x86_64 or arm64):

```sh
curl -fsSL https://github.com/maxdollinger/annatar/releases/latest/download/install.sh | sh
```

This installs the latest [release](https://github.com/maxdollinger/annatar/releases) to `~/.local/bin` and tells you if that folder is not on your `PATH`. To choose the version or folder, pass them to `sh`: `… | ANNATAR_VERSION=v0.1.0 ANNATAR_INSTALL_DIR="$HOME/bin" sh`.

From source, with Rust (stable): `cargo build --release` puts the binary in `target/release/annatar`.

## Before you start

You need:

- **git** on `PATH`. The repository to index must be a git checkout (history and ticket keys come from it).
- **Ollama**, running and reachable, with a chat model and an embedding model pulled:
  ```sh
  ollama pull qwen3.8
  ollama pull bge-m3
  ```
- **Jira API token** (optional, but without it descriptions get no ticket context). Set it in the environment, never in the config file:
  ```sh
  export ANNATAR_JIRA_TOKEN=...
  export ANNATAR_JIRA_EMAIL=you@example.com   # Jira Cloud
  ```

## First index run

1. **Create `annatar.toml`** in the folder you run `annatar` from, or pass `--config <file>`. Relative paths resolve against the file's folder:
   ```toml
   repo = "/path/to/your/repo"
   data_dir = ".annatar"           # index.db + cache.db
   ticket_regex = '\bABC-\d+'      # your Jira project key

   [jira]
   base_url = "https://your-site.atlassian.net"

   [ollama]
   url = "http://localhost:11434"
   chat_model = "qwen3.8:latest"
   embedding_model = "bge-m3:latest"
   ```
   All options are explained in this repository's [`annatar.toml`](./annatar.toml). For a scoped Jira token, see the [reference](./md/reference.md).
2. **Add the data directory to `.gitignore`** (`.annatar/`).
3. **Run the index:**
   ```sh
   annatar index
   ```
   The first run is slow: the LLM describes every class and method (≈ 2 s per symbol with a local 27B model, ≈ 20 min for 650 symbols). To try one package first: `annatar index --path src/main/java/com/acme/user`.

Rerun `annatar index` whenever the code changes. It rebuilds the index from scratch, but LLM results are cached in `cache.db`, so only new or changed code costs time.

## Using it

```sh
annatar search "who deletes expired tokens"     # the best matching files with their outline
annatar show com.acme.token.TokenCleanup        # one symbol: description, tickets, commits, parent, children
```

`search` returns files, best first: the class with its description, then its methods with line numbers; matching lines end with `*score`. `--symbols` lists single symbols instead.

For a coding agent, add this to the repository's `AGENTS.md` / `CLAUDE.md`:

```markdown
## Annatar

`annatar` is an index of this repository: what each class and method does and why (from Jira tickets and commits).
- `annatar search "<question>"` finds the relevant files. Use it before grepping.
- `annatar show '<fqn>'` explains one symbol with its tickets and commits.
Descriptions can be wrong; the code is the source of truth.
```

Useful flags: `index --offline` (no Jira requests), `index --no-llm` (cache only), `-v` (logs on stderr). Everything else (output formats, filters, `eval`, how each stage works) is in [`md/reference.md`](./md/reference.md).

## POC findings

Measured on one Java/Spring repository (654 symbols, 69 tickets); details in [`md/state.md`](./md/state.md).

- **Descriptions:** generated for every class and method with no invalid replies. A full cold run takes about 20 minutes, and a rerun with a warm cache makes 0 LLM calls (about 2 s).
- **Retrieval** (24 golden questions, `annatar eval`): search finds the right **file**, not the exact symbol. The expected file ranks first for 11 of 24 questions and is in the top 5 for 17 (20 counting alternates); the exact symbol ranks first for only 4. That is why `search` returns files.
- **Agents** (4 tasks × 5 runs, Claude Code via the CLI): with Annatar, agents used about 40–45 % fewer tokens and tool calls, cost about 25–30 % less and were as correct or slightly more (37 vs 35 of 40). Reasons that live only in tickets were found only with Annatar (5/5 vs 0/5). The weak spot is tracing a call chain across files, where search can skip a step.
- **Caveats:** small samples, one repository, tasks and questions written by the same author.

## Goals

- **Faster onboarding** for new engineers
- **Better decisions** by showing the original requirement and what depends on it
- **Self-service answers** for non-technical roles like product, support and QA
- **Fewer tokens** for coding agents, which find relevant code directly
- **Better agentic coding**, with changes that respect intent and don't break callers

## Where it fits

Long-lived codebases where the reasoning lives in old tickets and people's heads, teams that onboard often, and teams using coding agents on real repositories.

## Releasing

Run the **release** workflow in GitHub Actions. It builds the Linux and macOS binaries and publishes them with `install.sh`, tagged `v<version>` from `Cargo.toml` unless you give a tag.
