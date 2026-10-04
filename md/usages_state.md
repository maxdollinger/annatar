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
| Phase | 6 — Usages — **in progress** (2026-10-04; 6.1–6.3 done): `edges`, `used by`/`uses` in `show`, `annatar trace`, from tree-sitter without a build (D-cu, confirmed) |
| Step | 6.3 Member resolution — **done** (reviewed, findings fixed); next: `refactor(usages)` module split, then 6.4 |
| Last updated | 2026-10-04 (6.3) |
| Baseline | `argus` index and warm cache in `.annatar-local/argus/` (rerun with `--offline`: 0 chat calls); 5.5 T4 *with* runs for 6.7 in `.annatar-local/agent-trial/main-55/` |

### Done

- **6.3 Member resolution.** `call` edges to methods and constructors and
  `instantiate` edges to the constructor `new T(..)` / `T::new` chooses,
  in `src/usages.rs` on the same walk as 6.2. `EdgeKind::Call`,
  `Edge::ambiguous` (written to `edges.ambiguous`). The walk now types
  expressions (`FileWalk::expr` → `Ty`: an indexed type with array
  dimensions, a library/primitive simple name, `null`, a Lombok builder,
  unknown): literals, `new T(..)`, casts, `this`, parenthesized and array
  access, names (block-scoped typed locals and parameters — method, lambda,
  catch, `for`, resources, `instanceof` and type patterns — then fields of
  the classes around, inherited from indexed super types, then `@Slf4j`'s
  `log`, then static-imported constants), field access (a field of a type
  not around the node is a `reference` to the type declaring it) and calls
  (the declared return type, resolved in the declaring type's file; a type
  variable is unknown). Receivers per `usages.md`: none → the innermost
  class around with a member of that name (anonymous/local classes by their
  indexed super types), then static imports (single, then wildcard);
  `this`, `super`, `X.super`, a variable, a type (`T.m()`), a chain. Lookup
  in the type and its indexed super types, a method overridden nearer
  dropped (type variables of the farther one match any type). Overloads:
  count (varargs ≥ n − 1), then argument fit and most exact matches,
  fixed arity before varargs; several left → an edge to each, ambiguous.
  `super(..)` / `this(..)` → `call` to the constructor (an undeclared
  superclass constructor → `reference` to the superclass). Method
  references `this::m`, `x::m`, `T::m`, `super::m` → `call` to every method
  of that name (ambiguous when several), `T::new` → `instantiate` to the
  constructors. Implicit members → `reference` to their type, the chain
  continuing: Lombok `@Getter`/`@Setter`/`@Data`/`@Value` (type or field)
  accessors, `@Builder`/`@SuperBuilder` `builder()`/`toBuilder()`, any
  builder method, `build()` → the type; record accessors; enum `values()`,
  `valueOf(..)`, `name()`, `ordinal()`; an undeclared constructor (only the
  type's `instantiate` edge). `outer.new Inner()` now resolves `Inner` as a
  member type of `outer`'s type (6.2 counted it unresolved). Stops (no
  edge, counted): library receivers, unknown receivers (untyped lambda
  parameters, type variables, chains that left the index), a member missing
  on an indexed type (inherited from a library or `Object`), an unqualified
  call in a class whose superclass chain leaves the index or whose
  anonymous/local class declares that name; recursion makes no edge.
  `Usages` gains `calls` (`CallSites`: resolved, implicit, ambiguous,
  unresolved) and `unresolved_calls` keyed `Receiver#name` (`?` unknown,
  `this`, `super`). `IndexStats`: `edges_call`, `calls_resolved`,
  `calls_implicit`, `calls_ambiguous`, `calls_unresolved`; `index` output:
  `call` in the `edges:` line and a new line `call sites: N (R resolved to
  a member, I implicit, A ambiguous, U unresolved)`; `-v` logs
  the 20 most frequent unresolved calls next to the type names. D-dd's
  flat variable set is replaced by the typed block-scoped locals (D-dg);
  `Region` keeps only type variables and local type names. Benchmark: each
  generated method calls its namesake on the next class (200 references +
  800 calls asserted). Tests (fixtures): the `argus`-shaped
  `Dispatcher#dispatch → Config#getAddEventConsumer() →
  AddEventConsumer#consume(AddMessage)` (plus `forEach(this::dispatch)`),
  receivers by local / parameter / field / `this.f` / `new` / cast / `var`
  / static call / chain / inherited method and block scoping, unqualified
  calls inner-before-outer and `Outer.this.m()`, `super.m()`,
  `super(..)`, `this(..)`, `super::m`, an implicit superclass constructor,
  chains cut by `Optional#map`, an untyped lambda parameter and a type
  variable (typed lambda parameter resolves) with the unresolved keys and
  counts, overloads by count / argument type / subtype / varargs / a JDK
  result, an ambiguous pair, constructors chosen and implicit, `T::new`,
  method references and single/wildcard static-imported methods, Lombok
  type/field accessors (`isX`), builder chain, record accessor, enum
  `values()`/`valueOf()`/`name()`, field access on another type, the stop
  at a library superclass and at an anonymous class's own method; indexer:
  `call` rows with `ambiguous`, line dedup and call-site counters; the six
  6.2 tests that now see calls updated (and `outer.new Inner()` renamed).
  **`argus`** (`index --offline`): 1138 edges (392 `call`, 31
  `instantiate` to constructors), call sites 1386 = 586 resolved, 0
  ambiguous, 800 unresolved; the T4 row
  `SecurityDataEventDispatcher#dispatch(..) -call->
  SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage) :76`
  exists (with the getter call on :75 and `receiveAndDispatchMessages →
  dispatch` :67 through `this::dispatch`); none of the 569 6.2 rows lost;
  `symbols` rows identical, 0 chat / 0 embedding calls, `cache.db` md5
  unchanged; 60 random call / constructor edges and 25 new `reference`
  edges hand-checked against their lines, all correct (Measurements).
  *Review:* 3 high, 4 medium, 7 low findings (reviewer: 90/90 fresh
  call edges and all 37 overload picks on `argus` correct; the findings
  are wrong edges in small probes that `argus` does not exercise). All
  fixed with a fixture test each, except L3 and the module split:
  H1 an indexed argument no longer fits a `String`/box/`UUID` (or
  primitive) parameter; H2 a single candidate is type-checked too — no
  candidate taking the arguments means a method from `Object` or a
  library superclass: no edge, counted unresolved (a declared
  constructor that takes no such arguments likewise, unless the type is a
  record or has a Lombok constructor annotation); H3 an unqualified
  `Object` method (and an `Enum` method in an enum) stops at the
  innermost class instead of reaching an enclosing one; M1 overloads of
  one type no longer drop each other as "overridden"; M2 overload choice
  follows Java's phases — fixed arity (a varargs method taking an array)
  before varargs — and ranks by exact matches only when every argument's
  type is known (D-di narrowed, D-dk); M3 one shared super-type walk,
  `Resolver::lineage` (the indexed superclass chain, then interfaces
  breadth-first), replaces the six copies (`member_type`, `has_field`,
  `field`, `methods`, `implicit`, `is_subtype`), so a superclass method
  wins over an interface's; the superclass is memoized with the super
  types (`supers`); M4 `CallSites::implicit` / `calls_implicit` split
  generated members and undeclared constructors off "resolved", which
  now means a `call`/`instantiate` edge to one indexed member (recursion
  included, without an edge); L1 a pattern variable lives in its `if` /
  `while` / `?:` condition and then-branch only; L2 an anonymous or
  local class's own and inherited fields come before the outer locals;
  L4 a 0-argument call on an indexed `@interface` (an annotation
  element) is an implicit `reference` to it; L5 a builder's
  `toString()`/`hashCode()`/`equals(..)` return their JDK type and a
  builder class written in the source (`T.TBuilder`) is consulted
  first; an unresolved `toString()` etc. on an indexed receiver keeps its
  JDK result type; primitive arguments fit `Number`/`Comparable`/
  `Serializable`; L6 the `dimensions` parsing is one helper (`dims`).
  Recorded: L3 bounded type variables → open #48; L6 the file split →
  Next; L7 docs (`usages.md` Members / Overloads / Implicit members /
  Where resolution stops / Coverage, `reference.md`, `plan.md` wording).
  **`argus` after the fixes:** 1142 edges (+4 `reference`:
  `messageConsume.type()` in the four message dispatchers, L4), none of
  the 1138 before lost, 392 `call` unchanged; call sites 1386 = 423
  resolved to a member, 167 implicit, 0 ambiguous, 796 unresolved; T4
  row :76 present; 11 ms; 0 chat calls, `cache.db` md5 unchanged.
