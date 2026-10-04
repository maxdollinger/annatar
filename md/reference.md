# Annatar — Reference

Detailed behaviour of the CLI. The short setup guide is in the [README](../README.md).

Annatar needs `git` on `PATH`, and the configured `repo` must be a git work tree for history and ticket keys. Otherwise only the structure is indexed, with one warning.

Configure `annatar.toml` (the default file; `--config <FILE>` points elsewhere). Relative paths resolve against the config file's directory. Secrets come only from the environment: `ANNATAR_JIRA_TOKEN` (Cloud API token or Server/DC personal access token) and `ANNATAR_JIRA_EMAIL` (basic auth when set, bearer otherwise). Jira Cloud requires `ANNATAR_JIRA_EMAIL`: it rejects an API token sent as bearer. How to point `[jira] base_url` at Cloud depends on the token:

- **Classic API token:** `ANNATAR_JIRA_EMAIL` plus the site URL, e.g. `base_url = "https://your-site.atlassian.net"`.
- **Scoped API token:** `ANNATAR_JIRA_EMAIL` plus the Atlassian API gateway, `base_url = "https://api.atlassian.com/ex/jira/<cloudId>"` (scoped tokens are not accepted on the site URL), and the scope `read:jira-work`. Without `read:jira-user` the credential check below is inconclusive and `index` goes on to fetch; a token that lacks `read:jira-work` fails the run on its first fetch.

```sh
cargo build --release                     # binary in target/release/annatar
annatar index                             # rebuild .annatar/index.db
annatar index --path src/main/java/com/acme   # only part of the repo
annatar index --offline                   # no Jira requests; cached tickets only
annatar index --no-llm                    # no chat or embedding calls; cached summaries, descriptions and embeddings only
annatar show com.acme.user.UserRepository     # a symbol, its children (with descriptions), commits and tickets (with summaries)
annatar show -k 50 com.acme.user.UserRepository   # the same, with up to 50 entries per used by / uses list (default 20)
annatar trace 'com.acme.user.UserRepository#find(Long)'   # its transitive callers as a tree, up to the entry points
annatar trace --depth 8 -k 5 com.acme.user.UserRepository   # a type's callers 8 levels deep, 5 callers per symbol (default 6 and 10)
annatar search "who deletes expired tokens"   # the 5 files whose symbols match best, with their outline
annatar search --symbols "who deletes expired tokens"   # the 10 symbols whose descriptions match best
annatar search -k 3 --kind method --role repository "delete expired tokens"
annatar --path src/main/java/com/acme/token search "delete expired tokens"   # only symbols in files under the prefix
annatar eval golden.toml                  # retrieval quality: top-1 / top-5 / MRR of a golden set
annatar eval-usages usages.toml           # usage quality: precision / recall of each symbol's direct users
```

Results go to stdout, logs (`-v`, `RUST_LOG`) to stderr, without colours unless stderr is a terminal. A failing command prints one line, `error: …`, on stderr and exits with 1 (2 for a usage error such as an unknown `--kind`); warnings logged on the way (e.g. a retried request) may come before it.

`search` embeds the query with `ollama.embedding_model`, finds the most similar symbols and prints the files they are in, best file first:

```text
1. 0.770 src/main/java/com/acme/token/TokenCleanup.java
   class com.acme.token.TokenCleanup [service] :18-48 *0.758
     Deletes expired user tokens from the database periodically.
     #<init>(TokenRepository) :24-27
     #deleteExpiredTokens() :32-37 *0.770
     enum Mode :40-47
       #isStrict() :44-46
2. 0.701 src/main/java/com/acme/token/TokenRepository.java
   interface com.acme.token.TokenRepository [repository] :9-20
     Reads and deletes stored user tokens.
     #deleteByExpirationDateBefore(ZonedDateTime) :15-15 *0.701
     #findByUserId(UUID) :17-17
```

