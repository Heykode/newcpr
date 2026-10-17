# Selective Upstream Hardening

## 1. Scope

Applies to the local adaptation of upstream #298, #291, #286, #287, #295,
the verified gaps in #297, #225 and #224, and the approved non-plugin
v3.24.1 changes pinned at `eea16f5275cc20c8893ee9b568301152f329089d`.
Preserve the local Excel,
QX identity, finite concurrency, Redis wait ownership and transport policies.
This contract does not authorize production changes or wholesale upstream merges.

## 2. Signatures

- `RequestTuning.smart_scheduling: SmartSchedulingConfig` is frozen in the
  request plan and propagated to `AccountSelectionPolicy`.
- Runtime settings persist `requestTuning.smartScheduling` in the existing
  JSONB object; no schema migration is needed.
- `ResponseControl::activate` returns an `ActiveResponseInterrupt` owner.
  `interrupt(response_id)` targets only that live owner.
- `ResponseControl::bind` stores a weak reference to the existing upstream WS
  pump. `ResponseControlBinding::send` uses the captured connection; controls
  never select an account or start another execution.
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

WS control messages retain their raw text. Unknown modes, unknown/retired IDs
and non-JSON controls go transparently to the existing socket, where upstream
owns validation. They must not start inference or bypass queued-create admission.
For `mode: discard_partial_items`, retain interruption intent only for IDs from
`response.created` actually delivered during the current logical execution.
Coalesce duplicates until the response ID or socket binding changes; a replay-safe
pre-output retry retargets retained intent to the newly delivered ID. Clear the
logical execution's intent on terminal/cancellation. Controls after terminal may
still reach the original socket without recreating a request. A continuation after
`response.incomplete` with reason `interrupted` remains valid. While processing controls,
pin the same `ExecutionSession::next_event` future: settlement is not
cancel-safe. Deferred create frames and incoming frames share one bounded
32-slot budget.

Only explicitly marked replaceable native HTTP SSE/WS preludes (`response.created`, empty
`response.in_progress` and protocol metadata) may precede recovery. Unknown
events, semantic output and terminal failures reach the replay barrier. A prelude
marker is not replay authorization: preserve provider replay proof, attempt limits,
hard continuation owners and the prohibition on `UpstreamSendState::Ambiguous`.
Native events are forwarded immediately regardless of configured prefetch bytes;
only Excel retains that buffering setting. HTTP response commitment is distinct
from the monotonic business replay barrier.
Keep raw WS text separate from raw SSE; never infer success from opaque frames.
Filter protocol metadata headers in active, settlement and idle phases, retaining
business fields. Do not expose `codex.rate_limits` to downstream clients.

Oversized new chains may use HTTP under the existing fallback settings.
Oversized exact continuations fail before sending. A downstream WS delta
after HTTP `store:false` requires full replay; it is never silently sent as a
new chain. Keep the separate Excel replay contract.

## 4. Validation Matrix

### v3.28.0 Adaptation

Client-turn failure markers are scoped by client key, thread and turn. Keep
the 300-second TTL, 100ms per-operation budget and atomic compare-delete.
Only RetryThenReturnError transient retries are suppressed; never convert
this into account cooldown, client rejection or a transport-recovery limit.
Preserve capture factories when installing the retry port, in either order.

Group image policy is a frozen client-scope policy. Any bound group's denial
applies even when the group is disabled for selection. Image input is not image
generation. Missing update fields preserve the saved value.

Diagnostics export is an authenticated read of saved evidence, redacted only
at export. Keep ordinary detail views and request forwarding unchanged. Status
filters must be identical across list/count/minute-page queries and preserve
quality-check isolation without filtering summary/cost queries.

Initial concurrency changes apply only to fresh installations detected under
the migration lock. Configuration revisions cannot reliably identify a fresh
install because historical migrations also increment revisions. Existing
databases, including manually configured defaults, must retain their values.

| Condition | Required result |
| --- | --- |
| Wrong/retired response ID, unknown mode or opaque control | Forward unchanged on the existing socket; upstream validation; no extra admission |
| Duplicate valid active interrupt | At most one control per response ID/socket binding |
| Replay-safe pre-output retry changes response ID/socket | Retained interrupt follows the newly delivered ID once |
| Semantic/unknown output, terminal failure or ambiguous send | No recovery justified by a replaceable prelude |
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
  queued creates, overload, disconnect and settlement still work. Cover duplicate
  active interrupts, retired IDs, transparent unknown/non-JSON controls, retry
  retargeting, raw business messages and all metadata-delivery phases over real WS.
- Core/Provider: retain the existing ambiguous-send regression without weakening
  it; prove semantic and unknown output block recovery after a replaceable prelude.
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

Wrong: reset the interrupt's sent flag on every duplicate, or let a prelude
marker authorize retry of an ambiguous send.

Correct: reset only when the delivered response ID/socket binding changes;
check send certainty and explicit replay proof independently of the prelude.

Wrong: acquire capacity before verifying a stale account snapshot.

Correct: verify first, then reserve once; bounded re-read must not consume
the request interval or replace a hard continuation owner.

## 8. Timing, Reads And Resource Ownership

- `ResponseEventSignals.output_start` is an observation boundary, not delivery
  permission. Merge it in `CodexCanonicalDecoder` before passing it to Provider
  observation. Keep semantic, reasoning and text boundaries independent. Test
  structural-only frames through both real HTTP/SSE and WS, including a delayed
  request origin and subsequent content. A protocol-only unit test is insufficient.
- Usage-row throughput divides output tokens by total request milliseconds,
  matching the existing SQL aggregates; absent TTFT must not hide valid throughput.
- Dashboard counts and capacity share one minimal account-facts snapshot. Reuse
  the existing status resolver, including disabled-account priority. Bounded
  Redis reads (128 per batch) preserve every result and order; they are not a
  model-request concurrency limit. Synthetic account IDs must use `acct_`.
- HTTP drain keeps writer workers and managed egress alive. Then stop/join the
  workers under the existing deadline and stop egress. Dropping the host or its
  shutdown future must cancel/abort owned tasks, including handles already moved
  into a join loop. Use isolated child processes for global logging/signal tests.
- The downstream WS read/write futures belong to one managed pump task. Blocked
  writes cannot hide cancellation or Close; tungstenite owns automatic Pong and
  Close acknowledgements. Preserve heartbeat, queue sizes, delivery acknowledgements
  and active-response expiry behavior. Verify single Pong and Close over real TCP.
- Backup export creates its private partial file before spawning, owns process
  cancellation through both stdout copy and child wait, and removes only the
  partial file on drop. Completed files stay available for recovery. Multipart
  SDK futures are owned by the upload future, never detached workers; cancellation
  drops part retries before remote abort. Recovered completion also cleans staging.
- Reset-card mutex entries live while an active consumer or waiter owns them.
  Release the final strong reference under the map mutex before removing its
  dead weak entry; otherwise concurrent drops can leak indexes or split locks.
- Empty budget windows (`end <= start`) expose zero current usage and no reset
  timestamp. Do not change settlement, historical ledger or reset locking.
