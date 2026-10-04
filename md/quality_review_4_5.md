# Quality review 4.5 — what/why records on `argus`

> **Historical record.** After this review the product owner replaced the
> separate `what` / `why` with one short `description` per symbol (plan 4.6,
> `state.md` D-bm–D-bp). The verdicts below judge the old records; the 4.6
> spot check of this sample against the new descriptions is in `state.md`
> (4.6 Done entry).

Step 4.5 of [`plan.md`](./plan.md): are the generated records trustworthy
(POC question 1)? The agent reviewed 34 symbols and 13 ticket summaries of the
real `argus` index against the source and the ticket text, changed the
prompts where the evidence pointed, re-ran the full index and reviewed the
same items again. **The verdicts are the agent's, standing in for the product
owner, who confirms or overrides them (`state.md` open #34).**

fqns are written relative to the repository's root package (the employer's
package prefix is left out, as in the scrubbed fixtures and the rest of
`state.md`). No ticket text or code is copied here; notes paraphrase.

## Headline (read this first)

On the 34 reviewed symbols, `why` before → after (corrected in the 4.5 review
fixes, see below):

| `why` verdict | Before | After |
| --- | --- | --- |
| correct | 11 (32 %) | 12 (35 %) |
| wrong | 4 (12 %) | 3 (9 %) |
| vague | 19 (56 %) | 7 (21 %) |
| empty | 0 | 12 (35 %) |

- **Strictly correct whys barely moved (32 % → 35 %).** The gain in
  "acceptable" (C + E, 32 % → 71 %) is almost entirely vague whys turned
  into empty ones. That is the intended effect (an empty why no longer
  pretends to explain a symbol), but it is not "more reasons found".
- **The prompts were tuned on part of the sample.** 15 of the 34 symbols lie
  under the two `--path` prefixes used to iterate on the wording
  (`auth/authentication/token`, `messaging/consume/pactum`): acceptable
  33 % → 80 % there. On the 19 held-out symbols: **32 % → 63 %**. The
  held-out figure is the fairer estimate.
- **Index-wide the change is mostly deletion:** 240 member and 35 type whys
  became empty, none was gained. 195 of the 240 are accessors, constructors
  or `equals`/`hashCode`/`toString`; of the other 45, roughly 5–8 lost a
  reasonably supported reason (examples under *Index-wide* below).
- **The "always empty for trivial accessors and field-storing constructors"
  rule is followed about 85 %:** 13 of 120 trivial accessors and 12 of 44
  field-storing constructors still have a why. It is a prompt instruction,
  not enforced in code.
