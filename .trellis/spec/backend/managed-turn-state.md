# Managed Turn State

## Ownership and Opt-In

- The global policy, OpenAI account switch and canonical upstream model allowlist
  must all match. Defaults are disabled globally and per account.
- The provider owns parsing and injection; the store owns atomic account/model
  records. Opted-in new-chain selection requires a valid active for the requested
  canonical model, in ordinary selection and capacity-wait reloads. Do not change
  affinity seeds, UA/TLS, installation IDs, cookies or ordinary IPv6 selection.
- Preserve client-turn state and exact continuations. Only an eligible new chain
  without existing turn state receives managed state after account scoping.
- A missing/unavailable record excludes the opted-in account/model from new chains
  and queues acquisition. Disabled policy/account switches and excluded models retain
  ordinary behavior. Exact native owners retain existing continuations.
  Foreground lookup has a one-second upper bound. Do not introduce a stale local
  active-state cache that hides cross-process promotion.

## Observation and Storage

- Validate the case-sensitive `gAAAAA` prefix and plan-specific encoded length.
  Do not decode the opaque envelope or infer its upstream expiry. Locally assign
  a first-capture timestamp and a one-hour lifetime; these are policy, not proof
  of acceptance, cryptographic validity or upstream lifetime.
- Unknown prefixes/shapes cannot replace stored values. Only the explicitly
  recognized degraded shape can request standby promotion.
- Compare the final reported model before learning a completed/failed response's
  state. Preserve the existing immediate per-client state forwarding separately.
  Cancellation or missing state cannot invalidate stored active state.
- Passive learning never awaits PostgreSQL on the streaming path. Use the
  process-local, account/model-coalesced queue, bounded to 256 keys, and a Host
  daemon with cancellation and a one-second timeout per store operation.
  Queue saturation drops supplemental observations, not user requests.
- Repeated identical active values never increment `state_version` or restart
  expiry. An echoed standby also retains its original capture/expiry. Older
  candidates cannot replace newer active values. Preserve valid old active as
  standby on replacement; standby-only writes do not advance the version.
  Deduplication covers the persisted slots, not a permanent history of all
  previously discarded values. Do not claim detection of arbitrary historical
  replays or an upstream-guaranteed lifetime.
- Rejection promotes only a standby valid at the observation time. Proactive
  refresh promotes a standby only when it has over ten minutes left and active
  has at most ten minutes left (or is absent). Check version, policy, credential,
  and remaining lifetime under the row lock; preserve the promoted clock.
  Plan-shape changes clear
  both slots and the old observed length, and advance the version.
- State reads and writes are fenced by `turn_state_binding_revision`, separate
  from the credential-material write CAS. The legacy State row column named
  `credential_revision` stores this binding generation from migration 0027.
  Writes lock current
  global/model policy and account opt-in before locking the state row. A rotated
  credential cannot inherit the previous credential's active or standby.
- Identical accepted Cookie sets do not write. Provider-verified identical
  credentials or `__cf_bm`-only changes may preserve binding, clocks, slots and
  pool version. Other cookies, tokens, principal/device/client/scope changes
  remain hard invalidations. Only trusted provider code can request preservation;
  ordinary import/rotation paths default to hard invalidation. Generic credential
  CAS and device locking remain unchanged.
- Response State observations always use the original request lease's binding,
  not the account reloaded after response Cookie persistence. Maintenance checks
  binding ownership but reloads current request material between batches when
  the credential CAS changes. In-flight batches may finish with their original
  soft material; hard changes still cancel and fence persistence.
- Migration initializes binding from current credential CAS without rebinding
  stale State rows. Deploy with all old writers stopped: old binaries do not
  know the binding field and must not share this database with new writers.
  A downgrade requires a schema-aware restore or explicit State invalidation;
  never claim swapping only the old binary is a transparent rollback.
- Degraded observations apply only to the managed version actually injected.
  Late rejection of an older version cannot rotate the current active. If no
  valid standby exists, clear the rejected active and block new chains until acquisition.
- Normal passive observations carry the same injected-version fence. A late
  normal echo must not restore a rejected active or undo standby promotion.
  Coalescing prefers newer credential/version observations, then rejection over
  a normal echo at the same version.
