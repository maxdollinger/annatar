# Annatar — Project Overview

Oct 3, 2026 · @Max

> **This is the target.** It describes where Annatar is headed. The
> proof-of-concept being built now is a subset of it: see [`plan.md`](./plan.md).
> Everything here that the POC leaves out (SCIP usages, modules, analysis,
> central build, other languages) is listed under "Later" in the plan.

## Summary

Annatar is a repository index that records, for every class, method, function and type, what it does, why it exists and where it is used. The "why" comes from the tickets and commits that shaped the code. Engineers, non-technical colleagues and coding agents query it by meaning through an MCP server.

**The idea:** a map of the codebase at the level of intent, not implementation. A `UserRepository` reads as "organizes database access for the user entity, introduced for ticket X so that Y", with links to every place that uses it.

**The intent:** five outcomes that the same index serves at once:

1. Better onboarding for new engineers
2. Better engineering decisions
3. Self-service answers for non-technical roles
4. Lower token usage for coding agents
5. Higher overall quality of agentic coding

## Problem

The reason a piece of code exists is usually written down somewhere, just never next to the code. It sits in a user story, a bug ticket, a PR description or a commit message, and nobody connects it back to the class or method it explains.

- **People** reconstruct intent by asking colleagues, reading history by hand, or guessing. When the person who knew leaves, the knowledge goes with them.
- **Non-technical roles** cannot answer "what does this part of the system do?" without pulling an engineer away from their work.
- **Coding agents** rediscover the codebase on every task with grep and file reads. They spend much of their budget finding code instead of changing it, and they never see the ticket that explains why the code looks the way it does.

Existing tools cover pieces of this. Semantic code search indexes raw code, code graphs index structure, and wiki generators describe what code does. None of them join structure, high-level purpose and the ticket-level "why" at the symbol level.

## Goals

One index serves all five goals; each draws on a different part of it.

| Goal | What the index provides |
| --- | --- |
| Better human onboarding | Module and class summaries as an entry point, with the tickets that explain why each part exists |
| Better engineering decisions | The original requirement behind a symbol and everything that depends on it, before someone changes or removes it |
| Easy lookup for non-technical roles | Plain-language answers to "what does this part do and which feature is it for", without reading code |
| Lower agent token usage | Ranked symbols with file:line from one query, instead of many grep and read calls |
| Higher quality of agentic coding | The requirement and the usages next to the code, so changes respect intent and don't break callers |

## Users and use cases

| User | Typical question | What they get |
| --- | --- | --- |
| New engineer | "Where does user persistence live and why is it split this way?" | Module overview, the main symbols, the tickets behind them |
| Experienced engineer | "Can I remove this method, and who relies on it?" | The requirement it served, its direct and transitive callers |
| Product owner, support, QA | "Which part of the system handles refunds?" | A plain-language description and the user stories it implements |
| Coding agent | "Find the code responsible for session expiry" | Ranked symbols with file:line, description, usages, in one call |

## How it works

Five steps turn a repository into an intent index. Every step is incremental: a content hash per symbol means a change regenerates only the affected symbols and their parents.

1. **Structure:** tree-sitter extracts every class, method and function with its signature, doc comment and file:line.
2. **History:** `git log -L` per symbol yields the commits that shaped it and the ticket keys they mention.
3. **Tickets:** each referenced ticket is fetched once and summarised.
4. **Descriptions:** an LLM writes one short description per symbol (what it does and, where its history explains it, why it exists) from its code, its ticket summaries and its commit history, bottom-up from members to types. (Until POC step 4.5 this was a separate *what* and *why*; the product owner merged them in 4.6.)
5. **Embeddings:** the description text is embedded for search.

Embeddings are made from the description text, not from code, so similarity search matches intent: "where do we handle user persistence" finds `UserRepository`. Structure and history need no LLM; the LLM is used only for ticket summaries and the symbol descriptions.

## Interfaces

The `annatar` CLI is the primary interface, for people and coding agents alike: agents are good at driving CLIs through their normal shell tool, so there is no separate MCP server. Every result carries a file:line, so an agent jumps straight to the code with its normal read tools: Annatar answers *where* and *why*, the agent reads the code itself.

| Command | Returns |
| --- | --- |
| `annatar search "<query>" [--kind] [--role] [--path]` | Ranked symbols with name, kind, file:line and description; no code |
| `annatar show <fqn>` | Full description, parent (method → class → module), direct `used_by` and `uses`; linked tickets and commits on request (`--history`), since the description already condenses them |
| `annatar module <path>` | Module summary and its main symbols; the entry point for an unfamiliar area |
| `annatar trace <fqn> --depth <n>` | Transitive callers, so an agent or engineer sees what a change can break |

