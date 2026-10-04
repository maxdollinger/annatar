# Annatar — Usages

Design of Phase 6: which symbol uses which, `used by` / `uses` in `annatar show` and `annatar trace`. The steps and their *done when* are in [`plan.md`](./plan.md) (Phase 6, authoritative for scope); progress, decisions (D-cu onward) and open questions (#45 onward) are in [`usages_state.md`](./usages_state.md); open #42, the reason for the phase, is in [`state.md`](./state.md). `project.md` describes the target (`edges`, `show`, `trace`).

## Why

The agent trial's one correctness loss came from a search hit standing in for its caller (open #42). On the flow-trace task (T4) the agents found the message consumer through `annatar search` and started there, leaving out the queue dispatcher in front of it: 4 of 5 runs with the symbol output, 2 of 5 with the file output, 0 of 5 without `annatar`. An agent that sees *who calls* a hit would not stop at it.

The dispatcher shows what the usages have to handle. `SecurityDataEventDispatcher` never names `SecurityDataAddEventConsumer`; it reaches it through its config object:

```java
// SecurityDataEventDispatcher.java
private final SecurityDataEventDispatcherConfig config;            // field, injected

void receiveAndDispatchMessages() {
  queue.receiveMessages(queueDescriptor, SecurityDataMessageBearer.class).forEach(this::dispatch);
}

private void dispatch(SecurityDataMessageBearer<?> message) {
  switch (message.getMessageType()) {
    case EVENT_MFA_METHOD_ADDED, EVENT_PASSWORD_ADDED -> config.getSecurityDataAddEventConsumer()
            .consume((SecurityMethodAddMessage) message);
    …
```

A text search for the consumer's name, or edges from type names alone, find `SecurityDataEventDispatcherConfig` (field, constructor parameter, getter return type) but not the dispatcher. Finding it takes a resolved call through a chain: the type of `config`, the getter's declared return type, then `consume` on that type.

## What counts as a usage

Symbol **S uses T** when S's source names T or one of T's members, and the name resolves, by Java's own rules, to a symbol in the index (D-cv).

| Kind | Constructs | From → to |
| --- | --- | --- |
| `extends` | `class X extends T`, `interface X extends T` | type → type |
| `implements` | `class X implements T` | type → type |
| `overrides` | a method with the same name and parameters as one in a superclass or interface, to the nearest one (6.4, D-dn) | method → method |
| `instantiate` | `new T(..)`, `T::new` | → type, and → `T#<init>(..)` when the arguments match |
| `call` | `x.m(..)`, `m(..)`, `T.m(..)`, `super.m(..)`, `super(..)`, `this(..)`, `x::m`, `this::m`, static imports of methods | → method or constructor |
| `reference` | every other mention of a type: field, parameter, return, local or `var` with a known initializer, cast, `instanceof`, `catch`, `throws`, generic argument (`List<T>`), `T.class`, annotation `@T` and its arguments, a constant `T.X` or a static import of one, a field of T read or written, a Lombok accessor | → type |

The type a member is reached through is a `reference` as well: `T.m()`, `T::m` and `T.X` name `T` (6.2), and 6.3 adds the `call` to `T#m`, so the type side holds even where the member does not resolve. An anonymous class `new T() { .. }` is an `instantiate` of `T`; `new T[n]` creates no `T` and is a `reference`; the header of a local or anonymous class is a `reference` from the member around it (`extends` and `implements` are type → type) (D-dc).

**Where an edge starts.** The innermost method or constructor around the reference. Code in a lambda, an anonymous class or a local class belongs to that member (those classes are not symbols, 1.3). Field declarations and their initializers, static and instance initializer blocks, enum-constant bodies, the class header (`extends`, `implements`) and class annotations belong to the type.

**Not a usage:**

- an `import` — it is only a pointer and can be unused; a resolved mention in the code says the same and more;
- `{@link T}` in Javadoc;
- a member calling itself (recursion);
- anything that does not resolve to an indexed symbol: JDK, Spring and other libraries (`com.haufe.aws…`). These are counted and logged, not stored.

**Inside a type.** Calls between members of the same type are edges: `receiveAndDispatchMessages → dispatch` is needed so `trace` can walk through private helpers. A **type's `used by`** is its own incoming edges plus those of its members and nested types, computed when read and without the edges that start inside the type itself — otherwise a class's users would drown in its internal calls.

**Fields are not symbols.** The index has types, methods and constructors only. Accessing a field of another type (`cfg.queueName`, `T.CONSTANT`) and calling a Lombok-generated accessor are a `reference` to the type that owns the field.

## How a link is made

Tree-sitter gives syntax, not meaning: it says "a call `consume(..)` on the result of `config.getSecurityDataAddEventConsumer()`", never what `config` is. The links come from a resolver that applies the compiler's name rules to the tree, against the symbol table of the whole run. **A name is never matched by its simple name across the repository**: it becomes an fqn first, then it is looked up. What cannot become an fqn makes no edge (D-cw).

### Inputs

- **The symbol table** of the run, keyed by fqn: every type, method and constructor (already in `symbols`). This is why resolution is a second pass after all files are parsed.
- **Per file** (6.1, kept in memory): the package; single-type, wildcard and static imports; the nested types.
- **Per type:** superclass and interfaces as written; fields with name, declared type, `static`, and Lombok annotations on the field or the type.
- **Per method:** the declared return type, parameter names and types, varargs.

### Type names

A simple type name in a file resolves in this order — the order `javac` uses:

1. a type nested in the super type of an anonymous or local class around the name, then in the current type or in a type around it (also inherited nested types from an indexed superclass); a type's header (`extends`, `implements`, type bounds) and its annotations are outside it, so its own nested types are not in scope there;
2. a single-type import (`import a.b.Foo;`);
3. a type in the same package;
4. a wildcard import (`import a.b.*;`);
5. otherwise unresolved: `java.lang`, a library type, or a name the run does not know — no edge.

A qualified name (`Outer.Inner`, `com.acme.Foo`) resolves its first part this way and walks down from there; the edge goes to the type it ends on (`Inner`), not to `Outer` as well — `show Outer` reaches it through the roll-up of nested types. In `outer.new Inner()` the name is a member of `outer`'s type: no type edge in 6.2 (counted unresolved), 6.3 knows the receiver.

**The same simple name in different packages** is the normal case this order settles. `argus` has four `MessageBearer` classes (`cronus.role`, `cronus.person`, `cronus.user`, `pactum`) and three `MessageConsumer` interfaces. `RoleRelationMessageDispatcher` imports `com.haufe.greenland.argus.messaging.consume.cronus.role.message.MessageBearer`, so every `MessageBearer` in that file is that one, never the `person` or `user` one. Java itself refuses to compile a file in which a used simple name could mean two types, so for compiling code the order gives exactly one answer. The edge cases:

- two wildcard imports with the same name: only an error in Java when the name is used, so it does not happen in compiling code; if it does, no edge and a log line;
- a nested or same-package type shadows a wildcard import: handled by the order.

### Names in expressions

A simple name in an expression (`config`, `queue`, `LOGGER`): local variable → parameter → field of the current type and its indexed superclasses → field of an enclosing type → static import. Locals and parameters are block-scoped and typed by their declaration; the fields of an anonymous or local class are variables in its body; Lombok's `@Slf4j` `log` is a library `Logger`.

### Members

A member is looked up through the **static type of its receiver**:

| Receiver | Its type |
| --- | --- |
| `this`, none (`m()`) | the current type, then enclosing types |
| `super` | the superclass |
| a local, parameter or field | its declared type; `var` from its initializer's type (`new T()`, a cast, a resolved call) |
| `new T(..)`, `(T) x` | `T` |
| `T.m()`, `T.X` | `T` (static) |
| a call `a.b()` | the declared return type of the resolved `b` |

The member is looked up in that type, then its indexed superclasses, then its interfaces (a class's method wins over an interface's, as in `javac`); a method overridden nearer is left out — the `overrides` rule: same name and parameter count, each parameter of the same type once the farther type's type variables are replaced by the type arguments the nearer type gives them (a variable the nearer type leaves without an argument — raw, wildcard — or a type the run cannot resolve matches any type, a variable of the farther method any type of the nearer one; a variable of the nearer method or type matches only the same variable, so `<U> put(U)` and `Box<X>#put(X)` do not override `put(String)`; a varargs parameter is an array; a package-private method is overridden only in its package; D-dn) — so `x.publish(e)` on a class overriding `I<T>#publish(T)` is one edge, to the override; two overloads of one type never hide each other. An unqualified `m()` is looked up in the innermost class around the call that has a member `m` (an anonymous or local class by its indexed super types), then in the static imports; every class has `Object`'s methods (`toString()`, `equals(..)`, …) and every enum `Enum`'s (`compareTo(..)`), so such a call never goes past the innermost class. The edge's line is the line of the method's name, so a chain over several lines gives each call its own line.

**Overrides** (6.4, D-dn): every method with a row that is neither a constructor nor static nor private gets an `overrides` edge to each method it overrides *nearest* in its indexed superclasses and interfaces — the overridden methods that no other overridden method overrides in turn — on the line of its name. `class C extends B`, `B implements I`, all three declaring `m()`: `C#m → B#m` and `B#m → I#m`, not `C#m → I#m`; a class implementing `I` and `J extends I` overrides `J#m` only. Through generic super types the type arguments are substituted along the path: `AbstractUserMessageConsumer implements MessageConsumer<UserMessagePayload>`, so `UserCreatedMessageConsumer#perform(UserMessagePayload)` overrides `MessageConsumer#perform(T)`, while a `perform(Other)` beside it does not. A method overriding a library or `Object` method has no edge (the target is not indexed).

The dispatcher example, step by step:

1. `config` → field of `SecurityDataEventDispatcher`, declared `SecurityDataEventDispatcherConfig` → same package → `…janus.securityMethods.SecurityDataEventDispatcherConfig`.
2. `getSecurityDataAddEventConsumer()` with 0 arguments in that type → found; its declared return type `SecurityDataAddEventConsumer` resolves *in the config's file* (by its import) → `…messageConsumer.SecurityDataAddEventConsumer`.
3. `consume(..)` with 1 argument in that type → `SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage)`.
4. Edge: `SecurityDataEventDispatcher#dispatch(SecurityDataMessageBearer<?>)` —`call`→ `SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage)`, line 76. Step 2 adds a `call` edge to the getter as well.

### Implicit members

Members the compiler generates are not in the index. A call to one resolves to its **owner type** (`reference`) and carries its type on through a chain:

- Lombok `@Getter`, `@Setter`, `@Data`, `@Value` on the type or the field: `getX()` / `isX()` / `setX(..)` → field `x`, its declared type continues the chain;
- Lombok `@Builder` (and `@SuperBuilder`): `T.builder()` and `toBuilder()` → `T`'s builder, every builder method (a field's setter whatever its `setterPrefix`, a `@Singular` adder) → the builder again, `build()` → `T`, `Object`'s methods their JDK type; a method of a builder class written in the source (`T.TBuilder`) is a `call` to it;
- records: the accessor `r.name()` → the component's type; the canonical constructor;
- enums: `values()`, `valueOf(..)`, `name()`, `ordinal()`;
- annotation elements: `ann.type()` on an indexed `@interface` (elements are not symbols);
- a constructor that is not declared (the default one, Lombok's `@…ArgsConstructor`, a record's canonical one): `new T(..)` keeps only its `instantiate` edge to `T`; an implicit `super(..)` is a `reference` to the superclass. When `T` declares constructors and none takes the arguments, the call counts unresolved, unless `T` is a record or has a Lombok constructor annotation.