- Normal replacement leaves refresh status pending until a new standby is
  stored. Repeated identical values do not clear this pending work or restart
  the token's local capture clock. Existing pre-upgrade expiry is never extended.
- Opaque values must not appear in Debug, ordinary logs or admin list responses.

## Account-List Safe Projection

- The authenticated account list may expose only per-model control metadata:
  refresh status, active/standby presence, character count and local expiry.
  Preserve the existing required/ready model fields for compatible clients.
- Project a slot only when global/account policy, enabled status, model,
  State binding revision, plan length, issuance and local expiry all still match.
  Invalid, expired or mismatched rows appear as missing slots; never expose the
  opaque value, its prefix, preview, hash or any reversible derivative.
- `readyModels` remains active-only scheduling readiness. The detailed model
  list may include a valid standby and the current refresh status so the UI can
  distinguish ready, standby refill and acquisition. Slot counts are display
  metadata, not proof of upstream acceptance or future validity.
- Keep configured model order from the runtime allowlist. PostgreSQL regression
  must cover active-only and active-plus-standby projection together with the
  existing policy, revision and plan fences.

## Request-Level Admin Diagnostics

- The usage-log feature has explicit owner authorization to retain the full
  managed injected value in authenticated request detail. This is a narrow
  exception, not permission to add raw State to traces, ordinary logs or exports.
- `TurnStateCapture` is local to each provider attempt. Capture the final HTTP
  header or prepared WS payload after managed expiry cleanup, not a later read
  of the account's active value. Client passthrough is not managed injection.
- Persist a bounded `turnState` object in the existing provider observation:
  full `injectedState` (at most 2048 bytes) plus a separate `summary` containing
  `injected`, `preview`, `chars`, `returnedChars`, `returnedSame` and `transport`.
  Do not persist the full returned State in this diagnostic object.
- The usage list selects only `{turnState,summary}` from PostgreSQL and parses
  it through a typed presenter. Unknown fields cannot expose the full value.
  Missing evidence differs from known absence of managed injection.
- Detail keeps the existing admin authentication, `no-store` response and
  request-log retention. Do not add another data-path store call or migration.
- Supplemental evidence must not displace preexisting diagnostics at the
  provider metadata size limit. Preserve the original observation on failure.
- Return length and exact equality are independent facts, not proof of upstream
  acceptance, token lifetime or response quality. Missing echo is not rejection.
- Test HTTP expiry/rejection, WS payload evidence, request isolation, redacted
  Debug, metadata bounds, real PostgreSQL detail/list projection and safe export.

## WebSocket Pool

- Add model/version to a new-chain pool key only when managed state was actually
  injected. Disabled/unmanaged traffic retains its previous key and diagnostic
  hash behavior.
- Model/version partitions new chains. Exact response ownership still resolves
  the original physical connection using the existing account, downstream Key,
  conversation, route and response-ID boundaries, including after policy disable.
- Never globally evict an account's sockets on a state update. Previous owners
  remain under ordinary pool capacity and lifecycle rules; active continuations
  must not be migrated merely to apply a new state.
- Expiry is pool metadata, not a new hash/identity input. Retired/expired managed
  pools refuse new chains. Return/maintenance closes sockets without a response
  owner, while exact owners remain available under normal lifetime/capacity rules.
- Recheck expiry after opening/capacity waits, before sending the first business
  payload. Discard the unsent opening and return a local NotSent readiness failure
  on expiry or retirement; never silently send a managed new chain uninjected.
  Never retry an already-sent payload as a side effect of state refresh.

## Maintenance Isolation

- The scheduled leader cycle discovers targets; a separately supervised daemon
  drains a bounded, coalescing FIFO. Discovery resumes fairly after saturation.
  Passive active replacement wakes acquisition without waiting for discovery.
- Account/model tasks run independently without the former 32-task cap. A key
  never overlaps itself. Running-key wakeups coalesce separately so they cannot
  fill the bounded pending discovery queue; restart clears stale running ownership.
- Urgent acquisition sends one probe, then batches of ten until success or
  invalidation. Healthy-active standby acquisition sends one immediately, then
  one every six seconds after a miss. Re-evaluate current state/expiry between
  batches and during standby waits; active changes restart immediate refill.
  There is no 500-attempt or whole-task lifetime limit.
