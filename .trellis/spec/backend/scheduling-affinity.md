# Supplemental Scheduling Affinity

## 1. Scope / Trigger

Apply to OpenAI ingress session hints, account affinity, retry/continuation
handling, and standalone endpoint routing. Complement `client-continuity.md`;
do not replace transport identity, account authorization or billing contracts.

## 2. Signatures

- `gateway_protocol::openai::OpenAiSchedulingSessionHint`:
  `new`, `source`, `id`, `to_context_value`, `from_context`.
- API-owned protocol context:
  `openai_scheduling_session_hint: {"source":"x-session-id","id":"example"}`.
- Ordered sources: `x-session-affinity`, `x-session-id`, `x-opencode-session`,
  `x-conversation-id`, `x-qx-affinity-codex-session`,
  `x-qx-affinity-claude-session`.
- `CodexSessionAffinity::migration_key()` references the unchanged old key.
  Existing `ProviderSessionAffinityPort::{load,claim_or_load,compare_and_bind,admit_binding}`
  own persistence; there is no new schema/configuration or audit stream.

## 3. Contracts

- Accept exactly one header value. Trim then require 1..1024 ASCII graphic bytes.
  Skip invalid/repeated sources and try the next source. Do not forward any of
  these headers, including invalid/unselected ones, to the upstream.
- Read hints from ingress-created context, never from wire body or a WS frame's
  purported context. The shared provider validator rechecks the source/value.
  A client hint is not authenticated end-user identity.
- Preserve standard session, conversation, thread and explicit valid cache
  precedence. Native previous-response and restored provider state also win.
  An `account_state_owner` alone is NOT evidence of native continuation: Core
  adds it on ordinary retries too. Such retries must retain the new affinity key
  and associated existing transport recovery state.
- The new domain is `supplementary-session`; source and ID are NUL-delimited
  before hashing, within the existing client-key and provider scopes.
  Never log a new raw hint as an anchor.
- On a primary miss, the old binding is only a preference, not authorization.
  Recheck enabled status, model/account scope, exclusions, quota and lease
  capacity. Use atomic admission CAS against the observed generation (or observed miss) on the new key only.
  A primary hit never consults migration. Store unavailability is not a miss.
- Retain original root-plus-child rules. An explicit standalone thread
  suppresses new hints; do not invent a hint parent for an existing thread.
  Search/Images keep original session/Search `id` priority. Compact keeps
  session > explicit cache > hint and parses the raw body once.
- Keep content fallback, generated `lc_`, device/installation, UA, client
  session/thread metadata, payload and physical WS pool identity unchanged.
  An existing exact continuation still belongs to its original account/socket.
- Existing diagnostic affinity hashes may reflect the new routing key.
  They do not authenticate a user or select a native response owner.
- Do not add tools, mutable instruction suffixes or structured media URLs to
  the shared content fallback under this contract.

## 4. Validation & Error Matrix

| Input/state | Required behavior |
| --- | --- |
| No valid hint | Original behavior |
| Duplicate highest-priority source, valid next source | Skip duplicate, use next |
| Same ID, different source or client key | Different new affinity keys |
| Explicit standard thread plus changing hint | Keep original thread key |
| Standalone endpoint thread without a root/cache | Keep original lack of binding |
| New key absent, old account eligible | Claim new key with old account preference |
| Old account disabled/outside scope/excluded/exhausted/busy | Normal authorized selection |
| New key already bound | No legacy read or rewrite |
| Binding store unavailable or over 500 ms | SessionBound failure before send; no selection bypass or retry |
| Ordinary retry with state owner | Retain hint routing/recovery key |
| Native continuation with conflicting hint | Strict rejects a conflicting current binding with ContinuationOwnerChanged; other modes retain exact owner without restoring an old primary |
| Account changes during full replay | Original account-state stripping still applies |

## 5. Good / Base / Bad Cases

- Good: two different conversation hints inherit one old content binding, then
  one fails over without rewriting the other hint or the old binding.
- Base: absent hints keep the original content-based selection and wire fields.
- Bad: infer that all users behind one business key are the same conversation.
- Bad: treat different content hashes as proof of different people, or claim
  sharing an upstream account necessarily means sharing conversation state.

## 6. Tests Required

- Protocol validation and ingress HTTP/Chat/WS/raw endpoint projections; reject
  body-context spoofing and prove local headers are not forwarded.
- Affinity source/client/thread isolation, explicit priority, actual persisted
  HMAC `lc_` compatibility, and unchanged account projection.
