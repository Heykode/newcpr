# Database Guidelines

> Database patterns and conventions for this project.

---

## Overview

<!--
Document your project's database conventions here.

Questions to answer:
- What ORM/query library do you use?
- How are migrations managed?
- What are the naming conventions for tables/columns?
- How do you handle transactions?
-->

(To be filled by the team)

## Account Timestamp Invariant

`provider_accounts.updated_at` must not move backwards on an existing row.
Retain the stored value with `greatest(now(), updated_at, ...)` and include
all newly written observation times. The existing database constraint already
ensures the previous value covers retained creation and observation timestamps.
Credential, quota-document and quota-access observations can advance independently;
quota CAS must cover both the document and access observation times.

Do not rewrite observation timestamps, token expiry, reset times or revision
checks merely to satisfy this invariant. Do not increment timestamps artificially
or treat `updated_at` as a unique version. A clock rollback may temporarily keep
the displayed update time unchanged; credential/config revisions remain the
concurrency controls. Verify using isolated PostgreSQL with synthetic future-dated
rows, normal-time writes and stale/concurrent revision attempts, never by changing
the host clock.

---

## Query Patterns

<!-- How should queries be written? Batch operations? -->

(To be filled by the team)

---

## Migrations

<!-- How to create and run migrations -->

(To be filled by the team)

---

## Naming Conventions

<!-- Table names, column names, index names -->

(To be filled by the team)

---

## Common Mistakes

<!-- Database-related mistakes your team has made -->

(To be filled by the team)

## Scenario: Owned Observability Queries And Historical Key Names

### 1. Scope / Trigger

The approved v3.24.1 adaptation changes admin observability query ownership,
usage pagination and historical downstream Key projections. It does not change
model-request timeouts, account selection, cumulative settlement or log cleanup
rate. The database implementation remains SQLx/PostgreSQL.

### 2. Signatures

- `ObservabilityQueryBudget::run(&PgPool, operation, query)` passes one owned
  `ObservabilityConnection` to the query closure.
- `run_stream(&PgPool, operation, query)` holds the same owned connection until
  completion, error or stream drop.
- Helpers accept `&mut PgConnection`; owned billing streams take the connection
  by value. They must not reacquire from `PgPool` inside a budgeted operation.
- Migration `0069_usage_history_and_indexes.sql` adds
  `model_requests.client_api_key_name_snapshot text` and the partial indexes
  `model_requests_delivered_client_idx` / `model_requests_delivered_account_idx`.
  Append its digest to `.frozen-sha256`; never change earlier migrations.

### 3. Contracts

- The existing configured slot count and wait timeout remain authoritative.
  Queueing, connection acquisition and query consumption share the configured
  observability deadline (120 seconds by default). This applies only to admin
  observability, not inference. The owned connection captures and restores its
  original statement timeout after draining SQL and rollback. Failed restoration
  closes the connection instead of returning altered state to the pool.
- On cancellation, error or completion, the connection's SQLx return/rollback
  future keeps the semaphore permit until protocol cleanup finishes. Releasing
  the permit immediately on cancellation would oversubscribe PostgreSQL.
- Stream errors drop the inner database stream before yielding the error, even
  if the caller retains the returned stream without polling again.
- Usage page count and records share a read-only repeatable-read transaction.
  Order is always `(started_at desc, id desc)`. The first short page supplies its
  own count; later pages may scan the nearer end and restore descending order.
  Materialize only candidate IDs/timestamps before joining wide projections.
  Later usage/error pages use exact minute-bucket counts from that same snapshot
  to narrow the detail range. Preserve all NewCPR predicates, substring search,
  historical labels, quality isolation and stable cross-source ordering.
- Snapshot downstream Key names in both request-admission insert paths. Row
  details/list/search use the snapshot before the current Key name. Grouped
  native diagnostics use the latest request-time snapshot, then the Key ID, as upstream does;
  the local paginated Key/model extension retains its existing current-name fallback.
- Legacy backfill uses surviving Keys' names at upgrade time. A Key deleted
  before this migration has no recoverable old name; do not invent one.
- Keep account names/plans, quality-check and prewarm exclusion predicates,
  cumulative account costs and downstream budgets unchanged. New indexes must
  match NewCPR's existing delivered-usage predicate.
- Integration tests require isolated `CPR_TEST_DATABASE_URL`; do not treat a
  missing variable and skipped database tests as successful database validation.
- Enable CI mode and preflight both PostgreSQL and Redis authentication. Backup
  restore and StoreConfig fixtures require nonempty passwords in both test URLs,
  even when an isolated PostgreSQL instance explicitly uses trust authentication.
  Use test-only credentials satisfying StoreConfig limits, with matching-major
  pg_dump/psql; never change production credentials to satisfy a test harness.

### 4. Validation & Error Matrix

| Condition | Result |
| --- | --- |
| Zero slot count or zero wait timeout | `StoreError::InvalidData` |
| Slot wait expires or semaphore closes | `StoreError::Unavailable`, no query |
| Acquire/query/stream exceeds execution deadline | `StoreError::Unavailable`; owned cleanup retains permit |
| Query future or stream is dropped | Roll back unfinished transaction; release slot after protocol cleanup |
| Key renamed/deleted after admission | Original name remains in request rows and search |
| Key missing before legacy upgrade | Historical name remains unknown |
| Page beyond total | Empty page with accurate total and unchanged ordering |

### 5. Good / Base / Bad Cases

- Good: cancel a sleeping transaction; the next observation waits for rollback,
  then reads zero uncommitted rows. A retained failed stream releases its slot.
