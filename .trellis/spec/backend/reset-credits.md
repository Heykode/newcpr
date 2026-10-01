# Reset Credit Consumption

- Single-account and batch consumption both pass through `AccountsService.consume_reset_credit`.
  Preserve existing credential refresh and provider transport. Do not introduce model scheduling,
  alternate egress, fingerprint changes or transport retries.
- PostgreSQL owns operation identity, active-account exclusion and batch progress.
  Persist the request ID and exact card before forwarding; a completed request replays its stored
  result. Unknown results block new operations for that account indefinitely until resolved.
  Only an explicit retry may reuse the original request/card after the protection interval.
- Batch previews deduplicate accounts, select one available unexpired card with earliest expiry,
  and never guess across reset types or missing expiry metadata. Confirmation activates a frozen
  preview only once; execution rechecks the same card. A changed inventory skips rather than
  substituting another card. No automatic new-ID or next-card recovery is allowed.
- Worker claims are serialized by a short database transaction, with three global execution slots.
  The lock is never held over HTTP. Expired running claims become unknown, not queued.
  Identity snapshots and claim tokens fence changed accounts and late completions.
- Readback failure after confirmed consumption cannot become another consume attempt.
  Refresh inventory and quota on bounded deadlines; retain success and show readback warnings.
- API cache reads never call upstream. Cached failure is unknown, never zero.
  The consume ledger is not request-log retention data and must not be cleaned as ordinary logs.
- Migration 0060 deliberately leaves 0059 for parallel work. Do not change frozen migrations.

Verification: earliest-expiry/type unit tests, real isolated PostgreSQL deduplication/recovery,
authenticated API routes, single-account OAuth same-ID regression, batch UI cancellation,
confirmation replay, pending-operation restoration and responsive synthetic browser fixtures.