`argus` uses `@Getter` (22 imports), `@Data` (5), `@Builder` (20) and `@RequiredArgsConstructor` (25; its final fields are already field `reference`s); it has no records.

### Overloads

Candidates by argument count (varargs: at least n − 1), then by the simple type names of the arguments whose types are known. When more than one candidate is left: an edge to each, marked `ambiguous`, rather than a guess.

An argument's type is known for literals, `new T(..)`, casts, typed variables, fields and resolved calls, and for a few JDK results that often decide an overload (`toString()`, `String.valueOf(..)`, `Collections.singletonList(..)`, `List.of(..)`, …). An argument fits a parameter when the types are equal (an exact match), when it is an indexed subtype, a number for a number or box (or for `Number`, `Comparable`, `Serializable`), `null` for a reference type, or when either side is unknown or a library type that could be a super type; a `String`, a box or `UUID` parameter takes nothing else (final types, also not an indexed type), a primitive no reference. As in Java, the candidates of fixed arity are tried first (a varargs method taking an array), the varargs ones only when none fits. Every candidate is checked, a single one too: when no candidate takes the arguments, the call goes to a method the index does not have (from `Object` or a library superclass) and makes no edge. Of several candidates, the most exact matches win — but only when every argument's type is known; an unknown argument (a lambda parameter, a library chain) leaves them all, ambiguous. A method reference names no arity: every method of that name is a candidate (`Util::twice` with two overloads is two ambiguous edges).

