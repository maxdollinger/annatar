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
| Phase | 6 — Usages — **done** (2026-10-04; 6.1–6.7 and the 6.3a module split done): `edges`, `used by`/`uses` in `show`, `annatar trace`, from tree-sitter without a build (D-cu, confirmed) |
| Step | 6.7 agent trial (T4) on usages — **done** (graded blind; reviewed, findings fixed): dispatcher 5/5, full marks 5/5 (4/5 under a strict reading), no measurable cost increase vs 5.5 (mean +7 %, median −8 %, p = 1.0); #42 closed. Phase 6 complete; next: open items / `plan.md` Later with the product owner |
| Last updated | 2026-10-04 (6.7) |
| Baseline | `argus` index and warm cache in `.annatar-local/argus/` (rerun with `--offline`: 0 chat calls); 5.5 T4 *with* runs for 6.7 in `.annatar-local/agent-trial/main-55/` |

### Done

- **6.7 Agent trial (T4) on usages.** `scripts/agent_trial/run.py`
  `--with-prompt usages` (D-ed): the `files` prompt with one bullet
  inserted before its last line — `show` also lists `used by` (callers,
  grouped by file with lines, `via` for calls through an interface) and
  `uses`; `annatar trace '<fqn>'` prints the callers transitively up to the
  entry points such as `[entry: @Scheduled]` (`--depth N`, default 6);
  reflection, events, message queues, configuration and test code are not
  seen — 1495 vs 974 characters, the other lines byte-identical; the
  stream parser counts `annatar trace` apart (`annatar_trace` column in
  `runs.csv` and the summary) and knows `--depth` as a value option.
  README agent snippet: `show` names `used by` / what it uses, a `trace`
  line; POC findings line on call chains updated. *Tests:* 15 Python unit
  tests (+2: the usages prompt is `files` plus one bullet naming `used
  by`, `trace` and the default depth; `trace` and `--depth` in Bash
  commands), Rust unchanged (fmt, clippy, 392 + 14 CLI tests green).
  *Protocol:* the 6.7 amendment in the private `protocol.md`, fixed
  before any run (D-cs otherwise unchanged: Claude Code 2.1.289,
  `claude-sonnet-5` checked per run, effort medium, tools, limits, task
  prompts, answer key, rubric and the 5.4-regrade strictness, the read-only
  `argus` checkout at 8f4664a — `git status` unchanged after every run,
  auto-memory empty); changed: the wrapper runs a fresh release build of
  HEAD dea3461, the index is `data-real` with its Phase 6 `edges` (not
  rebuilt; `trace` checked through the wrapper:
  `BusinessFeatureMessageConsumer#perform` → `SubscriptionMessageDispatcher
  #performMessage` → `#dispatch` → `#processMessages() [entry:
  @Scheduled]`), the prompt `usages`. Only the *with* arm, T4 × 5
  (`.annatar-local/agent-trial/main-67/`), then T1–T3 once each as a
  regression check. Baseline: the 5 5.5 T4 runs (`main-55/`), 5.4 runs
  (`main/`) for reference. *Grading:* blind by a separate grader subagent:
  the 8 new answers among 30 old ones (all 15 5.5 T1–T3, the 5 5.5 T4, the
  10 5.4 T4 — re-shuffled, new codes, `annatar` → `[index]` and
  `[index] show|trace|search` → `[index]`, no arm labels; the bundle
  holds the 5.5 T1–T3 answers beyond the protocol's T4 answers, to blind
  the 3 new regression answers; bundle, grades and mapping in
  `.annatar-local/agent-trial/blind-67*`). It gave all 30 old answers
  their published grades (30 / 30 agree, dispatcher and fresh read flags
  included). Two new T4 answers it called borderline on the fresh read
  (`getAuthData` named "separately" / "e.g."), both scored 2; spot-checked,
  kept. A third borderline, on the location, it did not flag: T4-with-5
  names only `SubscriptionMessageDispatcher#performMessage(MessageBearer)`,
  not the scheduled `processMessages` poll of the key's MUST location —
  kept at 2 as graded (the protocol scores new answers from the grading),
  1 under a strict reading. *Results* (T4, mean of 5; 5.5 *files* in
  brackets; exact two-sided Mann-Whitney 5 vs 5 with mid-ranks, Fisher
  exact; pre-registered: tokens, cost, tool calls, dispatcher, full
  marks — the rest exploratory): **dispatcher named 5/5** (3/5, p = 0.44;
  5.4 symbols 1/5, p = 0.048; without 5/5), **full marks 5/5** (2/5, p =
  0.17; symbols 0/5, p = 0.008; without 5/5; strict reading 4/5: p = 0.52
  vs 5.5, p = 0.048 vs symbols), fresh read 5/5 (3/5); tokens 247 k
  (275 k, −10 %, p = 0.42), **cost $0.214 (0.200): no measurable cost
  increase (mean +7 %, median −8 %, p = 1.0; cache creation 35 k vs 29 k,
  +21 %, from the larger `show` output)**, tool calls 13.2 (14.8, p =
  0.41). Exploratory: Reads 3.6 (6.2, p = 0.21), `annatar search` 3.4
  (3.6), **`show` 6.4 (3.4, p = 0.12), `trace` 0 in every run**, wall
  33 s (37 s); against 5.4 *without*: −20 % tokens (p = 0.22), +13 % cost
  (p = 1.0), −36 % tool calls (p = 0.008), same score — on T4 `annatar`
  still saves tool calls, not money. How the dispatcher first appeared
  (first tool result naming it): in 3 of 5 runs from the consumer's
  `used by` in `show` (runs 1, 3, 5: the class or `perform` —
  `SubscriptionMessageDispatcher#performMessage(MessageBearer) via
  …MessageConsumer#perform(T) :122`), the intended mechanism; in 1 (run 2)
  from an incidental `reference` in the `used by` of the class
  `AuthDataEventService`, which lists five dispatcher methods
  (`processMessages() reference :98, 103 [entry: @Scheduled]` among them)
  because they log `AuthDataEventService.LOG_PREFIX` — a real edge that
  says nothing about the dispatcher calling the consumer; that agent never
  read the dispatcher nor `show`ed the consumer and inferred the dispatch;
  in 1 (run 4) from a search result (`"pactum message listener
  consume"`). `processMessages` came from reading the dispatcher (runs 1,
  3), the `AuthDataEventService` `used by` (2), the search outline (4); run
  5 does not name it. No agent ran `show` on the dispatcher or called
  `trace`: one `used by` hop reached the dispatcher's file, the rest was in
  it (→ open #53). T1–T3 (1 run each, regression check): all score 2 (T2
  / T3 reason 1/1); T1 72.8 k tokens / $0.065 (5.5: 51.6 k / $0.049, range
  50.4–54.0 k: above it, one more search and one more turn), T2 78.0 k /
  $0.081 (109 k), T3 74.8 k / $0.066 (75 k / $0.059, cost +12 %) —
  correctness unchanged; the cost of n = 1 is not interpretable. *#51
  cost:* `show` output read by the T4 agents 46.7 KB per run (5.5:
  26.3 KB), of it the `used by` / `uses` sections 14.9 KB and the package
  prefixes in them 4.3 KB (≈ 1.1 k tokens per run, < 2 % of a run's
  tokens even if read on every later turn's cache); of the 32 `show`
  calls most fqns were copied from earlier output (about half from usage
  lines), 2 were typed for a member inherited from a supertype
  (`AuthDataEventService#publishAsync…`, declared in
  `DynamoDBEventService`) and failed with "not found in the index" (→ open
  #54). No trace, so no depth data for #52 (Measurements). *Data:*
  `main-55/runs.csv` regenerated by `run.py summary` with the new
  `annatar_trace` column (all 0); every other value unchanged (checked
  against HEAD's `run.py`). *Spend:* $1.28 for 8 runs (estimate $1.2–1.7,
  abort $8), grader ≈ 75 k subagent tokens. *Decisions:* D-ed (prompt),
  D-ee (verdict: #42 closed), D-ef (#46: no usages in `search`), D-eg
  (#51: keep full fqns); #47, #49, #50 decided (see Open questions); new
  open #53, #54. *Review:* 11 findings, all addressed, no code change
  beyond a test: the mechanism is 3/5 via the consumer's `used by`, not
  4/5, and the `processMessages` provenance corrected (F1); cost worded as
  no measurable increase with the cache-creation driver (F2); the
  T4-with-5 location borderline recorded, strict 4/5 (F3); D-eg's premise
  corrected, inherited-member `show` → #54 (F4); the prompt's
  `[entry: @Scheduled]` example and "message queues" as mild cues in
  D-ed / D-ee (F5); deviations disclosed (bundle, exploratory tests, exact
  amendment time 20:47:57, first run ≈ 20:48:03) here and in a post-hoc
  note appended to `protocol.md` (F6); README findings line scoped to the
  re-run, cost wording, link to this file (F7); `main-55/runs.csv` and T1
  wording (F8); #50's evidence (F9); doc drift in `state.md` (Current
  state, #42, #40), `plan.md` (Phase 6 done, 6.7 *Done*, Later #47) and
  `usages.md` (#46, #47) (F10); `test_trace` uses the real CLI order
  (`--depth` after `trace`) (F11).

- **6.6 `annatar trace <fqn> [--depth N]`.** New module `src/trace.rs`
  (`render(conn, fqn, depth, limit)`, `DEFAULT_DEPTH` 6, `MAX_DEPTH` 10,
  `DEFAULT_LIMIT` 10) and the command `annatar trace <fqn> [--depth N]
  [-k N]`. `usage_query` gains `callers(conn, id, roll_up)`: `used_by`'s
  statement with a kind filter (`call`, `instantiate`, `reference`; no
  `extends` / `implements` / `overrides` hops) and, without `roll_up`, an
  `inside` of the symbol alone (`SELF`), so a type node below the root
  does not fan out to its members' users (6.5 review F9); `used_by` is the
  same private `users(..)` with the roll-up and no filter, unchanged.
  `Usage` gains `symbol_kind` (the other symbol's kind), `usage_query`
  `constructions(conn, id)` (the `instantiate` / constructor `call` edges
  of a type, for `[initializer]` types). The walk (D-dx, D-eb) is in Rust,
  one query per symbol reached, memoized: a breadth-first pass from the
  root finds each symbol's shallowest level within the `-k` caps, then a
  depth-first print with an explicit stack (no async recursion) expands
  each symbol at its first shallowest occurrence; callers grouped per
  symbol in the order of their first mention, links merged by kind /
  `ambiguous` / `via`. Root:
  `fqn [kind] file:start-end [entry: @A]`, its callers rolled up when it is
  a type (as `show`'s `used by`, without internal edges, with the members'
  `via` callers); below it per caller `fqn [kind] [ambiguous] [via I#m]
  file:lines; …` — the file left out when it is the file of the symbol it
  calls (D-dz) —, `[initializer]` (a type whose initializer calls the
  symbol above, followed through its constructions, D-dy) and `[entry:
  @A]` (`usage_query::entry_point`, on every copy) where they apply, ending
  in `(see above)` (expanded earlier), `(see below)` (an earlier, deeper
  copy of a symbol expanded further down at a shallower level, D-eb),
  `[type, not followed]` (a type below the root that only references the
  symbol above, D-dy), `[no callers in main sources]` (left out after an
  entry point) or `[N callers beyond depth D]`; an entry point with callers
  in code is followed on (D-ec), without them it ends the branch; at most `-k`
  callers per symbol (`--limit`, default 10, 1–100) with `… N more` (D-ea).
  `--depth` 1–10 and `-k` out of range are usage errors (exit 2, clap's
  value parser like `show -k`); an unknown fqn is `error: symbol `…` not
  found in the index` (exit 1). Reads only `index.db`; ≈ 8 ms per trace on
  `argus` including process start. *Tests:* `trace` on a 6-file Java
  fixture through `build_index` — the exact tree of a method with a `via`
  and a direct call from one caller (`:27; :28`), a diamond (`left` /
  `right` → `process`, the second `[entry: @Scheduled] (see above)`), a
  cycle (`first` ↔ `second`), an entry-point and a no-caller leaf; depth 2
  with `[1 caller beyond depth 2]`; the exact depth-2 tree of a type whose
  members are reached deep first (`(see below)` copies, each expanded at
  level 1); a 7-file chain fixture (`R ← A ← B ← X`, `R ← X ← Y ← Z
  [@Bean] ← W [@Scheduled]`, depth 4: `X` at level 3 reads `(see below)`,
  the level-1 `X` reaches `W`, through the `@Bean` entry `Z` its code
  caller; fails on the pre-review walk); `-k 1` with `… 1 more` on two
  levels; a type root (rolled-up callers, internal `audit` call left out,
  `[type, not followed]` on the `Dispatcher` field, `Retry`'s field
  initializer `[initializer]` → `Manual#run() instantiate`); an entry-point
  root without callers; unknown fqn; the caller-line format (`ambiguous`,
  `via`, kind, merged lines, file elision); `usage_query::callers` (kind
  filter, no roll-up, roll-up equal to `used_by` without `overrides` /
  `extends`); `tests/cli.rs`: the exact trace of `UserService#find(Long)`,
  `--depth 1 -k 1` on a type, `--depth 0`/`11`, `-k 0`/`101` exit 2, a
  missing fqn exit 1. 392 lib tests (+10) and 14 CLI tests (+1) pass; fmt
  and clippy `--all-targets -D warnings` clean. *Docs:* `reference.md`
  (command list, a `trace` paragraph with an example matching the binary
  on `show`'s flow fixture), README command list, `usages.md` Output (real
  `argus` trace, format, type root, D-dx), `plan.md` 6.6 (walk instead of
  one CTE, type leaves, `-k` per symbol, default depth 6, shallowest
  expansion, entry points with callers, initializer types). *`argus`*
  (index current, not rebuilt; `index.db` and `cache.db` not written;
  `trace --config real.toml`), final output after the review, default
  depth 6: the T4 consumer reaches the dispatcher's `@Scheduled` method
  through `dispatch` :76 and `receiveAndDispatchMessages` :67:

  ```text
  com.haufe.greenland.argus.messaging.consume.janus.securityMethods.messageConsumer.SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage) [method] backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/messageConsumer/SecurityDataAddEventConsumer.java:23-28
    com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcher#dispatch(SecurityDataMessageBearer<?>) backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/SecurityDataEventDispatcher.java:76
      com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcher#receiveAndDispatchMessages() :67
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcher#processMessages() :59 [entry: @Scheduled]
  ```

  The type `SecurityDataAddEventConsumer` (1947 bytes, 8 lines): the same
  chain, plus `SecurityDataEventDispatcherConfig reference …:24 [type, not
  followed]`, its constructor `reference :28 [no callers in main
  sources]` (Spring calls it) and its getter `reference :45` → `dispatch`
  :75 `(see above)`. A heavily used method,
  `DynamoDBEventService#publishAsync(DynamoDBEvent)`: 43 lines, 9.4 KB,
  every branch ending at a mapping, `@Scheduled` dispatcher, `@Bean` or
  the depth, `… 4 more` at the root (14 callers); the largest trace,
  `DynamoDBEvent`: 75 lines, 16.6 KB at depth 6 (56 / 13.6 KB at 4;
  Measurements). Before D-dy the type nodes were followed: `AuthDataEvent`
  75 lines / 17.6 KB at depth 4, mostly injection chains
  (`TokenValidationEventService reference` → `UserTokenService reference`
  → …). *Review* (`review-6.6.md`, 6 findings; the step met its done-when):
  **F1 (high)** `(see above)` could point to an expansion deeper in the
  tree whose subtree the depth had cut, hiding reachable callers (26
  `argus` traces at depth 4; `UserOrganizationService` never showed
  `RoleRelationMessageDispatcher#processMessages() [entry: @Scheduled]`,
  reachable at level 4) — fixed with the two-pass walk (D-eb): a
  breadth-first pass for each symbol's shallowest level, expansion only
  there, earlier deeper copies `(see below)`; the `cut` set is gone; a
  scan of all 654 symbols at depths 4, 5, 6, 8 and 10 (`-k 10` and
  `-k 100`) finds 0 copies pointing to a deeper or missing expansion (the
  review's `see.py`: 0 cases at 4 and 6, was 26), `UserOrganizationService`
  now reaches the role dispatcher's `@Scheduled`; 44 of 654 traces carry
  a `(see below)` (76 lines). **F2** default depth 4 → 6, #52 closed with
  the measured table (D-ea). **F3** a type reached through a `call` /
  `instantiate` from itself (a field initializer) is `[initializer]` and
  followed through its constructions instead of `[type, not followed]`
  (D-dy; 0 such edges on `argus`, output unchanged there). **F4** an entry
  point's label is printed on every copy (`[entry: @Scheduled] (see
  above)`), and an entry point with callers in code is followed on (D-ec;
  0 on `argus`). **F5** file elision documented as "the file of the
  symbol it calls" (`reference.md`, `usages.md`, D-dz, `main.rs`). **F6**
  Done and Measurements rewritten for the new rules and depth 6.

- **6.5 `show`: `used by` and `uses`.** New read-side module
  `src/usage_query.rs` (D-du): `used_by(conn, id)` and `uses(conn, id)`
  return one `Usage` per edge (the other symbol's id, fqn, file, the edge
  kind, line, `ambiguous`, `via`, entry point), each one SQL statement —
  a recursive `inside` CTE over `parent_id` (the symbol and everything
  nested in it), for `used_by` a recursive `overridden` CTE up the
  `overrides` edges of everything inside and a `targets` union, then plain
  joins, ordered fully (file, line, fqn, kind, `via`, `ambiguous`, the
  other end's fqn); `entry_point(kind, fqn, signature, annotations)` names
  `@Scheduled`, `@RequestMapping`, `@Get/Post/Put/Delete/PatchMapping`,
  `@EventListener`, `@ExceptionHandler`, `@PostConstruct`, `@PreDestroy`,
  `@Bean` (by simple name, qualified or with arguments) and `public static
  void main(String[])` — all three for 6.6 `trace` to reuse. `show` (only
  for the shown symbol, not its children, after its tickets and before
  the children): `- entry point: @Scheduled` when it is one, then `- used
  by:` and `- uses (lines in this symbol's file):` grouped by file (`used
  by`: the user's file; `uses`: the used symbol's file, the lines being in
  the shown symbol's file, which the title says), one line per symbol and
  link `fqn [kind] [ambiguous] [via <fqn>] :line, line [entry: @A]` (kind
  left out for `call`; same symbol + kind + ambiguous + via merged, lines
  deduplicated), capped after `-k` entries (`--limit`, default
  `show::DEFAULT_LIMIT` = 20, 1–100, like `search -k`) with `… N more`;
  empty lists read `- used by: none in main sources` and `- uses: none`
  (D-dt). `via` (D-dr): the callers of every method the symbol (or a
  member of the type) overrides, transitively up the `overrides` chain,
  outside the shown symbol, each caller once (also through a diamond);
  a method's incoming `overrides` edges are listed as such (the
  overriding methods), a type's roll-up leaves out the `overrides` edges
  into its members. A type's `uses` rolls up its members' and nested
  types' outgoing edges without the ones ending inside it and without its
  members' `overrides` (D-ds); a `new T(..)` that reaches a declared
  constructor shows only the constructor's `instantiate`, not the type's
  too. `show::render(conn, fqn)` keeps its signature (default cap),
  `render_limited(conn, fqn, limit)` is behind the CLI. No JSON or other
  output mode exists for `show`, so none was added. `show` still reads only
  `index.db` (no Ollama; ≈ 14 ms per call on `argus` including process
  start). *Tests:* `usage_query` — a method's `via` up a two-level chain
  and through a diamond (`C#m` overriding `B#m` and `J#m`, both overriding
  `I#m`: the `I#m` caller once, a caller of each of `B#m`, `J#m`), an
  `ambiguous` edge, an interface method listing both overriders, a
  method's `uses` keeping its own `overrides` and only the constructor of a
  `new`, a type roll-up without internal edges and without its members'
  `overrides` for both lists (nested type included), a symbol without
  users or uses, the entry-point rules (`@ExceptionHandler`, `@PreDestroy`,
  `main` only when public static `String[]`, a type's `@RequestMapping` is
  none, `@Scheduler` is none); `show` on a 5-file Java fixture through
  `build_index` — the exact output of a method with a direct and a `via`
  caller and its `uses` (`overrides`, internal call, `reference`), a type
  roll-up (`instantiate`/`reference` of a field, a getter's return, `via`,
  no member `overrides`), an entry point without users and the `[entry:
  @Scheduled]` marker with merged lines `:14, 15`, the cap (`… 2 more`),
  and one line format test (`ambiguous` apart from the plain edge on the
  same line, `via`, entry, kind); the two existing exact `show` tests gain
  the empty lists; `tests/cli.rs`: `show -k 1` caps, default lists all
  with `[entry: @GetMapping]`, `-k 0` is a usage error (exit 2). 382 lib
  tests (+9) and 13 CLI tests (+1) pass. *Docs:* `reference.md` (command
  list, a `show` paragraph with examples that match the binary's output),
  README `show` line, `usages.md` Output (the real `argus` output, `via`,
  `uses`, entry points), `plan.md` 6.5 (`via <fqn of I#m>`) and Later
  (test-code item names #45). **#45 decided** (D-dv): deferred to the Later
  test-code indexing item, nothing built. **Review** (10 findings, all
  handled): F1 `uses` paired the used symbol's path with lines of the
  shown file → title `- uses (lines in this symbol's file):` (D-dt); F2 a
  type's roll-up listed its subclasses' `overrides` into its members (18
  of `AbstractCache`'s 36 entries, pushing all 8 consumers calling
  `refresh` past the cap) → left out in both roll-ups, kept for a shown
  method (D-dr, D-ds; `AbstractCache` 36 → 18 entries); F3 a `new T(..)`
  listed both `T` and `T#<init>(..)` in `uses` → only the constructor; F4
  package repeated in every fqn → kept (fqns are what agents copy), `via`
  kept as a full fqn (D-dw), token cost → open #51 for 6.7; F5
  `@ExceptionHandler` (2 on `argus`) and `@PreDestroy` added, D-du's
  "`argus` uses only these" corrected; F6 `none in main sources`; F7
  `ORDER BY` made total; F8 tests for `ambiguous`, two `overrides` edges
  and a diamond; F9 reuse for `trace` (no roll-up, kind filter) noted in
  Next; F10 the D-dv wording, `via` wording in `plan.md`, the timing (≈ 14
  ms). *`argus`* (index current, not rebuilt; `cache.db` untouched; `show
  --config real.toml`), after the review:
  `show com.haufe.greenland.argus.messaging.consume.janus.securityMethods.messageConsumer.SecurityDataAddEventConsumer`:

  ```text
    - used by:
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/SecurityDataEventDispatcher.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcher#dispatch(SecurityDataMessageBearer<?>) :76
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/SecurityDataEventDispatcherConfig.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcherConfig reference :24
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcherConfig#<init>(SecurityDataAddEventConsumer,SecurityDataRemoveEventConsumer) reference :28
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcherConfig#getSecurityDataAddEventConsumer() reference :45
    - uses (lines in this symbol's file):
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/cache/securitymethods/SecurityMethodsCache.java
        com.haufe.greenland.argus.auth.authorization.cache.securitymethods.SecurityMethodsCache reference :20
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/event/AuthDataCacheUpdateEvent.java
        com.haufe.greenland.argus.auth.authorization.event.AuthDataCacheUpdateEvent#<init>(String) instantiate :27
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/event/AuthDataEventService.java
        com.haufe.greenland.argus.auth.authorization.event.AuthDataEventService reference :19
      backend/src/main/java/com/haufe/greenland/argus/auth/event/dynamoDB/DynamoDBEventService.java
        com.haufe.greenland.argus.auth.event.dynamoDB.DynamoDBEventService#publishAsync(DynamoDBEvent) :27
      backend/src/main/java/com/haufe/greenland/argus/common/cache/AbstractCache.java
        com.haufe.greenland.argus.common.cache.AbstractCache#refresh(K) :26
      backend/src/main/java/com/haufe/greenland/argus/integration/janus/SecurityData.java
        com.haufe.greenland.argus.integration.janus.SecurityData reference :24, 25
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/SecurityDataMessageBearer.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataMessageBearer#getPayload() :24, 25
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/message/SecurityMethodAddMessage.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.message.SecurityMethodAddMessage reference :23
  ```

  `show 'com.haufe.greenland.argus.messaging.consume.janus.securityMethods.messageConsumer.SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage)'`:

  ```text
    - used by:
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/SecurityDataEventDispatcher.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcher#dispatch(SecurityDataMessageBearer<?>) :76
    - uses (lines in this symbol's file):
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/event/AuthDataCacheUpdateEvent.java
        com.haufe.greenland.argus.auth.authorization.event.AuthDataCacheUpdateEvent#<init>(String) instantiate :27
      backend/src/main/java/com/haufe/greenland/argus/auth/event/dynamoDB/DynamoDBEventService.java
        com.haufe.greenland.argus.auth.event.dynamoDB.DynamoDBEventService#publishAsync(DynamoDBEvent) :27
      backend/src/main/java/com/haufe/greenland/argus/common/cache/AbstractCache.java
        com.haufe.greenland.argus.common.cache.AbstractCache#refresh(K) :26
      backend/src/main/java/com/haufe/greenland/argus/integration/janus/SecurityData.java
        com.haufe.greenland.argus.integration.janus.SecurityData reference :24, 25
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/SecurityDataMessageBearer.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataMessageBearer#getPayload() :24, 25
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/message/SecurityMethodAddMessage.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.message.SecurityMethodAddMessage reference :23
  ```

  `show com.haufe.greenland.argus.common.cache.AbstractCache` (18 entries,
  all within the default cap; before the review 36, the consumers below
  hidden in `… 16 more`):

  ```text
    - used by:
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/cache/AuthDataService.java
        com.haufe.greenland.argus.auth.authorization.cache.AuthDataService#getAuthData(UUID) :56, 57, 58, 63
        com.haufe.greenland.argus.auth.authorization.cache.AuthDataService#getPersonOrganizationRelations(UUID) :102
        com.haufe.greenland.argus.auth.authorization.cache.AuthDataService#deleteUserCache(UUID) :118, 119, 120
        com.haufe.greenland.argus.auth.authorization.cache.AuthDataService#deleteOrganizationCache(UUID) :125, 126
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/cache/contract/ContractCache.java
        com.haufe.greenland.argus.auth.authorization.cache.contract.ContractCache extends :15
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/cache/identificationmethods/IdentificationMethodsCache.java
        com.haufe.greenland.argus.auth.authorization.cache.identificationmethods.IdentificationMethodsCache extends :15
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/cache/person/AccessPersonOrganizationRelationCache.java
        com.haufe.greenland.argus.auth.authorization.cache.person.AccessPersonOrganizationRelationCache extends :15
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/cache/person/PersonOrganizationRelationCache.java
        com.haufe.greenland.argus.auth.authorization.cache.person.PersonOrganizationRelationCache extends :14
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/cache/roles/UserRoleCache.java
        com.haufe.greenland.argus.auth.authorization.cache.roles.UserRoleCache extends :15
      backend/src/main/java/com/haufe/greenland/argus/auth/authorization/cache/securitymethods/SecurityMethodsCache.java
        com.haufe.greenland.argus.auth.authorization.cache.securitymethods.SecurityMethodsCache extends :15
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/cives/identification/messageConsumer/IdentificationDataAddEventConsumer.java
        com.haufe.greenland.argus.messaging.consume.cives.identification.messageConsumer.IdentificationDataAddEventConsumer#consume(IdentificationDataAddMessage) :30
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/cives/identification/messageConsumer/IdentificationDataRemoveEventConsumer.java
        com.haufe.greenland.argus.messaging.consume.cives.identification.messageConsumer.IdentificationDataRemoveEventConsumer#consume(IdentificationDataRemoveMessage) :31
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/cronus/person/message/consumer/PersonOrganizationRelationCreatedMessageConsumer.java
        com.haufe.greenland.argus.messaging.consume.cronus.person.message.consumer.PersonOrganizationRelationCreatedMessageConsumer#perform(PersonOrganizationRelationMessagePayload) :35
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/cronus/role/message/consumer/RoleRelationsUpdatedMessageConsumer.java
        com.haufe.greenland.argus.messaging.consume.cronus.role.message.consumer.RoleRelationsUpdatedMessageConsumer#perform(RoleRelationMessagePayload) :90
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/messageConsumer/SecurityDataAddEventConsumer.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.messageConsumer.SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage) :26
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/janus/securityMethods/messageConsumer/SecurityDataRemoveEventConsumer.java
        com.haufe.greenland.argus.messaging.consume.janus.securityMethods.messageConsumer.SecurityDataRemoveEventConsumer#consume(SecurityMethodRemoveMessage) :25
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/pactum/message/messageConsumer/BusinessFeatureMessageConsumer.java
        com.haufe.greenland.argus.messaging.consume.pactum.message.messageConsumer.BusinessFeatureMessageConsumer#perform(BusinessFeatureMessage.OrganizationBusinessFeatures) :50
      backend/src/main/java/com/haufe/greenland/argus/messaging/consume/pactum/message/messageConsumer/ContractChangedMessageConsumer.java
        com.haufe.greenland.argus.messaging.consume.pactum.message.messageConsumer.ContractChangedMessageConsumer#perform(ContractMessage.Contract) :42
    - uses: none
  ```

  The type names the dispatcher config (field :24, constructor :28,
  getter :45) and, through `call`, the dispatcher's `dispatch` :76; the
  consumer implements no indexed interface, so no `via` here; its `uses`
  is correct against the source (lines 20–27 of
  `SecurityDataAddEventConsumer.java`). `via` on `argus`, a cronus user
  consumer (`UserCreatedMessageConsumer#perform(UserMessagePayload)`
  overrides `MessageConsumer#perform(T)`, called by the dispatcher only
  through the interface):

  ```text
  - used by:
    backend/src/main/java/com/haufe/greenland/argus/messaging/consume/cronus/user/UserMessageDispatcher.java
      com.haufe.greenland.argus.messaging.consume.cronus.user.UserMessageDispatcher#performMessage(MessageBearer) via com.haufe.greenland.argus.messaging.consume.cronus.user.message.MessageConsumer#perform(T) :123
  ```

  `show com.haufe.greenland.argus.messaging.consume.janus.securityMethods.SecurityDataEventDispatcher#processMessages()` prints `- entry
  point: @Scheduled` and `- used by: none in main sources`; both
  `ArgusExceptionHandler` methods now print `- entry point:
  @ExceptionHandler`. List sizes over all 654 symbols (`-k 100`): `used
  by` empty for 230, median 1 entry, more than 20 for 2 (max 29,
  `DynamoDBEvent`); `uses` empty for 289, median 1, more than 20 for 4
  (max 27, `RoleRelationsUpdatedMessageConsumer`).

- **6.4 Overrides and measured quality.** *`overrides` edges*
  (`EdgeKind::Overrides`, D-dn): after the walks, `Resolver::override_edges`
  gives every method with a row (not a constructor, static or private —
  `MethodFacts` gains `is_private`) an edge to each method it overrides
  nearest in `lineage` (the overridden ones no other overridden one
  overrides in turn), on the line of its name (`MethodFacts::line` is now
  the name's line, after annotations; it was used nowhere else). The
  override test is one `Resolver::overrides` shared with `methods()`'s
  "overridden nearer" rule: same name and count, parameter types equal as
  resolved `Ty`s after `Resolver::type_args` substitutes the farther type's
  type variables along the super-type path (`Base<X> implements
  Consumer<X>`, `Impl extends Base<AddMessage>`: `X` → `AddMessage`); a
  variable without an argument (raw super type, wildcard) or an
  unresolvable type matches any type, a variable of the nearer method or
  type only the same variable (after the review, D-dn). *Open #48* (decided on the measured
  misses, D-dp): the same substitution now types inherited members on a
  subtype — return types (`return_type` gets the receiver's types), fields
  and Lombok/record accessors (`Resolver::declared_via`) — and a variable
  of a **method's** bounded type variable is typed by its first bound
  (`Region::bounds`, from the tree). Not done: a receiver's own type
  arguments and lambda parameter types (open #49), class-level bounds.
  *Call-site report:* the `index` summary already had resolved /
  implicit / ambiguous / unresolved since 6.3; the `edges:` line gains
  `overrides` (`IndexStats::edges_overrides`, in `edges()`). *Usage
  golden set and evaluation* (D-dm, D-do): new module `src/usage_eval.rs`
  (`UsageSet` TOML `[[symbol]]` with `fqn`, `users`, optional
  `overridden_by`, `kind`, `note`, validated like `GoldenSet`; `check_index`
  over every listed fqn, reusing `golden::IndexProblem`; `evaluate` reads
  each symbol's users from `edges` — sources of its incoming non-`overrides`
  edges, a type's rolled up over its members and nested types by a
  recursive CTE on `parent_id`, without sources inside it — and its
  overriders; `format_report` per symbol and pooled for all / types /
  members / overriders) and the command `annatar eval-usages <set.toml>`
  (no Ollama, no `--path`). `golden` exposes `is_fqn`, `kind_fits`,
  `with_article`. The real set `.annatar-local/usages-argus.toml` (outside
  git): 29 symbols, 91 users, 5 with `overridden_by` (10); 54 symbols and
  107 users after the review. Tests
  (fixtures): overrides through a generic abstract class, an overload that
  does not override, `@Override` line, raw super type, interface → super
  interface, static/private/constructor/library super left out, an
  interface reached directly and through a sub-interface, type arguments through two levels
  (`Flipped<P> extends Pair<Sms, P>`), the overload choice beside them;
  inherited return / field / Lombok getter through a generic chain (and a
  receiver's own `Bearer<Data>` still unresolved), a bounded method type
  variable; indexer rows and counter; `usage_eval` parsing errors, `Match`,
  the report, the roll-up and overrider scoring, the index check; an
  integration test scoring `tests/fixtures/golden/usages.toml` against the
  sample project (all 1.000) and two CLI tests. 368 lib tests (383 run in
  all), fmt and clippy `--all-targets -D warnings` clean. **`argus`**
  (`index --offline`, `data-real` backed up to `data-real.bak-6.4`): 1200
  edges (+50 `overrides`, +7 `reference`, +1 `call` from the #48 change;
  none of the 1142 lost), call sites 1386 = 424 / 174 / 0 / 788, 11 ms;
  `symbols` identical, 0 chat / 0 embedding calls, `cache.db` md5
  unchanged; `eval-usages`: **P 1.000 R 0.967** (88/91) — 0.934 before the
  #48 change —, overriders 10/10 (Measurements).
  *Review* (11 findings, D-dq): fixed H1 (a type variable of the nearer
  method or type matched any type: wrong `overrides` and `call` edges) and
  H2 (a farther variable bound to a nearer variable; the test expected a
  false override) with `members::Sig` and `resolver::Arg::Var`; M1
  (varargs compared as an array); L1 (a bound no longer leaks into a
  same-named variable of the region); L2 (`MethodFacts::is_package_private`:
  no override across packages); L3 (header type arguments resolve around
  the type, `Resolver::declared_in`); L5 (summaries say "each symbol–user
  pair", index errors name the entry as `symbol N`). M2: the usage set
  grew by a reproducible draw of 25 methods and constructors
  (`.annatar-local/usages-argus-draw.py`, users found by hand before
  reading edges) and Measurements reports random-only and chosen-only
  scores. L4 recorded in `usages.md`. Tests: 5 new (373 lib tests, 388 run
  in all), the `Flipped` one corrected; fmt and clippy `--all-targets -D
  warnings` clean. **`argus`** (`index --offline`, `data-real` backed up
  to `data-real.bak-6.4-fix`): 1200 edges identical by fqn, kind and line,
  call sites 424 / 174 / 0 / 788, 0 chat / 0 embedding calls, `cache.db`
  md5 unchanged; `eval-usages` on 54 symbols: **P 1.000 R 0.972**
  (104/107), members 50/53, random only 23/23, overriders 10/10.
- **6.3a `refactor(usages)`: module split** (review L6 of 6.3; no
  behaviour change). `src/usages.rs` (4310 lines) becomes the module root
  plus `src/usages/`, laid out like `src/symbols.rs` +
  `src/symbols/facts.rs` (D-dl): `usages.rs` (module docs, API `EdgeKind`,
  `Edge`, `SourceFile`, `Usages`, `CallSites`, `resolve`, and the shared
  test helpers), `resolver.rs` (`TypeEntry`, `Scope`, `FieldFilter`,
  `Lookup`, `Ty`, `Resolver` with the type table, name lookup, `lineage`
  / `supers`, fields, `declared`), `members.rs` (`Method`, `Fit`, the
  `Resolver` methods for methods, constructors, static imports, implicit
  members, `choose` / `score`, and `fits` / `overrides` / the JDK
  tables), `walk.rs` (`Region`, `Unnamed`, `FileWalk`: the tree walk,
  scopes, locals, type mentions, `record` / `emit` / `push`, node
  helpers) and `expr.rs` (`Receiver` and the `FileWalk` methods that type
  expressions: calls, receivers, method references, field access, names).
  Code moved verbatim: items and fields used across the files are
  `pub(super)` (only those), imports split per file, rustfmt reflowed the
  longer signatures; every string literal (all fixtures) is unchanged.
  The 39 tests moved to the module they exercise (2 + 6 helpers stay in
  `usages.rs`, 10 in `resolver`, 10 in `members`, 8 in `walk`, 9 in
  `expr`); same 356 lib tests (373 listed in all), all green; fmt and
  clippy `--all-targets -D warnings` clean. **`argus`** (`index
  --offline` on a scratch copy of `data-real`): the same 1142 edges
  (identical by fqn and by row ids, `symbols` identical), call sites
  1386 = 423 / 167 / 0 / 796, 11 ms, 0 chat calls, `cache.db` md5
  unchanged. Review: approved, 3 doc nits fixed (test count, test
  share, wording of the code change in D-dl); the undocumented `edges`
  test helper is pre-existing, left as moved.
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

- Phase 6 is complete (6.7 reviewed and committed); `plan.md` has no
  further step. The next work is chosen with the product owner from the
  open items and `plan.md` Later — e.g. a multi-hop agent task to measure
  `trace` (open #53), `show` of an inherited member (open #54), SCIP as the
  precision upgrade (#49), test sources as edge sources (#45), the
  cross-phase questions in `state.md` (#39, #40, #41, #43, #44). Nothing
  started.

## Step log

| Step | Status | Notes |
| --- | --- | --- |
| Phase 6 plan | done | docs only: `usages.md` (design), `plan.md` Phase 6 (steps 6.1–6.7), this file; SCIP and callers in prompts in Later, `project.md` edge kinds; D-cu (confirmed by the product owner)–D-cx; open #45–#47, #42 updated in `state.md` |
| 6.1 Parser facts for resolution | done | `ParsedFile` gains `package`, `imports`, `types` (`TypeFacts`: super types, fields, Lombok, methods with return/parameter types and varargs, enum constants, record components); `src/symbols/facts.rs`; symbols unchanged (`argus` reindex identical); D-cy–D-da; review: 9 findings, 5 fixed (record varargs component, compact constructor params, annotated superclass, 2 doc nits), 4 recorded as limitations/notes (outer generic args, Lombok arguments, tree for 6.2, arity from `params`) |
| 6.2 Type resolution and `edges` | done | `edges` table; `src/usages.rs` resolver (nested/inherited → single import → same package → wildcard, qualified names, static-import constants, variable shadowing); `index_edges` after structure on the kept trees; stats, `edges:` output line, `-v` top unresolved names, `--path` warning, benchmark time; `argus`: 569 edges, 2048 unresolved mentions (214 names, all external), 7 ms, 0 chat calls; D-db–D-df; review: 8 findings — fixed: header/annotations resolve around the type, static-field `case` labels (+2 T4 dispatcher rows), anonymous/local class super-type scope, `outer.new Inner()` skipped, static-import qualifier, 2 doc breakages, output label, nits (`EdgeKind::ALL`, facts dropped after the stage); recorded: library super types' nested types, outer type of a qualified name (D-dc), enum switch caveat (D-df) |
| 6.3 Member resolution | done | `call` edges and `instantiate` to constructors; typed expressions and block-scoped typed locals (D-dd replaced, D-dg); receivers, chains through declared return types, unqualified lookup inside-out then static imports, `super`/`this` calls, method references, implicit Lombok/record/enum/constructor members (D-dh), overloads with `ambiguous` (D-di), stops (D-dj); call-site counts and `-v` top unresolved calls; benchmark calls; `argus`: 392 `call` + 31 constructor `instantiate`, 586 / 0 / 800 call sites, T4 row :76 present, 0 chat calls; open #48; review: 14 findings — fixed: indexed argument vs final JDK parameter (H1), single candidate type-checked (H2), unqualified `Object`/`Enum` methods stop at the innermost class (H3), same-type overloads (M1), Java's arity phases and exact ranking only on known types (M2), one `lineage` walk superclass-first (M3), `implicit` call-site count (M4), pattern-variable and anonymous-field scoping (L1, L2), annotation elements (L4), builder edge cases (L5), `dims` helper; recorded: bounded type variables (L3 → #48), module split (L6 → Next); D-dk; `argus` after: 1142 edges (+4 annotation-element references), call sites 423 / 167 implicit / 0 / 796 |
| 6.3a refactor(usages) split | done | review L6 of 6.3, no behaviour change: `src/usages.rs` (API, `resolve`, test helpers) + `src/usages/` `resolver.rs`, `members.rs`, `walk.rs`, `expr.rs` (D-dl); code moved verbatim, cross-file items `pub(super)`, tests moved with their code (39 tests, same names; 356 lib tests); `argus`: edges identical (1142, by fqn and by id), 0 chat calls; review: approved, 3 doc nits fixed |
| 6.4 Overrides and measured quality | done | `overrides` edges, nearest per Java with type-argument substitution through generic super types, shared with the "overridden nearer" lookup (D-dn); #48 decided: super-type arguments substituted in inherited members, method-level bounded type variables typed by the bound (D-dp), receivers' own type arguments → open #49; `edges:` line + `edges_overrides`; `src/usage_eval.rs` + `annatar eval-usages` (D-do); hand-built 29-symbol `argus` usage set (D-dm); `argus`: 1200 edges (50 `overrides`), call sites 424 / 174 / 0 / 788, P 1.000 R 0.967 (88/91), overriders 10/10, misses 3 × lambda; review: 11 findings — fixed: a type variable of the nearer method or type matched any type (H1, wrong `overrides` and `call` edges) and a farther variable bound to a nearer variable (H2, the test expected a false override) via `Sig` / `Arg::Var`, varargs vs single parameter (M1), the random part of the set made reproducible and grown to 33 symbols with random-only scores (M2), bounds leaking between same-named variables (L1), package-private across packages (L2, `is_package_private`), header type arguments around the type (L3), eval wording and `symbol N` in index errors (L5); recorded: L4 recall gaps (`usages.md`), L5's optional checks (D-dq); 54-symbol set P 1.000 R 0.972 (104/107), random only 23/23, members 50/53; `argus` edges unchanged |
| 6.5 `show`: `used by` / `uses` | done | `src/usage_query.rs`: `used_by` / `uses` as SQL joins (`inside` CTE over `parent_id`, `overridden` CTE up `overrides`), `entry_point` (D-du); `show`: `- entry point:`, `- used by:` / `- uses:` grouped by file, `fqn [kind] [ambiguous] [via I#m] :lines [entry: …]`, `-k`/`--limit` (default 20, 1–100) with `… N more`, `none` when empty, shown symbol only (D-dt); `via` = callers of every method it overrides, transitively; incoming `overrides` listed (D-dr); a type's `uses` rolls up its members' (D-ds); #45 decided: deferred to Later (D-dv); new open #50, #51; tests: `via`, roll-up, cap, no users, entry points, exact format, CLI `-k`; `argus`: `SecurityDataAddEventConsumer` names `SecurityDataEventDispatcherConfig` (:24, :28, :45) and `SecurityDataEventDispatcher#dispatch` (`call` :76), `via` on `UserCreatedMessageConsumer#perform` → `UserMessageDispatcher#performMessage` :123; index not rebuilt, 0 LLM calls; review: 10 findings — fixed: `uses` lines vs the used symbol's path (F1, title `uses (lines in this symbol's file)`), members' `overrides` left out of a type's roll-up (F2, `AbstractCache` 36 → 18 entries, its callers within the cap), duplicate `instantiate` of type and constructor (F3), `@ExceptionHandler` / `@PreDestroy` (F5), `none in main sources` (F6), total `ORDER BY` (F7), tests for `ambiguous`, two overrides and a diamond (F8), wording (F10); recorded: full fqns' token cost → open #51 with `via` kept as an fqn (F4, D-dw), `trace` reuse (F9 → Next) |
| 6.6 `annatar trace` | done | `src/trace.rs`: two-pass walk (BFS levels, DFS print, D-eb) over `usage_query::callers` (new: `used_by`'s query with a `call`/`instantiate`/`reference` filter, roll-up only at the root; `Usage::symbol_kind`), one query per printed symbol (D-dx); tree `fqn [kind] [ambiguous] [via I#m] file:lines; …`, file left out when unchanged (D-dz), `(see above)`, `[entry: @A]`, `[no callers in main sources]`, `[N callers beyond depth D]`, `(see below)`, each symbol expanded at its shallowest level (D-eb), types below the root `[type, not followed]` or `[initializer]` (followed through `usage_query::constructions`, D-dy), entry points labelled on every copy and followed when code calls them (D-ec); `--depth` 1–10 (default 6), `-k` per symbol 1–100 (default 10, D-ea), usage errors exit 2; tests: via, diamond, cycle, entry / no-caller leaves, depth, re-expansion, cap, type root, format, CLI ranges; `argus`: the T4 consumer → `dispatch` :76 → `receiveAndDispatchMessages` :67 → `processMessages` :59 `[entry: @Scheduled]`; sizes over 654 symbols (Measurements); index not rebuilt, 0 LLM calls; review: 6 findings, all fixed — `(see above)` into a depth-cut expansion hid callers (F1, 26 → 0 on `argus`; `UserOrganizationService` now reaches the role dispatcher's `@Scheduled`), default depth 6 and #52 closed (F2), initializer types followed (F3), entry label on repeats and entry points with code callers followed (F4), elision wording (F5), docs (F6) |
| 6.7 Agent trial (T4) on usages | done | `run.py --with-prompt usages` (files + one bullet on `used by` / `uses` / `trace`, D-ed), `trace` counted apart, +2 Python tests; README agent snippet; 6.7 protocol amendment; T4 × 5 + T1–T3 × 1, *with* arm, `main-67/`; blind grading 30/30 old grades agree; T4 dispatcher 5/5 (5.5: 3/5), full marks 5/5 (2/5), cost +7 % / tokens −10 % (n.s.), `show` 6.4 vs 3.4, `trace` 0; T1–T3 all 2; $1.28; D-ed–D-eg, #42 closed, #46/#47/#49/#50/#51 decided, open #53; review: 11 findings, all addressed — mechanism 3/5 via the consumer's `used by` (one hit via a `LOG_PREFIX` reference, one via search), cost "no measurable increase" (mean +7 %, median −8 %, p = 1.0; cache creation +21 %), T4-with-5 strict 4/5, D-eg premise, prompt cues, deviations disclosed (post-hoc note in `protocol.md`), README scope, doc drift, test CLI order; new open #54 |

## Measurements

Filled by 6.2–6.7, so the numbers of the phase sit in one place.

| Measure | Step | Value |
| --- | --- | --- |
| Edges per kind on `argus` | 6.2 / 6.3 / 6.4 | 6.4: 1200 rows — 36 `extends`, 11 `implements`, 70 `instantiate`, 640 `reference` (+7), 393 `call` (+1), 50 `overrides` (new); none of the 1142 rows before lost, `symbols` identical, 0 chat / 0 embedding calls, `cache.db` md5 unchanged. The +8 are the #48 change (D-dp), all hand-checked correct: `getPayload().getUserId()` → `SecurityData` in both janus consumers (lines 23–25) and → `IdentificationDataPayload` in both cives consumers (:24), `personOrganizationRelationCache.get(id).getPersonOrganizationRelations()` → `CachedPersonOrganizationRelations` (`AbstractCache<UUID, V>#get` returning `V`, :102), and `UserDataMessagePublisher#sendAsync(T) -call-> UserDataMessageBearer#getMessageType() :69` (bounded `T`). **`overrides`:** all 50 checked against the sources, all correct (6 `DatabaseCacheUserTokenStrategy` → `TokenStoreStrategy`, 18 cache subclasses' `supply`/`getCachePrefix`/`getCacheTTLSeconds` → `AbstractCache` (`supply(UUID)` → `supply(K)`), 10 `DynamoDBEvent` → `AuthDataEvent`, `DynamoDBEventService#publishAsync(DynamoDBEvent)` → `AuthDataEventService#publishAsync(T)`, 8 service impls → their interfaces, 7 message consumers → their package's `MessageConsumer#perform(T)`, 3 of them without `@Override`); completeness: of the 78 `@Override` sites in main, the 47 whose overridden method is indexed all have an edge, the other 31 override library or `Object` methods or sit in an anonymous class (not symbols). 6.3 after the review: 1142 rows — 633 `reference` (+4: `messageConsume.type()`, an annotation element, in each of the four message dispatchers), the rest unchanged; none of the 1138 rows before lost. 6.3 before the review: 1138 rows — 36 `extends`, 11 `implements`, 70 `instantiate` (39 to types as in 6.2, 31 to constructors), 629 `reference` (+146: fields of other types read or written, Lombok accessors and builders, enum members, implicit superclass constructors), 392 `call` (149 between members of one type); none of the 569 6.2 rows lost. T4: `SecurityDataEventDispatcher#dispatch(SecurityDataMessageBearer<?>) -call-> SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage) :76` and `… -call-> SecurityDataEventDispatcherConfig#getSecurityDataAddEventConsumer() :75`, `receiveAndDispatchMessages() -call-> dispatch(..) :67` (`this::dispatch`), `processMessages() -call-> receiveAndDispatchMessages() :59`. Spot checks: 60 random `call` / constructor `instantiate` edges and 25 random new `reference` edges against their source lines, all correct (e.g. `removeAllTokens(…getUserIdList())` → the `List<UUID>` overload, `new TokenInvalidationEvent(userId.toString())` → the `String` constructor, `this(Collections.singletonList(userId))` → the `List<String>` one). 6.2: 569 rows — 36 `extends`, 11 `implements`, 39 `instantiate`, 483 `reference` (567 before the review fixes; the +2 are the T4 dispatcher's `case EVENT_MFA_METHOD_ADDED, …` labels, static-imported constants of `SecurityDataMessageBearer`, now `reference` edges from `SecurityDataEventDispatcher#dispatch(..)` on lines 75 and 77). Spot checks: `SecurityDataEventDispatcherConfig` → `…messageConsumer.SecurityDataAddEventConsumer` `reference` from the type (field, :24), its constructor (:28) and `getSecurityDataAddEventConsumer()` (:45), none from the dispatcher (6.3's chain); each of the four `MessageBearer`s is reached only from its own package's dispatcher (by import: `cronus.role`/`person`/`user`, `pactum`), e.g. `RoleRelationMessageDispatcher#dispatch(MessageBearer)` → `cronus.role.message.MessageBearer` :107; `Privilege.GreenlandAdmin.X` in an annotation → the nested `Privilege.GreenlandAdmin`; 18 random edges checked against their source lines, all correct |
| Unresolved type names / call sites | 6.2 / 6.3 | 6.2: 2048 mentions of 214 distinct names, none the simple name of an indexed type (all JDK / Spring / Lombok / Jackson / `com.haufe.*` libraries): `String` 341, `UUID` 227, `Override` 78, `List` 70, `Set` 67, `ResponseEntity` 61, `Autowired` 52, … |
| Resolved / unresolved / ambiguous call sites | 6.3 / 6.4 | 6.4 (after #48, D-dp): 1386 call sites — **424 resolved to a member (30.6 %)**, 174 implicit (12.6 %), 0 ambiguous (0 %), 788 unresolved (56.9 %); before the #48 change 423 / 167 / 0 / 796. The 788 by cause (all keys listed with a scratch harness over `usages::resolve`, not only `-v`'s 20): 580 on a library receiver (`Logger#info` 45, `Logger#error` 44, `LoggerFactory#getLogger` 17, `HashSet#<init>` 15, `ResponseEntity#ok` 15, …), 165 on an unknown receiver (`?#build` 35, `?#collect` 13, `?#map` 10, `?#append` 8, …: library chains and untyped lambda parameters; only 13 are named like an indexed method — `?#getMessageType` 3, `?#getId` 2, `?#getKey` 2, the pactum `BusinessFeature` getters 4, `?#getPrivileges`, `?#toWebResponse` —, almost all on an untyped lambda parameter, an upper bound for indexed callees lost there), 35 on an indexed type lacking the member (inherited from a library class or `Object`: `CronusConnector#get` 5 from `AbstractServiceConnector`, `…Cache#findByKey` 4 from Spring Data, `MessageConsumer#getClass` 4, …), 8 unqualified in a class extending a library type (`this#getMessageId` 2 from `MessageBase`, …). 6.3 after the review (D-dk): 1386 call sites — 423 resolved to a member (31 %; a `call` or constructor `instantiate` edge, recursion included), 167 implicit (Lombok accessors and builder methods, enum members, annotation elements, undeclared constructors: a `reference` / `instantiate` to the type only), 0 ambiguous, 796 unresolved (the four annotation elements moved to implicit). Before the review: 1386 call sites (method calls, constructor calls, `super(..)`/`this(..)`, method references) — 586 resolved (42 %), 0 ambiguous, 800 unresolved: ≈ 580 on a library receiver (`Logger#info` 45, `Logger#error` 44, `LoggerFactory#getLogger` 17, `HashSet#<init>` 15, `ResponseEntity#ok` 15, …), 173 on an unknown receiver (`?#build` 35 — `ResponseEntity`/AWS builders —, `?#collect` 13, `?#map` 10: library chains, untyped lambda parameters, `Bearer<T>#getPayload()` returning `T`), 39 on an indexed type without the member (inherited from a library class or `Object`: `CacheableOrganizationUserRelation#getKey`, Spring Data `…Cache#findByKey`, `getClass()`), 8 unqualified / `super` (a library superclass). Before two fixes during the step: 31 ambiguous (a generic interface method not seen as overridden, a `String` parameter accepting any library type, JDK results unknown), then 14, then 0 |
| Usage golden set: precision / recall of direct users | 6.4 | `.annatar-local/usages-argus.toml` (D-dm), after the 6.4 review: 54 symbols (8 types, 46 members), 107 true direct users, 14 symbols with `overridden_by` (10 overriders). **Overall P 1.000 R 0.972 (104/107 users found, 0 extra)**; types P 1.000 R 1.000 (54/54, 8 symbols); **members P 1.000 R 0.943 (50/53, 46 symbols)**; **random only (symbols 20–27 and 30–54, 33 symbols) P 1.000 R 1.000 (23/23, 0 extra; 15 symbols without users, none given an extra)** — draw 1 (20–27, not reproducible) 7/7, draw 2 (30–54, `usages-argus-draw.py`) 16/16; chosen only (1–19) P 1.000 R 1.000 (80/80; members 29/29); lambda cases (28–29) R 0.250 (1/4); overriders P 1.000 R 1.000 (10/10). The review's own independent check (10 methods by `random.Random(2026).sample` over the method rows, users read off the edges and verified by grep/reading) found 7/7 users, 0 extra. The review fixes changed no `argus` edge (1200 rows identical by fqn, kind, line). Before the review (29 symbols): P 1.000 R 0.967 (88/91; types 54/54, members 34/37), overriders 10/10. Before the #48 change: P 1.000 R 0.934 (85/91; types 52/54, members 33/37). Per symbol (P / R, hits/expected; `argus` package prefix dropped): T4 `SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage)` 1/1; `SecurityDataAddEventConsumer` 4/4; `SecurityDataEventDispatcherConfig#getSecurityDataAddEventConsumer()` 1/1; private `SecurityDataEventDispatcher#dispatch(..)` (via `this::dispatch`) 1/1; `SecurityMethodAddMessage` 3/3; Lombok `SecurityData` 7/7 (5/7 before #48); Lombok/builder `CachedSecurityMethods` 3/3; `cronus.user…MessageConsumer` 3/3; interface method `MessageConsumer#perform(T)` 1/1, overriders 3/3; private `UserMessageDispatcher#dispatch(MessageBearer)` (from a lambda) 1/1; `UserTokenService` 13/13; overload `UserTokenService#removeAllTokens(List<UUID>)` 1/1; interface method `TokenStoreStrategy#removeAllTokensForUserIdExceptOfGivenJwtIds(..)` 1/1, overriders 1/1; `AbstractCache` 18/18; inherited `AbstractCache#refresh(K)` 7/7; constructor `TechnicalException#<init>(Throwable)` 4/4; constructor `TokenInvalidationEvent#<init>(String)` 3/3; generic `UserDataMessagePublisher#sendAsync(T)` 7/7; `UserDataMessageBearer#getMessageType()` (bounded `T`) 1/1 (0/1 before #48); random: `SecurityDataEventDispatcherConfig#getQueueName()` 1/1, `AuthDataCachePreWarmThreadPool#authDataCachePreWarmThreadPool()` 0/0 (no users, none found), `IdentificationDataRemoveMessage` 3/3, `UserData#getFirstName()` 0/0, `UserToken#setType(TokenType)` 1/1, `DynamoDBEvent#getEventType()` 1/1 (overriders 0/0), `AbstractCache#getCachePrefix()` 1/1 (overriders 6/6), `DynamoDBEvent#getEventId()` 0/0 (overriders 0/0); lambda cases: `AuthDataEvent#toWebResponse()` 0/1, `UserToken#getId()` 1/3; draw 2: every symbol with users 1/1 or 2/2 (`UserTokenBearer#getValidTokens()`, `UserToken#getType()`, inherited protected `DynamoDBEventService#retrieveEventsSortedByCreatedDate(String)` 2/2, `authorization.event.AuthDataEventService#getEventsSince(String)` beside the interface of the same name, package-private `IdentificationDataEventDispatcherConfig#getQueueName()`, a static `from(..)`, the `UUID` overload `UserTokenService#removeAllTokens(UUID)` 2/2, the `String` constructor of `PersonOrganizationRelationCacheUpdateEvent`, `CivesConnector#createSystemJwt(..)`, protected static `createRoles(..)`, `new UserDataCreatedMessage(..)` 2/2, two private methods), the 12 without users 0 found. No false positive in any symbol |
| Misses by cause (lambda, library chain, generics, other) | 6.4 | After #48: **3 misses, all lambda** — `AuthDataEvent#getWebListResponse(..)` → `toWebResponse()` (`events.forEach(event -> … event.toWebResponse())`), `UserTokenBearer#removeTokenById(String)` → `UserToken#getId()` (`validTokens.removeIf(token -> token.getId()…)`), `UserTokenService#isTokenValid(..)` → `UserToken#getId()` (`getValidTokens().stream().anyMatch(userToken -> …)`, a library chain as well); library chain alone 0, generics 0, other 0; **0 false positives**. Before #48: 6 misses — lambda 3 (the same), generics 3 (`getPayload()` returning `T` on `SecurityMethodAddMessage` / `SecurityMethodRemoveMessage extends SecurityDataMessageBearer<SecurityData>` → the two consumers' `SecurityData` references; `message.getMessageType()` on `<T extends UserDataMessageBearer>`). All three lambda misses need a receiver's own type argument (`List<UserToken>`, `List<AuthDataEvent>`) and the lambda parameter's type from a library functional interface (open #49) |
| Edge stage time (benchmark, `argus`) | 6.2 / 6.3 / 6.4 | 6.4: `argus` 11 ms (release, overrides and substitution included); benchmark (release) ≈ 5 ms per run (no overrides in it). 6.3 after the review: `argus` 11 ms, benchmark ≈ 26–27 ms. 6.3: `argus` 10–11 ms (release); benchmark (200 files, 1000 edges, debug) ≈ 25 ms per run. 6.2: `argus` 7 ms (release, 162 files, `index --offline` 2.2 s wall in all); synthetic benchmark (200 files, debug) ≈ 12 ms per run |
| `show` list sizes on `argus` (654 symbols, `-k 100`) | 6.5 | after the review (members' `overrides` out of type roll-ups, no type `instantiate` beside its constructor's): `used by`: empty 230, median 1 entry, > 20 entries 2 (max 29, `DynamoDBEvent`; `AbstractCache` 18); `uses`: empty 289, median 1, > 20 4 (max 27, `RoleRelationsUpdatedMessageConsumer`); ≈ 14 ms per `show` including process start, so the default cap 20 cuts 2 + 4 lists. Before the review: `used by` > 20 for 5 (max 36, `AbstractCache`, half of it its subclasses' `overrides`), `uses` > 20 for 5 (max 31) |
| `trace` sizes on `argus` (654 symbols) | 6.6 | after the review (D-eb shallowest expansion), `-k 10`: **default `--depth 6`**: only the root line (no callers) 237, median 3 lines / 538 bytes, p90 37 lines / 8.4 KB, max 75 lines / 16.6 KB (`DynamoDBEvent`), > 20 lines 109, 1.46 MB in all; a `@Scheduled` reached in 238 traces, cut by the depth 36 (33 of them on a `…Dispatcher#` line); a `… N more` in 20; a `(see below)` in 44 (76 lines). By depth (`-k 10`; reaching `@Scheduled` / cut / cut on a dispatcher line / p90 / max / total): 4: 165 / 136 / 92 / 7.2 KB, 27 L / 13.6 KB, 56 L / 1.20 MB; 5: 206 / 76 / 60 / 8.1 KB, 33 L / 15.7 KB, 68 L / 1.37 MB; **6: 238 / 36 / 33 / 8.4 KB, 37 L / 16.6 KB, 75 L / 1.46 MB**; 8: 261 / 2 / 2 / 8.8 KB, 38 L / 19.9 KB, 82 L / 1.50 MB; `--depth 10 -k 100`: 262 / 0, max 103 lines / 23.6 KB (`DynamoDBEvent`), no trace reaches depth 10. ≈ 235 bytes per line, most of it the full path and fqn (#51). The T4 trace 830 bytes, 4 lines; the type `SecurityDataAddEventConsumer` 1947 bytes, 8 lines; `DynamoDBEventService#publishAsync(DynamoDBEvent)` 43 lines / 9.4 KB (all unchanged from depth 4). Before the review (depth 4, `cut` re-expansion only at the limit): reaching `@Scheduled` 162, cut 141, same p90 / max; `--depth 4 -k 20` max 84 lines / 20.6 KB. Before D-dy (type nodes followed, depth 4): max 75 lines / 17.6 KB (`AuthDataEvent`), > 20 lines 97. ≈ 8 ms per trace including process start |
| T4: dispatcher named / full marks / cost vs 5.5 | 6.7 | **dispatcher 5/5 (5.5: 3/5, Fisher p = 0.44; 5.4 symbols 1/5, p = 0.048), full marks 5/5 (2/5, p = 0.17; symbols 0/5, p = 0.008; 4/5 under a strict reading of T4-with-5, p = 0.52 / 0.048), fresh read 5/5 (3/5)**; per run tokens 266 / 174 / 402 / 204 / 191 k, mean 247 k (275 k, −10 %, MW p = 0.42), cost $0.214 (0.200): no measurable increase (mean +7 %, median −8 %, p = 1.0), tool calls 13.2 (14.8, p = 0.41); exploratory: Reads 3.6 (6.2), search 3.4 (3.6), `show` 6.4 (3.4, p = 0.12), `trace` 0, cache creation 35 k (29 k, +21 %, the larger `show` output), wall 33 s (37 s). Dispatcher first seen in the consumer's `used by` in 3 runs, in an incidental `reference` (`LOG_PREFIX`) in `AuthDataEventService`'s `used by` in 1, in a search result in 1. T1–T3 once each: score 2, 72.8 / 78.0 / 74.8 k tokens. `show` output read per T4 run 46.7 KB (26.3 KB), usage sections 14.9 KB, package prefixes in them 4.3 KB; no `trace` call, so no run hit the depth. Spend $1.28 |

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
| D-dl (6.3a) | The split follows the repo's layout, not the `mod.rs` the 6.3 review's Next named: `src/usages.rs` stays the module root (docs, API, `resolve`) with `src/usages/{resolver,members,walk,expr}.rs` as private submodules, like `src/symbols.rs` + `src/symbols/facts.rs`. `FileWalk` (≈ 1400 lines) is split once more: the walk, scopes and type mentions in `walk.rs`, expression typing and calls in `expr.rs` (a second `impl FileWalk`). Siblings, not nested modules; only what another file uses is `pub(super)`. Tests move with the code they exercise (all are fixture-driven through `resolve`, assigned by topic); the fixture helpers (`resolve_all`, `edges`, `calls`, ..) stay in `usages.rs`'s `tests` as `pub(super)` | `src/usages/mod.rs`; one `walk.rs` with all of `FileWalk`; nested `walk::expr` (no `pub(super)` on `FileWalk`'s fields); a shared `tests.rs` | Matches the neighbouring module; four files of 0.8–1.2k lines (a third to a half tests) instead of one of 4.3k; 6.4 adds `overrides` edges next to `overrides()` in `members.rs`. Inside item bodies nothing changed; outside them, only the `pub(super)` markers, the per-file imports, the `#[cfg(test)] mod tests` blocks, the second `impl` blocks and three signatures rustfmt reflowed |
| D-dm (6.4) | **The usage golden set is built by hand, not from an IDE's *Find usages*** (the plan's source): no IDE runs in this environment (the IntelliJ MCP server was not connected). Per symbol: an exhaustive grep of `backend/src/main` for its simple name and for the variables of its type (fields, locals, parameters — inherited methods are called through subclass fields), every hit read and judged by Java's rules (receiver's static type, overload, which of three same-named `MessageConsumer`s), the enclosing symbol taken from the index's `symbols` rows (lines only, never `edges`) — all before the edges were looked at. **Direct user** = the source of an edge (D-cv): the innermost method or constructor around a mention (lambdas, anonymous classes included), else the type (field, header, annotation); every kind but `overrides` counts; for a type, its members' and nested types' users roll up and the sources inside the type are left out (`usages.md`, "a type's used by"), so a subclass `extends` and a call of an inherited method both count. Overriding is not use: the overriders are a separate, optional `overridden_by` list scored on its own. Selection: 19 symbols chosen to cover the plan's list (types, public/private methods, interface methods, a Lombok getter, the T4 consumer) plus generics, overloads and constructors, 8 drawn at random from the index's methods and types (seed 64 — the population and tool were not recorded, so this draw cannot be reproduced), 2 added as lambda cases, and (6.4 review, M2) 25 methods and constructors drawn reproducibly: `.annatar-local/usages-argus-draw.py` (beside the set) takes every `method` and `constructor` row of `argus`'s `index.db` (argus `8f4664a4`), sorted by fqn, minus the set's symbols 1–29, and prints `random.Random(6404).sample(population, 25)`; their users were found by hand the same way, before any edge was read, and every drawn symbol is kept, with or without users (12 of 25 have none: framework entry points, Jackson setters, interface-only callers). **The headline rests on the chosen symbols**: they give 80 of the 107 users (54 of them two type roll-ups, mostly field and parameter `reference`s, the easiest edges); the 33 random symbols give only 23 users, so Measurements reports all, members, types, random-only, chosen-only and lambda-only apart, and the members and random lines are the telling ones. Scores are pooled (micro: each symbol–user pair once; a user under two symbols counts twice) | Wait for an IDE; take the resolver's output as ground truth and review it; only random symbols | A ground truth read off the edges would measure nothing; grep plus reading is what *Find usages* does for code that compiles, and its blind spots (reflection, YAML) are the edges' too (`usages.md`, not visible). The risk is a human miss in the truth — every extra the evaluator reports is a prompt to re-read the code (none came) |
| D-dn (6.4) | **`overrides` edges go to the nearest overridden methods**: from a method (with a row, not a constructor, static or private) to each method in its `lineage` it overrides that no other overridden method overrides in turn — JLS 8.4.8 makes `C#m` override `I#m` too, but the chain `C#m → B#m → I#m` holds the same information and keeps `via` (6.5) and `trace` (6.6) one step per level; two unrelated super types (a superclass and an interface it does not implement) each get an edge. "Overrides" = same name and parameter count, each parameter's resolved type equal once the farther type's type variables are replaced by the type arguments of the super-type path (`Resolver::type_args`, BFS from the subtype; an argument that is itself a type variable of the type below is looked up there); each side compared as a `Sig` (6.4 review, H1/H2): a resolved type, the method's own type variable by position (`<U> put(U)` overrides `<T> put(T)`), a type variable of the nearer type (`type_args` keeps a variable of the subtype as `Arg::Var`, so `Flipped<P> extends Pair<Sms, P>`: `put(Sms, P)` overrides `Pair#put(A, B)`, `put(Sms, Mail)` does not), or *any* — a variable the subtype leaves without an argument (raw super type, wildcard) or a type the run cannot resolve; *any* matches everything, a variable of the farther method matches any type of the nearer one (its erasure is not known), otherwise the two must be equal — so a variable of the nearer method or type never matches a concrete type (`<U> put(U)` and `Box<X>#put(X)` are overloads of `Base#put(String)`, as in Java); a varargs parameter counts as an array (`m(String...)` overrides `m(String[])`, not `m(String)`; M1); a package-private farther method outside an interface is overridden only from its own package (`MethodFacts` gains `is_package_private`; L2); header type arguments resolve around the type, like its super types (L3); generic arguments of a parameter's own type are not compared (Java forbids two methods with the same erasure anyway). The same `Resolver::overrides` replaces 6.3's simple-name rule in `methods()`. Line: the method's name line (`MethodFacts::line` now points at the name, after `@Override`); `MethodFacts` gains `is_private` | Edges to every overridden method (transitive closure); simple-name comparison as in 6.3; the declaration's first line | Nearest edges are what `via I#m` names and what a recursive CTE walks; resolved types tell the three same-named `MessageConsumer`s apart where simple names would not; on `argus` 50 edges, all correct, every `@Override` with an indexed target covered (Measurements) |
| D-do (6.4) | The evaluation is a command, **`annatar eval-usages <set.toml>`**, in a new module `src/usage_eval.rs` (set parsing and validation, the index check, scoring and report together), next to `eval` / `golden` and in their style: the check runs first and any missing or wrong-kind fqn is the error (reusing `golden::IndexProblem`), per-symbol lines then pooled summaries; it reads only `index.db` (no `[ollama]`), takes no `--path`. The users query (`usage_eval::users`: incoming non-`overrides` edges, a type rolled up over `parent_id` by a recursive CTE, sources inside it excluded) is the one 6.5's `used by` needs | An `#[ignore]` test reading env vars (as `real_golden_set_matches_the_real_index`); a flag on `eval`; extending `golden.rs` | `eval` needs Ollama and scores retrieval, a different set format; a command is how `project.md` measures quality (the golden-set loop) and runs in a second on a warm index, so every later resolver change (6.5+, SCIP) is re-measured the same way |
| D-dp (6.4) | **Open #48 decided on the measured misses: implemented the part that is cheap and precise, recorded the rest.** Of the 6 golden-set misses before the change, 3 were generics: 2 a super type's type argument (`getPayload()` returning `T` on `SecurityMethodAddMessage extends SecurityDataMessageBearer<SecurityData>`), 1 a bounded method type variable (`<T extends UserDataMessageBearer> sendAsync(T message)`). Done: inherited members on a subtype take the type arguments of the super-type path (`Resolver::declared_via` / `declared_on` over `type_args`, the machinery `overrides` needs anyway) for return types, fields and Lombok/record accessors; a variable or parameter typed by a **method-level** (or other member-region) bounded type variable takes its first bound (`Region::bounds` from the tree's `type_bound`; the bound is the erasure, as precise as a declared type). `argus`: +8 edges, all correct, recall 0.934 → 0.967, precision unchanged. Not done: the type arguments of a receiver's own declared type (`Box<Item> box; box.get()`, `List<UserToken>`) — `Ty` carries no type arguments, and on `argus` they only matter together with lambda parameter inference (the 3 remaining misses); class-level bounds (`class Box<T extends Msg>`, not seen on `argus`) | Leave #48 to SCIP; a full generic `Ty` (type arguments on every expression type) now | The super-type part fixes every generics miss measured, at ≈ 60 lines and no facts change beyond `is_private`; a generic `Ty` plus lambda typing from library functional interfaces is a type checker's job (SCIP, Later) and buys 3 users of 91 on `argus` (open #49) |
| D-dq (6.4 review) | The 6.4 review's findings, handled as follows. **H1/H2/M1/L2/L3** fixed in `Resolver::overrides` and `type_args` (D-dn as corrected), each with a fixture test (`<U> put(U)` and `Box<X>#put(X)` against `Base#put(String)` with the calls going to `Base#put`; `Flipped#put(Sms, P)` overriding and `put(Sms, Mail)` not; `log(String...)` vs `log(String)` / `all(String[])`; package-private `m()` across packages; a header argument naming the outer `Message` beside a nested one). **L1**: a type variable declared twice in one region (a method of an anonymous class) gets no bound — the simplest rule that never types a variable by another's bound; taking bounds per declaring member would also keep the inner one, not worth the scope tracking. **M2**: the set gained a reproducible draw and random-only scores (D-dm, Measurements); the old seed-64 draw is kept and labelled not reproducible. **L4** (recall: `super.get()` through a generic super type; an unrelated super type's method hiding an interface method on a multi-parent receiver) recorded in `usages.md` (where resolution stops), not fixed: both need the receiver threaded through `methods()` / `overrides`, and neither occurs on `argus`. **L5**: the summaries say "each symbol–user pair"; a missing fqn names its entry as `symbol N`; not done: rejecting a type's `users` entry that lies inside the type (a silent permanent miss — the hand-built set has none) and an override pair in the sample-project fixture (it has none; the unit test scores overriders on a synthetic index) | Fix L4 now; keep the unreproducible draw unlabelled; drop the seed-64 draw | L4 is recall-only and absent from `argus`; the old draw's symbols are still correct truth, only its selection cannot be repeated |
| D-dr (6.5) | **`via`: a method is used by the callers of every method it overrides, transitively up the `overrides` chain** (`C#m → B#m → I#m`: callers of `B#m` and of `I#m`), each listed `via` the method they call; only non-`overrides` edges into those methods (siblings overriding the same method are not users); a type's roll-up includes its members' `via` callers; callers inside the shown symbol are left out like any other internal edge. A shown method's incoming `overrides` edges are listed in `used by` as `overrides` (the methods overriding it), its outgoing ones in `uses`; **amended in the review (F2):** a type's roll-up leaves out the `overrides` edges into its members (and, in `uses`, out of them) — the subtype's `extends` / `implements` line already says it, and on `argus` they were 18 of `AbstractCache`'s 36 entries, pushing all 8 consumers that call `refresh` past the cap | Only the nearest overridden method (one step, as the edges are stored, D-dn); `via` only on methods, not in a type's roll-up; `overrides` left out of `used by` as in `usage_eval::users`; members' `overrides` kept in a roll-up but ordered after the usage kinds (review alternative: the list stays twice as long) | Dynamic dispatch: a call to `I#m` can run any override below it, whatever the depth, and on `argus` the cronus consumers are called only through `MessageConsumer#perform(T)` (`UserMessageDispatcher#performMessage` :123) — without `via` their `used by` would be empty. Listing overriders gives an interface method its implementations, which a flow trace needs next (`project.md`: "links to every place that uses it"); the eval keeps scoring them apart (`overridden_by`), so its numbers do not change. `trace` (6.6) follows the same chain |
| D-ds (6.5) | **A type's `uses` rolls up its members' and nested types' outgoing edges**, without the ones ending inside the type, the mirror of `used by`'s roll-up; grouped by the used symbol's file, with the lines in the shown type's file. Review (F2, F3): without its members' `overrides` (the type's `extends` / `implements` says it), and an `instantiate` of a type is left out when the same source and line also instantiates one of that type's constructors (`new T(..)` with a declared constructor stores both edges); `used by` needs no such rule, the two edges merge there | Only the type's own edges (header, fields, annotations) | `project.md` promises "direct `used_by` and `uses`" of a symbol: the roll-up is still direct (no transitive hop), and a type's own edges alone miss everything its methods call — the dependencies an agent or engineer asks about; children are printed without lists (D-dt), so the roll-up is the only place they show |
| D-dt (6.5) | **Format and cap:** the lists belong to the shown symbol only (not its children, which would repeat them), after its tickets and before the children, as `- used by:` / `- uses:` field lines; per file its path, then one line per symbol and link (`fqn`, the kind unless `call`, `ambiguous`, `via I#m`, `:line, line`, `[entry: @A]`), files by path, lines by first line; an empty list is `- used by: none in main sources` (review F6: test code is not indexed, D-dv; a bare `none` reads as dead code) or `- uses: none`. **`uses` is titled `- uses (lines in this symbol's file):`** (review F1): its file headers are the used symbols' files, its lines the shown symbol's, so an agent would otherwise open `SecurityMethodsCache.java:20` for a mention on line 20 of the consumer; one title per list costs less than ` at` on every line. Each list is capped at **20 entries** (lines with a symbol, not files) with `… N more`; `-k`/`--limit N` (1–100) changes it — the flag `search` and `eval` use for "how many" | Cap by files; no flag (fixed cap); lists under every child; target member named in a type's roll-up; `uses` lines as ` at :20` per entry, or `uses` grouped by line | 20 keeps the default output short — on `argus` only 2 + 4 of 654 lists are longer after the review (Measurements) — and `-k` lets an agent see the rest without a new convention; the error and range follow `search -k` (exit 2). Naming the member a type's user reaches would lengthen every line; open #50 |
| D-du (6.5) | The read side of the edges lives in **`src/usage_query.rs`** (`used_by`, `uses`, `Usage`, `entry_point`, `ENTRY_ANNOTATIONS`), apart from the resolver (`usages`) and the eval (`usage_eval`), so `trace` reuses the CTEs and the entry-point test. Entry points: method annotations by simple name (qualified or with arguments), `@Scheduled`, `@RequestMapping`, `@Get/Post/Put/Delete/PatchMapping`, `@EventListener`, `@ExceptionHandler`, `@PostConstruct`, `@PreDestroy`, `@Bean`, several joined (`@ExceptionHandler` and `@PreDestroy` added in the review, F5); `public static void main(String[])` (or `String...`) by fqn and signature; types never | Detection in `show`; by fqn suffix only; Spring's meta-annotations (`@Schedules`, `@TransactionalEventListener`, listener annotations of messaging libraries such as `@KafkaListener`/`@SqsListener`, none on `argus`) | The list is `usages.md`'s plus `@ExceptionHandler` (the framework calls it on an exception, no caller in code) and `@PreDestroy` (the partner of `@PostConstruct`). On `argus` the framework-called methods are 10 `@Scheduled`, 14 `@Bean`, 11 `@PostConstruct`, 25 mappings, 2 `@ExceptionHandler`, 1 `main` (the developer's "`argus` uses only these" missed the two `@ExceptionHandler`s, review F5); no meta-annotations. An annotation by simple name can be a project's own of that name — acceptable for a label. `usage_eval::users` keeps its own query (excludes `overrides`, no `via`), unchanged |
| D-dv (6.5, open #45) | **#45 decided: test callers are not indexed in Phase 6**; they belong to the Later test-code indexing item (`plan.md`), first as sources of usages only (rows marked as test, never described, embedded or searched; `used by` / `trace` list them apart). `reference.md` says test callers never show | Index test files for edges now | `edges.src_id` references `symbols`, so test sources need symbol rows, which `search`, `describe` (LLM cost), `eval` and the golden sets would all have to skip; that is a schema and pipeline change beyond 6.5–6.7. `project.md`'s "what a change can break" arguably includes tests, but the T4 gap (#42) is a main-code caller; 6.7 shows whether main-code usages suffice |
| D-dw (6.5 review, F4) | **Usage lines keep full fqns, `via` included** (`via com.acme…MessageConsumer#perform(T)`, not `via MessageConsumer#perform(T)`); `plan.md` 6.5 now reads `via <fqn of I#m>`. The token cost of the repeated package is open #51 for 6.7 | Members relative to the file's top-level type, as `search`'s file view does (−28 to −35 % bytes on `argus` lists); only `via` shortened, as the plan's `via I#m` wording had it | Every printed symbol is something an agent copies into `show` / `trace`, which need the exact fqn (AGENTS.md: symbols are identified by fqn in the CLI); a relative `via` would be the one name in the output an agent cannot paste. Whether the bytes matter is measured in the 6.7 trial, not guessed |
| D-dx (6.6) | **`trace` walks the tree in Rust, one SQL query per printed symbol** (`usage_query::callers`: `used_by`'s recursive `inside` / `overridden` CTEs with a kind filter, the roll-up only for the root), depth first with an explicit stack; not one recursive CTE over the whole tree as `plan.md` 6.6 had it (now reworded) | A recursive CTE yielding (depth, path, caller) rows, the tree rebuilt from them in Rust; a CTE for the reachable set plus per-node queries | The output's rules are about print order: "each symbol once, a repeat `(see above)`" is the first occurrence depth first, the cap is per printed symbol, a depth-cut symbol is counted and expanded where it recurs higher up, entry points and types stop a branch. A CTE enumerating paths needs path strings for cycle safety and grows with every path (diamonds multiply), and its rows would still be re-walked in Rust to apply those rules. The per-symbol query is the 6.5 statement (shared `via` and roll-up logic, no second SQL dialect of the same rules); a trace costs one query per printed line (≈ 8 ms per trace on `argus` with process start) |
| D-dy (6.6) | **A type traces its members: the root type's callers are rolled up as `show`'s `used by`** (its members' and nested types' incoming `call` / `instantiate` / `reference` edges from outside, `via` callers included); **below the root a type that only references the symbol above is a leaf, `[type, not followed]`**: a type mentioning the symbol above (a field, a header type argument, an annotation) is shown with its lines, its own users are not traced. **Amended in the review (F3):** a type with a `call` / `instantiate` link of its own to the symbol above (a field initializer, an initializer block) is marked `[initializer]` and followed through the code constructing it (`usage_query::constructions`: `instantiate` edges into the type, `call` / `instantiate` into its constructors, from outside them) — the initializer runs whenever the type is constructed. Not followed: a subclass's construction (implicit `super()`), static initializers run at class load (both read as constructions or not at all) | Root as a tree of its members, each traced (names the member each caller reaches, #50, but prints every member without callers and the internal calls); a type below the root traced through its own incoming edges (no roll-up, the 6.5 review's suggestion) or rolled up | `project.md`: transitive callers, "what a change can break" — for a type, the users of any of its members, as `show` lists them. A type node's own users only hold it (fields, constructor parameters: the injection graph); the members that use the field reach the symbol through their own `call` edges and appear as callers on their own (the T4 type: `SecurityDataEventDispatcherConfig reference :24` is a leaf, its getter :45 leads to `dispatch`). Following them on `argus` printed injection chains (`AuthDataEvent`: 75 lines → 50, max over all symbols 17.6 → 13.6 KB) and no caller a member did not already show. An agent wanting the type's users runs `trace` on it. Initializer code is different: it executes on construction, so its blast radius is whoever constructs the type (`project.md`: "what a change can break"); a distinct leaf label would name the gap but leave it. On `argus` type-sourced edges are only `reference` / `extends` / `implements` (0 initializer edges), so the `argus` measurement behind this decision rests on reference links alone and the output there is unchanged |
| D-dz (6.6) | **Format:** root `fqn [kind] file:start-end` (+ `[entry: @A]`, or `[no callers in main sources]`); one line per caller symbol, two spaces per level: `fqn`, then its links (kind unless `call`, `ambiguous`, `via <fqn>`, `:lines`) joined by `; `, the caller's file on the first link **only when it differs from the file of the symbol it calls** (the line it is indented under; a chain inside one file reads `:67`, `:59`, as the design's example; wording fixed in the review, F5 — a later sibling compares with its callee, not with the line printed above it, so siblings from one file each print the path); `[initializer]` and `[entry: @A]` next, the branch's end marker last (entry points: D-ec) | One line per link (as `show`; a caller with a direct and a `via` call would print twice and its subtree once); the file on every line; the file name only; the file once per group as in `show`'s headers | One line per symbol keeps "each symbol once" literal and the tree a tree; the full path is what an agent opens, the elision saves it on same-file hops (dispatcher chains) without making a line ambiguous — the callee is the line the caller is indented under, whose file is on its own line or above it. Full fqns as D-dw (pasteable into `show` / `trace`); token cost with #51 |
| D-ea (6.6) | **`-k` caps callers per symbol** (each printed symbol's list, `… N more` under it), the flag meaning "entries per list" as in `show`; **default 10**, not `show`'s 20; `--depth` 1–10, out of range a usage error like `-k`; **default depth 6** (review F2, closes #52; `plan.md` had 4) | A cap per tree level (all symbols at one depth); default 20 as `show` | A trace multiplies lists: at 20 the largest `argus` trace is 84 lines / 20.6 KB, at 10 56 / 13.6 KB, and only 20 of 654 traces hit 10 (none 20). Per level would cut a deep branch for a wide sibling. Depth (measured after D-eb, `-k 10`, 654 symbols): 4 reaches `@Scheduled` in 165 traces and cuts 136 (92 on a dispatcher line — a symbol two calls below a consumer needs 5–6 hops to its `processMessages`), 6 reaches it in 238 and cuts 36 (33), 8: 261 / 2; p90 7.2 → 8.4 KB, max 13.6 → 16.6 KB, median unchanged (3 lines). 6.7's agents use the default, so deciding after the trial would measure the wrong setting; 8 adds 23 traces for +20 % max bytes |
| D-eb (6.6 review, F1) | **Each symbol is expanded at its shallowest occurrence**: a breadth-first pass from the root (same `-k` caps, one memoized query per symbol, also for the symbols at the depth limit to count their callers) records each symbol's shallowest level; the depth-first print expands a symbol at its first occurrence on that level, prints `(see above)` for a later copy and `(see below)` for an earlier, deeper one; the `cut` set (re-expansion only of a symbol sitting exactly at the limit) is gone | Store the remaining depth per expanded symbol and expand again when a repeat has more (correct, but prints shared subtrees twice); the first occurrence depth first as before | The first-occurrence rule let the first-listed branch decide how deep a shared symbol was expanded: a symbol expanded one level above the limit had its callers cut, and its copy at level 1 read `(see above)` — 26 `argus` traces hid callers within the depth, among them a cronus dispatcher's `@Scheduled` (the T4 chain 6.7 grades). By induction every symbol within the depth is expanded once, at its shallowest level, so the depth never hides a reachable caller and `[N callers beyond depth D]` appears only on a symbol really at the limit; the query count is unchanged |
| D-ec (6.6 review, F4) | **An entry point is labelled on every copy and followed on when code calls it**: `[entry: @A]` before `(see above)` / `(see below)` / the children; an entry point without callers in main sources ends the branch with just the label (no `[no callers in main sources]`), one with code callers (an inter-bean `@Bean` call, a controller method reused internally) is traced like any other symbol | Stop at every entry point (design "trace stops there"), silently dropping code callers; stop and count them (`[entry: @Bean; N callers not followed]`) | `project.md`'s blast radius: a change to the method also breaks its code callers, which the framework entry does not replace. On `argus` no entry method has an incoming `call` / `instantiate` / `reference` edge, so every entry still ends its branch there and the output only gains the label on repeats (an agent grepping for `@Scheduled` finds every copy) |
| D-ed (6.7) | **The 6.7 *with* prompt is the 5.5 `files` prompt with one descriptive bullet inserted before its last line** (`run.py --with-prompt usages`): `show`'s `used by` / `uses`, `trace` with its default depth and entry-point marks, and what usages cannot see; no instruction to check who calls a hit. Fixed before any run in the protocol amendment; only the *with* arm on T4 (5 runs) plus T1–T3 once | Add "before answering a flow question, check the callers" (the #42 prompt lever); describe `used by` inside the `show` bullet; re-run both arms | The question is whether the tool output closes #42, not whether an instruction does: an instruction would confound the two. Keeping the other lines byte-identical keeps the comparison with 5.5 to one change (the tool and its description). The *without* arm has no `annatar`, so nothing in it changed (as D-cs). *Caveat (review F5):* neutral in instruction, but its one example, `[entry: @Scheduled]`, is the annotation of the T4 target `processMessages`, and "message queues" in the limits names the T4 topic — mild cues; probably of small effect (no agent called `trace`, and `@Scheduled` is in the `show` / Read output anyway). A future re-run uses a neutral example, as the README's "(`@Scheduled`, HTTP mappings, …)" |
| D-ee (6.7) | **Verdict: usages close #42.** With `used by` in `show`, all 5 T4 agents named the queue dispatcher and got full marks (5.5: 3/5 and 2/5; 5.4 symbols: 1/5 and 0/5, p = 0.048 / 0.008), with cost not measurably different from 5.5 (mean +7 %, median −8 %, MW p = 1.0, n = 5; tokens −10 %); T1–T3 unchanged in correctness. The agents reached the dispatcher mostly through `show`'s `used by`, not `trace` (0 calls) | Call #42 narrowed until `trace` is used; keep it open for more runs | #42 is a correctness question and the dispatcher went to 5/5 (full marks 5/5, 4/5 under a strict reading of T4-with-5). The plan's cost criterion is met in the sense of no measurable increase, not strictly: the +7 % mean comes from cache creation (+21 %) from the larger `show` output (46.7 vs 26.3 KB per run), the real price of usages; vs 5.4 *without* T4 costs +13 % (n.s.) for −36 % tool calls. Vs 5.5 the gain is not significant at n = 5 (p = 0.44 / 0.17), vs the 5.4 symbol output it is. The intended mechanism, the consumer's `used by` naming `SubscriptionMessageDispatcher#performMessage … via MessageConsumer#perform`, shows in 3 of 5 streams; 1 found the dispatcher through an incidental `LOG_PREFIX` `reference` in another class's `used by`, 1 through search. `trace`'s value for agents is untested by T4 (one hop suffices) → #53. Caveats as D-ct: one task, 5 runs, the same kind of LLM grades, baselines ≈ 6.5 h older; and the prompt's mild cues (D-ed) |
| D-ef (6.7, #46) | **No usages in the `search` file view**; callers stay one `show` away | `used by N` per hit, or the callers' files under each hit | The agents called `show` on their own in every T4 run (6.4 per run) and read `used by` there; a count or callers on every search result would cost output on every query (T1–T3 never needed them) to save at most one call. `project.md` keeps search as the entry and `show` / `trace` as the drill-down |
| D-eg (6.7, #51) | **Usage lists keep full fqns** (D-dw stands) | Members relative to the file's top-level type, as `search` prints them | The package prefixes in the usage sections the T4 agents read were 4.3 KB per run (≈ 1.1 k tokens, < 2 % of a run's tokens even when re-read on later turns). The fqns the agents passed to `show` were mostly copied from earlier output (about half from usage lines), which a relative name would break; 2 typed ones for an inherited member failed (→ #54, not a naming issue). The usage sections do cost: the T4 cost rose +7 % (n.s.) through cache creation from the larger `show` output (D-ee), but the prefixes are a small part of it |

## Open questions

| # | Question | Raised at | Status |
| --- | --- | --- | --- |
| 45 | Test sources are not indexed, so `used by` and `trace` never show test callers ("who relies on it" stops at main code). Index test files for edges only (as sources of usages, not as symbols or descriptions)? | 6 plan | **decided (6.5, D-dv):** deferred to the Later test-code indexing item (edge sources only), nothing built in Phase 6 |
| 46 | Show usages in the `search` file view (e.g. `used by N` or the callers' files under a hit) so an agent sees the caller without a second command? Costs output size on every query | 6 plan | **decided (6.7, D-ef):** no — agents called `show` on their own in every T4 run (6.4 per run); 3 of 5 found the dispatcher in the consumer's `used by` |
| 47 | Callers in the description prompts (plan Later): a member's callers could sharpen what and why, but every prompt changes and the whole LLM cache misses (≈ 20 min on `argus`) | 6 plan | **decided (6.7):** stays in Later, nothing built — the trial measured navigation, which `used by` already fixed (D-ee); it gives no evidence that callers would improve descriptions, and the change costs a full cold run. Revisit with a description-quality measure |
| 48 | Substitute a super type's type arguments in inherited members (`SecurityMethodAddMessage extends SecurityDataMessageBearer<SecurityData>` → `getPayload()` returns `SecurityData`) and in receivers' generic types (`Box<Item>#get()`)? Today a type variable stops the chain (`usages.md`, where resolution stops): on `argus` every message consumer's `message.getPayload().getUserId()` is unresolved. Also a **bounded** type variable (`<T extends UserDataMessageBearer> void sendAsync(T message)` → `message.getMessageType()`): its erasure, the bound, is as precise as a declared type, but 6.1 keeps only the type variables' names (6.3 review L3) | 6.3 | **closed (6.4, D-dp):** super-type arguments are substituted in inherited members (return types, fields, Lombok/record accessors) and in `overrides`; a method's bounded type variable types its variables by the bound — the 3 generics misses of the usage golden set are gone (+8 correct edges on `argus`). A receiver's own type arguments (`Box<Item>#get()`) and class-level bounds stay out → #49 |
| 49 | A receiver's own type arguments and lambda parameter types (`List<UserToken> tokens; tokens.removeIf(t -> t.getId()…)`, `events.forEach(e -> e.toWebResponse())`): `Ty` keeps no type arguments and an untyped lambda parameter has no type, so the call is unresolved. All 3 misses of the 6.4 usage golden set (3 of 91 users), at most 13 of `argus`'s 1386 call sites. Model type arguments on `Ty` plus the parameter types of common JDK functional interfaces (`forEach`, `removeIf`, `stream().map/filter/anyMatch`), or leave it to SCIP? | 6.4 | **decided (6.7):** left to SCIP (Later) — no 6.7 answer or stream shows a caller missing for this reason; nothing built in Phase 6 |
| 50 | A type's `used by` line names the user and the lines, not which member of the type it uses (`…Dispatcher#dispatch(..) :76` under `SecurityDataEventDispatcherConfig` could be `getQueueName()` or the getter). Name the member (`→ #getX()`), at the cost of longer lines, or leave it to `show` of the member? | 6.5 | **decided (6.7):** leave it to `show` of the member — one case seen, n = 1: `AuthDataEventService`'s `used by` lists five `SubscriptionMessageDispatcher` methods as `reference` without the member they use (they log the `LOG_PREFIX` constant, exactly this ambiguity); not misread, but only because that agent did not need to know what was referenced. Weak evidence; revisit if a trial shows a misreading |
| 51 | Usage lists repeat the full package in every fqn under a file header that already contains it (`show` of `AbstractCache -k 100`: 6595 bytes, 4275 without the package, −35 %; `SecurityDataAddEventConsumer` −28 %). Print members relative to the file's top-level type, as `search`'s file view does, at the cost of names an agent cannot paste into `show` / `trace` unchanged (D-dw)? `trace` (6.6) prints the full path and fqn on most lines too, ≈ 235 bytes per line on `argus` (Measurements) | 6.5 review | **decided (6.7, D-eg):** keep full fqns — prefixes ≈ 1.1 k tokens per T4 run, agents paste them into `show` |
| 52 | `trace`'s default depth 4 cuts 141 of 654 `argus` traces (`-k 100`; 85 at 5, 46 at 6, 2 at 8, none at 10), and of the 669 cut lines in them most fall on the message dispatchers (pactum, cronus, cives, janus: `performMessage` / `dispatch` / `receiveAndDispatchMessages`, one or two hops below their `@Scheduled` `processMessages`) — the T4 pattern, for a symbol a few calls below a consumer — then `AuthDataService#preWarm(UUID)` (35). Raise the default (6 cuts a third as many, at more output) or keep 4 and let agents pass `--depth`? | 6.6 | **closed (6.6 review, F2): default depth 6** (D-ea): after D-eb, 4 → 6 raises the traces reaching `@Scheduled` 165 → 238 and lowers the cut ones 136 → 36 (dispatcher lines 92 → 33) for p90 7.2 → 8.4 KB; 6.7 records whether any trial trace still ends at the depth |
| 53 | No agent called `trace` in 6.7 (0 of 8 runs, though the prompt named it): on T4 one `used by` hop reaches the dispatcher's file and the rest is in that file. Does `trace` help agents where several hops are needed (a blast-radius task: "what breaks if this changes", `project.md`), or should `show`'s `used by` point to it? | 6.7 | open — Later: needs a multi-hop agent task; nothing built |
| 54 | `show` of a member inherited from a supertype, named through the subtype (`show '…AuthDataEventService#publishAsync(AuthDataCacheUpdateEvent)'`, declared in `DynamoDBEventService`), is "not found in the index": 2 of 32 `show` calls in the 6.7 T4 runs, both typed by the agent. Resolve it to the declaring type's member, or suggest it in the error? Agent friction (`project.md` agent quality) | 6.7 review | open — Later; nothing built |