- **6.2 Type resolution and the `edges` table.** New table `edges` in
  `INDEX_TABLES` (schema as `usages.md`: `UNIQUE(src_id, dst_id, kind,
  line)`, indexes on `dst_id` and `src_id`, `ambiguous` for 6.3). New
  module `src/usages.rs`: a `Resolver` over the run's type table (types
  with a row, keyed by fqn, with their 6.1 facts and file) resolves a
  simple type name in Java's order — nested in the current or an
  enclosing type (also inherited from an indexed super type, super types
  resolved lazily in their own file and memoized) → single-type import
  (an import of an external type stops the lookup; a static import of an
  indexed nested type counts) → same package → wildcard import (two
  indexed matches: no edge, an info log) — and a qualified name from its
  first part or as package-qualified, walking nested types down. A
  `FileWalk` over each file's tree emits `extends` / `implements` from a
  named type's header, `instantiate` for `new T(..)` (also anonymous) and
  `T::new`, and `reference` for every other `type_identifier` /
  `scoped_type_identifier` (fields, parameters incl. varargs, returns,
  locals, casts, `instanceof`, `catch`, `throws`, generic arguments,
  `new T[n]`, `T.class`, type bounds), annotation names and arguments,
  type qualifiers in expressions (`T.X`, `T.m()`, `T::m`, `Outer.this`,
  `Outer.Inner.X`, package-qualified) and constants from static imports
  (single or wildcard, to an indexed type with that static field or enum
  constant). The source is the innermost member symbol, else the type
  (header, annotations, fields, initializers, enum constants); symbols are
  matched to tree nodes by byte span, so a duplicate fqn's copy is skipped
  whole. Not types: `var`, type variables (of enclosing types, from the
  facts, and anything declared in the member), local classes; a variable
  or field of the same name (region scan; own, inherited indexed and
  enclosing types' fields) shadows a type in a qualifier. Self edges (src
  = dst) are dropped. Indexer: `ParsedFile` keeps the tree
  (`tree: Option<Tree>`), `IndexedFile` keeps package, imports, type facts
  and tree; `index_edges` runs right after `index_structure`, writes
  `INSERT OR IGNORE` and drops the trees. `IndexStats`: `edges_extends`,
  `edges_implements`, `edges_instantiate`, `edges_reference`
  (`edges()` sums), `unresolved_types` (mentions), `unresolved_type_names`
  (distinct), `edges_time`; new `index` output line `edges: N (… extends,
  … implements, … instantiate, … reference); U unresolved type names (D
  distinct); T ms`; the 20 most frequent unresolved names logged at `-v`;
  the `--path` warning adds "usages from outside it are missing". The
  benchmark's classes each hold a field of the next one and it prints the
  stage's time per run (asserting 200 edges). Tests (resolver, fixture
  sources): header kinds (generic and package-qualified super types, enum
  and record `implements`, interface `extends`), each reference construct
  and its source member / type, lambda / anonymous / local class / enum
  constant body attribution, nested types inside-out, inherited nested
  types and qualified names, `MessageBearer` in three packages chosen by
  same package / single import / wildcard-vs-same-package / nested over
  import, wildcard last and two wildcards ambiguous, external and
  unresolved names counted (an imported external shadows a same-package
  type), type variables, local classes and variables not types, imports,
  Javadoc and self references not usages, static-import constants (and a
  parameter shadowing one, a static-imported method not a type edge),
  inherited fields shadowing a type; indexer: rows, kinds, line dedup and
  counters, duplicate fqn (edges to the first definition, none from the
  dropped copy), the `--path` cut. **`argus`** (`index --offline`): 567
  edges, 0 chat / 0 embedding calls, `symbols` rows identical to the index
  before, `cache.db` byte-identical; spot checks below (Measurements).
  *Review:* 8 findings, all handled (D-df). Fixed, each with a fixture
  test: a type's header and class annotations now resolve around the
  type (its own nested types were in scope there, a wrong `extends` /
  `reference` when a nested type shadows the parent's name); a
  static-imported static field in a `case` label is a `reference` (an
  enum constant there is not: in an enum switch it is the selector's
  constant) — the two missing T4 dispatcher rows; the nested types
  (and fields) of an anonymous or local class's indexed super types are
  in scope in its body (a scope stack, `FileWalk::unnamed`, that 6.3
  reuses for members); `outer.new Inner()` makes no type edge (counted
  unresolved); a static-imported constant as a qualifier (`MAX.length()`)
  is a `reference` to its owner instead of an unresolved type name.
  Docs: a duplicated line and a blank line that split the Decisions
  table here; the `edges:` line now says `unresolved type mentions (N
  names)`; the outer type of a qualified name gets no edge (D-dc,
  `usages.md`); `EdgeKind::ALL` dropped; `IndexedFile` drops package,
  imports and type facts with the trees. **`argus` after the fixes:**
  569 edges (+2: `SecurityDataEventDispatcher#dispatch(..)` →
  `SecurityDataMessageBearer` on the `case` lines 75 and 77), none of
  the 567 before lost, unresolved unchanged, 7 ms, 0 chat calls,
  `cache.db` md5 unchanged.
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

- **`refactor(usages)`: split `src/usages.rs`** (≈ 4300 lines, ≈ 2600
  logic) before 6.4 adds `overrides`, as its own commit with no behaviour
  change (review L6): `src/usages/mod.rs` (API: `Edge`, `EdgeKind`,
  `Usages`, `CallSites`, `resolve`), `resolver.rs` (type table, name
  lookup, `lineage`, fields), `members.rs` (methods, implicit members,
  overloads: `choose`/`score`/`fits`/`overrides`/JDK table), `walk.rs`
  (`FileWalk`), tests with their module; `argus` edges identical.
- **6.4 Overrides and measured quality** (`plan.md` Phase 6): `overrides`
  edges (a method → the method with the same name and parameter count,
  then simple parameter types, in an indexed superclass or interface,
  through generic interfaces); `Resolver::methods` already drops a method
  overridden nearer with that rule (`overrides()` in `src/usages.rs`, a
  type variable of the farther one matching any type) — reuse it. The
  call-site counts are in the `index` summary since 6.3 (the plan put them
  in 6.4); 6.4 adds the usage golden set and precision / recall, and
  classifies the misses: the unresolved keys (`-v`) already separate
  library receivers from `?` (lambda, generics, library chain) — open #48
  (type-argument substitution, bounded type variables) is the generics
  bucket's candidate fix. The "share of resolved call sites" is
  `resolved to a member` (implicit ones apart, D-dk); `Resolver::lineage`
  gives the super types in lookup order.

## Step log

| Step | Status | Notes |
| --- | --- | --- |
| Phase 6 plan | done | docs only: `usages.md` (design), `plan.md` Phase 6 (steps 6.1–6.7), this file; SCIP and callers in prompts in Later, `project.md` edge kinds; D-cu (confirmed by the product owner)–D-cx; open #45–#47, #42 updated in `state.md` |
| 6.1 Parser facts for resolution | done | `ParsedFile` gains `package`, `imports`, `types` (`TypeFacts`: super types, fields, Lombok, methods with return/parameter types and varargs, enum constants, record components); `src/symbols/facts.rs`; symbols unchanged (`argus` reindex identical); D-cy–D-da; review: 9 findings, 5 fixed (record varargs component, compact constructor params, annotated superclass, 2 doc nits), 4 recorded as limitations/notes (outer generic args, Lombok arguments, tree for 6.2, arity from `params`) |
| 6.2 Type resolution and `edges` | done | `edges` table; `src/usages.rs` resolver (nested/inherited → single import → same package → wildcard, qualified names, static-import constants, variable shadowing); `index_edges` after structure on the kept trees; stats, `edges:` output line, `-v` top unresolved names, `--path` warning, benchmark time; `argus`: 569 edges, 2048 unresolved mentions (214 names, all external), 7 ms, 0 chat calls; D-db–D-df; review: 8 findings — fixed: header/annotations resolve around the type, static-field `case` labels (+2 T4 dispatcher rows), anonymous/local class super-type scope, `outer.new Inner()` skipped, static-import qualifier, 2 doc breakages, output label, nits (`EdgeKind::ALL`, facts dropped after the stage); recorded: library super types' nested types, outer type of a qualified name (D-dc), enum switch caveat (D-df) |
| 6.3 Member resolution | done | `call` edges and `instantiate` to constructors; typed expressions and block-scoped typed locals (D-dd replaced, D-dg); receivers, chains through declared return types, unqualified lookup inside-out then static imports, `super`/`this` calls, method references, implicit Lombok/record/enum/constructor members (D-dh), overloads with `ambiguous` (D-di), stops (D-dj); call-site counts and `-v` top unresolved calls; benchmark calls; `argus`: 392 `call` + 31 constructor `instantiate`, 586 / 0 / 800 call sites, T4 row :76 present, 0 chat calls; open #48; review: 14 findings — fixed: indexed argument vs final JDK parameter (H1), single candidate type-checked (H2), unqualified `Object`/`Enum` methods stop at the innermost class (H3), same-type overloads (M1), Java's arity phases and exact ranking only on known types (M2), one `lineage` walk superclass-first (M3), `implicit` call-site count (M4), pattern-variable and anonymous-field scoping (L1, L2), annotation elements (L4), builder edge cases (L5), `dims` helper; recorded: bounded type variables (L3 → #48), module split (L6 → Next); D-dk; `argus` after: 1142 edges (+4 annotation-element references), call sites 423 / 167 implicit / 0 / 796 |
| `refactor(usages)` module split | next | review L6, before 6.4 |
| 6.4 Overrides and measured quality | planned | |
| 6.5 `show`: `used by` / `uses` | planned | |
| 6.6 `annatar trace` | planned | |
| 6.7 Agent trial (T4) on usages | planned | |

## Measurements

Filled by 6.2–6.7, so the numbers of the phase sit in one place.

| Measure | Step | Value |
| --- | --- | --- |
| Edges per kind on `argus` | 6.2 / 6.3 | 6.3 after the review: 1142 rows — 633 `reference` (+4: `messageConsume.type()`, an annotation element, in each of the four message dispatchers), the rest unchanged; none of the 1138 rows before lost. 6.3 before the review: 1138 rows — 36 `extends`, 11 `implements`, 70 `instantiate` (39 to types as in 6.2, 31 to constructors), 629 `reference` (+146: fields of other types read or written, Lombok accessors and builders, enum members, implicit superclass constructors), 392 `call` (149 between members of one type); none of the 569 6.2 rows lost. T4: `SecurityDataEventDispatcher#dispatch(SecurityDataMessageBearer<?>) -call-> SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage) :76` and `… -call-> SecurityDataEventDispatcherConfig#getSecurityDataAddEventConsumer() :75`, `receiveAndDispatchMessages() -call-> dispatch(..) :67` (`this::dispatch`), `processMessages() -call-> receiveAndDispatchMessages() :59`. Spot checks: 60 random `call` / constructor `instantiate` edges and 25 random new `reference` edges against their source lines, all correct (e.g. `removeAllTokens(…getUserIdList())` → the `List<UUID>` overload, `new TokenInvalidationEvent(userId.toString())` → the `String` constructor, `this(Collections.singletonList(userId))` → the `List<String>` one). 6.2: 569 rows — 36 `extends`, 11 `implements`, 39 `instantiate`, 483 `reference` (567 before the review fixes; the +2 are the T4 dispatcher's `case EVENT_MFA_METHOD_ADDED, …` labels, static-imported constants of `SecurityDataMessageBearer`, now `reference` edges from `SecurityDataEventDispatcher#dispatch(..)` on lines 75 and 77). Spot checks: `SecurityDataEventDispatcherConfig` → `…messageConsumer.SecurityDataAddEventConsumer` `reference` from the type (field, :24), its constructor (:28) and `getSecurityDataAddEventConsumer()` (:45), none from the dispatcher (6.3's chain); each of the four `MessageBearer`s is reached only from its own package's dispatcher (by import: `cronus.role`/`person`/`user`, `pactum`), e.g. `RoleRelationMessageDispatcher#dispatch(MessageBearer)` → `cronus.role.message.MessageBearer` :107; `Privilege.GreenlandAdmin.X` in an annotation → the nested `Privilege.GreenlandAdmin`; 18 random edges checked against their source lines, all correct |
| Unresolved type names / call sites | 6.2 / 6.3 | 6.2: 2048 mentions of 214 distinct names, none the simple name of an indexed type (all JDK / Spring / Lombok / Jackson / `com.haufe.*` libraries): `String` 341, `UUID` 227, `Override` 78, `List` 70, `Set` 67, `ResponseEntity` 61, `Autowired` 52, … |
| Resolved / unresolved / ambiguous call sites | 6.3 / 6.4 | 6.3 after the review (D-dk): 1386 call sites — 423 resolved to a member (31 %; a `call` or constructor `instantiate` edge, recursion included), 167 implicit (Lombok accessors and builder methods, enum members, annotation elements, undeclared constructors: a `reference` / `instantiate` to the type only), 0 ambiguous, 796 unresolved (the four annotation elements moved to implicit). Before the review: 1386 call sites (method calls, constructor calls, `super(..)`/`this(..)`, method references) — 586 resolved (42 %), 0 ambiguous, 800 unresolved: ≈ 580 on a library receiver (`Logger#info` 45, `Logger#error` 44, `LoggerFactory#getLogger` 17, `HashSet#<init>` 15, `ResponseEntity#ok` 15, …), 173 on an unknown receiver (`?#build` 35 — `ResponseEntity`/AWS builders —, `?#collect` 13, `?#map` 10: library chains, untyped lambda parameters, `Bearer<T>#getPayload()` returning `T`), 39 on an indexed type without the member (inherited from a library class or `Object`: `CacheableOrganizationUserRelation#getKey`, Spring Data `…Cache#findByKey`, `getClass()`), 8 unqualified / `super` (a library superclass). Before two fixes during the step: 31 ambiguous (a generic interface method not seen as overridden, a `String` parameter accepting any library type, JDK results unknown), then 14, then 0 |
| Usage golden set: precision / recall of direct users | 6.4 | – |
| Misses by cause (lambda, library chain, generics, other) | 6.4 | – |
| Edge stage time (benchmark, `argus`) | 6.2 / 6.3 | 6.3 after the review: `argus` 11 ms, benchmark ≈ 26–27 ms. 6.3: `argus` 10–11 ms (release); benchmark (200 files, 1000 edges, debug) ≈ 25 ms per run. 6.2: `argus` 7 ms (release, 162 files, `index --offline` 2.2 s wall in all); synthetic benchmark (200 files, debug) ≈ 12 ms per run |
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
| D-db (6.2) | The edges stage walks the trees of the structure stage's single parse: `ParsedFile` gains `tree: Option<Tree>`, `IndexedFile` keeps package, imports, type facts and tree, `index_edges` runs right after `index_structure` (before history) and drops the trees. Symbols are matched to tree nodes by byte span (`start_byte` and `end_byte`), so only written rows are sources, and a duplicate fqn's dropped copy (and everything in it) is skipped | Parse every file again in the stage (`usages.md`'s first plan); join by recomputed fqns | One parse cannot drift from the symbols and costs nothing extra (the tree is already built); byte spans are exact where recomputing fqns would duplicate `symbols.rs`'s rules. Memory: the trees, imports and type facts live only until the stage ends (7 ms on `argus`; 6.3 runs in the same stage) |
| D-dc (6.2) | Edge kinds at the edges of the definitions: an anonymous class `new T() { .. }` is `instantiate` of `T`; `new T[n]` is `reference` (no `T` is created); the header of a local or anonymous class is `reference` from the enclosing member (`extends`/`implements` stay type → type); a type qualifier (`T.X`, `T.m()`, `T::m`, `Outer.this`) is `reference` to `T` — 6.3 adds the `call` to the member next to it, as `new T()` gets both `instantiate` edges. A qualified name (`Outer.Inner`, `Outer.Inner::m`) is an edge to the type it ends on only, not to `Outer`; 6.5's roll-up of nested types covers `show Outer`. Self edges (src = dst, e.g. `class Node { Node next; }`) are dropped; a member naming its own type is kept (the roll-up leaves internal edges out at read time, `usages.md`) | Only the `call` for `T.m()`; `extends` from a member for a local class | The type side stays complete where 6.3 cannot resolve the member (Lombok builder, an overload it cannot choose); a type → type kind from a member would make `extends` mean two things |
| D-dd (6.2; variables replaced by D-dg in 6.3) | Names in expressions, as far as 6.2 needs them: the first name of a qualifier is a variable (no edge) when it is declared anywhere in the current region (member, field declaration, initializer, enum constant: locals, parameters, lambda, catch, for, resource and pattern variables, anonymous-class fields) or is a field, enum constant or record component of an enclosing type or its indexed super types; else a type (`resolve_prefix`: the longest prefix that is an indexed type), else a package-qualified name. Regions are flat name sets, not block scopes; type variables and local class names in a region are not types. Bare identifiers count only in expression positions (an allowlist of parents), for static-import constants | Block scopes now; uppercase-means-type | Java's own rule (variable before type, JLS 6.5.2) without a heuristic; a flat set can only hide an edge (a lowercase local named like a type is the only miss, and types are capitalized), never invent one (wrong edges from missing scopes are D-df's). 6.3 needs typed, block-scoped locals and replaces the scan |
| D-de (6.2) | Unresolved counting and the `--path` cut: a type-position name that does not resolve (JDK, library, unknown), or a qualifier that a single-type import names outside the index, counts by the name as written; an unresolved qualifier in an expression counts only when it starts with an uppercase letter (`Math.abs`, not `log.info` from `@Slf4j`). With `--path` the table holds only the prefix's types, so usages from outside are missing (warning), and a name whose true target is outside could fall through to a later rule (an inherited nested type of an outside superclass, a same-package type outside the prefix vs a wildcard) — only for a prefix that splits a package or hierarchy, accepted for a fast-iteration mode. Also not handled: the type in a record pattern (`case Point(..)`, an `identifier` in the grammar; `argus` has no records); a nested type inherited from a **library** super type (`class X extends LibBase { Builder b; }` with a same-package `Builder`) cannot be seen and may give a wrong edge to the same-package type | Count every unresolved identifier; resolve `--path` against a full symbol table (parse the whole repo) | Counting lowercase names would mix variables of external super types into the type-name count; `--path` exists for fast prompt iteration (ground rules), a full parse would defeat it |
| D-df (6.2 review) | Scopes as `javac` builds them, where a wrong edge would follow otherwise: a named type's header (`extends`, `implements`, `permits`, type bounds) and its annotations resolve around the type (its type variables in scope, its nested types not), its body and record components inside it — the rule `supertypes()` already used; the body of an anonymous class `new T() { .. }` and of a local class sees the nested types and fields of its **indexed** super types first (a stack `FileWalk::unnamed`, innermost last, searched before the enclosing named types); in `outer.new Inner()` the type is skipped (counted unresolved, 6.3 resolves it from the receiver); a static-imported constant is a variable before a type, also as a qualifier (`MAX.length()` → `reference` to its owner); in a `case` label only a static-imported static **field** counts — an enum constant there names the selector's constant whatever is imported. Residual: an enum switch whose constant shares its name with a static-imported static field of an indexed type would get a wrong edge (the selector's type is unknown in 6.2) | Leave unnamed bodies in the outer scope and record it (review option b: no edge when the super type has such a nested type); treat every `case` identifier as an enum constant | Precision over recall (D-cw): each fix removes a wrong edge or adds a right one, and the stack is what 6.3 needs anyway for members called inside anonymous classes (`m()` from the super type). The enum-switch residual needs a field and an enum constant of the same `UPPER_CASE` name in one file — none on `argus` — while the `String`-switch dispatcher is the T4 pattern itself |
| D-dg (6.3) | Names in expressions resolve against **typed, block-scoped locals**, replacing D-dd's flat per-region set: a frame per member, block, constructor body, `for`, `catch`, try-with-resources, switch block or rule and lambda; method, lambda (typed or not), catch, `for`, resource, `instanceof` and type-pattern variables declared where Java declares them (a pattern variable lives to the end of the enclosing block); an anonymous or local class's own fields are variables in its body. A local's type is its declared type; `var` takes its initializer's type whatever the initializer (`new T()`, a cast, a resolved call, a literal) — Java's own rule, wider than `usages.md`'s "new or cast". `Region` keeps only type variables and local type names. `this` inside an anonymous or local class stands for its indexed super types | Keep the flat set for shadowing and add a typed map beside it; `var` from `new`/cast only | One scope model for both jobs (shadowing a type, typing a receiver); a flat set would type `value.b()` in one block by a `value` of another. A `var`'s type is the static type of its initializer by definition, so taking a resolved call's return type is as precise as the chain itself |
| D-dh (6.3) | Implicit members are a `reference` to the type declaring them and carry the chain: Lombok accessors per `@Getter`/`@Setter`/`@Data`/`@Value` on the type or field (`isX` / field `isX` for a primitive `boolean`; setters return `void`); `@Builder`/`@SuperBuilder`: `builder()`/`toBuilder()` → the builder, **every** builder method → the builder (the field setters whatever `setterPrefix`, which D-da does not keep — `argus` uses `setterPrefix = "with"` —, `@Singular` adders), `build()` → the type; record accessors; enum `values()` (an array), `valueOf(..)`, `name()`, `ordinal()`. A constructor that is not declared (default, Lombok, canonical) is resolved to the type, whose `instantiate` edge is already there (no second edge); an undeclared superclass constructor in `super(..)` is a `reference` to the superclass. A call to an implicit member counts as *implicit*, apart from the resolved ones (D-dk). `@Slf4j`'s `log` is typed as the library `Logger` (its calls count as library calls, not as unknown receivers) | Builder methods only for field names (missed every `withX` on `argus`: 35 → `?#build`); implicit constructors unresolved | The builder's methods can only lead back to its type, so accepting any of them adds no wrong edge — only the owner's `reference`; a missing constructor in compiling code means a generated one, which belongs to the type |
| D-di (6.3; narrowed in the review, D-dk) | Overload choice, beyond count: an argument fits a parameter when the types are equal (exact), an indexed subtype, number for number or box, `boolean`/`Boolean`, a primitive for `Number`/`Comparable`/`Serializable`, `null` for a non-primitive, or either side unknown or a library type (it may be a super type); a `String`, box or `UUID` parameter (final JDK types) takes nothing else — no indexed type either —, a primitive no reference. Java's phases: the candidates of fixed arity (a varargs method taking an array) every argument fits; only when there is none, the varargs ones (at least n − 1 arguments). Of several, the most exact matches win, and only when every argument's type is known (an unknown or builder argument leaves all, ambiguous). Every candidate is type-checked, a single one too; none fitting is no edge (the method is one the index lacks: `Object`'s, a library superclass's). Argument types also come from a small table of JDK results (`toString()`, `hashCode()`, `equals(..)`, `String` factories, `Collections`/`List`/`Set`/`Map`/`Arrays` factories). A method overridden nearer is not a candidate (same name, parameter count and simple types, a type variable of the farther method or type matching any type). A method reference has no arity: every method of the name is a candidate. A chain continues after an ambiguous call only when the candidates' return types agree | Count only, the rest ambiguous; resolve the JDK's types fully | On `argus` the table and the override rule took ambiguous call sites from 31 to 0, every remaining pick checked by hand. "Most exact" stands in for Java's *most specific*: with every argument known, Java's pick has at least as many exact matches as any other applicable candidate, so a tie is left ambiguous rather than guessed; with an unknown argument the other arguments' matches prove nothing (D-cw). The fit itself over-approximates for library types (a library argument may fit a library parameter), so a pick can still rest on a fit Java would reject — none found on `argus` (review: 37 picks checked). A full JDK model is SCIP's job (Later) |
| D-dj (6.3) | Where member resolution stops for precision (no edge, counted): an unqualified `m()` in a class whose superclass chain leaves the index (`extends Thread`, an anonymous `new TimerTask() {..}`, an anonymous class of a library type) or in an anonymous / local class that declares `m` itself — Java would find `m` there, not in the enclosing class; an unqualified `Object` method (`toString`, `equals`, …), or `Enum` method in an enum (`compareTo`, …), that the innermost class does not declare (review H3); a member an indexed type lacks (inherited from a library class or `Object`), also when its overloads do not take the arguments (H2). Field access on a type that is not around the node (also a field inherited from an indexed superclass) is a `reference` to the declaring type; a `case` label makes no such edge. An edge's line is the line of the called name (a multi-line chain gives each call its line), of the type for `new T(..)`, of the method reference or `super(..)` itself. Call sites are method invocations, `new T(..)` (also library types: `HashSet#<init>`), `super(..)`/`this(..)` and method references except array constructors; unresolved ones are keyed `Receiver#name` with the receiver's simple type name, `?` when unknown, `this`/`super` for unqualified ones, `XBuilder` for a builder. The plan's 6.4 summary counts are already in 6.3's `index` output | Fall through to the enclosing class / static imports regardless | A wrong caller misleads more than a missing one (D-cw); the keys make 6.4's miss classification (library, lambda, generics) a matter of reading the `-v` line |
| D-dk (6.3 review) | The review's findings, handled as follows. **Call-site counts:** *resolved to a member* = a `call` (or constructor `instantiate`) edge to one indexed method or constructor, recursion included (resolved, the edge dropped per D-cv); *implicit* = a generated member (Lombok accessor or builder method, enum/record member, annotation element) or an undeclared constructor (only a `reference`, or the type's `instantiate`); *ambiguous*; *unresolved* — also a declared constructor that takes no such arguments, unless the type is a record or has a Lombok `@…ArgsConstructor` (then implicit). 6.4's "share of resolved call sites" is the first. **One super-type walk** (`Resolver::lineage`): the indexed superclass chain first, then interfaces breadth-first, each type once — `javac`'s order for methods (a class method beats an interface method, JLS 8.4.8); used for nested types, fields, methods, implicit members and subtyping; the superclass memoized with the super types. **Scopes:** a pattern variable lives in its `if`/`while`/`?:` condition and then-branch (or body) only; Java's flow scoping (`if (!(o instanceof Foo f)) return; f.go();`, the `else` of a negated test, `&&` outside a condition) is not followed — recall only. An anonymous or local class's own fields and its indexed super types' fields come before the locals of the code around it. **Implicit members:** a 0-argument call on an indexed `@interface` other than an `Object` method is an annotation element (a `reference` to the annotation type); a builder's `Object` methods return their JDK types, and a builder class written in the source (`T.TBuilder`) is looked up first (its methods are `call`s, returning it continues the builder). **Known limits, recorded:** a receiver typed by a bounded type variable (`<T extends Msg>`) stays unknown — the bound is not in the 6.1 facts (folded into open #48); a local enum's unqualified `Enum` methods are not stopped (only named enums are); numeric arguments fit any number type (no narrowing check — only a call Java rejects would differ) | Leave "resolved" mixed and document it; fix only the three high findings; split the module in the same commit | Each fix removes a wrong edge or a misleading number (D-cw) at no cost on `argus` (no `call` edge lost or changed, +4 correct references); the counts must mean edges before 6.4 reports them as quality; the module split is a refactor and gets its own commit (AGENTS: one step per change) |

## Open questions

| # | Question | Raised at | Status |
| --- | --- | --- | --- |
| 45 | Test sources are not indexed, so `used by` and `trace` never show test callers ("who relies on it" stops at main code). Index test files for edges only (as sources of usages, not as symbols or descriptions)? | 6 plan | open — decide after 6.5 |
| 46 | Show usages in the `search` file view (e.g. `used by N` or the callers' files under a hit) so an agent sees the caller without a second command? Costs output size on every query | 6 plan | open — after 6.7, depending on whether agents call `trace` / `show` on their own |
| 47 | Callers in the description prompts (plan Later): a member's callers could sharpen what and why, but every prompt changes and the whole LLM cache misses (≈ 20 min on `argus`) | 6 plan | open — after 6.7 |
| 48 | Substitute a super type's type arguments in inherited members (`SecurityMethodAddMessage extends SecurityDataMessageBearer<SecurityData>` → `getPayload()` returns `SecurityData`) and in receivers' generic types (`Box<Item>#get()`)? Today a type variable stops the chain (`usages.md`, where resolution stops): on `argus` every message consumer's `message.getPayload().getUserId()` is unresolved. Also a **bounded** type variable (`<T extends UserDataMessageBearer> void sendAsync(T message)` → `message.getMessageType()`): its erasure, the bound, is as precise as a declared type, but 6.1 keeps only the type variables' names (6.3 review L3) | 6.3 | open — 6.4 counts the generics misses; decide there |
