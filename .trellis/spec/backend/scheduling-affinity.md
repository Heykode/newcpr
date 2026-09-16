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
  Existing `ProviderSessionAffinityPort::{load,claim_or_load,compare_and_bind}`
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
  capacity. Reuse atomic first claim and compare-and-bind on the new key only.
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
| Store load unavailable | Original fail-open selection, not migration |
| Ordinary retry with state owner | Retain hint routing/recovery key |
| Native continuation with conflicting hint | Original exact owner wins |
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
- Require canonical completion in fixtures used to assert successful affinity
  renewal; a wire event alone does not prove that success was recorded.

## 7. Wrong vs Correct

Wrong: clear local hints whenever `account_state_owner` is present.

Correct: protect actual restored/native continuation; an ordinary retry carries
an owner for account-state stripping and must keep its original routing key.

Wrong: strengthen a shared content seed with expiring signed URLs.

Correct: retain the seed unless representative fixtures demonstrate benefits
without introducing unacceptable splits; do not change `lc_` incidentally.
