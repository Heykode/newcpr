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

- Validate the URL-safe base64 envelope, version, cipher-block count and timestamp.
  Personal/team plans use ten/twelve blocks; optional padding does not change qualification.
  Team shape includes `team`, `business`, `self_serve_business_prolite` and
  `self_serve_business_usage_based`, case-insensitively after trimming; keep the
  provider qualifier and store admin projection aligned.
  Estimate expiry as issuance plus one hour, tolerating thirty seconds of future clock skew.
  This cannot prove signature validity, upstream lifetime or quality.
- Probe publication requires HTTP 200 and explicit SSE `response.completed`.
  Failures, including a later error in the same stream, win over completion.
  Do not reject solely because the reported response model differs.
- Business observations never publish candidates. Missing State does not invalidate.
  Two consecutive suspect observations for the injected version invalidate it;
  a healthy observation resets one strike. Explicit State rejection is immediate.
- Passive learning never awaits PostgreSQL on the streaming path. Use the
  process-local, account/model-coalesced queue, bounded to 256 keys, and a Host
  daemon with cancellation and a one-second timeout per store operation.
  Queue saturation drops supplemental observations, not user requests.
- Repeated identical active values never increment `state_version` or restart
  expiry. An echoed standby also retains its original capture/expiry. Older
  candidates cannot replace newer active values. Do not recycle old active as
  standby on replacement; standby-only writes do not advance the version.
  Deduplication covers the persisted slots, not a permanent history of all
  previously discarded values. Do not claim detection of arbitrary historical
  replays or an upstream-guaranteed lifetime.
- Rejection promotes only a standby with more than one minute remaining.
  Proactive refresh starts at fifteen minutes remaining and promotes a newer
  standby only when active has at most one minute left (or is absent).
  Check version, policy, credential,
  and remaining lifetime under the row lock; preserve the promoted clock.
  Plan-shape changes clear
  both slots and the old observed length, and advance the version.
- State reads and writes are fenced by `turn_state_binding_revision`, separate
  from the credential-material write CAS. The legacy State row column named
  `credential_revision` stores this binding generation from migration 0027.
  Writes lock current
  global/model policy and account opt-in before locking the state row. A rotated
  credential cannot accept the previous credential's late writes.
- Identical accepted Cookie sets do not write. Provider-verified identical
  credentials or response Cookie material changes may preserve binding, clocks,
  slots and pool version. Token changes still advance the authentication binding.
  ProviderDeviceCodec authorizes same-owner retention independently: require a
  resolved principal, same installation/client/scope, and Store-verified nonempty
  user/workspace and unchanged normalized plan. Rebind slots in the credential
  transaction from the immediately previous binding to the new binding; bump
  state_version, stop old refresh ownership and discard expired slots, without
  renewing any clock or clearing State rejection evidence. Apply this to Core
  refresh CAS, Store CAS, admin JSON upsert and admin OAuth/relogin rotation.
  Unknown codecs/identity/plan deny retention. Cookie soft CAS retains its cheap
  path and does not run State retention queries. Generic credential CAS and
  device locking remain unchanged.
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
  Coalescing prefers newer credential/version observations and retains two ordered outcomes.
  Two strikes or explicit rejection cannot be erased by a later same-version echo.
- Active publication finishes acquisition immediately. A healthy active does not
  trigger permanent standby refill. Save capture timestamps independently of issue/expiry.
  Publication carries the version seen before the batch, not a newly read version.
- Opaque values must not appear in Debug, ordinary logs or admin list responses.

## Account-List Safe Projection

- The authenticated account list may expose only per-model control metadata:
  refresh status, active/standby presence, character count and local expiry.
  Preserve the existing required/ready model fields for compatible clients.
- Cached slot projection requires model, binding revision, plan shape, issuance and
  expiry to match. Off/error accounts can still inspect these cached clocks.
  Invalid, expired or mismatched rows appear as missing slots; never expose the
  opaque value, its prefix, preview, hash or any reversible derivative.
- `readyModels` additionally requires enabled policy/account, healthy credentials,
  quota, no shared cooldown, and more than one minute remaining. The detailed model
  list may include a valid standby and the current refresh status so the UI can
  distinguish ready, queued, collecting, cooldown and awaiting switch. Slot counts are display
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
- WS opening profiles additionally partition by a full digest of authentication
  headers, never raw Tokens. This applies to unmanaged new chains too, so keeping
  an unchanged State cannot reuse a previous Token's connection. It does not
  alter thread or prompt-cache identity. Exact owners bypass opening-profile
  changes under the existing continuation lifetime/capacity rules.
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

- `turnStateProbeProxyId` is a nullable saved-proxy reference: null selects IPv6,
  omission during settings replacement preserves selection. Resolve credentials
  from the catalog at snapshot compilation; never duplicate them into settings.
  A selected proxy must have passed its catalog test. The FK restricts deletion.
  Take the runtime settings lock before the proxy row lock, matching proxy edits.
