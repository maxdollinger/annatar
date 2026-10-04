# Annatar — Usages State

Living document for Phase 6 (usages). Updated after every Phase 6 step,
like [`state.md`](./state.md) for the phases before it. Tracks where the
phase stands, the decisions taken and the questions it opened.

- **Design:** [`usages.md`](./usages.md)
- **Steps:** [`plan.md`](./plan.md), Phase 6 (authoritative for scope)
- **Project state:** [`state.md`](./state.md) — Phases 0–5, the POC verdicts,
  open #42 (the reason for this phase) and every decision and question
  before Phase 6

Decision and question numbers continue the series of `state.md` (D-cu,
#45 onward) and are allocated here while Phase 6 runs, so an ID stays
unique across both files.

## Current state

| | |
| --- | --- |
| Phase | 6 — Usages — **planned** (2026-10-04): `edges`, `used by`/`uses` in `show`, `annatar trace`, from tree-sitter without a build (D-cu, confirmed) |
| Step | Phase 6 plan — **done** (docs only) |
| Last updated | 2026-10-04 |
| Baseline | `argus` index and warm cache in `.annatar-local/argus/` (rerun with `--offline`: 0 chat calls); 5.5 T4 *with* runs for 6.7 in `.annatar-local/agent-trial/main-55/` |

### Done

- **Phase 6 plan (usages).** Docs only (`docs(plan)`). New design doc
  `md/usages.md` and this state file; `plan.md` gains Phase 6 (steps,
  linking to it) after the POC, aimed at open #42: what counts as a usage (edge
  kinds `extends`, `implements`, `overrides`, `instantiate`, `call`,
  `reference`; the innermost member as source; imports, Javadoc links,
  recursion and unresolved names are not usages; a type's `used by` rolls
  up its members, without its own internal edges), how a name resolves
  (Java's lookup order to an fqn, never by bare name; members through the
  receiver's static type and declared return types; Lombok, record and
  enum implicit members; overloads by count then argument types, else
  every candidate marked `ambiguous`), what stays invisible (reflection,
  YAML wiring, events and brokers, derived queries, tests) and entry
  points as `trace` leaves; steps 6.1 parser facts, 6.2 type resolution and
  `edges`, 6.3 member resolution, 6.4 overrides and a measured quality
  (usage golden set, precision / recall, resolved share), 6.5 `show`,
  6.6 `trace`, 6.7 the T4 trial re-run. **Later:** SCIP becomes the
  precision upgrade of the same table; callers in prompts and
  multi-module resolution are their own items. `project.md`: the `edges`
  kinds (no `import`). *Review of the first draft* added: fields and
  return types are not in the index yet (6.1 parses them, in memory),
  field and constant access and Lombok accessors as `reference` to the
  owner type, own-type calls kept for members (private helpers in a
  chain) but left out of a type's roll-up, recursion dropped, lambdas and
  anonymous classes attributed to the enclosing member, `throws`/`catch`/
  annotation arguments/generic arguments, chains through library types
  and untyped lambda parameters stop, duplicate fqns, the `--path` cut and
  its warning, stats and benchmark, a golden set checked against the index,
  the README agent snippet and a `run.py` prompt variant. `argus` facts
  used: one Gradle module, no wildcard imports, no records, 4
  `MessageBearer` / 3 `MessageConsumer` simple-name duplicates in
  different packages, 35 lambdas, 15 `.stream()` chains, 0 Spring events.

### Next

- **6.1 Parser facts for resolution** (`plan.md` Phase 6): imports, super
  types, fields with Lombok annotations, return and parameter types on the
  parsed file; no index change.

## Step log

| Step | Status | Notes |
| --- | --- | --- |
| Phase 6 plan | done | docs only: `usages.md` (design), `plan.md` Phase 6 (steps 6.1–6.7), this file; SCIP and callers in prompts in Later, `project.md` edge kinds; D-cu (confirmed by the product owner)–D-cx; open #45–#47, #42 updated in `state.md` |
| 6.1 Parser facts for resolution | next | |
| 6.2 Type resolution and `edges` | planned | |
| 6.3 Member resolution | planned | |
| 6.4 Overrides and measured quality | planned | |
| 6.5 `show`: `used by` / `uses` | planned | |
| 6.6 `annatar trace` | planned | |
| 6.7 Agent trial (T4) on usages | planned | |

