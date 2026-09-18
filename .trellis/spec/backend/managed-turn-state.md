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

- Parse the opaque value's Fernet-shaped envelope, not its plaintext. Timestamp
  and length checks are local policy, not cryptographic verification or proof of
  an upstream risk classification.
- Reject malformed, unknown-shape, future-issued or expired observations. Only
  the explicitly recognized degraded shape can request standby promotion.
- Compare the final reported model before learning a completed/failed response's
  state. Preserve the existing immediate per-client state forwarding separately.
  Cancellation or missing state cannot invalidate stored active state.
- Passive learning never awaits PostgreSQL on the streaming path. Use the
  process-local, account/model-coalesced queue, bounded to 256 keys, and a Host
  daemon with cancellation and a one-second timeout per store operation.
  Queue saturation drops supplemental observations, not user requests.
- Repeated identical active values never increment `state_version`. Older
  candidates cannot replace newer active values. Preserve valid old active as
  standby on replacement; standby-only writes do not advance the version.
- Promote only a standby valid at the observation time. Plan-shape changes clear
  both slots and the old observed length, and advance the version.
- State reads and writes are fenced by credential revision. Writes lock current
  global/model policy and account opt-in before locking the state row. A rotated
  credential cannot inherit the previous credential's active or standby.
- Degraded observations apply only to the managed version actually injected.
  Late rejection of an older version cannot rotate the current active. If no
  valid standby exists, clear the rejected active and block new chains until acquisition.
- Normal passive observations carry the same injected-version fence. A late
  normal echo must not restore a rejected active or undo standby promotion.
  Coalescing prefers newer credential/version observations, then rejection over
  a normal echo at the same version.
- Normal replacement leaves refresh status pending until a new standby is
  stored. Repeated identical values do not clear this pending work or restart
  the token's issuance clock.
- Opaque values must not appear in Debug, ordinary logs or admin list responses.

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
- Up to 32 account/model tasks run concurrently per replica. A key never overlaps
  itself; pending followups stay coalesced. A shared 100-probe semaphore bounds
  transport resources without sharing per-key acquisition budgets.
- Each active or standby acquisition has its own 500-attempt budget. Batch sizes
  are 1, 10, 20, ..., capped at 100, with the last batch truncated to the remainder.
  A smaller IPv6 pool is cycled rather than silently shortening the budget. Only
  configured enabled addresses are used; 500 globally unique addresses cannot be
  promised when fewer are configured.
- Never read/acquire business scheduling leases, advance rotation, write affinity
  or update business account feedback. Business load cannot starve maintenance.
- Individual probes are bounded to thirty seconds, excluding semaphore waiting.
  There is no whole-task ten-minute cutoff. HTTP errors, missing states and unusable
  candidates continue until success or 500 attempts.
  Poll account/revision/policy every second and recheck before each batch and
  persistence. Switch disable, model removal or credential replacement cancels
  outstanding probes. Cancellation performs revision-fenced status cleanup even
  after opt-out; it cannot finish a newer credential's task. Store unavailability
  prevents starting collection.
- Stop a batch at the first accepted fresh state and cancel remaining probes.
  A repeated active/standby is not a new acquisition. Require over ten minutes
  of remaining lifetime for active and over thirty minutes for standby.
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
- Cover concurrent duplicate observations, older values, expiry/future time,
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
