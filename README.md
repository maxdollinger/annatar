# Annatar

> [!WARNING]
> **Vibe-coded proof of concept.** Expect rough edges and breaking changes. Not ready for real use.

Annatar maps a codebase by **intent**: what each class, method and function does, why it exists, and where it is used. The "why" comes from the tickets and commits that shaped the code.

## Goals

- **Faster onboarding** for new engineers
- **Better decisions** by showing the original requirement and what depends on it
- **Self-service answers** for non-technical roles like product, support and QA
- **Fewer tokens** for coding agents, which find relevant code directly
- **Better agentic coding**, with changes that respect intent and don't break callers

## Key finding

Early tests show that an agent using Annatar spent **roughly half the tokens** and cost **around 30% less** than the same agent without it, without losing correctness. Details and caveats under [POC findings](#poc-findings).

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

  A scoped Cloud token needs only the scope `read:jira-work` and works only through the Atlassian gateway: `base_url = "https://api.atlassian.com/ex/jira/<cloudId>"`.

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

   All options are explained in this repository's [`annatar.toml`](./annatar.toml).
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
annatar show com.acme.token.TokenCleanup        # one symbol: description, parent, children, used by / uses (--history: also commits and tickets)
annatar trace 'com.acme.token.TokenRepository#deleteByExpirationDateBefore(ZonedDateTime)'   # its callers, transitively, up to the entry points
```

`search` returns files, best first: the class with its description, then its methods with line numbers; matching lines end with `*score`. `--symbols` lists single symbols instead.

For a coding agent, add this to the repository's `AGENTS.md` / `CLAUDE.md`:

```markdown
## Annatar

`annatar` is an index of this repository: what each class and method does and why (from Jira tickets and commits).
- `annatar search "<question>"` finds the relevant files. Use it before grepping.
- `annatar show '<fqn>'` explains one symbol: its description, who uses it (`used by`) and what it uses. `--history` adds its commits and tickets.
- `annatar trace '<fqn>'` lists its callers transitively, up to the entry points (`@Scheduled`, HTTP mappings, …).
Descriptions can be wrong; the code is the source of truth.
```

Useful flags: `index --offline` (no Jira requests), `index --no-llm` (cache only), `-v` (logs on stderr).

## POC findings

On one Java/Spring repository (654 symbols, 69 tickets).

- **Search** finds the right file (top 5 for 17 of 24 questions), rarely the exact symbol, so it returns files.
- **Usages** are precise: precision 1.000, recall 0.972 on 54 hand-checked symbols; calls inside lambdas and into libraries stay unresolved.
- **Agents** (Claude Code, 4 tasks × 5 runs, with vs without): about half the tokens and tool calls, a quarter lower cost (significant on one task of four), 39 vs 34 of 40 correct. Reasons that live in tickets were found only with Annatar (5/5 vs 0/5). With `used by`, all agents found the step in a call chain that search had made them skip before (5/5, earlier 1–3/5). Agents never called `trace`.
- **Caveats:** small samples, one repository, tasks written by the same author; usages miss reflection, configuration wiring, message brokers and test callers.
