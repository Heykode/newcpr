# Precommit Controls And Quota Diagnostics

## Sources And Scope

Selective adaptations of upstream `zyycn/codex-proxy-rs` #301 (`4634d8d`) and
#304 (`3bf7030`). Do not replace New CPR's identity, account egress, quota
authority, finite scheduling, Excel repair, or continuation ownership contracts.

## Settings Contract

- `RequestTuning.stream_prefetch_bytes` / `streamPrefetchBytes` defaults to
  131072 bytes. The existing JSONB overrides accept omission/null as inheritance.
- The runtime snapshot resolves the override; an attempt uses its frozen tuning,
  never the live settings handle while streaming.
- The Web control displays KiB and converts by exactly 1024. Zero disables
  additional Provider precommit buffering and the lower new-chain WS lifecycle
  wait: the first complete parsed client event, including created/in_progress,
  can be released without waiting for later output or a grace timer. Inspect an
  immediate first-event connection-limit rejection before delivery so its existing
  recovery remains available. A rejection after a released lifecycle event cannot
  transparently replay. Pass the frozen AttemptContext threshold to transport;
  standalone client calls snapshot the threshold before opening, not while streaming.
  A positive value bounds lower WS lifecycle buffering with the same original
  SSE-frame byte count used by Provider. Replaying that buffered prefix into the
  Provider does not add a second budget. Transport passes the original grace
  start time to Provider, so a lower-layer timeout cannot start another 2.5-second
  wait. Exact continuation ownership and HTTP/Excel transport selection stay intact.
  This does not bypass parsing, authentication, settlement, account isolation,
  exact continuation ownership, or nonstream JSON aggregation.
- Accept nonnegative integral byte values. Backend storage uses `u64`; the Web
  form rejects numbers it cannot represent exactly. Do not add a product-specific
  size cap or silently clamp/round a saved value.
- A positive threshold counts original chunks, not re-encoded events. Release
  when the count exceeds the configured threshold, at the fixed 2500 ms grace,
  or immediately on semantic output, terminal or EOF. A last chunk may cross the
  threshold: this is not a hard memory ceiling or a request rejection limit.
- Zero must not mark an incomplete, non-client-visible chunk committed. Once
  events are released, later failures must not become transparent replay.
- Buffering does not create retry attempts. Existing same-account transport
  budgets, account-switch budgets and the independent HTTP fallback setting
  remain authoritative. A zero/exhausted transport retry budget with fallback
  disabled produces no transport retry/fallback intent. Core still requires
  replay proof and an uncommitted downstream before any configured retry.
- Trace `provider.precommit.released` with reason, prefetchedBytes, limitBytes,
  and waitMs. No request bodies, credentials, or device identifiers are added.

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
