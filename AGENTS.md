# Annatar — Agent Instructions

Annatar is a Rust POC that builds an intent index of a Java/Spring repo (Jira
tickets + Ollama + libSQL) and serves it through its CLI (`search`, `show`).
The work is driven by the plan, not by ad-hoc requests.

## Start here

1. Read [`md/plan.md`](./md/plan.md) — the POC plan (authoritative for scope).
2. Read [`md/state.md`](./md/state.md) — current phase/step, decisions and open
   questions.
3. [`md/project.md`](./md/project.md) is the long-term target; `plan.md` is the
   POC subset. Don't build "Later" items unless asked.

## Workflow

1. **Pick the next step** from `md/plan.md` (usually the `Next` entry in
   `state.md`). Implement **exactly one step** — one step is one small PR.
   Do not start the next step in the same change.
2. **Implement** it following existing code conventions. Match the style of
   neighbouring modules; do not add comments unless asked.
3. **Test** it. Every step ships with tests. Unit tests use fixtures and never
   need network, Jira or Ollama. Tests that do are marked `#[ignore]`.
4. **Verify** before committing:
   - `cargo fmt`
   - `cargo clippy --all-targets -- -D warnings` (lints tests and the
     benchmark too)
   - `cargo test`
   All must be clean/green. If a command fails, fix it — don't skip it.
5. **Update [`md/state.md`](./md/state.md)** after the implementation:
   - the `Current state` table (phase, step, last updated),
   - `Done` with a short summary of what changed,
   - `Next` with the following step,
   - the `Step log` table,
   - `Decisions and tradeoffs` for anything that was a judgement call,
   - `Open questions` — add new ones, resolve closed ones.
6. **Commit** after each implementation. Use Conventional Commits matching the
   existing history (`feat(...)`, `refactor(...)`, `docs(...)`, `test(...)`).
   Only commit when the step is done and verified — no work-in-progress commits.

## After each step: check for drift before moving on

Before starting the next step, explicitly check:

- **Doc drift** — does `plan.md` (or `project.md`) still match what was built?
  If a step changed an assumption, update the docs in the same change.
- **Missing prerequisites** — does the next step need something the plan
  assumed exists but doesn't? Note it, or add it.
- **Open questions** — does the finished step resolve or invalidate one? Update
  `state.md`.
- **Schema** — new tables go in `src/schema.rs` (`INDEX_TABLES` vs
  `CACHE_TABLES`). No migrations: `index.db` is disposable.

## Ground rules (from the plan)

- **Two database files.** `index.db` is rebuilt from scratch each run (temp
  file + atomic rename); `cache.db` persists only expensive, content-keyed
  results. If a cache table's schema changes, drop the table.
- **Symbols are identified by fqn** everywhere outside a single run (caches,
  golden set, CLI). Row IDs change every rebuild.
- **Secrets only from the environment** (`ANNATAR_*`); never in
  `annatar.toml` and never committed.
- **`--path <prefix>`** limits a run to part of the repo for fast prompt
  iteration.

## Conventions

- Rust edition 2024, `anyhow` for errors, `tracing` for logs.
- Binary + library split (`src/lib.rs` + `src/main.rs`); unit tests live with
  their modules, integration tests in `tests/`.
- Prefer adding a config option to changing an existing default; document it in
  the `Decisions` table.
- Keep changes scoped: don't refactor unrelated code in the same step.
