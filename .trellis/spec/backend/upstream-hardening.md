# Selective Upstream Hardening

## 1. Scope

Applies to the local adaptation of upstream #298, #291, #286, #287, #295
and the verified gaps in #297, #225 and #224. Preserve the local Excel,
QX identity, finite concurrency, Redis wait ownership and transport policies.
This contract does not authorize production changes or wholesale upstream merges.

## 2. Signatures

- `RequestTuning.smart_scheduling: SmartSchedulingConfig` is frozen in the
  request plan and propagated to `AccountSelectionPolicy`.
- Runtime settings persist `requestTuning.smartScheduling` in the existing
  JSONB object; no schema migration is needed.
- `ResponseControl::activate` returns an `ActiveResponseInterrupt` owner.
  `interrupt(response_id)` targets only that live owner.
- `ProviderModelCapabilities::with_catalog_accounts` restricts discovery
  visibility; the runtime snapshot carries the source map across cached
  catalog publication.
- `ProviderLeasePort::load_waiting_counts` reads existing Redis wait leases.

## 3. Contracts

Smart fields are `loadWeight`, `quotaWeight`, `healthWeight`, `latencyWeight`,
`resetWeight`, `queueWeight` and `preferHigherWeight`. Defaults are
`1, 0.8, 1, 0.5, 0, 0, false`. Weights are finite, `0..=10`, in tenths,
with at least one positive value. Smart and Sticky share scores. Higher
account-weight switchback is opt-in; required/native/replay owners never
relax. Queue pressure is read only during fallback wait admission, without
advancing the round-robin cursor. Expired or unknown quota reset times score zero.

The frontend renders these defaults below Smart/Sticky, without a master
switch, and provides a reset command. Clone nested defaults and saved values.
Missing/null overrides inherit defaults; invalid edits must not reach storage.
Downgrading to a strict older binary may require removing only the new
`smartScheduling` override after backing up runtime settings.

Disabled scheduling and credential health are independent. OAuth refresh and
revision-fenced health writes may update disabled accounts but cannot enable
them. Preserve quota/profile clocks and reject stale credential revisions.
Execution completion is clamped to `started_at` when the wall clock rolls back.

An immediate selection validates the full account snapshot before acquiring
a scheduling lease, then rechecks account facts after acquisition and before
claiming affinity. Retry snapshot conflicts at most three times, within the
initial candidate universe and frozen scope. Corrupt unpinned credentials
remain locally excluded, not account-health failures or snapshot retries.
All-busy snapshots and lease races without retry hints are capacity errors.

Account switching strips top-level `client_metadata.parent_response_id`;
nested opaque metadata stays intact. Ingress and Provider both reject
`x-openai-account-routing-override` and `x-openai-fedramp`.

WS interruption accepts only `mode: discard_partial_items` and the exact
active response ID. It uses the current execution's original socket, coalesces
duplicates, retires on terminal/cancellation, and allows a continuation after
`response.incomplete` with reason `interrupted`. While processing controls,
pin the same `ExecutionSession::next_event` future: settlement is not
cancel-safe. Deferred create frames and incoming frames share one bounded
32-slot budget.

Oversized new chains may use HTTP under the existing fallback settings.
Oversized exact continuations fail before sending. A downstream WS delta
after HTTP `store:false` requires full replay; it is never silently sent as a
new chain. Keep the separate Excel replay contract.

## 4. Validation Matrix

| Condition | Required result |
| --- | --- |
| Wrong/retired response ID or invalid interrupt mode | 400; do not abort the active response |
| Duplicate valid interrupt | At most one upstream control frame |
| Smart weights invalid/all zero | Reject save; preserve old settings |
| Switchback off | Original preferred account remains preferred |
| Switchback on with hard owner | Original owner remains mandatory |
| Repeated snapshot conflict | Bounded infrastructure failure, no lease reservation |
| Snapshot only saturated | Capacity unavailable, not missing credentials |
| Model discovered only outside the frozen account scope | Hide from public catalog, do not turn discovery into inference authorization |
| Disabled OAuth refresh | Update credentials/health, preserve disabled state |
| Clock rollback | Complete once, never leave a request running |

## 5. Good / Base / Bad Cases

- Good: interrupt the current response, consume its terminal, then continue
  on the same account/socket.
- Base: old settings with no Smart override retain previous scoring and
  preferred-account behavior.
- Bad: introduce a second queue, use live weights mid-request, retry an
  ambiguous send, or permit a model merely because a different account
  discovered it.

## 6. Required Tests

- Core: weight validation/default equivalence, score tolerance scaling,
  reset times, frozen plans and discovery scope.
- Provider: original-socket interruption and continuation, oversized HTTP
  seed followed by rejected delta, disabled refresh, bounded snapshot races,
  finite capacity and both Smart selection paths.
- API: control frames must not recreate the pending execution future;
  queued creates, overload, disconnect and settlement still work.
- Store: real isolated PostgreSQL credential CAS/clocks and finalization;
  real Redis password authentication and a named ACL user with default disabled.
- UI: save/reload/reset and responsive controls with all API/upstream traffic
  blocked in the standalone fixture.
- Full-size transport fixtures share a test-only lock without relaxing payload
  sizes or deadlines. Resume paused Tokio time before waiting on real TCP so
  automatic virtual-time advancement cannot win the I/O timeout race.

## 7. Wrong vs Correct

Wrong: recreate `next_event()` for each interrupt/control frame.

Correct: pin it once and process controls alongside that future until it
resolves, preserving exactly-once settlement and response ownership.

Wrong: acquire capacity before verifying a stale account snapshot.

Correct: verify first, then reserve once; bounded re-read must not consume
the request interval or replace a hard continuation owner.