- Proxy-mode probes need no IPv6 runtime and never advance its cursor. Cache one
  dedicated HTTP client keyed by proxy value and custom-CA configuration, separate
  from business clients. Use HTTP/1, no idle pool and `Connection: close` for a new
  connection on every probe. Keep TLS validation; never copy insecure TLS options.
  New connections do not guarantee a different provider-assigned public IP.
- Freeze the resolved proxy per collector; recheck policy on dispatch, response
  processing and the cancellation watcher. Changed egress requeues collection,
  but does not invalidate usable State. These provider-side checks are not a
  database-atomic proxy-revision fence. Identity/policy/state-version fences in
  the store remain authoritative. Proxy failures must never fall back to direct
  or account business egress.
- The settings UI persists only the proxy ID and preserves it when catalog loading
  fails. Disabled/untested catalog entries cannot be newly selected.

- The scheduled leader cycle discovers targets; a separately supervised daemon
  drains a bounded, coalescing FIFO. Discovery resumes fairly after saturation.
  Passive active replacement wakes acquisition without waiting for discovery.
- Existing account-facts callbacks after import, rotation and relogin also queue
  eligible targets immediately. Preserve a follow-up when the old binding's task
  is still running; callbacks must not create a second collector or bypass admission.
  Periodic discovery remains the fallback for automatic recovery and missed callbacks.
- At most five distinct collecting accounts run concurrently, with FIFO dynamic refill.
  Needed models within an admitted account may run concurrently; keys never overlap.
  One completed model cannot release its account's slot while siblings are running.
  Running-key wakeups coalesce separately; restart clears stale running ownership.
- Each account/model acquisition starts with three single-request batches.
  From batch four onward, use `turnStateProbeConcurrency` (default 3, integer 1-10).
  Settings omission preserves the persisted value; migration defaults old rows to 3
  without touching State, progress or clocks. Validate API, use case, store and snapshot
  bounds; enforce the database constraint. Read the live policy at each batch boundary:
  changing only concurrency must not restart the collector, reset its warmup, cancel
  in-flight probes or invalidate State. Batches remain sequential with at most ten
  concurrent requests and no 500-request or whole-task lifetime limit.
  Do not add a hidden model-task cap or six-second pacing. Recheck eligibility
  before each probe and between batches. Misses may hold the five slots indefinitely.
- Each request allocates one enabled/nonblocked source from the process-wide
  sequential IPv6 cursor, independently of business egress. Each probe creates
  a source-bound client; a finite pool wraps and cannot guarantee unique inflight addresses.
- Never read/acquire business scheduling leases, advance rotation, write affinity
  or update business account feedback. Business load cannot starve maintenance.
- Reuse the Responses encoder, account scoping, request profile, QX body/header
  projection and native TLS/custom-CA implementation. Fixed maintenance prompts
  have no client environment/tool metadata; retain independent source-bound
  connections and never inject an old State or enter the business WS pool.
- Use the normal provider failure classifier and revision-fenced account writes
  for confirmed authentication, identity-verification, ban and quota facts.
  Expired refreshable access tokens retain automatic OAuth recovery; revoked
  credentials require reauthorization. Do not invoke business `apply_failure`
  or its feedback, session exclusion or account-wide WS eviction.
  A transient probe rate limit uses a separate account/model/binding cooldown in
  the State row, never the business account cooldown. Unknown scope does not
  upgrade it into an account freeze. Cancel that model's remaining batch, release
  its collector, and rediscover after expiry. Siblings remain independent. Business
  429 handling remains unchanged and can still block the account's collectors.
  Prefer the parsed upstream retry duration, otherwise the existing
  `rate_limit_cooldown_seconds` setting (default 60; explicit zero is respected).
  Clamp untrusted delays to u32 seconds for representable deadlines. A local
  binding-fenced deadline also protects retries when the supplemental store write fails;
  it is not an active-State cache. Persisted cooldown survives process restart
  and switch toggles, but is never part of business readiness or WS pool identity.
  Do not clear historical business cooldowns on upgrade: their origin is ambiguous.
- Exclude non-ready credentials, expired access tokens and exhausted quota at
  discovery, batch boundaries and the one-second cancellation watcher. Store
  reads/writes also enforce this under the existing owner lock. A late response
  cannot resurrect readiness after failure or invalidate replacement credentials.
  Recovery re-enters through ordinary discovery; it does not reset State clocks.
- Individual probes are bounded to thirty seconds.
  Transient HTTP errors, missing states and unusable candidates continue on the
  next batch without an attempt budget. Confirmed account rejection ends that task; other models of that account
  stop at the next eligibility check. Other accounts remain independent.
  Poll account/binding/policy every second and recheck before each batch and
  persistence. Switch disable, model removal or credential replacement cancels
  outstanding probes. Cancellation performs revision-fenced status cleanup even
  after opt-out; it cannot finish a newer credential's task. Store unavailability
  prevents starting collection.
- A lightweight existing SSE decoder checks explicit completion and errors without
  billing/content projection; bound the full probe response to 1 MiB.
  Error bodies have a 16 KiB
  and one-second bound for authentication classification. Oversize, truncated,
  unreadable or timed-out bodies are discarded, keeping status/header evidence;
  do not classify partial text. Failed/truncated streams never publish candidates.