Per file: its rank, its best symbol's cosine similarity (1 = identical) and its path; then each top-level type of the file in source order as `kind fqn [role] :start-end` (the role only when the type has a Spring role) with its description on the next line; under it every member (methods, constructors) and nested type of the file in source order, one line each without a description: the part of the fqn after its enclosing type (`#name(params)`, a constructor `#<init>(params)`, a nested type `kind Name [role]`) and its lines, indented two more spaces per nesting level. A symbol that matched the query ends with `*score`: the hits that rank above the best hit of the first file not shown, so every file has at least one; when no further file is among the hits (a narrow `--path` or filter), each file's best hit and the 10 best hits overall. A file with more than 40 member and nested-type lines shows its matching symbols and the first lines in source order up to 40, then `… N more` under each top-level type that has hidden lines (`show` on the type lists all). Files are ranked from the 100 most similar symbols by their best one. `-k/--limit` sets the number of files (default 5, at most 20); when the 100 symbols are in fewer files, those are printed with a note on stderr.

`--symbols` prints the most similar symbols instead, two lines each:

```text
1. 0.770 com.acme.token.TokenCleanup#deleteExpiredTokens() [method] src/main/java/com/acme/token/TokenCleanup.java:32-37
   Schedules the deletion of expired user tokens every 5 minutes.
2. 0.758 com.acme.token.TokenCleanup [class] role=service src/main/java/com/acme/token/TokenCleanup.java:18-48
   Deletes expired user tokens from the database periodically.
```

rank, cosine similarity, fqn, `[kind]`, `role=<role>` when the symbol has a Spring role, `file:start-end`; the fqn is the third space-separated field, and the location is everything after the `[kind]` and `role=` tokens (a path may contain spaces); then the description on one line, indented by three spaces. With `--symbols`, `-k` is the number of symbols (default 10, at most 100).