For people, a thin search page over the same index covers non-technical roles who don't use an agent. It shows the same records as the CLI, in plain language and without code.

## Architecture and deployment

The index is built centrally and served locally. Symbols are identified by their fully qualified name everywhere outside a single build, since row IDs change on every rebuild.

**Storage:** two libSQL files, with libSQL's native vector index (`F32_BLOB` + `libsql_vector_idx`).

- `index.db` is the portable artifact: rebuilt from scratch on every run into a temp file and atomically renamed into place, so a reader never sees a half-built index.
- `cache.db` persists only expensive, content-keyed results (history per symbol, tickets, LLM outputs, embeddings) across rebuilds and is never shipped.

Tables in `index.db`:

- `symbols`: name, kind, file, line, description, content hash
- `edges`: src, dst, kind (call, instantiate, reference, extends, implements, overrides), call-site line; imports are not usages (POC plan, Phase 6)
- `symbol_vectors`: embeddings of the fqn and description text, not of the code
- `tickets`: one cached summary per ticket ID
- `index_meta`: the settings the index was built with (embedding model, dimension, embedded-text options), so search can refuse a query model that does not match

Vector search finds candidate symbols; plain joins and recursive CTEs over `edges` answer usage and blast-radius questions. No separate graph database is needed.

**Central build** (CI or a server, on every merge to main):

- The LLM summaries are the expensive step; generating them once per commit saves the cost on every machine.
- Only the central indexer holds credentials for the ticket system.
- Everyone queries the same descriptions, so answers are consistent.
- Output: `index-<commit>.db` published as an artifact.

**Local use** (the CLI next to the agent):

- Downloads the index for the nearest main commit.
- Re-extracts changed files on the current branch with tree-sitter, so structure and edges always match the working copy.
- Marks descriptions on changed symbols as `stale`, or regenerates it with a small local model; new symbols get a structure-only entry until the next central build.

A separate hosted service only becomes worthwhile when several teams or many repositories need cross-repo search and access control.

## Scope and non-goals

**In scope:** structure, high-level purpose, the ticket-level "why" and usages for classes, methods, functions and types, served through the `annatar` CLI.

**Non-goals:**

- Explaining how a function works internally. Annatar describes purpose and role; the agent or engineer reads the code for details.
- Replacing a language server or IDE navigation. SCIP is used offline to resolve references, not as a live server.
- Being a general documentation or wiki platform.

**Later, built on the same data:**

- Hotspots: centrality (in-degree, PageRank) combined with recent churn and bug tickets per symbol.
- Temporal coupling: symbols that change together without an edge between them, pointing to hidden dependencies.
- Risk hints in `annatar show`, so an agent knows when it is about to edit a high-risk symbol.

Analysis starts from bugs and churn and then investigates the code structure there, rather than classifying design patterns up front.

## Risks and open questions

- **Ticket coverage.** The "why" is only as good as the links from commits to tickets. Where commits carry no ticket ID, fall back to the commit message and PR description. Open: what share of commits in the target repos reference a ticket?
- **History extraction cost.** `git log -L` per symbol is slow on large repos with long history. Cache by symbol hash and last touching commit; only reprocess files changed since the last run.
- **Misleading history.** Refactors and reformatting commits hide the original change. Use `.git-blame-ignore-revs` and collect every ticket that touched a symbol, not just the last.
- **Summary quality and drift.** LLM summaries can be vague or wrong, and go stale as code changes. Hash gates regenerate changed symbols and their parents; stale entries are marked as such.
- **Ticket confidentiality.** Ticket text sent to a cloud LLM leaves the network. A local model is enough for ticket summaries. Open: which model runs which step?
- **Language coverage.** SCIP indexers exist for most mainstream languages; others fall back to tree-sitter with less precise usages.

## Roadmap and success measures

The intent index and its CLI come first; analysis follows on the data already collected.

1. **MVP on one real repo:** symbols and edges, per-symbol history and tickets, ticket summaries, symbol descriptions, embeddings, the four CLI commands.
2. **Central build and local use:** CI job that publishes the index per commit, local overlay for branches.
3. **Human access:** a search page for non-technical roles.
4. **Analysis:** hotspots, temporal coupling, risk hints in `annatar show`.

**How to tell it works:**

- **Agent efficiency:** on tasks where the relevant code is known in advance, compare tokens and tool calls with and without Annatar. A good sign is an agent going from `annatar search` straight to the right file without grepping around.
- **Agent quality:** fewer changes that break callers or contradict the original requirement.
- **Summary accuracy:** hand-check a sample of symbol descriptions against the code and tickets before relying on them.
- **Onboarding:** new engineers find the right area and its reasoning without asking a colleague first.
- **Self-service:** non-technical roles answer "what does this part do" without pulling in an engineer.
