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
  diagnostics prefer a current name, then the latest snapshot, then the Key ID.
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