`--kind` (`class`, `interface`, `enum`, `record`, `annotation`, `method`, `constructor`) and `--role` (`controller`, `service`, `repository`, `component`, `configuration`, `entity`; a method or constructor matches by its enclosing type's own role, so members of a nested type match by the nested type's role) filter which symbols can match and may be repeated; in the file output they decide which files rank and which symbols are marked, while each file is still shown whole. The global `--path <PREFIX>` applies to `search` too: only symbols in files at or under the prefix (whole path components; relative to the repository or absolute inside it; `.` or the repository itself means no filter; a prefix outside the repository, also by `..`, is an error). A filter that matches nothing prints nothing on stdout and a note on stderr. An out-of-range `-k` is a usage error (exit 2) naming both ranges. `search` needs the `[ollama]` section and a reachable server; it reads only `index.db` (the query's vector is not cached) and refuses an index without vectors, or one embedded with another `embedding_model` or dimension. Use `show <fqn>` on a type or member for the detail view (description, parent, children, file and lines, commits, tickets).

`index` fetches every ticket key found in the history from Jira once and caches it in `cache.db` (403/404 and other permanent failures — another 4xx except 408/425/429, an issue that does not parse — are cached as unavailable; 5xx, timeouts, 429 and non-JSON answers are retried next run), so later runs make no Jira requests for known keys; the `tickets:` line of its output counts cached, fetched, unavailable and failed keys and the Jira requests made. Without a `[jira]` section or without `ANNATAR_JIRA_TOKEN` it warns once and uses cached tickets only; `--offline` does the same on purpose. Before fetching, `index` checks the credentials once (`/rest/api/2/myself`, retried on 429 like fetches); rejected credentials fail the run and nothing is cached. If Jira is unreachable or still rate limiting after retries, `index` stops fetching for that run with one warning, counts the remaining keys as not fetched and retries them next run. `show` lists each ticket with its type and summary, or `(unavailable)`.

`[ollama]` configures the LLM client (Ollama's OpenAI-compatible `/v1` API: chat model, embedding model, `reasoning_effort`, `temperature`, `max_tokens` (a safety cap against runaway replies, not a length limit); see `annatar.toml`). `index` asks the chat model for a short English summary and purpose of every available ticket (tickets are often German) and stores them in the index; `show` prints them under each ticket. Replies are cached in `cache.db`, so a rerun makes no chat calls; the `summaries:` line of the output counts summarised, invalid, failed and skipped tickets and the chat calls made (with the largest prompt and completion token counts of one reply, to spot a prompt near the model's context size). Without `[ollama]` it warns once and tickets get no summaries; `--no-llm` makes no chat calls and uses only the summaries already cached. The purpose is left out when the ticket gives no reason. Before building, `index` checks once that the Ollama server has the chat model and the embedding model: a missing model fails the run (`--no-llm` skips the check). A reply that names a ticket key (the `ticket_regex`) is invalid, here and in the descriptions below. A reply that is still invalid after one retry is skipped for that ticket (retried next run); if the chat model fails (unreachable, HTTP error, timeout) or five replies in a row are invalid, `index` makes no further chat calls that run, warns once and keeps using cached summaries. After the ticket summaries, `index` asks the chat model for a `description` of every method and constructor — what it does and, where its code, tickets or commits give one, the reason it was created or changed, as short as possible without losing information (no length limit) — and stores it in the index; `show` prints it under each member's signature. The prompt carries the member's Javadoc, its source (capped, `describe.body_chars`; the signature instead when it is `0`), the enclosing type and its Spring role, the summaries of its first ticket plus its most recent few (`describe.recent_tickets`) and always its most recent commit subjects (`describe.commit_subjects`, merge commits left out); a ticket whose summary was invalid stands in with its Jira title; unavailable tickets are left out (without Jira configured, so are tickets not in the cache). A member whose chosen ticket has not been fetched yet (`--offline`, a failed fetch) or has no summary because the breaker tripped or `--no-llm` is left without a description and asked again on a later run, so nothing built from incomplete input is cached; one warning lists the blocking ticket keys. The `describe:` line of the output counts described (and how many came from the cache), invalid, failed, incomplete and skipped members and the chat calls made; the breaker and `--no-llm` work as for the summaries. Then every type gets a description the same way, nested types first so their outer type can use them: the prompt carries the type's Javadoc, Spring role, enclosing type, its declaration with members and nested types cut out together with the comments directly above them (annotations, header, fields, enum constants; enum constant bodies and initializer blocks shortened to `{ … }`; capped by `describe.body_chars`), the descriptions of up to `describe.type_members` members and nested types (public ones first, the rest counted), its first plus most recent tickets (`describe.type_recent_tickets`) and its most recent commit subjects. A type whose listed member or nested type has no description this run waits for a later run, like a member waiting for a ticket; one whose reply was invalid is listed as not described. `show` prints descriptions for types too, and the `types:` line counts them like the `describe:` line. Last, `index` embeds every symbol that has a description — its fqn and description, one per line (`embedding.parent_description = true` adds a method's or constructor's enclosing type's description) — with the embedding model and stores the vectors in the index (`symbol_vectors`, an `F32_BLOB` column of the dimension the model returns, with a cosine `libsql_vector_idx` index for `vector_top_k`) and records the embedding model, the dimension and `embedding.parent_description` in the `index_meta` table. Vectors are cached in `cache.db` by model and text, so a rerun makes no embedding calls; the `embeddings:` line counts embedded (and how many came from the cache), failed and skipped symbols, the embedding calls and texts sent and the dimension. `--no-llm` and a tripped breaker use cached embeddings only; a failed embedding request warns once, keeps the cached ones and leaves the rest for the next run; symbols left without a vector because their embedding is not cached get one warning. Cached vectors of another length than the model now returns (the model behind the name changed) are re-embedded. An index without any vector has no `symbol_vectors` table (inspect it with libSQL: the stock `sqlite3` shell sees no rows in it). The live checks run with `cargo test llm -- --ignored` (`ANNATAR_OLLAMA_URL`, `ANNATAR_OLLAMA_CHAT_MODEL`, `ANNATAR_OLLAMA_EMBEDDING_MODEL`, `ANNATAR_OLLAMA_REASONING_EFFORT` override `annatar.toml`).