### Where resolution stops

No edge, counted as unresolved:

- a chain through a library type: `repo.findById(id).map(..)` stops after `findById`, because `Optional#map` is not in the index; same for `Stream`, `List#get`;
- an untyped lambda parameter: `list.forEach(m -> m.consume())` (a method reference `forEach(this::dispatch)` does resolve);
- a type variable the receiver's type does not bind: a super type's type arguments are substituted (6.4, D-dp: `Bearer<T>#getPayload()` called on a `SecurityMethodAddMessage extends Bearer<SecurityData>` returns `SecurityData`; inherited fields and Lombok accessors alike), but not the arguments of a receiver's own declared type (`Box<Item> box; box.get()`, `List<UserToken> tokens` — `Ty` keeps no type arguments; open #49);
- a method that may be inherited from a library: a call on an indexed type that does not have it (`repository.findById(..)` on a Spring Data interface, `getClass()`), and an unqualified `m()` in a class whose superclass chain leaves the index (`class Worker extends Thread`, `new TimerTask() { .. }`) — the search stops there instead of trying the enclosing class;
- calls in an anonymous or local class to its own methods (they are not symbols), and unqualified `Object` / `Enum` methods in a class that does not declare them;
- a receiver typed by a bounded type variable of its **type** (`class Box<T extends Msg> { T m; .. m.type(); }`); one of its **method** is typed by its first bound since 6.4 (`<T extends Msg> void send(T m) { m.type(); }`, D-dp);
- a pattern variable outside its `if`'s then-branch (`if (!(o instanceof Foo f)) return; f.go();`): Java's flow scoping is not followed;
- a method inherited through a generic super type and called on `super` (`super.get().id()` in `Add extends Bearer<Data>`): a `super` receiver is looked up in the super types, not substituted through the class around it;
- a method hidden as overridden by one of an unrelated super type of the receiver (`abstract class C extends S implements I<Sms>`, `S#m(Mail)`, `I<T>#m(T)`: `c.m(sms)` makes no edge) — the override check substitutes through the nearer method's type, not the receiver's (review 6.4, L4).

