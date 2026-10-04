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
| Phase | 6 — Usages — **in progress** (2026-10-04; 6.1 done): `edges`, `used by`/`uses` in `show`, `annatar trace`, from tree-sitter without a build (D-cu, confirmed) |
| Step | 6.1 Parser facts for resolution — **done** (in memory, no index change) |
| Last updated | 2026-10-04 (6.1) |
| Baseline | `argus` index and warm cache in `.annatar-local/argus/` (rerun with `--offline`: 0 chat calls); 5.5 T4 *with* runs for 6.7 in `.annatar-local/agent-trial/main-55/` |

### Done

- **6.1 Parser facts for resolution.** `JavaParser::parse` now also returns,
  on the `ParsedFile`, the `package`, the `imports` (path, `is_static`,
  `wildcard`) and one `TypeFacts` per named type (same fqn and order as its
  `Symbol`; `parent` links nested types): type variables, `superclass` and
  `interfaces` (an interface's `extends` list) as `TypeRef`s (name as
  written, generic arguments apart, wildcards, array dims, line), Lombok
  annotations, fields (one per declarator: name, declared type, `static`
  — interface constants included — Lombok annotations, line), methods and
  constructors (`MethodFacts`: fqn equal to the symbol's, declared return
  type, parameter names and types, `varargs`, `static`, type variables,
  line), enum constants and record components. New module
  `src/symbols/facts.rs`; `symbols.rs` collects symbols and facts in one
  walk (`Walk`), the `Symbol` records are unchanged. Nothing is stored:
  no schema, cache or prompt change. Tests: each import form, generic
  super types (nested arguments, wildcards, qualified names, interface
  `extends`, enum and record `implements`), fields with and without
  Lombok (Spring `@Value` vs Lombok, `lombok.*`, qualified `@lombok.X`,
  multi-declarator, legacy array), return/parameter types incl. varargs,
  generic and legacy-array methods, nested types and local/anonymous
  classes, facts one-to-one with the symbols, a broken file. **`argus`
  check:** all 217 `.java` files (main and test) parse, facts match the
  symbols one-to-one; 231 types (14 nested), 97 superclasses, 40
  interfaces, 535 fields, 729 methods/constructors (5 varargs), 1956
  imports (0 wildcard, 37 static), 63 types with Lombok annotations
  (`Getter` 26, `Setter` 26, `RequiredArgsConstructor` 25, `Builder` 23,
  `Slf4j` 22, `Jacksonized` 16, `NoArgsConstructor` 10,
  `AllArgsConstructor` 7, `Data` 5), none on fields. `index --offline`
  on `argus`: the `symbols` rows (fqn, parent, kind, role, file, lines,
  signature, Javadoc, annotations, content hash, description) are
  identical to the index before the change; 0 chat calls. *Review:* 9
  findings, 5 fixed, 4 recorded — a varargs record component is now an
  array (`String[]`, like its field and accessor); a compact
  constructor's facts take the record components as parameters (fqn
  still `R#<init>()`); `extends @Ann Base` keeps its superclass; a stale
  doc reference and the phase status fixed; tests for each, plus one
  pinning that a receiver parameter is in the fqn but not in `params`.
  Recorded, not changed: arguments of a qualifying outer type
  (`Outer<A>.Inner<B>`) are dropped (D-cz); Lombok annotation arguments
  and member-level `@Builder` are not kept (D-da); the tree is not on
  `ParsedFile` (6.2 note below); overload arity comes from `params`, not
  the fqn text (D-cy).
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

- **6.2 Type resolution and the `edges` table** (`plan.md` Phase 6):
  resolve the 6.1 `TypeRef`s and the type mentions in bodies to fqns
  (nested → single-type import → same package → wildcard import), write
  `extends`, `implements`, `reference` and `instantiate` (type side) edges
  in a new `edges` table in `INDEX_TABLES`; stats and `--path` cut.
  Prerequisite: `JavaParser::parse` drops the tree, so 6.2 must return it
  (or a body walk) from the same parse, or parse the file again, to walk
  method bodies and attribute mentions to the innermost member (join by
  symbol byte spans / fqn).
- **6.3 notes from the 6.1 review:** count arguments against
  `MethodFacts::params`, never the fqn text (receiver parameters and
  compact constructors differ); fields of local and anonymous classes are
  absent (D-cy), so `f.m()` on such a field stays unresolved — list it
  under where resolution stops.

## Step log

