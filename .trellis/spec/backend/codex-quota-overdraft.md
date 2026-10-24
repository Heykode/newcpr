# Codex Quota Overdraft

## 1. Scope / Trigger

The feature is default-off and applies only to OpenAI OAuth Codex text turns. It adds
final-wire shaping and a probe-approved scheduling override for accounts whose existing
QuotaState is Exhausted. It never replaces quota access, credential, cooldown, identity,
egress, affinity, Guardian, concurrency or request-spacing facts.

## 2. Signatures

- Runtime settings: codexQuotaOverdraftEnabled and codexQuotaOverdraftProbeModel.
- Database: runtime settings columns plus codex_quota_overdraft_states keyed by account_id.
- Core account projection: codex_quota_overdraft_passed boolean loaded only for the
  current config revision and credential revision while the switch is enabled.
- Admin quota projection: quota.overdraft with status, attempt, reason, timestamps and
  exact-window child statistics.

## 3. Contracts

- Invalid JSON and configured decompression limits remain gateway-api responsibilities.
  Do not add the reference implementation's separate 32 MiB business-request limit.
- Eligibility is decided once from the structured CodexResponsesRequest. The final input
  item must be a user message; image, compact, Excel, API-key, Live and tool-output-only
  turns are excluded.
- Injection occurs after account scoping and compatibility normalization and before
  transport selection. HTTP, SSE, WS and allowed fallback share the same request clone
  and deterministic call ID. Original Operation and replay history remain unchanged.
- Every eligible enabled request is injected. Only a probe request or a passed account
  still carrying QuotaState::Exhausted is marked as active overdraft. Active overdraft
  successes use PreserveAccess; normal injected successes retain ObserveAccess.
- A passed state overrides only QuotaExhausted local availability. All other scheduling
  filters and ranking remain authoritative. Failed/inconclusive states create no second
  long cooldown and keep the original quota state.
- A new explicit QuotaExhausted / UsageLimitExhausted fact atomically reclaims any
  passed/recovered state to pending before the account can be selected again. Runtime
  and admin projections also require the state's cycle key to match the current quota
  reset/access observation. Ordinary transient 429 cooldowns do not reclaim the pass.
- reset_at makes exhaustion refresh due; it is never recovery proof. A new explicit
  Allowed observation ends the cycle.
- Probe claims are fenced by account subject, credential revision, config revision,
  cycle, generation and owner token. The exact configured model bypasses global aliases.
- Account totals, Key billing and cumulative costs use the normal completed-request path
  exactly once. Overdraft window statistics query the same facts as a subset.

## 4. Validation & Error Matrix

- Switch off -> no injection, claim or selection override.
- Empty, untrimmed, control-character or over-128-byte model -> reject settings update.
- Model unavailable -> inconclusive/model_unavailable; never choose a fallback model.
- Completed probe -> passed without changing QuotaState.
- Explicit quota denial -> failed/quota_limited and original exhaustion remains.
- Auth failure -> failed/authentication_failed plus existing credential handling.
- Timeout, transport, 5xx or incomplete terminal -> inconclusive and bounded retry.
- Stale config, identity, credential, generation or owner -> ignore result.

## 5. Good / Base / Bad Cases

- Good: a 100% OAuth text account probes successfully, remains Exhausted, and only
  eligible injected text turns can select it.
- Base: the switch stays off after migration and all existing request bytes and routing
  decisions remain unchanged.
- Bad: probe success changes QuotaState to Allowed, allowing images or compact turns;
  or a separate pause outlives the original quota reset.

## 6. Tests Required

- Provider unit tests: payload allowlist, idempotent deterministic injection and requests
  above 32 MiB.
- Core selector test: passed state only overrides quota exhaustion when request-local
  eligibility allows it.
- PostgreSQL test: migration, single claim, retry, stale fencing, passed and recovered.
- API/frontend tests: settings roundtrip, invalid models, state projection, exact window
  keys, desktop/mobile and missing-field compatibility.
- Isolated integration: HTTP JSON, SSE, native WS, HTTP-to-WS, WS-to-HTTP fallback,
  quality checks and switch-off rollback.

## 7. Wrong vs Correct

Wrong: clear QuotaState after an injected probe completes.
Correct: persist passed separately and let selector override exhaustion only for eligible turns.

Wrong: append the synthetic pair to the canonical Operation or replay history.
Correct: mutate only the selected account's final CodexResponsesRequest clone.
