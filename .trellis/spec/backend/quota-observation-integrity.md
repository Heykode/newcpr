# Quota Observation Integrity

## Owners And Authority

Protocol parses quota-only facts. Transport stamps observations when received.
Provider execution chooses passive-write authority. The existing quota service
owns persistence and recovery; PostgreSQL independently fences document and
access clocks by credential revision. Do not add a second scheduler or epoch.

`QuotaRefreshAuthority::PreserveAccess` on a passive failure preserves access and
does not learn plan changes. Active refresh retains its established recovery
policy. Successful inference uses `ObserveAccess`: a 100% header is not a denial,
and stale snapshot data must not prevent a real success from restoring access.

Transparent terminal-only responses can lack canonical completion facts. Observe
their explicit successful terminal for quota only; do not change canonical
billing, raw delivery, affinity or continuation to obtain that evidence. EOF,
malformed terminals and explicit failed statuses are not success.

## Capture And Merge

- Carry `CodexRateLimitObservation { rate_limits, observed_at }` through buffering.
- Sort a buffered batch by capture time and reject samples older than the stored
  document. Never stamp old opening headers with failure or flush time.
- Metadata-only samples may merge metadata but retain the quota-window clock.
- Current SSE/WS error headers use the Codex quota whitelist. Exclude plan,
  authentication, Cookies and turn state. Respect SSE event labels when JSON
  omits `type`; do not rewrite downstream error bytes.
- Generic windows belong to an explicitly active named bucket when no named
  facts exist, including a name-only descriptor. Keep `premium` as core and
  preserve existing explicit-bucket and mirrored-window handling.

## Reset Facts

For explicit quota exhaustion, accept positive numeric or numeric-string
`resets_at` only when future and representable. Otherwise use a positive,
checked `resets_in_seconds`. The relative fallback is anchored when the error
classifier parses the envelope; it is not an exact socket-arrival clock.
Never convert ordinary Retry-After into a quota reset or a missing reset into
permanent exhaustion. Periodic recovery and rate-limit policies remain unchanged.

## Validation

Cover HTTP errors, HTTP SSE, WS, State probes and standalone JSON callers.
Keep success recovery, original raw error, plan, identity revisions and
HTTP/WS fingerprint assertions. Test original sample time, out-of-order batches,
stale-success access, malformed reset and active-only named buckets.

The in-memory account store must emulate PostgreSQL's independent access clock,
including a quota snapshot with no access timestamp. Real PostgreSQL/Redis tests
must run against disposable stores, not silently skipped integration targets.
Do not reset an existing account, rotate its credential or replace a running
container to make validation pass.

## Acceptance Boundaries

Local and Linux protocol/provider regression, real PostgreSQL account tests,
Redis credential tests and application bootstrap tests cover this change.
An isolated full application with synthetic upstream verifies failure authority,
successful recovery, error-envelope quota facts and independent named buckets.
Keep successful synthetic inference distinct from a real-upstream quota denial.
An upstream rejection verifies error handling, not successful inference or WS
acceptance. Do not clear exhaustion to turn such a result into a pass.

Operational evidence stays outside Git. Capture a pre-test container baseline,
use independent stores and resource limits, and retain failed attempts alongside
successful reruns. A test-store capacity failure or an unresolved pre-send lease
failure is not proof of an upstream quota or identity defect. Do not fold lease
redesign into this quota patch without a separately verified cause and scope.

## Live Error Evidence

Treat upstream `invalid_encrypted_content` as an input/history rejection, not
proof of credential corruption. The existing mapping to
`continuation_recovery_required` can coexist with `continuation_requested=false`:
the rejected data may be full replay input rather than a native previous-response
binding. Do not repair telemetry by mutating error semantics, dropping opaque
history or allowing unproven post-send replay.

Acceptance should pair each intentionally malformed request with a valid request
and assert credential/quota state, finalization and lease cleanup. For encryption,
first obtain valid upstream history, verify unmodified HTTP/WS replay and compare
outbound hashes, then mutate one character and restore the original. A fabricated
bad ciphertext alone neither tests preservation nor establishes production cause.

Use successful trace `upstream.connection` events for connection-reuse proof;
dedicated connection-failure columns may be empty on successful turns. Client-Key
rate rejection, upstream quota denial and account authentication failures are
different test outcomes. A one-account run cannot validate account-switch
cleaning, and unchanged tokens/device identity do not imply an unchanged
credential CAS or Cookie material.
