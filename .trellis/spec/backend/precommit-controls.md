# Precommit Controls And Quota Diagnostics

## Sources And Scope

Selective adaptations of upstream `zyycn/codex-proxy-rs` #301 (`4634d8d`) and
#304 (`3bf7030`). Do not replace New CPR's identity, account egress, quota
authority, finite scheduling, Excel repair, or continuation ownership contracts.

## Native Delivery And Timing

Aligned narrowly with official `743a3aba70a4f78f27fb53d80a6e719599d2fe5e`
(`d814582`, `faf50c2`, `7b8339e`), without importing unrelated upstream code.

- Native HTTP SSE and WebSocket deliver each parsed client event immediately,
  independent of `stream_prefetch_bytes`. There is no lower WS pre-read or grace
  clock passed to Provider. This removes local waiting, not upstream generation time.
- Empty created/in_progress, recognized response metadata and comment-only SSE
  frames are replaceable preludes. Their delivery does not close Core's replay
  barrier. Preserve provider replay proof, budgets, cancellation, deadlines and
  continuation ownership; marking a prelude does not itself authorize a retry.
- Semantic/tool/compaction output, unknown events and terminals close replay
  monotonically. A later prelude never reopens it. Preserve raw wire bytes/order
  and atomic failure handling without duplicating already-forwarded preludes.
- Restartable WS checks the first non-prelude frame for a connection-limit error
  while polling the body. The error retains its typed recovery classification.
  Exact continuations retain their existing ownership and recovery restrictions.
- Provider first-token timing starts at `response.output_item.added`, matching
  upstream. First-event, first-text and first-reasoning clocks remain distinct;
  metadata, content-part-added and terminal-only events do not invent that boundary.

## Excel Settings Contract

- `RequestTuning.stream_prefetch_bytes` / `streamPrefetchBytes` remains compatible
  and defaults to 131072 bytes. Omission/null inherits; the Web control converts
  exact KiB by 1024 and explicitly states that only Excel uses this threshold.
- Excel uses frozen AttemptContext tuning, never a live settings read midstream.
  Zero releases the first complete client event; positive thresholds count raw
  chunks and release when exceeded, at a single 2500 ms grace, or immediately on
  semantic output, terminal or EOF. This is not a request limit or memory ceiling.
- Incomplete chunks do not commit zero-buffer delivery. Excel released output
  closes its existing replay boundary. Early failures preserve buffered atomic events.
- Accept nonnegative integral bytes; reject inexact Web numbers without imposing
  a product-specific cap or silently rounding/clamping saved settings.
- Preserve Excel repair/metering, account identity/egress and nonstream aggregation.
  No database migration, retry budget or transport selection change is required.
- Trace `provider.precommit.released` retains reason, prefetchedBytes, limitBytes
  and waitMs; native preludes do not emit buffer-release events. No raw payloads
  or credentials may enter diagnostic traces.

## Signatures And Error Matrix

`create_response_stream_with_pool_account(request, context, pool_account_id)`
returns an unbuffered native stream; `precommit_started_at` and the transport
prefetch parameter are removed. `PreCommitClientEvents::stage` still owns Excel
buffering and enables immediate native delivery with `with_replayable_prelude(true)`.
`ProviderEvent::with_replayable_prelude` is evidence consumed by Core's independent
replay barrier, not an instruction to retry.

| Case | Delivery / retry result |
| --- | --- |
| Native empty created then approved overload | Immediate prelude; bounded recovery allowed |
| Native metadata after text/tool/unknown output | Immediate; replay remains closed |
| Restartable WS connection-limit after only preludes | Typed body error; preserve raw error, status and request ID |
| Ambiguous connection loss | Existing no-replay safety remains |
| Excel positive threshold, below limit | Wait for semantic/terminal/EOF, byte crossing or fixed grace |
| Native missing output_item.added | No invented provider first-token time; text/reasoning remain available |

Good: first prelude arrives while a oneshot gate holds the next upstream event.
Base: Excel keeps byte/grace release and atomic failures. Bad: calling an already
sent prelude committed business output or restarting retries after tool output.

## Required Regression Assertions

- HTTP and WS oneshot fixtures deliver under zero, saved 15 KiB, default and large
  thresholds before upstream continuation; bytes and order remain intact.
- Early failure batches exclude already-forwarded preludes. Retry budgets and
  exact continuation restrictions remain effective; ambiguous/late output is not replayed.
- Core can recover after HTTP commitment of a replaceable prelude and cannot
  reopen replay after an unknown event followed by another prelude.
- WS body-error observations retain current failure status and request ID, while
  opening diagnostics remain separate.
- Metadata never starts provider TTFT; output_item.added starts it before content.
- Excel zero/byte/grace/content/terminal/EOF and early atomic failure behavior stays.
- Settings save/reload and isolated 1440/390/320 px views preserve exact values.

Wrong: removing only Provider buffering leaves transport waiting before the
Provider sees the event. Correct: remove native waiting at both layers and keep
replay eligibility independent from physical response commitment.

## Quota Error Contract

Preserve optional HTTP status and a bounded ASCII error code. Map known states
to static public messages; never echo raw upstream bodies. Quota endpoint auth
rejection remains separate from OAuth terminal credential evidence. Existing
authoritative quota-exhaustion classification and observation clocks remain.

## Verification And Rollback

Cover inherited/default/null, zero, custom values, invalid numbers, API and real
PostgreSQL round trips, immutable snapshots, raw event preservation, HTTP/WS
precommit recovery, semantic/tool/terminal/EOF release, timeout and late errors.
Keep Excel, QX/CPR identity, UA changes, account proxies, and continuation tests.

No schema migration is required. Before a future deployment, back up runtime
settings. An older strict binary may need only `streamPrefetchBytes` removed
from the override JSON on rollback; do not replace the whole settings object.

Connection-default A/B cases live in `tests/transport/http_transport_review.rs`.
After the follow-up approval, standard HTTP follows a5a844a; source-bound IPv6
retains its independent policy. Preserve the pre-alignment builder as an explicit
test-only legacy control, so current-vs-upstream does not compare two identical
builders without a baseline. Local H1/H2 success is not proof of Linux upstream
performance, idle keepalive behavior, proxy/IPv6 acceptance, or risk-control outcomes.
