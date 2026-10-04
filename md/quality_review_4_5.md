# Quality review 4.5 — what/why records on `argus`

Step 4.5 of [`plan.md`](./plan.md): are the generated records trustworthy
(POC question 1)? The agent reviewed 34 symbols and 13 ticket summaries of the
real `argus` index against the source and the ticket text, changed the
prompts where the evidence pointed, re-ran the full index and reviewed the
same items again. **The verdicts are the agent's, standing in for the product
owner, who confirms or overrides them (`state.md` open #34).**

fqns are written relative to the repository's root package (the employer's
package prefix is left out, as in the scrubbed fixtures). No ticket text or
code is copied here; notes paraphrase.

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
| 3 | `auth.authentication.token.boundary.TokenValidationRest#deleteAllFromList(UserTokenDeleteAllForUsersRequestTO)` | method | C → C | C → V | after: drops the organisation-deletion origin and claims a super-admin restriction the code (system role) does not have |
| 4 | `auth.authentication.token.boundary.TokenPreValidationRest#preValidateJwt(HttpServletRequest)` | method | C → C | C → C | |
| 5 | `…databaseCacheStrategy.UserTokenEntryRepository#deleteTokensBeyondNewest(UUID,int)` | method | C → C | V → C | before also borrowed the sibling scheduler's monitoring goal |
| 6 | `…databaseCacheStrategy.DatabaseCacheUserTokenStrategy#removeToken(UUID,String)` | method | C → C | W → C | before: reason of a later research ticket; after: the storage migration that created it |
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
| `why` correct | 11 (32 %) | 12 (35 %) |
| `why` empty | 0 | 12 (35 %) |
| `why` vague | 18 (53 %) | 7 (21 %) |
| `why` wrong | 5 (15 %) | 3 (9 %) |
| `why` acceptable (C + E) | **11 (32 %)** | **24 (71 %)** |
| misleading (W + I, both fields) | 6 | 3 |

No reviewed `why` that was correct before became empty; one became vague (#3)
and one vague one became wrong (#10).

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
35/69 purposes empty (25 before).

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
ticket added exactly those flags kept their why (correct). Full run, then the
same 34 symbols and 13 tickets reviewed again (tables above).

Index-wide after the full run: member whys empty 242/479 (2 before; 197 of
them getters, setters, `is…`, constructors, `equals`/`hashCode`/`toString`),
type whys 35/175 (0 before); `what` median 96 chars (100 before), max 159; no
meta text ("ticket", "commit", "javadoc", "likely", "unclear"); no invented
"mobile". Among 40 non-accessor symbols whose former why was unique, a few
lost a weak but real reason (event-publishing members of the "cache is
updated immediately" ticket, which has a goal-like summary and no purpose).

## Conclusions

- **Question 1 — answered with a qualification.** `what` is trustworthy:
  33/34 correct, the remaining one incomplete rather than wrong; it names
  real effects (events, limits, bypasses) because the source is in the
  prompt (D-at). `why` is trustworthy when present on a member with its own
  ticket (all reviewed service/controller methods with a dedicated ticket are
  correct), and an empty `why` now means "the history does not explain this
  symbol" instead of a borrowed project theme. 3/34 reviewed whys are still
  wrong (9 %), so a `why` should be read as "the ticket context of this
  code", not as verified fact.
- **Still weak:**
  - multi-ticket members and types: a recent ticket can lend its reason (#10,
    #11, #31) — the model cannot tell which part of a ticket touched which
    member;
  - tickets whose purpose promotes one bullet of a list (GRLD-27116 → #30);
  - classes created by a large initiative keep its theme verbatim (33 symbols
    share one "parallel caching structure" why — true, but not specific);
  - commit-subject-only members restate the subject (#15, #16, #28);
  - a few real but goal-like reasons were lost with the stricter rule (see
    above).
- **For the product owner to verify:** the verdicts on #3, #10, #11, #30
  (the wrong/vague ones), whether empty whys on getters/setters/constructors
  and nested types are acceptable for search (they embed fqn + what only),
  GRLD-27116's purpose, and a handful of the 242 empty member whys in a
  package they know (are any reasons lost that the tickets do state for that
  member?).