`argus` has 35 lambdas and 15 `.stream()` chains. Measured in 6.4 (`usages_state.md`, Measurements): of 1386 call sites 788 stay unresolved — 580 on a library receiver, 165 on an unknown one (at most 13 of them named like an indexed method, mostly untyped lambda parameters; the rest library chains), 35 on an indexed type inheriting the member from a library class or `Object`, 8 unqualified in a class extending a library type; the usage golden set's only misses are 3 users behind untyped lambda parameters.

## Not visible from the source

- reflection and `ApplicationContext#getBean`;
- wiring from `application.yml`, `@Value`, `@ConfigurationProperties`;
- Spring events and message brokers between publisher and listener (`argus` has no Spring events; its queue dispatchers are ordinary calls and resolve);
- Spring Data derived queries;
- test code: test sources are not indexed, so test callers never show (open #45).

**Entry points.** Methods the framework calls have no caller in the code: `@Scheduled`, `@RequestMapping` and `@GetMapping`/`@PostMapping`/…, `@EventListener`, `@PostConstruct`, `@Bean` methods, `public static void main`. `trace` stops there and labels them with the annotation (already stored in `symbols.annotations`).

## Modules

Two modules that define the same fqn (`com.acme.util.Json` in two services) cannot be told apart by names — that needs the build files: which module a file belongs to and which modules it depends on. Today the indexer keeps the first definition of a duplicate fqn with a warning; edges to that fqn point at it. `argus` is one Gradle module (`backend/`) without duplicate fqns. Module-aware resolution (module per file, only the modules a file can see) belongs to the **Multi-module handling** item of **Later** and is a prerequisite before indexing a multi-module repository (D-cx).

## Data model

One table in `index.db` (`INDEX_TABLES`), rebuilt every run like the rest of the index; no cache table, no LLM call, no prompt change, so a warm `cache.db` stays warm.

```sql
CREATE TABLE edges (
    id INTEGER PRIMARY KEY,
    src_id INTEGER NOT NULL REFERENCES symbols(id),
    dst_id INTEGER NOT NULL REFERENCES symbols(id),
    kind TEXT NOT NULL,          -- extends | implements | overrides | instantiate | call | reference
    line INTEGER NOT NULL,       -- the reference's line in src's file
    ambiguous INTEGER NOT NULL DEFAULT 0,
    UNIQUE(src_id, dst_id, kind, line)
);
CREATE INDEX edges_dst_id ON edges(dst_id);
CREATE INDEX edges_src_id ON edges(src_id);
```

One row per call site, so `show` can print every line. Roll-ups (a type's `used by`) and transitive callers are queries: plain joins and a recursive CTE, as `project.md` plans.

**Pipeline.** A structure-side stage after the symbols are written (`index_edges`, module `src/usages.rs`). It walks the syntax trees the structure stage parsed, kept in memory until then and dropped after it (D-db); 7 ms on `argus`'s 162 files. `IndexStats` and the `index` summary add edges per kind and unresolved type names and call sites (the most frequent unresolved names at `-v`); the benchmark reports the stage's time.

**`--path`.** The index holds only the prefix, so only edges between symbols under it are kept; the existing `--path` warning adds that usages from outside the prefix are missing.

## Output

Illustrative, from the dispatcher example; the exact format is fixed in 6.5 and 6.6 and documented in `reference.md`.

`annatar show …SecurityDataAddEventConsumer` gains:

```
used by:
  …/janus/securityMethods/SecurityDataEventDispatcher.java
    …SecurityDataEventDispatcher#dispatch(SecurityDataMessageBearer<?>) :76
  …/janus/securityMethods/SecurityDataEventDispatcherConfig.java
    …SecurityDataEventDispatcherConfig reference :24
    …SecurityDataEventDispatcherConfig#<init>(SecurityDataAddEventConsumer, SecurityDataRemoveEventConsumer) reference :28
    …SecurityDataEventDispatcherConfig#getSecurityDataAddEventConsumer() reference :45
uses:
  …
```

Callers grouped by file with their lines; the kind named when it is not `call`; `ambiguous` marked; callers of an overridden interface method labelled `via I#m`; each list capped with `… N more`.

`annatar trace '…SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage)'`:

```
…SecurityDataAddEventConsumer#consume(SecurityMethodAddMessage)
  …SecurityDataEventDispatcher#dispatch(SecurityDataMessageBearer<?>) :76
    …SecurityDataEventDispatcher#receiveAndDispatchMessages() :67
      …SecurityDataEventDispatcher#processMessages() :59 [entry: @Scheduled]
```

Default depth 4 (at most 10); each symbol once (a repeat prints `(see above)`), so cycles and diamonds end; leaves are entry points or `[no callers in main sources]`; through `overrides` with `via`; a type traces its members.

## Steps

Authoritative wording and *done when* in [`plan.md`](./plan.md), Phase 6.

| Step | What |
| --- | --- |
| 6.1 | Parser facts: imports, super types, fields (with Lombok), return and parameter types — in memory, no index change |
| 6.2 | Type resolution and the `edges` table: `extends`, `implements`, `reference`, `instantiate` |
| 6.3 | Member resolution: calls, chains, method references, implicit members, overloads |
| 6.4 | `overrides` edges; measured quality: usage golden set, precision / recall, resolved and ambiguous share |
| 6.5 | `show`: `used by` and `uses` |
| 6.6 | `annatar trace <fqn> [--depth N]` |
| 6.7 | T4 agent trial re-run with usages in the prompt; README agent snippet |

## Measuring

- **Usage golden set** (6.4, `annatar eval-usages`): 54 `argus` symbols — 19 chosen to cover types, public and private methods, interface methods, Lombok getters, generics, the T4 consumer, 8 drawn at random (not reproducible), 2 lambda cases, 25 methods and constructors drawn reproducibly (`random.Random(6404)` over the sorted index rows, the script beside the set; D-dm) — with their true direct users in main sources, found by hand (no IDE in this environment: an exhaustive grep of each name and its variables, every hit judged by Java's rules, before looking at the edges; D-dm). A direct user is the symbol an edge starts at (D-cv); a type's users roll up its members' and nested types' (without its own internal ones); `overrides` is not a use, the overriders are scored apart (`overridden_by`). Kept outside git like `golden-argus.toml` (D-bl) and checked against the index first, so a renamed symbol fails loudly. Reported: precision and recall of direct users per symbol and pooled (each symbol–user pair once), overall, for types and members, and — in `usages_state.md` — for the random symbols alone, since the chosen ones were picked knowing the design; every miss classified (lambda, library chain, generics, other).
- **Coverage:** the share of call sites resolved to a member, implicit (a generated member: only a `reference` to its type), ambiguous and unresolved, in the `index` summary.
- **Agent trial** (6.7): T4 *with* arm, 5 runs, the D-cs protocol and grading, with a prompt variant describing `used by` and `trace`. Success: the dispatcher in 5 of 5 answers at no higher cost than the 5.5 runs.

## Alternatives and later work

- **SCIP (scip-java)** resolves as the compiler does: generics, lambda parameters, library chains and modules included. It needs the repository to build on the indexer (no Gradle cache for `argus` in this environment, Maven Central behind the proxy allowlist, a build per repository centrally). It stays in **Later** as a precision upgrade that writes the same `edges` table; 6.4's misses are the trigger (D-cu).
- **Test callers** as edge sources only (open #45).
- **Usages in the `search` file view** (`used by N`, callers' files), so an agent sees the caller without a second command (open #46).
- **Callers in the description prompts:** changes every prompt, so the whole LLM cache misses (≈ 20 min on `argus`); decide after 6.7 (open #47).