## Measurements

Filled by 6.2–6.7, so the numbers of the phase sit in one place.

| Measure | Step | Value |
| --- | --- | --- |
| Edges per kind on `argus` | 6.2 / 6.3 | – |
| Unresolved type names / call sites | 6.2 / 6.3 | – |
| Resolved / unresolved / ambiguous call sites | 6.4 | – |
| Usage golden set: precision / recall of direct users | 6.4 | – |
| Misses by cause (lambda, library chain, generics, other) | 6.4 | – |
| Edge stage time (benchmark, `argus`) | 6.2 / 6.3 | – |
| T4: dispatcher named / full marks / cost vs 5.5 | 6.7 | – |

## Decisions and tradeoffs

| # | Decision | Alternatives | Rationale / consequence |
| --- | --- | --- | --- |
| D-cu (6 plan) | **Confirmed by the product owner (2026-10-04): "defer SCIP for later".** Usages come from a resolver over the tree-sitter trees the index already parses, applying Java's name rules against the run's symbol table; SCIP (scip-java) is a later precision upgrade writing the same `edges` table | SCIP first (the old Later item); bare-name matching | SCIP needs the repository to compile on the indexer (no Gradle cache for `argus` here, Maven Central behind the proxy allowlist, a build per repository centrally); `argus` has no wildcard imports and its Spring calls are mostly on declared field types; `project.md` already plans tree-sitter as the fallback. Cost: generics, lambda parameters and chains through library types stay unresolved — measured in 6.4, the trigger for SCIP |
| D-cv (6 plan) | A usage is a resolved mention of an indexed symbol: kinds `extends`, `implements`, `overrides`, `instantiate`, `call`, `reference`; source = innermost method/constructor (lambdas, anonymous and local classes belong to it; fields, initializers, header and class annotations to the type); not imports, Javadoc links, recursion or external symbols; field access and Lombok accessors are a `reference` to the owner type; a type's `used by` = its own and its members' incoming edges without its internal ones, computed when read | Imports as edges (`project.md` had `import`); type-only edges; storing roll-ups | An import can be unused and says nothing a resolved reference does not; type-only edges miss the T4 dispatcher, which reaches the consumer only through `config.getX().consume(..)`; internal calls are needed for `trace` through private helpers but would drown a type's external users |
| D-cw (6 plan) | Resolve to an fqn or make no edge: precision over recall. Overloads by argument count, then known argument types; still several → an edge to each, `ambiguous = 1` | Guess by simple name; pick the first overload | A wrong caller misleads an agent more than a missing one, and missing edges are measurable (6.4 recall, unresolved share); `ambiguous` keeps the overload case visible instead of hiding it |
| D-cx (6 plan) | Same fqn in two modules is out of scope for Phase 6: edges point at the kept first definition (the existing duplicate-fqn rule); multi-module resolution (module per file, visible modules only) is a prerequisite before indexing a multi-module repository | Module-aware resolution now | `argus` is one Gradle module without duplicate fqns; names alone cannot separate two modules' `com.acme.X` — that needs the build files, which belongs to the Later multi-module item |

## Open questions

| # | Question | Raised at | Status |
| --- | --- | --- | --- |
| 45 | Test sources are not indexed, so `used by` and `trace` never show test callers ("who relies on it" stops at main code). Index test files for edges only (as sources of usages, not as symbols or descriptions)? | 6 plan | open — decide after 6.5 |
| 46 | Show usages in the `search` file view (e.g. `used by N` or the callers' files under a hit) so an agent sees the caller without a second command? Costs output size on every query | 6 plan | open — after 6.7, depending on whether agents call `trace` / `show` on their own |
| 47 | Callers in the description prompts (plan Later): a member's callers could sharpen what and why, but every prompt changes and the whole LLM cache misses (≈ 20 min on `argus`) | 6 plan | open — after 6.7 |
