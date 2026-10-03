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

Configure `annatar.toml` (the default file; `--config <FILE>` points elsewhere). Relative paths resolve against the config file's directory. Secrets come only from the environment (`ANNATAR_JIRA_TOKEN`, `ANNATAR_JIRA_EMAIL`).

```sh
cargo build --release                     # binary in target/release/annatar
annatar index                             # rebuild .annatar/index.db
annatar index --path src/main/java/com/acme   # only part of the repo
annatar show com.acme.user.UserRepository     # a symbol, its children, commits and tickets
```

## Goals

- **Faster onboarding** for new engineers
- **Better decisions** by showing the original requirement and what depends on it
- **Self-service answers** for non-technical roles like product, support and QA
- **Fewer tokens** for coding agents, which find relevant code directly
- **Better agentic coding**, with changes that respect intent and don't break callers

## Where it fits

Long-lived codebases where the reasoning lives in old tickets and people's heads, teams that onboard often, and teams using coding agents on real repositories.