- Base: ordinary usage pages and grouped names retain existing API shapes,
  filters, cost totals and quality-check exclusion semantics.
- Bad: drop a permit while its SQL is still running, join full rows before
  pagination, or rewrite an applied migration to add the new column.

### 6. Tests Required

- Real isolated PostgreSQL: saturated-slot queue and timeout, cancellation during
  SQL, rollback, stream completion/error/drop, and execution deadline release.
- Both admission paths: snapshot survives rename/delete and remains searchable
  in usage and errors; grouped diagnostics fall back correctly after deletion.
- Pagination: timestamp ties, filters, exclusions, first/last/partial/empty pages
  and counts. Upgrade existing 0068 data through 0069 and verify all freeze hashes.

### 7. Wrong vs Correct

Wrong: `budget.run(..., |_| query.fetch_all(&pool))` or release a permit before
`PoolConnection::return_to_pool()` finishes.

Correct: use the closure's owned connection for all SQL in the operation:

```rust
budget.run(&pool, "report", |mut connection| async move {
    report(&mut connection).await
}).await
```

The connection owner starts return/rollback on drop and releases its permit
only when that cleanup future finishes.


## Scenario: Official Native Request Analytics

### 1. Scope / Trigger

Native observability now uses the official analytics architecture at `0f5bf03`.
This supersedes earlier exclusions based on the mistaken plugin-only classification.
Permanent billing and account identities remain separate authoritative data.

### 2. Signatures

- Migration `0094_request_analytics.sql`: source view, narrow facts, 15m/1h aggregates,
  daily count index, pending request/bucket queues and build-state singleton.
- `StoreBundle::request_analytics_worker(Arc<dyn UsageBillingProjector>)` is registered
  by App bootstrap after Provider initialization. `maintain(batch_size)` accepts 1..=10000.
- `StorePoolConfig.analytics_projection_work_mem_mb: Option<u32>` defaults to None;
  valid values are 1..=2097151 MB. Host timezone is supplied with `with_timezone`.
- Analytics API metadata: coverageStart/coverageEnd/generatedAt, lagSeconds, state,
  percentilesApproximate, generation, schemaVersion and histogramVersion.

### 3. Contracts

- Projection/backfill uses repeatable-read plus the official schema advisory lock;
  ACK uses a separate read-committed transaction and exact observed change versions.
  Late commits, source deletion, rollback and restarted bucket work cannot lose work.
- Work memory is transaction-local only in projection/backfill. It is not a global
  connection setting and does not change ACK, bucket rebuilding or online queries.
- General stats consume published generations; uncovered backfill is Unavailable.
  Account usage and usage pagination reconcile current source changes in the same snapshot.
- Success page counts use the completed-usage predicate. All pages add its complete
  complement from business source rows, including uncommitted/recovered/prewarm successes;
  an outcome-only list is not equivalent. Error pages keep the existing source predicate.
- Cache keys include exact ranges and full filters. Official limits are 64 entries,
  15s TTL (Dashboard 2s), and <=60s stale only after Unavailable and within the same generation.
  Waiting for the per-key lock counts against the query deadline. Native diagnostics cache
  metadata; the local paginated Key/model extension stays raw and omits analytics metadata.
- Counts, tokens and amounts remain exact. Histograms are approximate with explicit metadata;
  official tests permit 3% relative error. Narrow summaries omit unused distributions;
  insights and selected metric queries supply the relevant full distributions.
- Cache hits increase lagSeconds from generatedAt even when data is unchanged. Cache
  coalescing tests compare business values and generation separately from elapsed age;
  use an explicitly aged fixture, not an assumption that the test finishes within one second.
- Account health timelines retain failures/noncompletion and their five-minute resolution.
  Source search and protocol-specific prewarm/quality exclusions keep NewCPR predicates.
  Retention triggers retract derived facts; permanent ledgers are never recomputed by analytics.
- 0093 retains the historical quota-weight migration identity; do not reuse excluded Excel 0092.

### 4. Validation & Error Matrix

| Condition | Result |
| --- | --- |
| Invalid projection work memory | InvalidData before connecting |
| Backfill has not covered requested range | Unavailable, not zero |
| New queue version commits before old ACK | New version remains pending |
| Source/ops update rolls back | No phantom queue contribution |
| Projection reset interrupted | Persisted progress resumes |
| Cache generation changes | Old data cannot be served, including stale fallback |
| Source logs removed | Published stats retract after worker; ledgers remain |

### 5. Good / Base / Bad Cases

Good: mutate source, run the worker, observe one corrected contribution; during the
pending phase account/page reads remain correct via reconciliation.
Base: empty database becomes ready after its first worker cycle.
Bad: synchronously project every admin query to preserve old tests, or double-count
restored hourly baseline as invented request rows.

### 6. Tests Required

Use real isolated PostgreSQL with CI mode: lifecycle/rollback/late ACK, concurrent deletes,
resumable reset, large-history bucket locality, projection work memory restoration,
empty and unknown/zero costs, saved billing snapshots, exact All/Success/Error pagination,
quality/prewarm isolation, deployment timezone across DST and two-round backup restoration.
Keep official cache generation/stale tests. Update old immediate-read fixtures explicitly;
never weaken production consistency to satisfy obsolete fixtures.

### 7. Wrong vs Correct

Wrong: All count = delivered successes + failed/running outcomes; this loses other
successful records intentionally visible in NewCPR's All page.
Correct: delivered-success analytics counts plus the exact source predicate complement
in one snapshot. Keep detailed source filtering identical for count and record queries.