- **10 ticket purposes became empty** (25 → 35 of 69); one of them
  (GRLD-40363) caused a reviewed member why to lose its origin (#3).

## Method

- *Index before:* the 4.4 review-fixes index (`data-real`, 162 files, 654
  symbols, 69 tickets). *Index after:* the full run with the 4.5 prompts
  (D-bj, D-bk), same repository state.
- *Sample:* 20 members (services, controllers, a repository query, strategy
  code, getters/setters, constructors, a scheduled method, a config bean,
  members with 1–11 tickets and members with commit subjects only) and 14
  types (enum, nested class and enum, an empty marker class, exception,
  entity, Lombok DTO, configuration, annotation, controller, service,
  event) — chosen across packages and roles before looking at the new
  output.
- *Verdicts per field:*
  - **C** correct — true and specific to this symbol;
  - **E** empty — no `why`; counted as acceptable when the history does not
    explain this symbol (a getter, a field-storing constructor, a nested type
    whose file history is about something else);
  - **V** vague — true of the context but not of this symbol (the ticket's
    project theme, a reason that fits every member of the class) or missing
    an important effect;
  - **W** wrong — a reason borrowed from an unrelated ticket or contradicted
    by the code;
  - **I** invented — content that no input supports.
- *Rates:* "acceptable" = C + E; "misleading" = W + I.

## Symbols

| # | fqn (relative) | kind | what before → after | why before → after | note |
| --- | --- | --- | --- | --- | --- |
| 1 | `auth.authentication.token.UserTokenService#removeAllTokens(List<UUID>)` | method | C → C | C → C | bulk invalidation + async event; reason is the emergency-logout ticket that created it |
| 2 | `auth.authentication.token.UserTokenService#isTokenValid(UUID,JwtUtils.JwtInfo)` | method | C → C | C → C | admin bypass and the log-noise fix both from its own tickets |
| 3 | `auth.authentication.token.boundary.TokenValidationRest#deleteAllFromList(UserTokenDeleteAllForUsersRequestTO)` | method | C → C | C → V | after: drops the organisation-deletion origin and claims a super-admin restriction the code (system role) does not have. Cause: D-bk emptied the purpose of the creating ticket GRLD-40363 (its description is only a chat link, so it counts as title-only); the only stated reason left in the history is GRLD-37944's (emergency logout), and the model took it |
| 4 | `auth.authentication.token.boundary.TokenPreValidationRest#preValidateJwt(HttpServletRequest)` | method | C → C | C → C | |
| 5 | `…databaseCacheStrategy.UserTokenEntryRepository#deleteTokensBeyondNewest(UUID,int)` | method | C → C | V → C | before also borrowed the sibling scheduler's monitoring goal |
| 6 | `…databaseCacheStrategy.DatabaseCacheUserTokenStrategy#removeToken(UUID,String)` | method | C → C | V → C | before: reason of a later research ticket — that ticket did change this method (stop writing tokens to the cache), so the reason is real context but not why the method exists (V; was W in the first version of this review); after: the storage migration that created it |
| 7 | `auth.authentication.token.UserToken#setType(TokenType)` | method (setter) | C → C | V → E | before: the proof-of-concept theme |
| 8 | `auth.authentication.token.UserTokenBearer#getUserId()` | method (getter) | C → C | V → E | same |
| 9 | `auth.authorization.cache.AuthDataService#getAuthData(UUID)` | method (6 tickets) | C → C | C → C | origin ticket's reason (parallel caches); misses the token invalidation branch in `what` both times (minor) |
| 10 | `auth.authorization.boundary.AuthDataCacheRest#delete(String)` | method (7 tickets) | C → C | V → W | after: borrows the person-list access reason of a recent ticket |
| 11 | `messaging.consume.cronus.role.message.consumer.RoleRelationsUpdatedMessageConsumer#perform(RoleRelationMessagePayload)` | method (11 tickets) | V → V | W → W | `what` misses publishing the update event and refreshing the role cache; `why` borrows person lists |
| 12 | `integration.pactum.SubscriptionDataService#getSubscriptionFromPactum(UUID)` | method (6 tickets) | C → C | C → C | |
| 13 | `common.exception.TechnicalException#<init>(String)` | constructor | C → C | W → E | before: the event-consumer ticket's reason for a generic exception constructor |
| 14 | `auth.event.dynamoDB.DynamoDBEvent#<init>(String,String,List<String>)` | constructor | C → C | V → E | |
| 15 | `messaging.consume.pactum.SubscriptionMessageDispatcher#processMessages()` | method (commit subjects) | C → C | V → V | restates the commit subject |
| 16 | `authDataCache.authData.organizationUserRelation.OrganizationUserRelationInMemoryCache#checkExpiration()` | method (commit subjects) | C → C | V → V | |
| 17 | `configuration.ArgusErrorController#handleError()` | method | C → C | C → C | hides the framework's default error page |
| 18 | `configuration.DataSourceRoutingConfig#routingDataSource(DataSourceProperties)` | method (config bean) | C → C | V → E | ticket is title-only ("Argus") |
| 19 | `oAuth2.oAuth2Request.OAuth2RequestCacheService#wrapKey(UUID)` | method | C → C | W → E | before: the ticket purpose (object size estimate) is about another bullet |
| 20 | `common.cache.AbstractCache#supply(K)` | method (abstract) | C → C | V → V | the caching initiative's theme |
| 21 | `auth.authentication.token.TokenType` | enum | I → C | V → E | before: read "mobile" into a name suffix (open #32); after: names the constants only |
| 22 | `common.privilege.Privilege` | class | C → C | V → V | created during a framework upgrade; the upgrade is the stated reason |
| 23 | `common.privilege.Privilege.User` | nested class (empty) | C → C | V → E | |
| 24 | `messaging.consume.pactum.ContractMessage.Contract.LifecycleStatus` | nested enum | C → C | V → E | before: the file's initiative |
| 25 | `integration.pactum.SubscriptionDataService.UnprocessableContractException` | nested class | C → C | C → C | its own ticket (redirect loop) |
| 26 | `auth.authentication.token.tokenStoreStrategy.databaseCacheStrategy.UserTokenEntry` | class (entity) | C → C | C → C | |
| 27 | `configuration.ArgusJacksonConfig` | class (configuration) | C → C | V → E | title-only ticket |
| 28 | `messaging.consume.pactum.message.MessageConsume` | annotation | C → C | V → V | commit subject restated |
| 29 | `auth.authorization.cache.contract.CachedContract` | class (Lombok DTO) | C → C | C → C | |
| 30 | `oAuth2.oAuth2Request.OAuth2RequestCacheService` | class (service) | C → C | W → W | the ticket purpose promotes one bullet (object size) to the whole ticket's reason |
| 31 | `auth.authorization.boundary.AuthDataCacheRest` | class (controller) | C → C | V → V | mixes two tickets' themes |
| 32 | `messaging.consume.pactum.ContractMessage.Contract.Customer` | nested class | C → C | V → E | |
| 33 | `auth.authentication.token.event.TokenInvalidationEvent` | class | C → C | V → E | |
| 34 | `messaging.publish.user.UserDataMessagePublisher` | class (service) | C → C | C → C | |

### Rates (34 symbols)

| Field | Before | After |
| --- | --- | --- |
| `what` correct | 32 (94 %) | 33 (97 %) |
| `what` vague / invented | 1 / 1 | 1 / 0 |
| `why` correct | **11 (32 %)** | **12 (35 %)** |
| `why` wrong | **4 (12 %)** | **3 (9 %)** |
| `why` vague | 19 (56 %) | 7 (21 %) |
| `why` empty | 0 | 12 (35 %) |
| `why` acceptable (C + E) | 11 (32 %) | 24 (71 %) |
| misleading (W + I, both fields) | 5 | 3 |

| Subset | Symbols | `why` acceptable before | after |
| --- | --- | --- | --- |
| tuning (under the two `--path` prefixes: #1–8, #15, #21, #24, #26, #28, #32, #33) | 15 | 5 (33 %) | 12 (80 %) |
| held out (all others) | 19 | 6 (32 %) | 12 (63 %) |

No reviewed `why` that was correct before became empty; one became vague (#3)
and one vague one became wrong (#10).

*Corrections in the 4.5 review fixes:* #6 before is V, not W (before-wrong
5 → 4, misleading 6 → 5); the tuning/held-out split and the strict
correct/wrong/vague/empty rows lead; #3's cause is the emptied GRLD-40363
purpose. Verdicts were checked against the accessor/constructor rule (below):
none of the 12 after-correct whys is on a trivial accessor or a field-storing
constructor, so no reviewed verdict changes; the rule is applied in the
iteration notes, where kept flag-accessor whys were first called correct.

## Ticket summaries (13 tickets)

| Ticket (kind of input) | summary before → after | purpose before → after | note |
| --- | --- | --- | --- |
| GRLD-20961 (title only) | C → C | V → E | before: purpose restated the title |
| GRLD-35843 (link only) | C → C | V → E | same |
| GRLD-88420 (bug, title only) | C → C | V → E | same |
| GRLD-90364 (bug, title only) | C → C | V → E | same |
| GRLD-37944 (description) | C → C | C → C | emergency logout of everyone |
| GRLD-68743 (description) | C → C | C → C | memory problems at night |
| GRLD-98118 (description) | C → C | C → C | redirect loop |
| GRLD-109036 (description) | C → C | C → C | |
| GRLD-86428 (description) | C → C | C → C | data accuracy, order independence |
| GRLD-108384 (description) | C → C | C → C | |
| GRLD-96679 (title "Argus") | V → V | E → E | nothing more can be said |
| GRLD-28742 (title only) | C → C | E → E | |
| GRLD-27116 (bullet list) | C → C | W → W | one bullet promoted to the purpose; causes #30 above |

Summaries: 12/13 correct before and after (the 13th has nothing to
summarise). Purposes acceptable (C + E): **8/13 → 12/13**. Index-wide: 30 of
the 31 tickets without a description now have an empty purpose (22 before);
35/69 purposes empty (25 before). The 10 that became empty all restated their
title, but restating was not harmless everywhere: GRLD-40363 (description
only a chat link, so title-only) stated the organisation-deletion origin of
`TokenValidationRest#deleteAllFromList`, and without it the member's why took
the emergency-logout reason of a later ticket (#3). The ticket's summary
still names the organisation deletion; the model preferred the ticket that
has an explicit purpose.

## Prompt changes and how they were tested

1. *Member `why` (D-bj):* the reason must be the history's reason **for this
   member**; the history lists changes that touched whole files (features
   built around it, upgrades, migrations, refactorings); a reason that fits
   every member of the enclosing type is not specific enough; a history that
   only names a project or initiative ("a proof of concept for X") gives no
   reason; `why` is always empty for getters, setters, `equals`, `hashCode`,
   `toString` and constructors that only store their arguments.
2. *Type `why` (D-bj):* the same, plus for nested types: they share the
   enclosing file's history, so a reason only when the history explains the
   nested type itself.
3. *Type `what` (D-bj, open #32):* also "do not expand abbreviations or parts
   of names that nothing below explains".
4. *Ticket `purpose` (D-bk, open #28):* a title alone (or a link-only
   description) rarely gives a reason; then a purpose only when the title
   names a goal or effect beyond the change; fixing a named error or doing the
   named task is not a reason by itself.

Iteration with `--path` on `auth/authentication/token` (20 files, 93 members,
20 types): the first wording (specific reason, whole-file changes, "usually
empty for boilerplate") emptied only 13/93 member whys — accessors still
carried the proof-of-concept theme; adding the project-only and the
"always empty for accessors" sentences emptied 49/93 member and 7/20 type
whys and fixed reviewed items 5–8, 21. Then `--path messaging/consume/pactum`
(33 members, 14 types incl. three-level nesting): nested `ContractMessage`
types empty instead of repeating the file's initiative; flag accessors whose
ticket added exactly those flags kept their why. The first version of this
review called those kept whys correct; they are true, but they break the
"always empty for accessors" rule and are counted as non-compliance below,
not as correct (the rule is what makes empty whys predictable for search).
Full run, then the same 34 symbols and 13 tickets reviewed again (tables
above). Because the wording was tuned on these two prefixes, the 15 reviewed
symbols under them are not an independent test (split above).

## Index-wide

After the full run: member whys empty 242/479 (2 before), type whys 35/175
(0 before); `what` median 96 chars (100 before), max 159; no meta text
("ticket", "commit", "javadoc", "likely", "unclear"); no invented "mobile".

- *What was lost:* 240 member whys and 35 type whys that existed before are
  empty now; no symbol gained a why. 195 of the 240 members are getters,
  setters, `is…`/`has…`, constructors or `equals`/`hashCode`/`toString` (by
  name). Of the other 45, most carried a project or initiative theme shared
  with many symbols (the proof of concept, the login split, the AWS SDK
  upgrade, a title-only "Argus" ticket), but **roughly 5–8 lost a reasonably
  supported reason**, e.g. `auth.event.dynamoDB.DynamoDBEventService#publishAsync(DynamoDBEvent)`
  and `DynamoDBEventTableService#init()` / `#tableExists()` (the
  "cache is updated immediately" ticket created them; its summary is
  goal-like and it has no purpose), `auth.authentication.token.UserTokenService#removeToken(UUID,String)`
  (the move to single expiring token entries), `oAuth2.oAuth2Request.OAuth2RequestCacheService#find(UUID)`
  (UUID keys with a prefix, per its own commits) and
  `integration.cronus.CronusConnector#createSystemJwt(GrldSystems.SystemRoles)`.
  The losses are reasons the history implies rather than states for the
  member, which is exactly the line D-bj draws; whether they matter for
  search is for 5.3 to show.
- *Accessor/constructor rule compliance (prompt-only, D-bj):* of 120 trivial
  accessors (a `get`/`is`/`set` method whose body only returns or assigns a
  field) **13 still have a why** (89 % empty); of 44 constructors whose body
  only stores arguments (optionally after `super(…)`) **12 still have a why**
  (73 % empty); together 25/164 kept (≈ 85 % compliance). Examples: the
  request-object getters of the bulk-logout endpoints, the flag accessors of
  `BusinessFeatureMessage.OrganizationBusinessFeatures.Entry`, the
  event-dispatcher config constructors and the user-data message
  constructors. Not in these counts: 12 `getCachePrefix()` /
  `getCacheTTLSeconds()` overrides that only return a constant, which also
  keep a why. Most kept whys are true, but they contradict the rule, so the
  rule is best-effort, not a guarantee.
- *Per-initiative theme:* the "a reason that fits every member is not
  specific enough" instruction had **little effect** on classes built by one
  initiative: one why ("… handle authorization data independently of the
  core by building a parallel caching structure") appears verbatim on 33
  symbols and in variants on 54, among them 6 of the constant-returning cache
  prefix/TTL overrides. Kept open in `state.md` #30.

## Conclusions

- **Question 1 — answered with a qualification.** `what` is trustworthy:
  33/34 correct, the remaining one incomplete rather than wrong; it names
  real effects (events, limits, bypasses) because the source is in the
  prompt (D-at). `why` is trustworthy when present on a member with its own
  ticket (all reviewed service/controller methods with a dedicated ticket are
  correct), and an empty `why` now means "the history does not explain this
  symbol" instead of a borrowed project theme. Strictly correct whys stayed
  about where they were (32 % → 35 %); the improvement is fewer vague and
  wrong ones (wrong 12 % → 9 %), measured partly on the tuning prefixes
  (held out: acceptable 32 % → 63 %). 3/34 reviewed whys are still wrong, so
  a `why` should be read as "the ticket context of this code", not as
  verified fact.
- **Still weak:**
  - multi-ticket members and types: a recent ticket can lend its reason (#10,
    #11, #31) — the model cannot tell which part of a ticket touched which
    member;
  - tickets whose purpose promotes one bullet of a list (GRLD-27116 → #30);
  - classes created by a large initiative keep its theme verbatim (33 symbols
    share one "parallel caching structure" why — true, but not specific);
  - commit-subject-only members restate the subject (#15, #16, #28);
  - roughly 5–8 real but implied reasons were lost with the stricter rule,
    and one emptied title-only purpose (GRLD-40363) cost #3 its origin;
  - the accessor/constructor rule holds for ≈ 85 % only (prompt-only);
  - suspected, not seen on `argus`: record accessors (`name()`, no `get`;
    `argus` has no records), accessors with non-`get` names (German names
    such as `holeX`) and the type prompt's "this {kind}" wording echoed into
    the why — the rule names accessors by prefix, so these may keep a why
    (`state.md` open #35).
- **For the product owner to verify:** the verdicts on #3, #10, #11, #30
  (the wrong/vague ones), whether empty whys on getters/setters/constructors
  and nested types are acceptable for search (they embed fqn + what only),
  GRLD-27116's purpose, the 5–8 lost reasons named above, and a handful of
  the 242 empty member whys in a package they know (are any reasons lost that
  the tickets do state for that member?).
