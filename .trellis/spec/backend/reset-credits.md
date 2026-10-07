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

## Automatic Account Policies

### Scope / Trigger
Account-scoped opt-in reset-card checks, not paid Codex balance spending or a Core hook.
Reuse AccountsService, existing identity/egress, ledger and three batch execution slots.

### Signatures
AdminAuth/no-store GET `/api/admin/accounts/reset-credits/automatic?accountId=...`;
POST same route: `{accountId,revision,config:{enabled,fiveHourUsedMillis,sevenDayUsedMillis}}`.
Result: `{accountId,revision,config,checkedAt,message}`. Migration 0067 adds policies/jobs.
Default off, thresholds 100000; integers are thousandths of one percentage point.

### Contracts
- Accept zero or 100..100000; zero ignores that window. Both zero skips checks.
  Only AccountWide 18000/604800-second windows qualify, not feature buckets.
  Require fresh post-refresh timestamps, finite used percentages and future resets.
- Durable 120-second check leases, at most three across instances, bounded scans and
  approximately 60-second due times. Disabled policies perform no upstream checks.
- Native Codex OAuth only. Bind creation/upstream identity, not token revision;
  normal refresh preserves authorization. Replaced/deleted identities do not inherit it.
  Templates/imports do not overwrite the independent policy table.
- Freeze earliest eligible card and request ID; recheck policy, identity and manual
  operation fences at queue, claim and consume. Refresh quota again before consume.
  Keep account enablement, ordinary routing and all fingerprint code unchanged.
- Record last_trigger atomically with begin_consume, not enqueue. Unsent cancellation
  can be checked again. Sent/unknown operations keep the original request. Unchanged
  high usage with a new timestamp cannot spend another card. Confirmed recovery rearms.
- Unknown results block indefinitely; explicit original-ID resolution remains possible
  after disabling automation. Disabling cannot undo an already admitted request.

### Validation & Error Matrix
Missing admin -> 401; JSON schema -> 422; threshold -> 400; stale revision/non-native
enable -> conflict; missing store -> 503. Missing/stale quota or invalid card -> no consume.
Consume success plus readback failure -> success with warning, never another card.

### Good / Base / Bad
Good: 75.5%/0 checks only five-hour usage. Base: old accounts stay off.
Bad: assume a new observation timestamp proves the upstream reset took effect.

### Tests Required
Isolated PostgreSQL: CAS, identity/token rotation, concurrent claims, manual competition,
queued/claimed disable, unknown retry, unchanged quota, zero windows, unsent re-enable.
Run migration/reopen, API validation and synthetic mobile/desktop UI; no real redemption.

### Wrong vs Correct
Wrong: mark a cycle spent at enqueue, suppressing valid checks for days after cancellation.
Correct: set the fence with the durable consume permit under the ledger lock.