`index` also records which symbol uses which type or member, in the `edges` table of `index.db` (resolved from the parsed sources: no LLM call, no cache): `extends` and `implements` from a type's header, `instantiate` for `new T(..)` (with or without a body) and `T::new`, and `reference` for every other mention of a type — field, parameter, return and local types, casts, `instanceof`, `catch`, `throws`, generic arguments, `new T[n]`, `T.class`, annotations and their arguments, a type qualifier (`T.X`, `T.m()`, `T::m`, `Outer.this`) and a constant brought in by a static import. A mention belongs to the innermost method or constructor around it (code in lambdas, anonymous and local classes included), otherwise to the type (header, annotations, fields and their initializers, initializer blocks, enum constants); one row per source, target, kind and line. Type names resolve as `javac` resolves them — a nested type of an anonymous or local class's super type, of the current or an enclosing type (also inherited from an indexed super type; a type's header and annotations see only the types around it), a single-type import, the same package, a wildcard import — and a qualified name from its first part; a variable or field of the same name wins over a type, type variables and local classes are not types. Imports, Javadoc and a type naming itself are not usages. A name that does not resolve to an indexed type (the JDK, libraries) makes no edge and is counted.

Calls are `call` edges to the method or constructor (`x.m(..)`, `m(..)`, `T.m(..)`, `super.m(..)`, `super(..)`, `this(..)`, a static-imported method, and the method references `this::m`, `x::m`, `T::m`, `super::m`), and `new T(..)` and `T::new` add an `instantiate` edge to the constructor the arguments choose. The method is looked up through the static type of its receiver — a local variable or parameter (block-scoped; `var` takes its initializer's type), a field, a type, `this`, `super`, or the declared return type of the call before it, so `config.getConsumer().consume(m)` reaches `consume` through the getter — in that type and its indexed super types; an unqualified `m()` in the classes around the call, innermost first, then in static imports. Overloads are chosen as Java does on the argument types that are known: candidates every argument can be passed to, fixed arity before varargs (at least n − 1), then the most exact matches when every argument's type is known; when several remain, each gets an edge with `ambiguous = 1`, and when none takes the arguments the call makes no edge (the method comes from `Object` or a library superclass). A method reference names no arity, so every method of that name is a candidate. Members the compiler or Lombok generates — `@Getter`/`@Setter`/`@Data`/`@Value` accessors, `@Builder`'s `builder()` and builder methods, record accessors, enum `values()`, `valueOf(..)`, `name()`, `ordinal()`, an annotation element (`ann.type()`), a constructor that is not declared — make a `reference` to their type (a `new T(..)` without a declared constructor keeps only its `instantiate` edge to the type) and carry a chain on with the field's or component's type. Reading or writing a field of another type is a `reference` to the type declaring it. A call on a receiver outside the index — a library type, an untyped lambda parameter, a method returning a type variable no super type of the receiver's type binds (`box.get()` on a `Box<Item>`), a class extending a library type that may declare the method — makes no edge and is counted unresolved; a method calling itself makes none either. A method that overrides another (same name and parameter count, each parameter of the same type once the type variables of the overridden method's type are replaced by the type arguments the overriding type gives them, so `consume(AddMessage)` overrides `MessageConsumer<T>#consume(T)` in a class implementing `MessageConsumer<AddMessage>`; a type variable of the overriding method or type matches only the same variable, a varargs parameter is an array, and a package-private method is overridden only in its own package) gets an `overrides` edge to each method it overrides nearest in its indexed superclasses and interfaces — `C#m → B#m` and `B#m → I#m`, not `C#m → I#m` — on the line of its name; constructors, static and private methods neither override nor are overridden. A method inherited from a generic super type returns, on a subtype, the type argument the subtype gives its type variable (`message.getPayload().getUserId()` on `AddMessage extends Bearer<Data>` reaches `Data`), as do inherited fields and Lombok accessors; a parameter or local typed by a bounded type variable of its method (`<T extends Msg> void send(T m)`) is typed by the bound. The type arguments of a receiver's own declared type (`Box<Item> box; box.get()`) are not used. The `edges:` line of the output gives the rows per kind, the unresolved type-name mentions (and distinct names) and the stage's time; the `call sites:` line the method calls, constructor calls and method references by outcome — *resolved to a member* (a `call` or constructor `instantiate` edge to one indexed method or constructor; a method calling itself counts here without an edge), *implicit* (a generated member or undeclared constructor: only a `reference`, or `instantiate`, to its type), *ambiguous*, *unresolved*; `-v` logs the 20 most frequent unresolved type names and calls (`Receiver#name`, `?` for an unknown receiver). See [`usages.md`](./usages.md).

`show` prints these usages for the symbol it shows (not for its children), after its tickets:

```text
com.acme.flow.AddConsumer#consume(Message) [method]
  - file: src/main/java/com/acme/flow/AddConsumer.java:4-7
  - parent: com.acme.flow.AddConsumer
  - signature: public void consume(Message message)
  - annotations: @Override
  - used by:
    src/main/java/com/acme/flow/Dispatcher.java
      com.acme.flow.Dispatcher#dispatch(Message) :19
      com.acme.flow.Dispatcher#dispatch(Message) via com.acme.flow.Consumer#consume(Message) :20
  - uses (lines in this symbol's file):
    src/main/java/com/acme/flow/AddConsumer.java
      com.acme.flow.AddConsumer#log() :6
    src/main/java/com/acme/flow/Consumer.java
      com.acme.flow.Consumer#consume(Message) overrides :5
    src/main/java/com/acme/flow/Message.java
      com.acme.flow.Message reference :5
```

and, for the `@Scheduled` method that calls `dispatch` twice, `show -k 1 'com.acme.flow.Dispatcher#dispatch(Message)'` lists

```text
  - used by:
    src/main/java/com/acme/flow/Dispatcher.java
      com.acme.flow.Dispatcher#process() :14, 15 [entry: @Scheduled]
  - uses (lines in this symbol's file):
    src/main/java/com/acme/flow/AddConsumer.java
      com.acme.flow.AddConsumer#consume(Message) :19
    … 3 more
```

`used by` lists every edge into the symbol, grouped by the user's file (files by path, then by line): one line per user and link, `fqn :line[, line…]` with the lines of the mentions in that file, the edge kind after the fqn unless it is `call`, `ambiguous` for one of several overloads the call could mean, and `[entry: …]` when the user is an entry point. An overriding method is *used by* the callers of every method it overrides, up the chain (`C#m → B#m → I#m`): they call the overridden method and reach this one at run time, so they are listed `via` the fqn of the method they call (each caller once, also through a diamond); a method's own list also names the methods overriding it (`overrides`). A type's list rolls up the edges into its members and nested types (and their `via` callers), leaving out the ones that start inside the type itself and the `overrides` edges into its members (the subtype's `extends` / `implements` line already names it). `uses` lists every edge out of the symbol the same way, grouped by the used symbol's file — but the lines are where the **shown** symbol mentions it, in the shown symbol's own file, so the title reads `- uses (lines in this symbol's file):`; a type's list rolls up its members' and nested types' edges, leaving out the ones to symbols inside it and its members' `overrides` (its `extends` / `implements` line says the same), and a `new T(..)` that reaches a declared constructor shows only the constructor's `instantiate`, not the type's as well. Each list stops after `-k` entries (`--limit`, default 20, 1 to 100) with `… N more`; an empty one reads `- used by: none in main sources` (test code is not indexed, so its callers never show) or `- uses: none`. A method is an entry point when the framework calls it: `@Scheduled`, `@RequestMapping`, `@GetMapping`, `@PostMapping`, `@PutMapping`, `@DeleteMapping`, `@PatchMapping`, `@EventListener`, `@ExceptionHandler`, `@PostConstruct`, `@PreDestroy`, `@Bean` (by simple name), or a `public static void main(String[])`; the shown symbol then gets `- entry point: @Scheduled` before its lists. `show` reads only `index.db` (no Ollama).

`trace` follows the callers transitively, from the symbol up to the code nothing in main sources calls. On the same project, `annatar trace 'com.acme.flow.AddConsumer#consume(Message)'` prints

```text
com.acme.flow.AddConsumer#consume(Message) [method] src/main/java/com/acme/flow/AddConsumer.java:4-7
  com.acme.flow.Dispatcher#dispatch(Message) src/main/java/com/acme/flow/Dispatcher.java:19; via com.acme.flow.Consumer#consume(Message) :20
    com.acme.flow.Dispatcher#process() :14, 15 [entry: @Scheduled]
```

The first line is the symbol, `fqn [kind] file:start-end`. Below it, indented two spaces per level, one line per caller: its fqn, then each way it reaches the symbol above — the edge kind unless `call`, `ambiguous`, `via` the fqn of the overridden method it calls, the lines of the mentions — joined by `; `; the first carries the caller's file, unless it is the file of the symbol it calls, the line it is indented under (`:14, 15` is in `Dispatcher.java`). Under each caller its own callers, depth first. `trace` follows `call`, `instantiate` and `reference` edges, and a method's callers include those of every method it overrides, as in `used by`; `extends`, `implements` and `overrides` are not followed. A type as the symbol traced starts from the callers of the type, its members and nested types (without the ones inside it, as `show`'s `used by`); below it nothing is rolled up, and a type that only mentions the symbol above (a field, a parameter of its header, an annotation) ends its branch with `[type, not followed]` — the members using that field appear as callers of their own; `trace` the type for its users. A type whose field initializer or initializer block calls or instantiates the symbol above runs that code whenever it is constructed: it is marked `[initializer]` and followed through the code constructing it (`new T(..)`, calls of its constructors). Each symbol is expanded once, at its shallowest level (the first such line, depth first), so the depth never hides a caller the tree reaches higher up: a later copy prints `(see above)`, an earlier, deeper one `(see below)`; cycles and diamonds end. An entry point is marked `[entry: @Scheduled]` (the annotations `show` names) on every copy; its callers in code, if any (an inter-bean `@Bean` call, a mapping method reused), are traced like any other's, so usually it ends the branch. A branch ends at a symbol without callers (`[no callers in main sources]`, left out after an entry point; test code is not indexed) or at the depth, `[N callers beyond depth D]` (`--depth`, default 6, 1 to 10). Each symbol lists at most `-k` callers (`--limit`, default 10, 1 to 100), the rest as `… N more`. Out-of-range values are usage errors (exit 2), an unknown fqn an error (exit 1). `trace` reads only `index.db`, one query per symbol reached.

A golden set (questions with the fqns that answer them, the benchmark retrieval is measured against) is a TOML file of `[[question]]` tables (`text`, `expect` = the fqn the question is about first, then acceptable alternates, optional `kind` and `note`; see `src/golden.rs` and `tests/fixtures/golden/sample.toml`). A real set names a private repository's symbols and stays out of git (`.annatar-local/` is ignored). Check it against a full index (a run without `--path`) with `ANNATAR_GOLDEN_SET=<set.toml> ANNATAR_GOLDEN_DATA_DIR=<data dir holding index.db> cargo test real_golden_set -- --ignored`: it fails on any expected fqn the index does not hold and on a first fqn of another kind.

`annatar eval <golden.toml>` measures retrieval against a golden set: it runs that check first (any problem is the error, nothing is searched), then searches every question exactly like `annatar search --symbols "<question>"` (no filter, `-k` hits, default 10, at least 5 because the summary counts top-5) and like `annatar search "<question>"` (the same number of files) and prints one line per question and six summary lines:

```text
eval: embedding_model=bge-m3:latest dim=1024 parent_description=false k=10
1. 1 1 file 1 1 [members] com.acme.token.TokenCleanup#deleteExpiredTokens()
2. 3 1 file 1 1 [types] com.acme.token.TokenCleanup top=com.acme.token.TokenCleanup#deleteExpiredTokens()
3. - - file 4 2 [types] com.acme.user.User top=com.acme.user.UserService
all 3: primary top-1 1 top-5 2 mrr 0.444; any top-1 2 top-5 2 mrr 0.667
types 2: primary top-1 0 top-5 1 mrr 0.167; any top-1 1 top-5 1 mrr 0.500
members 1: primary top-1 1 top-5 1 mrr 1.000; any top-1 1 top-5 1 mrr 1.000
files all 3: primary top-1 2 top-5 3 mrr 0.750; any top-1 2 top-5 3 mrr 0.833
files types 2: primary top-1 1 top-5 2 mrr 0.625; any top-1 1 top-5 2 mrr 0.750
files members 1: primary top-1 1 top-5 1 mrr 1.000; any top-1 1 top-5 1 mrr 1.000
```

the index's embedding settings and `k`; per question its number, the rank of the first expected fqn (primary), the best rank of any expected fqn (`-` = not in the top `k`), after `file` the rank of the primary's file and the best rank of a file holding any expected fqn among the `k` files of the file output, `types` or `members` (by the primary fqn), the primary fqn and, when it is not first, the first hit; then for all questions, types and members the number of questions, top-1 and top-5 counts and the mean reciprocal rank (a miss counts 0, so MRR depends on `k`), for the primary and for any expected fqn, first by symbol, then (`files …`) by file. It needs the `[ollama]` section like `search` and takes no `--path`.

`annatar eval-usages <usages.toml>` measures the edges against a usage golden set: a TOML file of `[[symbol]]` tables (`fqn`, `users` = the true direct users, optional `overridden_by` on a method, `kind` and `note`; see `src/usage_eval.rs` and `tests/fixtures/golden/usages.toml`; a real set stays out of git like a golden set). A direct user is the symbol an edge starts at: the innermost method or constructor around a mention (lambdas and anonymous classes included), else the type; a type's users roll up those of its members and nested types, without the ones inside the type; every kind but `overrides` counts, and the overriding methods are scored apart when `overridden_by` is given. It first checks every fqn of the set against the index (any missing symbol, user or overrider, or a symbol of another kind, is the error naming the entry as `symbol N`, nothing is scored), then prints per symbol `P precision R recall (hits/expected, N found) [types|members] fqn` with the `missed` and `extra` users under it (and `overridden by: …` when listed), then the same pooled over all symbols, types and members (each symbol–user pair counts once, so a user listed under two symbols counts twice) and over the overriders; `-` is a ratio over nothing:

```text
1. P 1.000 R 0.500 (1/2, 1 found) [members] com.acme.Consumer#consume(AddMessage)
   missed com.acme.Dispatcher#retry(Message)
   overridden by: P 1.000 R 1.000 (1/1, 1 found)
all 1: P 1.000 R 0.500 (1/2, 1 found)
types 0: P - R - (0/0, 0 found)
members 1: P 1.000 R 0.500 (1/2, 1 found)
overridden by 1: P 1.000 R 1.000 (1/1, 1 found)
```

It reads only `index.db` (no `[ollama]` section needed) and takes no `--path`.

`--path <PREFIX>` is for fast iteration on part of the repo, not for refreshing a slice: a `--path` run still replaces the whole `index.db`, which then holds only that prefix (empty if the prefix matches nothing), and its `edges` only the usages between symbols under it. The run logs a warning saying so, and that usages from outside the prefix are missing; run `annatar index` without `--path` to get the full index back.