- The IPv6 transport owns a separate 240-connection guard, not the former
  collector-level 100-probe semaphore. It is not an account business limit.
  Waiting for capacity spends no per-request timeout. Each probe creates a
  non-reused source-bound client. Cycle shuffled enabled addresses; do not
  promise unlimited unique addresses from a finite pool.
- Never read/acquire business scheduling leases, advance rotation, write affinity
  or update business account feedback. Business load cannot starve maintenance.
- Individual probes are bounded to thirty seconds, excluding semaphore waiting.
  HTTP errors, missing states and unusable candidates continue on the applicable
  urgent/background cadence without special 429 backoff or an attempt budget.
  Poll account/binding/policy every second and recheck before each batch and
  persistence. Switch disable, model removal or credential replacement cancels
  outstanding probes. Cancellation performs revision-fenced status cleanup even
  after opt-out; it cannot finish a newer credential's task. Store unavailability
  prevents starting collection.
- Probes accept HTTP 200 and eligible headers only, then drop both success and
  error bodies without draining them or checking SSE/model output. This is
  capture, not proof that generation completed. Business response processing
  and its final-model observation checks remain unchanged.
- Stop a batch at the first accepted fresh state and cancel remaining probes.
  A repeated active/standby is not a new acquisition. No decoded candidate-age
  gates remain. Both slots use the ten-minute refresh threshold; a newly
  captured distinct value receives its local one-hour clock.
- These are Tokio tasks under the existing Host, not dedicated per-account OS
  threads or resource-isolated processes. Their network/CPU cost still exists.
- The explicit local rejection-code allowlist is `invalid_turn_state`,
  `turn_state_expired`, and `turn_state_mismatch`. It is not a claim about an
  upstream documented contract. Generic 401/429/5xx, missing state and answer
  content are not rejection evidence.

## Verification

- Private parser/queue tests are narrowly audited at `provider/turn_state.rs`;
  never expose production test hooks or exempt a directory.
- PostgreSQL tests must actually run: enable CI fail-on-missing-environment
  checks and supply isolated test-schema endpoints.
- Cover concurrent duplicate observations, older values, local capture/expiry,
  plan reset, model/account separation, bounded observation handling, exact WS
  ownership across version updates and switch disable, and mixed-provider UI.

## Scenario: Concurrent Responses and Independent Switches

### 1. Scope / Trigger

Apply when coalescing passive state observations, persisting candidates, or
changing an account's State switch while other requests or administrators act.

### 2. Signatures

- `ProviderTurnStateCandidate.expected_active_version: Option<u64>` carries the
  managed version injected into a business request; independent probes use `None`.
- `PendingObservations::enqueue` orders by credential revision, optional injected
  version and anomaly priority.
- `POST /api/admin/accounts/batch-update` accepts a single `accountIds` entry and
  `turnStateInjectionEnabled` without other account fields.

### 3. Contracts

Check the passive candidate's expected version while holding the state-row lock,
before changing slots, status or timestamps. Keep existing credential/policy
fences and probe behavior. Menu toggles patch only the State field and reread the
authoritative account list. Ordinary enable/disable omits the State field.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Late normal echo after promotion or clearing | Return unchanged current record |
| Same-version rejection followed by normal echo | Keep queued rejection |
| Newer-version candidate followed by old rejection | Keep newer candidate |
| Independent fresh probe | Existing candidate validation and replacement |
| Stale account row during State toggle | Do not submit old scheduling fields |
| Repeated menu click while pending | One mutation; existing guard remains |

### 5. Good / Base / Bad Cases

Good: rejecting A promotes B, and A's delayed normal echo cannot restore A.
Base: a current-version normal observation retains existing learning behavior.
Bad: accepting an echo merely because its issue timestamp equals B's timestamp.

### 6. Tests Required

Use real PostgreSQL for promotion/clearing followed by a late normal echo.
Exercise queue ordering in both directions and run the actual account mutation
composable against concurrent scheduling changes, duplicate clicks and non-OpenAI
accounts. Require final full backend/frontend regression after the fixes.

### 7. Wrong vs Correct

Wrong: let the last queued response win, or submit the entire visible account row
when changing only State injection.
Correct: preserve versioned rejection evidence and send the smallest supported
partial account update. These tests do not prove real-upstream state acceptance.