- Migration on real misses, unavailable store, old-account restrictions,
  independently evolving hints, and barrier-synchronized concurrent first claims.
- Ordinary retry with owner keeps the new key. A real loopback WS continuation
  with a conflicting hint keeps its socket and account.
- Real local HTTP body/header equality and complete Provider/API/Protocol
  regressions. Synthetic fallback experiments are not runtime acceptance.
- Assert primary binding claims, migrations and renewals at admission, before
  sending. Only Guardian parent success records require canonical completion;
  a wire event alone does not prove that success was recorded.
- Direct-selector CAS and lease-release fixtures must enable the simulated
  capacity coordinator; otherwise the after-acquire hook never runs and the
  guard is a no-op. Assert the winning account and released lease on both paths.
- Allow only the timer's millisecond rounding when checking paused-time storage
  deadlines; do not loosen a 100 ms policy budget to the 500 ms affinity budget.
- Align upstream 5877b0cb with a 500 ms budget for binding reads, initial claims,
  renewal and turn aliases; cyber-policy hints retain their independent 100 ms
  budget. Do not treat an unavailable binding store as a missing key. Cover
  direct and queued admission with 250 ms success and 750 ms rejection, no
  outbound send, prohibited retry and released capacity. Optional guardian
  parent preferences and post-completion observations remain best effort.

## 7. Wrong vs Correct

### Optional Preferred Mode

- Migration 0068 only widens the existing check constraint. Strict remains the
  default; upgrades do not rewrite existing settings or account identity.
- Preferred uses the root binding for children and selects an eligible primary
  before weight-based alternatives. Temporary spillover renews the observed
  primary generation; streaming and compact success must not move that primary.
- Guardian parent observation still records the account actually used. This is
  not a second primary binding and cannot be dropped to simplify binding counts.
- Prefer ready alternatives over the soft sticky queue in Preferred. Hard native
  and required owners remain pinned; model authorization, exclusions, quotas,
  credential validation and live capacity still gate every candidate.
- A retained alias without a primary may use the existing atomic initial claim.
  Unavailable storage is not a missing binding. Connection-local replay is
  rejected before selection, leases, alias writes or primary renewal.
- Relaxed now shares the root binding. It may move the root on admission; do not
  restore the previous independent-child behavior or migrate the primary on completion.
  Cover restoration, native-owner conflicts, concurrent claims, successful
  spillover/return, compact completion, model restrictions and capacity-wait selection.
- Align turn aliases with upstream `e39041782`: new records name the root
  session and retain the child's follow-only fact independently of the current
  mode. Resolve legacy child-plus-root records to the root and renew the original
  record without rewriting it. Accept siblings of the same root; reject another
  root. Thread transport keys and actual continuation/socket ownership remain
  separate. Do not restore the obsolete test that rejected a sibling solely for
  having a different thread ID.
- Preferred retains a temporarily blocked primary (capacity, interval, cooldown,
  model restriction, quota reset within the recovery window). Deleted, disabled,
  expired, invalid or banned accounts and unknown/far/stale exhausted-quota resets
  migrate at admission. Use the official session hash for stable spillover ordering.
- Strict and Preferred prioritize the binding ahead of account weights. Per the
  user's explicit strict-mode contract, a busy bound root cannot spill even when
  ordinary waiting is disabled. Authorized Core retries that exclude the old root
  may admit another eligible account and CAS the root before send; completion of
  an old request cannot undo it. Strict children remain follow-only and wait for
  the root to recover or migrate rather than independently changing the root.
- Redis admission accepts legacy raw and JSON bindings, upgrades to a new token,
  and fences both observed-miss and observed-generation races. Conflict must not
  refresh TTL; an expired observed generation cannot recreate a key.
- The 200 ms projection cache is bounded to 32 entries / 8192 candidates, coalesces
  fills and never grants capacity. Scope, client, model, policy and concurrency
  pool are isolated; local route/path/overdraft and request eligibility fields
  also partition the cache. Selected-account credential/live-capacity/revision
  checks still run after acquisition. Contention or invalidation refreshes the pool.
  The frozen scope defines the authorized ID universe; a cached projection must
  not permanently exclude a currently eligible ID that its earlier read omitted.

Wrong: clear local hints whenever `account_state_owner` is present.

Correct: protect actual restored/native continuation; an ordinary retry carries
an owner for account-state stripping and must keep its original routing key.

Wrong: strengthen a shared content seed with expiring signed URLs.

Correct: retain the seed unless representative fixtures demonstrate benefits
without introducing unacceptable splits; do not change `lc_` incidentally.
