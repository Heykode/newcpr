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
- Resolve real account sessions from per-frame client metadata, turn metadata,
  then explicit session IDs, as upstream `1240f225` specifies. Conversation,
  thread-only, generated `lc_` and cache/content keys are not real session identities.
  Supplemental hints remain explicit local routing hints. Native previous-response
  and restored provider state take precedence.
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
- Share the root account binding across children. A thread alone does not create
  a real session; local hints may still supply their independent routing binding.
  Search/Images keep original session/Search `id` priority. Compact keeps
  session > explicit cache > hint and parses the raw body once.
- Keep generated `lc_`, device/installation, UA, transport session/thread metadata,
  payload and physical WS pool identity independent from account preference.
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
| Thread-only plus changing hint | No real session; use the independent hint binding |
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
- Base: absent hints use upstream cache-preference selection without creating a real session; wire identity is independent.
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
- Strict and Preferred prioritize the binding ahead of account weights. Strict
  waits for its busy bound root when ordinary waiting is enabled; full/timeout
  cannot spill. With ordinary waiting disabled, a root may select another eligible
  account and CAS the new primary before send, as upstream does. Authorized Core
  retries that exclude the old root may also migrate; late completion cannot undo it.
  A local interval-only blocker does not add another Strict migration prohibition. Strict children remain follow-only and wait for
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

Wrong: change the transport identity seed while integrating account cache preference.

Correct: keep the existing transport `lc_` identity independent. Account cache preference
uses the official algorithm below; obsolete content-as-session tests must not restore
that retired routing behavior.

## 8. Sessionless cache preference (upstream 1240f225)

- Use upstream length-prefixed SHA-256 fields: model, instructions, initial
  system/developer texts, then the first user text. Ignore later turns, tools and
  media URLs; no first user text means no content preference. Explicit nonempty
  prompt cache keys take priority. Both namespaces are client-key isolated.
- Strict/Preferred honor the configured mode; Relaxed skips preference storage.
  Preferred may discard only optional cache preference on store failure, across
  direct, cached projection, waiting and admission paths. A CAS conflict still
  releases capacity and retries. Real session/Strict failures remain fail-closed.
- Cache preferences do not supply turn aliases, continuation observations, state
  owner proof or transport recovery keys. Suppress account_switch for preferences;
  do not infer Excel state ownership from their affinity_hit. Guardian success
  observations remain independently recorded.
- Responses turn reuse checks root key AND raw follow-only identity, independent
  of current affinity mode. Sibling children are compatible; root/child changes
  and different roots are rejected. Independent endpoints compare root and OR
  follow-only constraints, retaining the original alias on renewal.
- Historical hint migration fixtures must use actual pre-update stored keys;
  do not seed them by invoking the new sessionless preference implementation.

- Responses turn aliases use logical turn metadata from the original payload,
  independently of the header-authoritative transport `client_turn_id`, matching
  upstream `remember_generate_turn`. A shared transport header cannot merge distinct
  logical turns; different transport headers cannot hide a cross-root logical turn conflict.

## Successful Round-Robin Admission (v3.45.2)

### 1. Scope / Trigger

OpenAI immediate selection and capacity waiting share the official successful
allocation cursor. Affinity hits must not distort the order of new sessions.

### 2. Signatures

- `ProviderSessionAffinityPort::round_robin_cursor(provider, client_key) -> Result<u64, ProviderStoreError>` (async).
- `admit_account(provider, ProviderAccountAdmission::{Session, RoundRobin}) -> Result<bool, ProviderStoreError>` (async).
- `ProviderLeaseGuard::release(self: Box<Self>) -> BoxFuture<'static, Result<(), ProviderStoreError>>`.

### 3. Contracts

- The cursor is scoped to Client Key and Provider, with a 24-hour idle TTL. Read
  without advancing; atomically compare the observed cursor and session generation
  before writing either new binding or next cursor.
- Only an immediate, non-pinned RoundRobin allocation that misses affinity consumes
  a position. Hits, explicit/diagnostic picks, failed leases, queued promotion and
  alternate selection after queueing do not consume one. Ready before enqueue is immediate.
- Keep Strict/Preferred/Relaxed migration rules and Preferred's deterministic order
  with an existing binding. Optional Preferred cache binding failure can be discarded,
  but a real RR allocation must still commit the cursor without that claim.
- Keep local generation tokens and raw-ID/JSON legacy upgrade support. No second
  binding store is added; a late completion must not restore an old generation.

### 4. Validation & Error Matrix

| Result | Required action |
| --- | --- |
| `Ok(true)` | Deliver the admitted lease; cursor/binding changes are atomic |
| `Ok(false)` | Await actual lease release and refresh selection; do not use the three credential-snapshot retries |
| Session claim store error | Existing binding-store failure; no selection bypass except optional cache preference |
| Cursor-only store error / release error | Ordinary Store error; do not claim allocation success |
| Cancellation / elapsed deadline | Stop under original request control; do not reopen a budget |

### 5. Good / Base / Bad Cases

- Good: two concurrent first requests for one session share one binding and one position.
- Base: immediate account admission advances once, a later affinity hit advances zero times.
- Bad: binding or cursor has changed; neither write occurs and the reserved slot is released.

### 6. Tests Required

Keep the official new-session, concurrent-first-request, busy-lease and optional-cache
regressions. Real Redis `round_robin_commits_only_with_the_current_cursor_and_session_version`
checks atomic rejection; local `round_robin_wait_path_commits_only_before_queue_registration`
checks immediate, ready-before-enqueue and promoted branches. Existing dual-pool cleanup
checks that explicit release makes the slot available before returning.

### 7. Wrong vs Correct

Wrong: increment during snapshot reads, or `drop(guard)` then immediately reselect
while the slot may remain occupied. Correct: `admit_account(...)` after acquisition;
on CAS conflict, `guard.release().await?` before refreshing the current selection.