| Step | Status | Notes |
| --- | --- | --- |
| Phase 6 plan | done | docs only: `usages.md` (design), `plan.md` Phase 6 (steps 6.1–6.7), this file; SCIP and callers in prompts in Later, `project.md` edge kinds; D-cu (confirmed by the product owner)–D-cx; open #45–#47, #42 updated in `state.md` |
| 6.1 Parser facts for resolution | done | `ParsedFile` gains `package`, `imports`, `types` (`TypeFacts`: super types, fields, Lombok, methods with return/parameter types and varargs, enum constants, record components); `src/symbols/facts.rs`; symbols unchanged (`argus` reindex identical); D-cy–D-da; review: 9 findings, 5 fixed (record varargs component, compact constructor params, annotated superclass, 2 doc nits), 4 recorded as limitations/notes (outer generic args, Lombok arguments, tree for 6.2, arity from `params`) |
| 6.2 Type resolution and `edges` | next | |
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
| D-cy (6.1) | The facts are collected on the symbol walk (one `Walk` over the tree) and returned on `ParsedFile` as `package`, `imports` and `types`, one `TypeFacts` per named type in symbol order, with its members' `MethodFacts` keyed by the symbol's fqn; nested types are found through `parent`. Local and anonymous classes stay invisible (their fields and methods belong to nobody). Extras beyond the plan's list because 6.2/6.3 need them and they are free on the same walk: type variables of types and methods (a `T` is not a class), method `static`, enum constants (`T.X`), record components (accessors, canonical constructor), a line on every `TypeRef`, field and method | A second walk in a separate module; a flat list of facts keyed by fqn | One walk cannot drift from the symbol rules (the same containers, the same fqns, verified one-to-one on `argus`); the resolver joins facts to symbols by fqn, as everything outside a run does. Arity is `params`, not the fqn text: a receiver parameter is in the fqn (unchanged `render_params`) but not a parameter, and a compact constructor (fqn `R#<init>()`) has the record components as `params` |
| D-cz (6.1) | `TypeRef` = the name as written without generic arguments (`Foo`, `Outer.Inner`, `com.acme.Foo`, primitives, `void`), the arguments as nested `TypeRef`s, array dimensions (also `int x[]` and `int m()[]`), line; a wildcard is `?` with its bound (if any) as its one argument; type annotations are dropped. A varargs parameter's type is its element type, with `varargs` on the method; a varargs record component is an array (it is the field's and accessor's type). Known limitation: the arguments of a qualifying outer type are dropped (`Outer<A>.Inner<B>` is `Outer.Inner<B>`) — only a `reference` edge to `A` is lost, rare in Spring code. Nothing is resolved here | Raw text only; resolving while parsing | Resolution needs the whole run's symbol table (second pass, D-cv); keeping the arguments apart lets 6.2 emit `reference` edges for generic arguments (`List<T>`) and 6.4 match `MessageConsumer<T>#consume(T)`; element type plus flag is what overload choice by count (≥ n − 1) needs |
| D-da (6.1) | A Lombok annotation is recognised by Java's rules, not by name alone: `@lombok.X`, or `@X` with a single-type import from a `lombok` package, or a wildcard `lombok`/`lombok.*` import for the known Lombok names (`Getter`, `Setter`, `Data`, `Value`, `Builder`, `SuperBuilder`, `With`, the three constructor annotations, `ToString`, `EqualsAndHashCode`, `Singular`, `Slf4j`); a single-type import of another `Value` shadows the wildcard. All Lombok annotations are kept by simple name; which generate accessors is the resolver's call (6.3). Annotation arguments are not kept (`@Getter(AccessLevel.NONE)`, `@Accessors(fluent = true)`, `@Builder(setterPrefix = ..)`) and `@Builder` on a constructor or method is not seen — recall only (no wrong edge), revisit in 6.4 if misses show up | Match simple names (`Getter`, `Value`) anywhere | Spring's `@Value` on fields and Lombok's `@Value` on types share a name; a wildcard says nothing about which names its package has, so without the list `@Deprecated` would count. `argus` has no wildcard imports, so the list is a safety net only |

## Open questions

| # | Question | Raised at | Status |
| --- | --- | --- | --- |
| 45 | Test sources are not indexed, so `used by` and `trace` never show test callers ("who relies on it" stops at main code). Index test files for edges only (as sources of usages, not as symbols or descriptions)? | 6 plan | open — decide after 6.5 |
| 46 | Show usages in the `search` file view (e.g. `used by N` or the callers' files under a hit) so an agent sees the caller without a second command? Costs output size on every query | 6 plan | open — after 6.7, depending on whether agents call `trace` / `show` on their own |
| 47 | Callers in the description prompts (plan Later): a member's callers could sharpen what and why, but every prompt changes and the whole LLM cache misses (≈ 20 min on `argus`) | 6 plan | open — after 6.7 |