- Observe response quota metadata and allowlisted Cookies through the existing
  provider handlers. Cookie material saves retain normal write CAS and hard/soft
  State-binding rules. Supplemental Cookie CAS conflicts do not discard captured
  State on their own; the current binding/eligibility checks and transaction
  fence still reject late candidates after hard changes or authentication failure.
  Progress/terminal logs include account ID and safe failure category, not tokens.
- Stop at the first committed qualified value and cancel remaining probes.
  Repeated values are not new captures; next must expire later than current.
  Display attempts and safe last-failure codes, never raw State or source addresses.
  `probe_attempts` is the current collection round; `probe_total_attempts` adds
  monotonic deltas across rounds. Migration seeds it from the existing round only,
  so older unrecorded requests cannot be reconstructed. Starting a new round never
  resets the total. Duplicate progress writes do not double-count. Store the last
  returned character count independently from passive business observations.
  The admin model projection includes the probe deadline, HTTP status, allowlisted
  error code and upstream-vs-configured delay source, never free-text error bodies.
  A model with valid active can show both ready and probe cooldown. Account errors
  override that display; an elapsed countdown is not evidence of successful retry.
  Publish changing attempt counts at most once per second during waiting batches.
  Record the winning attempt's dispatch ordinal separately from all attempts started;
  reset this per-task success marker only when a new collection starts or binding changes.
  A Cookie material CAS during a probe must not hide a rejection: reload only the
  same binding generation before recording account failure. Hard identity changes stay fenced.
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
- Cover probe/business wire identity equivalence, bounded error-body stalls,
  revocation versus refreshable expiry, late 401 after rotation, independent
  healthy accounts, recovery, and transient errors that must not invalidate
  credentials. PostgreSQL checks must reject candidates and hide readiness while
  credentials/quota are ineligible, retaining their clocks on same-binding recovery.

## Scenario: Live Probe Concurrency

### 1. Scope / Trigger

Editing the State probe concurrency in runtime settings, including during acquisition.

### 2. Signatures

- `turnStateProbeConcurrency`: optional `u32` on settings update, required on response.
- `runtime_settings.turn_state_probe_concurrency`: INTEGER, default 3, CHECK 1-10.
- `OpenAiTurnStatePolicy::probe_concurrency()` carries the compiled live value.

### 3. Contracts

See Maintenance Isolation for the 1,1,1,N batch contract. The setting applies per
account/model, not to the five-account admission limit or ordinary request concurrency.
Omission/null preserves the stored value. Frontend fallback for an older response is 3.

### 4. Validation & Error Matrix

| Input/event | Expected result |
| --- | --- |
| 1, 3, 10 | Persist, reload and publish to the next batch |
| 0, 11 | Reject before mutation; database also enforces the range |
| Fraction, negative, string or boolean | Reject JSON field decoding |
| Numeric edit while responses are pending | Finish that batch at its original size |
| Migration from 0030 | Default 3; leave all State rows and old settings unchanged |

### 5. Good / Base / Bad Cases

Good: changing 3 to 10 during batch four leaves three inflight, then starts ten.
Base: no edit yields 1,1,1,3,3,... until success or existing stop conditions.
Bad: treating a numeric edit as a credential change or resetting warmup every batch.

### 6. Tests Required

Hold local proxy responses to verify batch sizes and the cancellation watcher.
Test settings validation/omission, real PostgreSQL migration and snapshot roundtrip,
and browser save/reload/bounds at desktop and mobile widths.

### 7. Wrong vs Correct

Wrong: cache concurrency when a long-lived collector starts or use it as a pool key.
Correct: read it at batch boundaries; preserve State, identity, WS ownership and egress.

## Scenario: Concurrent Responses and Independent Switches

### 1. Scope / Trigger

Apply when coalescing passive state observations, persisting candidates, or
changing an account's State switch while other requests or administrators act.

### 2. Signatures

- `ProviderTurnStateCandidate.expected_active_version: Option<u64>` carries the
  version read before probe dispatch. Business responses cannot publish candidates.
- `PendingObservations::enqueue` orders by credential revision, optional injected
  version and anomaly priority.
- `POST /api/admin/accounts/batch-update` accepts a single `accountIds` entry and
  `turnStateInjectionEnabled` without other account fields.

### 3. Contracts

Check the probe candidate's expected version while holding the state-row lock,
before changing slots, status or timestamps. Keep existing credential/policy
fences and probe behavior. Menu toggles patch only the State field and reread the
authoritative account list. Ordinary enable/disable omits the State field.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Late normal echo after promotion or clearing | Return unchanged current record |
| Same-version rejection followed by normal echo | Keep queued rejection |
| Newer-version observation followed by old rejection | Keep newer observation |
| Independent fresh probe | Existing candidate validation and replacement |
| Stale account row during State toggle | Do not submit old scheduling fields |
| Repeated menu click while pending | One mutation; existing guard remains |

### 5. Good / Base / Bad Cases

Good: rejecting A promotes B, and A's delayed normal echo cannot restore A.
Base: a current-version normal observation resets one suspect strike without publishing a value.
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
